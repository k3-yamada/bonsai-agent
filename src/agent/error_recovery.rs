use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

/// 失敗モードの大分類
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureMode {
    ParseError(ParseErrorDetail),
    ToolExecError(ToolErrorDetail),
    ReasoningError,
    LoopDetected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseErrorDetail {
    InvalidJson,
    MissingField(String),
    UnexpectedFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolErrorDetail {
    PermissionDenied,
    CommandNotFound,
    Timeout,
    InvalidArguments(String),
    ExitCodeNonZero(i32),
    Unknown(String),
}

/// リカバリアクション
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryAction {
    /// プロンプトを修正してリトライ
    RetryWithFix(String),
    /// temperature変動でリトライ
    RetryWithTemperatureDelta,
    /// 別のツールを提案
    SuggestAlternative(String),
    /// 即座に打ち切り
    Abort(String),
    /// ユーザーに説明して終了
    ExplainAndStop(String),
    /// コンテキスト圧縮+再計画指示を注入
    Replan(String),
}

/// Continue Sites: 段階的回復エスカレーション（CC記事P3 + 松尾研知見）
pub const MAX_CONSECUTIVE_FAILURES: usize = 3;

pub struct ContinueSite {
    consecutive_failures: usize,
    last_failure_mode: Option<FailureMode>,
}

impl ContinueSite {
    pub fn new() -> Self {
        Self {
            consecutive_failures: 0,
            last_failure_mode: None,
        }
    }

    /// 成功を記録（連続失敗カウンタをリセット）
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.last_failure_mode = None;
    }

    /// 失敗を記録
    pub fn record_failure(&mut self, mode: FailureMode) {
        self.consecutive_failures += 1;
        self.last_failure_mode = Some(mode);
    }

    /// 連続失敗数
    pub fn consecutive_failures(&self) -> usize {
        self.consecutive_failures
    }

    /// 段階的エスカレーション: 1-2回→RetryWithFix、3回→Replan、4+回→ExplainAndStop
    pub fn decide_escalated_recovery(&self, max_retries: usize) -> RecoveryAction {
        let mode = self
            .last_failure_mode
            .as_ref()
            .cloned()
            .unwrap_or(FailureMode::ReasoningError);

        if self.consecutive_failures >= MAX_CONSECUTIVE_FAILURES + 1 {
            // Stage 3: 安全停止
            return RecoveryAction::ExplainAndStop(format!(
                "{}回連続で失敗しました。安全のため中断します。最後の原因: {:?}",
                self.consecutive_failures, mode
            ));
        }

        if self.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
            // Stage 2: 再計画
            return RecoveryAction::Replan(
                "前のアプローチが失敗しました。目標を再確認し、別の方法で再計画してください。"
                    .to_string(),
            );
        }

        // Stage 1: 通常のリカバリ
        decide_recovery(&mode, self.consecutive_failures.saturating_sub(1), max_retries)
    }
}

impl Default for ContinueSite {
    fn default() -> Self {
        Self::new()
    }
}

/// 失敗モードに応じたリカバリ戦略を決定
pub fn decide_recovery(mode: &FailureMode, attempt: usize, max_retries: usize) -> RecoveryAction {
    if attempt >= max_retries {
        return RecoveryAction::ExplainAndStop(format!(
            "{}回のリトライを行いましたが解決できませんでした。原因: {:?}",
            max_retries, mode
        ));
    }

    match mode {
        FailureMode::ParseError(detail) => match detail {
            ParseErrorDetail::InvalidJson => RecoveryAction::RetryWithFix(
                "前回の出力はJSON形式が不正でした。正しいJSON形式で<tool_call>を生成してください。"
                    .to_string(),
            ),
            ParseErrorDetail::MissingField(field) => RecoveryAction::RetryWithFix(format!(
                "前回のツール呼び出しにフィールド '{field}' が不足しています。必須フィールドを含めて再生成してください。"
            )),
            ParseErrorDetail::UnexpectedFormat => RecoveryAction::RetryWithTemperatureDelta,
        },
        FailureMode::ToolExecError(detail) => match detail {
            ToolErrorDetail::PermissionDenied => RecoveryAction::SuggestAlternative(
                "権限不足です。別のアプローチを検討してください。".to_string(),
            ),
            ToolErrorDetail::CommandNotFound => RecoveryAction::SuggestAlternative(
                "コマンドが見つかりません。代替コマンドを使用してください。".to_string(),
            ),
            ToolErrorDetail::Timeout => RecoveryAction::RetryWithFix(
                "前回のコマンドがタイムアウトしました。より短時間で完了する方法を試してください。"
                    .to_string(),
            ),
            ToolErrorDetail::InvalidArguments(msg) => RecoveryAction::RetryWithFix(format!(
                "引数エラー: {msg}。引数を修正して再試行してください。"
            )),
            ToolErrorDetail::ExitCodeNonZero(code) => RecoveryAction::RetryWithFix(format!(
                "コマンドが終了コード {code} で失敗しました。エラー内容を確認して修正してください。"
            )),
            ToolErrorDetail::Unknown(msg) => RecoveryAction::RetryWithFix(format!(
                "不明なエラー: {msg}。別のアプローチを試してください。"
            )),
        },
        FailureMode::ReasoningError => RecoveryAction::RetryWithTemperatureDelta,
        FailureMode::LoopDetected => RecoveryAction::Abort(
            "同じ操作の繰り返しを検出しました。ループを回避するため中断します。".to_string(),
        ),
    }
}

/// サーキットブレーカー: 連続失敗するツールを一時的に無効化
pub struct CircuitBreaker {
    failure_counts: HashMap<String, (usize, Instant)>,
    threshold: usize,
    cooldown: Duration,
}

impl CircuitBreaker {
    pub fn new(threshold: usize, cooldown: Duration) -> Self {
        Self {
            failure_counts: HashMap::new(),
            threshold,
            cooldown,
        }
    }

    /// ツールが使用可能か判定
    pub fn is_available(&self, tool_name: &str) -> bool {
        if let Some((count, last_failure)) = self.failure_counts.get(tool_name)
            && *count >= self.threshold
        {
            if last_failure.elapsed() >= self.cooldown {
                return true;
            }
            return false;
        }
        true
    }

    /// 失敗を記録
    pub fn record_failure(&mut self, tool_name: &str) {
        let entry = self
            .failure_counts
            .entry(tool_name.to_string())
            .or_insert((0, Instant::now()));
        entry.0 += 1;
        entry.1 = Instant::now();
    }

    /// 成功を記録（カウンタリセット）
    pub fn record_success(&mut self, tool_name: &str) {
        self.failure_counts.remove(tool_name);
    }

    /// 特定ツールの連続失敗回数
    pub fn failure_count(&self, tool_name: &str) -> usize {
        self.failure_counts
            .get(tool_name)
            .map(|(c, _)| *c)
            .unwrap_or(0)
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new(5, Duration::from_secs(300))
    }
}

/// ループ検出器: 2層検出（ハッシュ+頻度）
pub struct LoopDetector {
    // 既存: 完全文字列一致（後方互換）
    recent_actions: Vec<String>,
    // Layer 2: salient fieldハッシュによる近似検出
    recent_hashes: Vec<u64>,
    // Layer 3: ハッシュ別累計頻度
    action_frequency: HashMap<u64, usize>,
    max_history: usize,
    repeat_threshold: usize,
    frequency_threshold: usize,
}

impl LoopDetector {
    pub fn new(max_history: usize, repeat_threshold: usize) -> Self {
        Self {
            recent_actions: Vec::new(),
            recent_hashes: Vec::new(),
            action_frequency: HashMap::new(),
            max_history,
            repeat_threshold,
            frequency_threshold: 30,
        }
    }

    /// 頻度閾値を設定
    pub fn with_frequency_threshold(mut self, threshold: usize) -> Self {
        self.frequency_threshold = threshold;
        self
    }

    /// salient fieldハッシュ: ツール名 + ソート済みトップレベル引数キーのみ
    fn salient_hash(action: &str) -> u64 {
        // action形式: "tool_name:args_json" or "tool_name"
        let normalized = if let Some((tool, args)) = action.split_once(':') {
            // JSONからキーのみ抽出
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(args) {
                if let Some(obj) = val.as_object() {
                    let mut keys: Vec<&str> = obj.keys().map(|k| k.as_str()).collect();
                    keys.sort();
                    format!("{tool}:{}", keys.join(","))
                } else {
                    tool.to_string()
                }
            } else {
                tool.to_string()
            }
        } else {
            action.to_string()
        };

        let mut hasher = DefaultHasher::new();
        normalized.hash(&mut hasher);
        hasher.finish()
    }

    /// 循環パターン検出（A→B→A→B）
    fn detect_cycle(&self) -> bool {
        let n = self.recent_hashes.len();
        if n < 4 {
            return false;
        }
        // 周期2チェック: 最後4つが [a,b,a,b] パターン
        let last4 = &self.recent_hashes[n - 4..];
        if last4[0] == last4[2] && last4[1] == last4[3] && last4[0] != last4[1] {
            return true;
        }
        false
    }

    /// アクションを記録し、ループが検出されたらtrueを返す
    pub fn record_and_check(&mut self, action: &str) -> bool {
        // 完全文字列一致（既存ロジック）
        self.recent_actions.push(action.to_string());
        if self.recent_actions.len() > self.max_history {
            self.recent_actions.remove(0);
        }

        // Layer 2: ハッシュ
        let hash = Self::salient_hash(action);
        self.recent_hashes.push(hash);
        if self.recent_hashes.len() > self.max_history {
            self.recent_hashes.remove(0);
        }

        // Layer 3: 頻度カウント
        let freq_count = {
            let freq = self.action_frequency.entry(hash).or_insert(0);
            *freq += 1;
            *freq
        };

        // 判定1: 完全文字列一致による連続検出
        if self.recent_actions.len() >= self.repeat_threshold {
            let last_n = &self.recent_actions[self.recent_actions.len() - self.repeat_threshold..];
            if last_n.iter().all(|a| a == &last_n[0]) {
                return true;
            }
        }

        // 判定2: ハッシュ一致による近似検出
        if self.recent_hashes.len() >= self.repeat_threshold {
            let last_n = &self.recent_hashes[self.recent_hashes.len() - self.repeat_threshold..];
            if last_n.iter().all(|h| h == &last_n[0]) {
                return true;
            }
        }

        // 判定3: 循環パターン検出
        if self.detect_cycle() {
            return true;
        }

        // 判定4: 頻度閾値超過
        if freq_count >= self.frequency_threshold {
            return true;
        }

        false
    }

    pub fn reset(&mut self) {
        self.recent_actions.clear();
        self.recent_hashes.clear();
        self.action_frequency.clear();
    }
}

impl Default for LoopDetector {
    fn default() -> Self {
        Self::new(10, 3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- decide_recovery ---

    #[test]
    fn test_recovery_parse_invalid_json() {
        let action = decide_recovery(
            &FailureMode::ParseError(ParseErrorDetail::InvalidJson),
            0,
            3,
        );
        assert!(matches!(action, RecoveryAction::RetryWithFix(_)));
    }

    #[test]
    fn test_recovery_tool_permission_denied() {
        let action = decide_recovery(
            &FailureMode::ToolExecError(ToolErrorDetail::PermissionDenied),
            0,
            3,
        );
        assert!(matches!(action, RecoveryAction::SuggestAlternative(_)));
    }

    #[test]
    fn test_recovery_tool_timeout() {
        let action = decide_recovery(&FailureMode::ToolExecError(ToolErrorDetail::Timeout), 0, 3);
        assert!(matches!(action, RecoveryAction::RetryWithFix(_)));
    }

    #[test]
    fn test_recovery_loop_detected() {
        let action = decide_recovery(&FailureMode::LoopDetected, 0, 3);
        assert!(matches!(action, RecoveryAction::Abort(_)));
    }

    #[test]
    fn test_recovery_max_retries_exceeded() {
        let action = decide_recovery(
            &FailureMode::ParseError(ParseErrorDetail::InvalidJson),
            3,
            3,
        );
        assert!(matches!(action, RecoveryAction::ExplainAndStop(_)));
    }

    #[test]
    fn test_recovery_reasoning_error() {
        let action = decide_recovery(&FailureMode::ReasoningError, 0, 3);
        assert!(matches!(action, RecoveryAction::RetryWithTemperatureDelta));
    }

    // --- ContinueSite ---

    #[test]
    fn test_continue_site_stage1_retry() {
        let mut cs = ContinueSite::new();
        cs.record_failure(FailureMode::ParseError(ParseErrorDetail::InvalidJson));
        let action = cs.decide_escalated_recovery(3);
        // 1回目の失敗→リトライ
        assert!(matches!(action, RecoveryAction::RetryWithFix(_)));
    }

    #[test]
    fn test_continue_site_stage2_replan() {
        let mut cs = ContinueSite::new();
        for _ in 0..MAX_CONSECUTIVE_FAILURES {
            cs.record_failure(FailureMode::ParseError(ParseErrorDetail::InvalidJson));
        }
        let action = cs.decide_escalated_recovery(3);
        assert!(matches!(action, RecoveryAction::Replan(_)));
    }

    #[test]
    fn test_continue_site_stage3_safe_stop() {
        let mut cs = ContinueSite::new();
        for _ in 0..=MAX_CONSECUTIVE_FAILURES {
            cs.record_failure(FailureMode::ReasoningError);
        }
        let action = cs.decide_escalated_recovery(3);
        assert!(matches!(action, RecoveryAction::ExplainAndStop(_)));
    }

    #[test]
    fn test_continue_site_success_resets() {
        let mut cs = ContinueSite::new();
        cs.record_failure(FailureMode::ReasoningError);
        cs.record_failure(FailureMode::ReasoningError);
        cs.record_success();
        assert_eq!(cs.consecutive_failures(), 0);
        // リセット後は1回目→リトライ
        cs.record_failure(FailureMode::ParseError(ParseErrorDetail::InvalidJson));
        let action = cs.decide_escalated_recovery(3);
        assert!(matches!(action, RecoveryAction::RetryWithFix(_)));
    }

    #[test]
    fn test_continue_site_different_failures() {
        let mut cs = ContinueSite::new();
        cs.record_failure(FailureMode::ParseError(ParseErrorDetail::InvalidJson));
        cs.record_failure(FailureMode::ToolExecError(ToolErrorDetail::Timeout));
        cs.record_failure(FailureMode::ReasoningError);
        // 3回連続失敗（種類は異なる）→Replan
        let action = cs.decide_escalated_recovery(3);
        assert!(matches!(action, RecoveryAction::Replan(_)));
    }

    #[test]
    fn test_continue_site_default() {
        let cs = ContinueSite::default();
        assert_eq!(cs.consecutive_failures(), 0);
    }

    // --- CircuitBreaker ---

    #[test]
    fn test_circuit_breaker_initially_available() {
        let cb = CircuitBreaker::default();
        assert!(cb.is_available("shell"));
    }

    #[test]
    fn test_circuit_breaker_trips_after_threshold() {
        let mut cb = CircuitBreaker::new(3, Duration::from_secs(60));
        cb.record_failure("shell");
        cb.record_failure("shell");
        assert!(cb.is_available("shell"));

        cb.record_failure("shell");
        assert!(!cb.is_available("shell"));
    }

    #[test]
    fn test_circuit_breaker_success_resets() {
        let mut cb = CircuitBreaker::new(3, Duration::from_secs(60));
        cb.record_failure("shell");
        cb.record_failure("shell");
        cb.record_success("shell");
        assert!(cb.is_available("shell"));
        assert_eq!(cb.failure_count("shell"), 0);
    }

    #[test]
    fn test_circuit_breaker_cooldown() {
        let mut cb = CircuitBreaker::new(2, Duration::from_millis(50));
        cb.record_failure("shell");
        cb.record_failure("shell");
        assert!(!cb.is_available("shell"));

        std::thread::sleep(Duration::from_millis(60));
        assert!(cb.is_available("shell"));
    }

    #[test]
    fn test_circuit_breaker_independent_tools() {
        let mut cb = CircuitBreaker::new(2, Duration::from_secs(60));
        cb.record_failure("shell");
        cb.record_failure("shell");
        assert!(!cb.is_available("shell"));
        assert!(cb.is_available("file_read"));
    }

    // --- LoopDetector (既存+拡張) ---

    #[test]
    fn test_loop_detector_no_loop() {
        let mut ld = LoopDetector::default();
        assert!(!ld.record_and_check("ls"));
        assert!(!ld.record_and_check("cat"));
        assert!(!ld.record_and_check("pwd"));
    }

    #[test]
    fn test_loop_detector_detects_repeat() {
        let mut ld = LoopDetector::new(10, 3);
        assert!(!ld.record_and_check("ls"));
        assert!(!ld.record_and_check("ls"));
        assert!(ld.record_and_check("ls"));
    }

    #[test]
    fn test_loop_detector_different_actions_no_loop() {
        let mut ld = LoopDetector::new(10, 3);
        assert!(!ld.record_and_check("ls"));
        assert!(!ld.record_and_check("cat"));
        assert!(!ld.record_and_check("ls"));
    }

    #[test]
    fn test_loop_detector_reset() {
        let mut ld = LoopDetector::new(10, 3);
        ld.record_and_check("ls");
        ld.record_and_check("ls");
        ld.reset();
        assert!(!ld.record_and_check("ls"));
    }

    // --- 2層ループ検出テスト ---

    #[test]
    fn test_loop_detector_near_duplicate() {
        // salient hashが同一になるケース（引数のキーが同じ、値のみ違う）
        let mut ld = LoopDetector::new(10, 3);
        assert!(!ld.record_and_check(r#"file_read:{"path":"README.md"}"#));
        assert!(!ld.record_and_check(r#"file_read:{"path":"./README.md"}"#));
        // salient hash = "file_read:path" で同一 → 3回目で検出
        assert!(ld.record_and_check(r#"file_read:{"path":"/tmp/README.md"}"#));
    }

    #[test]
    fn test_loop_detector_cyclic_pattern() {
        // A→B→A→B パターン
        let mut ld = LoopDetector::new(10, 3);
        assert!(!ld.record_and_check("file_read"));
        assert!(!ld.record_and_check("shell"));
        assert!(!ld.record_and_check("file_read"));
        assert!(ld.record_and_check("shell")); // 4つ目で循環検出
    }

    #[test]
    fn test_loop_detector_frequency_threshold() {
        let mut ld = LoopDetector::new(100, 50).with_frequency_threshold(5);
        // 同じアクションを間隔を置いて5回 → 頻度閾値で検出
        for i in 0..4 {
            assert!(!ld.record_and_check("shell"));
            assert!(!ld.record_and_check(&format!("other_{i}")));
        }
        assert!(ld.record_and_check("shell")); // 5回目
    }

    #[test]
    fn test_salient_hash_normalization() {
        // 同じツール名+同じキー → 同じハッシュ
        let h1 = LoopDetector::salient_hash(r#"file_read:{"path":"a.txt"}"#);
        let h2 = LoopDetector::salient_hash(r#"file_read:{"path":"b.txt"}"#);
        assert_eq!(h1, h2, "同じキーなのでハッシュが一致すべき");

        // 異なるキー → 異なるハッシュ
        let h3 = LoopDetector::salient_hash(r#"shell:{"command":"ls"}"#);
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_loop_detector_backward_compat() {
        // 既存の完全文字列一致テストが引き続き動作
        let mut ld = LoopDetector::new(10, 3);
        assert!(!ld.record_and_check("exact_same_string"));
        assert!(!ld.record_and_check("exact_same_string"));
        assert!(ld.record_and_check("exact_same_string"));
    }
}
