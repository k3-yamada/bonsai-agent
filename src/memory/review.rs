//! Adaptive review and freshness scheduling.
//!
//! Ported design from cerememory ADR-011 (Cerememory, MIT, Copyright 2026 CORe Inc.,
//! commit b08d201, https://github.com/co-r-e/cerememory).
//! See `docs/adr/011-adaptive-review-and-freshness.md` of the source repository
//! for full design rationale.
//!
//! ## 設計の核心 (ADR-011)
//! Strength (durability/decay) と Freshness (still-safe-truth) を分離する。
//! SRS (Spaced Repetition System) の前提「再 recall = 再強化」は study tools には
//! 妥当だが、agent memory では「変化する事実」(deployment state / API behavior /
//! credentials policy / dependency versions) を再強化することが**逆に危険**になる。
//!
//! ## bonsai 用法 (項目 218 候補)
//! - `HeuristicStore::save` で volatility と next_review_at を default 値投入
//! - `HeuristicStore::review_tick(now)` で due な heuristic ID を取得
//! - `HeuristicStore::record_review(id, outcome)` で freshness 更新 + 次回 review 計算
//! - `inject_heuristics` で freshness gate (env=enabled かつ < threshold で skip)
//! - `BONSAI_REVIEW_ENABLED=1` で有効化、unset で legacy 経路 (default OFF)
//!
//! ## Phase 1 Red (TDD strict)
//! 4 主要関数 + ReviewOutcome::apply_to は `todo!()` stub。Phase 2 Green で実装。

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// `BONSAI_REVIEW_ENABLED=1` (or "true"、case-insensitive) で freshness gate opt-in。
///
/// production default = env unset = false 返却 = legacy inject 既定化。
/// `BONSAI_REVIEW_ENABLED=1` で freshness gate + review scheduler を有効化。
///
/// 設計方針: 項目 214 (`BONSAI_ERL_ENABLED`) / 項目 217 (`BONSAI_DECAY_ENABLED`)
/// と env name は対称、「production default は既存挙動」という基本方針は一貫。
pub(crate) fn is_review_enabled() -> bool {
    std::env::var("BONSAI_REVIEW_ENABLED")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// review status enum (ADR-011 §"Data Model")。SQLite には TEXT で保管。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewStatus {
    Unknown,
    Current,
    Due,
    Stale,
    Superseded,
    NeedsEvidence,
    Pinned,
}

impl ReviewStatus {
    /// SQLite TEXT 表現に変換。
    pub fn as_db_str(&self) -> &'static str {
        todo!("Phase 2 Green で実装 (Plan B §4.1)")
    }

    /// SQLite TEXT から復元。
    /// ADR-011 §"Data Model" 「Defaults must be backward-compatible」要件で
    /// 未知文字列は Unknown に復元 (typo / V10 legacy 互換)。
    pub fn from_db_str(_s: &str) -> Self {
        todo!("Phase 2 Green で実装 (Plan B §4.1)")
    }
}

/// Cerememory ADR-011 ReviewState 構造体 (1:1 port、Strength と Freshness を分離)。
///
/// 全 numeric 0.0..=1.0 normalize、defaults `serde(default)` で V10→V12 後方互換
/// (ADR-011 §"Data Model" 明示要求)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewState {
    pub status: ReviewStatus,
    pub importance: f64,
    pub volatility: f64,
    pub freshness: f64,
    pub source_confidence: Option<f64>,
    pub last_reviewed_at: Option<DateTime<Utc>>,
    pub next_review_at: Option<DateTime<Utc>>,
    pub review_count: u32,
    pub stale_count: u32,
}

impl Default for ReviewState {
    fn default() -> Self {
        Self {
            status: ReviewStatus::Unknown,
            importance: 0.5,
            volatility: 0.5,
            freshness: 1.0,
            source_confidence: None,
            last_reviewed_at: None,
            next_review_at: None,
            review_count: 0,
            stale_count: 0,
        }
    }
}

/// volatility 推定 (HeuristicStore.save 時に default 値として投入)。
/// category 4 値の MVP マッピング、将来 MetaMemory plane で精緻化 (R1)。
///
/// failure_recovery=0.7 (環境依存助言が陳腐化しやすい) /
/// verification=0.5 (中庸) / efficiency=0.3 (普遍助言が多い)。
pub(crate) fn estimate_volatility_from_category(_category: &str) -> f64 {
    todo!("Phase 2 Green で実装 (Plan B §4.1)")
}

/// 次回 review 日時 (volatility 高ほど短間隔)。
///
/// ```text
/// scale = max(volatility * 4 + 1, 1.0)
/// next_review_at = now + (base_secs / scale) seconds
/// ```
///
/// volatility=1.0 → base/5 (例: 30 day base → 6 day 後)
/// volatility=0.5 → base/3 (例: 30 day base → 10 day 後)
/// volatility=0.0 → base/1 (例: 30 day base → 30 day 後)
pub(crate) fn compute_next_review_at(
    _now: DateTime<Utc>,
    _volatility: f64,
    _base_secs: i64,
) -> DateTime<Utc> {
    let _ = Duration::seconds(0); // unused import 警告抑制
    todo!("Phase 2 Green で実装 (Plan B §4.1)")
}

/// freshness gate: env=enabled かつ freshness < threshold で skip。
///
/// env=disabled で常に false 返却 = legacy inject 完全互換。
pub(crate) fn should_skip_for_freshness(_state: &ReviewState, _threshold: f64) -> bool {
    todo!("Phase 2 Green で実装 (Plan B §4.1)")
}

/// review 後の outcome 種別 (Cerememory `lifecycle.record_review` 引数)。
#[derive(Debug, Clone, Copy)]
pub enum ReviewOutcome {
    /// freshness ← 1.0、status=Current、stale_count reset
    Confirmed,
    /// freshness ← min(1.0, freshness+0.2)、status=Current
    StillCurrent,
    /// freshness ← max(0.0, freshness-0.3)、status=Stale、stale_count++
    Stale,
    /// freshness ← 0.0、status=Superseded
    Superseded,
    /// freshness 不変、status=NeedsEvidence
    NeedsEvidence,
}

impl ReviewOutcome {
    /// outcome を ReviewState に適用 (in-place mutation)。
    pub(crate) fn apply_to(&self, _s: &mut ReviewState) {
        todo!("Phase 2 Green で実装 (Plan B §4.1)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // env mutation race を避けるため module-local Mutex で serialize する
    // (項目 214 ERL_TEST_LOCK / 項目 217 DECAY_TEST_LOCK と同パターン)。
    static REVIEW_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn reset_review_env() {
        unsafe {
            std::env::remove_var("BONSAI_REVIEW_ENABLED");
        }
    }

    // ── ReviewState Default values (Plan B §5 Phase 1 Red) ───────────────

    #[test]
    fn t_review_state_default_values() {
        let s = ReviewState::default();
        assert_eq!(s.status, ReviewStatus::Unknown);
        assert!((s.importance - 0.5).abs() < 1e-9, "importance default 0.5");
        assert!((s.volatility - 0.5).abs() < 1e-9, "volatility default 0.5");
        assert!((s.freshness - 1.0).abs() < 1e-9, "freshness default 1.0");
        assert!(s.source_confidence.is_none());
        assert!(s.last_reviewed_at.is_none());
        assert!(s.next_review_at.is_none());
        assert_eq!(s.review_count, 0);
        assert_eq!(s.stale_count, 0);
    }

    // ── ReviewStatus DB roundtrip ────────────────────────────────────────

    #[test]
    fn t_review_status_db_str_roundtrip() {
        for status in [
            ReviewStatus::Unknown,
            ReviewStatus::Current,
            ReviewStatus::Due,
            ReviewStatus::Stale,
            ReviewStatus::Superseded,
            ReviewStatus::NeedsEvidence,
            ReviewStatus::Pinned,
        ] {
            let s = status.as_db_str();
            assert_eq!(
                ReviewStatus::from_db_str(s),
                status,
                "roundtrip: {status:?}"
            );
        }
        // ADR-011 backward-compat: 未知文字列 → Unknown
        assert_eq!(ReviewStatus::from_db_str("garbage"), ReviewStatus::Unknown);
        assert_eq!(ReviewStatus::from_db_str(""), ReviewStatus::Unknown);
    }

    // ── estimate_volatility_from_category (4 cases) ──────────────────────

    #[test]
    fn t_estimate_volatility_failure_recovery_high() {
        let v = estimate_volatility_from_category("failure_recovery");
        assert!(
            (v - 0.7).abs() < 1e-9,
            "failure_recovery は環境依存助言が陳腐化しやすく 0.7、got {v}"
        );
    }

    #[test]
    fn t_estimate_volatility_efficiency_low() {
        let v = estimate_volatility_from_category("efficiency");
        assert!(
            (v - 0.3).abs() < 1e-9,
            "efficiency は普遍助言が多く 0.3、got {v}"
        );
        // 未知 category は 0.5 default
        let unknown = estimate_volatility_from_category("unknown_category");
        assert!((unknown - 0.5).abs() < 1e-9);
    }

    // ── compute_next_review_at scaling ───────────────────────────────────

    #[test]
    fn t_compute_next_review_at_volatility_scaling() {
        let now = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let base_secs: i64 = 2_592_000; // 30 day

        let high = compute_next_review_at(now, 1.0, base_secs);
        let mid = compute_next_review_at(now, 0.5, base_secs);
        let low = compute_next_review_at(now, 0.0, base_secs);

        assert!(high < mid, "volatility=1.0 → 短間隔 (high < mid)");
        assert!(mid < low, "volatility=0.5 → 中間隔 (mid < low)");

        // volatility=0.0 で base/1 = 30 day 後
        let expected_low = now + chrono::Duration::seconds(base_secs);
        assert_eq!(low, expected_low, "volatility=0.0 で base そのまま");

        // volatility=1.0 で base/5 = 6 day 後
        let expected_high = now + chrono::Duration::seconds(base_secs / 5);
        assert_eq!(high, expected_high, "volatility=1.0 で base/5");
    }

    // ── ReviewOutcome::apply_to (Confirmed / Stale) ──────────────────────

    #[test]
    fn t_review_outcome_confirmed_resets_freshness() {
        let mut s = ReviewState {
            freshness: 0.3,
            stale_count: 5,
            ..Default::default()
        };
        ReviewOutcome::Confirmed.apply_to(&mut s);
        assert!((s.freshness - 1.0).abs() < 1e-9, "Confirmed で freshness=1.0");
        assert_eq!(s.status, ReviewStatus::Current);
        assert_eq!(s.stale_count, 0, "Confirmed で stale_count reset");
    }

    #[test]
    fn t_review_outcome_stale_decreases_freshness() {
        let mut s = ReviewState {
            freshness: 0.8,
            stale_count: 1,
            ..Default::default()
        };
        ReviewOutcome::Stale.apply_to(&mut s);
        assert!(
            (s.freshness - 0.5).abs() < 1e-9,
            "Stale で freshness -0.3 = 0.5、got {}",
            s.freshness
        );
        assert_eq!(s.status, ReviewStatus::Stale);
        assert_eq!(s.stale_count, 2, "Stale で stale_count++");

        // freshness floor: 0 を下回らない
        let mut s2 = ReviewState {
            freshness: 0.1,
            ..Default::default()
        };
        ReviewOutcome::Stale.apply_to(&mut s2);
        assert!(
            s2.freshness >= 0.0,
            "freshness は 0.0 以上にクランプ、got {}",
            s2.freshness
        );
    }

    // ── should_skip_for_freshness (env-gated) ────────────────────────────

    #[test]
    fn t_should_skip_for_freshness_gated_by_env() {
        let _g = REVIEW_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_review_env();

        let low_fresh = ReviewState {
            freshness: 0.20,
            ..Default::default()
        };
        let high_fresh = ReviewState {
            freshness: 0.80,
            ..Default::default()
        };

        // env unset → 常に false (legacy 互換)
        assert!(
            !should_skip_for_freshness(&low_fresh, 0.35),
            "env unset で skip しない (legacy 互換)"
        );

        // env=1 → freshness < threshold で skip
        unsafe {
            std::env::set_var("BONSAI_REVIEW_ENABLED", "1");
        }
        assert!(
            should_skip_for_freshness(&low_fresh, 0.35),
            "env=1 で freshness < 0.35 は skip"
        );
        assert!(
            !should_skip_for_freshness(&high_fresh, 0.35),
            "env=1 で freshness >= 0.35 は inject 通過"
        );
        reset_review_env();
    }

    // ── is_review_enabled toggle (env reading sanity) ────────────────────

    #[test]
    fn t_is_review_enabled_default_unset() {
        let _g = REVIEW_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_review_env();
        assert!(
            !is_review_enabled(),
            "env unset で false (production default OFF、項目 218 候補)"
        );
    }
}
