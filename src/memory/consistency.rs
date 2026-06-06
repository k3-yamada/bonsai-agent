//! Goodhart's Law 監視 — 指標一貫性チェッカー。
//!
//! 全システム指標が同時に単調増加し、かつ検索重みが一切更新されていない状態を
//! 「指標が形骸化している」兆候 (Goodhart's Law) として検出する。
//!
//! ## bonsai 用法
//! - 呼び出し側が現時点の `heuristic_mean_score` / `decay_fidelity_mean` /
//!   `retrieval_alpha` / `retrieval_beta` を集計し `record_snapshot` に渡す
//! - `detect_goodhart_pattern` で直近 window の一貫性を評価
//! - `BONSAI_GOODHART_CHECK=1` で有効化、unset で常に Acceptable (default OFF)
//!
//! ## LabStagnationDetector との非重複
//! `LabStagnationDetector` は「改善が止まった」(delta 停滞・分散崩壊) を検出する。
//! 本 checker は「改善が続いているのに重みが固定」という逆方向の異常を見る。
//! 判定対象が直交するためロジック重複はない (VecDeque リングバッファ構造のみ共有)。

use std::collections::VecDeque;
use std::time::SystemTime;

/// `BONSAI_GOODHART_CHECK=1` で Goodhart 検出を opt-in。
///
/// production default = env unset = false 返却 = 常に Acceptable。
/// 設計方針: [`crate::memory::decay::is_decay_enabled`] /
/// [`crate::memory::review::is_review_enabled`] と env name は対称。
pub(crate) fn is_goodhart_check_enabled() -> bool {
    std::env::var("BONSAI_GOODHART_CHECK")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// 指定時点のシステム指標スナップショット。
#[derive(Debug, Clone)]
pub struct MetricSnapshot {
    pub timestamp: SystemTime,
    pub heuristic_mean_score: f32,
    pub decay_fidelity_mean: f32,
    pub retrieval_alpha: f32,
    pub retrieval_beta: f32,
}

/// Goodhart's Law リスク評価結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoodhartRisk {
    /// 形骸化の兆候なし。
    Acceptable,
    /// 全指標が同時に単調増加かつ重み固定 — 形骸化の可能性。
    Suspicious(String),
    /// placeholder (今回は生成しない)。
    Critical(String),
}

/// 直近 window のスナップショットから指標形骸化を監視するチェッカー。
pub struct MetricConsistencyChecker {
    snapshots: VecDeque<MetricSnapshot>,
    /// 監視対象スナップショット数 (デフォルト 10)。
    window_size: usize,
}

impl MetricConsistencyChecker {
    /// window_size 指定で生成。
    pub fn new(window_size: usize) -> Self {
        Self {
            snapshots: VecDeque::new(),
            window_size,
        }
    }

    /// スナップショットを記録。window_size 超過時は最古を FIFO で破棄。
    pub fn record_snapshot(&mut self, s: MetricSnapshot) {
        self.snapshots.push_back(s);
        while self.snapshots.len() > self.window_size {
            self.snapshots.pop_front();
        }
    }

    // [VALUES V6] 不確実な指標改善を
    // 断定的に解釈しない。
    // 単調増加は成功ではなく、
    // 形骸化の可能性として扱う。

    // [VALUES V7] 自己更新するシステムは
    // 気づかないうちに別のものになりうる。
    // この checker は月次監査の
    // 自動補完として機能する。
    /// 直近 window_size 件の指標一貫性から Goodhart リスクを評価。
    ///
    /// env=disabled / 履歴不足 (window_size 未満) のときは常に Acceptable。
    pub fn detect_goodhart_pattern(&self) -> GoodhartRisk {
        if !is_goodhart_check_enabled() {
            return GoodhartRisk::Acceptable;
        }
        if self.snapshots.len() < self.window_size {
            return GoodhartRisk::Acceptable;
        }

        let score_increasing = strictly_increasing(&self.snapshots, |s| s.heuristic_mean_score);
        let fidelity_increasing = strictly_increasing(&self.snapshots, |s| s.decay_fidelity_mean);
        let alpha_fixed = all_equal(&self.snapshots, |s| s.retrieval_alpha);

        if score_increasing && fidelity_increasing && alpha_fixed {
            return GoodhartRisk::Suspicious(format!(
                "全指標が{}件連続で\n単調増加かつ重み固定。\n指標が形骸化している可能性あり。",
                self.window_size
            ));
        }

        GoodhartRisk::Acceptable
    }
}

impl Default for MetricConsistencyChecker {
    fn default() -> Self {
        Self::new(10)
    }
}

/// 全隣接ペアで prev < next (strictly increasing、等値は非単調)。
fn strictly_increasing(
    snapshots: &VecDeque<MetricSnapshot>,
    extract: impl Fn(&MetricSnapshot) -> f32,
) -> bool {
    snapshots
        .iter()
        .zip(snapshots.iter().skip(1))
        .all(|(prev, next)| extract(prev) < extract(next))
}

/// 全隣接ペアで完全一致 (f32 ==、固定値判定)。
fn all_equal(
    snapshots: &VecDeque<MetricSnapshot>,
    extract: impl Fn(&MetricSnapshot) -> f32,
) -> bool {
    snapshots
        .iter()
        .zip(snapshots.iter().skip(1))
        .all(|(prev, next)| extract(prev) == extract(next))
}

#[cfg(test)]
mod tests {
    use super::*;

    // env mutation race を避けるため module-local Mutex で serialize する
    // (decay.rs DECAY_TEST_LOCK / review.rs REVIEW_TEST_LOCK と同パターン)。
    static GOODHART_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn snap(score: f32, fidelity: f32, alpha: f32) -> MetricSnapshot {
        MetricSnapshot {
            timestamp: SystemTime::UNIX_EPOCH,
            heuristic_mean_score: score,
            decay_fidelity_mean: fidelity,
            retrieval_alpha: alpha,
            retrieval_beta: 0.0,
        }
    }

    #[test]
    fn strictly_increasing_triggers_suspicious() {
        let _g = GOODHART_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("BONSAI_GOODHART_CHECK", "1");
        }

        let mut checker = MetricConsistencyChecker::new(3);
        checker.record_snapshot(snap(0.50, 0.40, 0.5));
        checker.record_snapshot(snap(0.60, 0.50, 0.5));
        checker.record_snapshot(snap(0.70, 0.60, 0.5));

        let risk = checker.detect_goodhart_pattern();
        assert!(
            matches!(risk, GoodhartRisk::Suspicious(_)),
            "単調増加かつ alpha 固定で Suspicious、got {risk:?}"
        );

        unsafe {
            std::env::remove_var("BONSAI_GOODHART_CHECK");
        }
    }

    #[test]
    fn non_monotone_returns_acceptable() {
        let _g = GOODHART_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("BONSAI_GOODHART_CHECK", "1");
        }

        let mut checker = MetricConsistencyChecker::new(3);
        checker.record_snapshot(snap(0.50, 0.40, 0.5));
        // 途中で score が下がる → 非単調
        checker.record_snapshot(snap(0.45, 0.50, 0.5));
        checker.record_snapshot(snap(0.70, 0.60, 0.5));

        let risk = checker.detect_goodhart_pattern();
        assert_eq!(
            risk,
            GoodhartRisk::Acceptable,
            "score が非単調なら Acceptable"
        );

        unsafe {
            std::env::remove_var("BONSAI_GOODHART_CHECK");
        }
    }

    #[test]
    fn disabled_env_returns_acceptable() {
        let _g = GOODHART_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("BONSAI_GOODHART_CHECK");
        }

        let mut checker = MetricConsistencyChecker::new(3);
        checker.record_snapshot(snap(0.50, 0.40, 0.5));
        checker.record_snapshot(snap(0.60, 0.50, 0.5));
        checker.record_snapshot(snap(0.70, 0.60, 0.5));

        assert_eq!(
            checker.detect_goodhart_pattern(),
            GoodhartRisk::Acceptable,
            "env unset で常に Acceptable (default OFF)"
        );
    }

    #[test]
    fn insufficient_window_returns_acceptable() {
        let _g = GOODHART_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("BONSAI_GOODHART_CHECK", "1");
        }

        let mut checker = MetricConsistencyChecker::new(3);
        checker.record_snapshot(snap(0.50, 0.40, 0.5));
        checker.record_snapshot(snap(0.60, 0.50, 0.5));

        assert_eq!(
            checker.detect_goodhart_pattern(),
            GoodhartRisk::Acceptable,
            "履歴不足 (2 < 3) で Acceptable"
        );

        unsafe {
            std::env::remove_var("BONSAI_GOODHART_CHECK");
        }
    }
}
