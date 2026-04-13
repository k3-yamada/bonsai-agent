use std::collections::HashMap;
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
            // クールダウン期間が経過していればリセット
            if last_failure.elapsed() >= self.cooldown {
                return true; // クールダウン完了 → 再試行可能
            }
            return false; // まだクールダウン中
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

/// ループ検出器: 同じツール呼び出しパターンの繰り返しを検出
pub struct LoopDetector {
    recent_actions: Vec<String>,
    max_history: usize,
    repeat_threshold: usize,
}

impl LoopDetector {
    pub fn new(max_history: usize, repeat_threshold: usize) -> Self {
        Self {
            recent_actions: Vec::new(),
            max_history,
            repeat_threshold,
        }
    }

    /// アクションを記録し、ループが検出されたらtrueを返す
    pub fn record_and_check(&mut self, action: &str) -> bool {
        self.recent_actions.push(action.to_string());
        if self.recent_actions.len() > self.max_history {
            self.recent_actions.remove(0);
        }

        // 直近N回が全て同じアクションかチェック
        if self.recent_actions.len() >= self.repeat_threshold {
            let last_n = &self.recent_actions[self.recent_actions.len() - self.repeat_threshold..];
            if last_n.iter().all(|a| a == &last_n[0]) {
                return true;
            }
        }
        false
    }

    pub fn reset(&mut self) {
        self.recent_actions.clear();
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
        assert!(cb.is_available("shell")); // まだ2回

        cb.record_failure("shell");
        assert!(!cb.is_available("shell")); // 3回目でトリップ
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
        assert!(cb.is_available("shell")); // クールダウン完了
    }

    #[test]
    fn test_circuit_breaker_independent_tools() {
        let mut cb = CircuitBreaker::new(2, Duration::from_secs(60));
        cb.record_failure("shell");
        cb.record_failure("shell");
        assert!(!cb.is_available("shell"));
        assert!(cb.is_available("file_read")); // 別ツールは影響なし
    }

    // --- LoopDetector ---

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
        assert!(ld.record_and_check("ls")); // 3回連続でループ検出
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
        assert!(!ld.record_and_check("ls")); // リセット後は1回目
    }
}
