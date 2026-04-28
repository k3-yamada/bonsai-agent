#![allow(clippy::collapsible_if)]
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

/// 失敗モードの大分類（hermes-agent知見で12種に拡張）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureMode {
    ParseError(ParseErrorDetail),
    ToolExecError(ToolErrorDetail),
    ReasoningError,
    LoopDetected,
    /// コンテキストオーバーフロー（トークン上限超過）
    ContextOverflow,
    /// レート制限（API制限到達）
    RateLimited,
    /// ネットワークエラー（接続断、DNS失敗等）
    NetworkError,
    /// サーバー切断（llama-server停止/クラッシュ）
    ServerDisconnect,
}

/// エラー分類の回復ヒント（hermes-agent ClassifiedError パターン）
#[derive(Debug, Clone)]
pub struct RecoveryHint {
    /// リトライ可能か
    pub retryable: bool,
    /// コンテキスト圧縮すべきか
    pub should_compress: bool,
    /// 待機時間（秒、0ならすぐリトライ）
    pub backoff_secs: u64,
    /// 環境障害による待機リトライ（GLM-5.1知見: 再計画ではなく待機が正解）
    pub wait_and_retry: bool,
}

impl RecoveryHint {
    pub fn for_failure(mode: &FailureMode) -> Self {
        match mode {
            FailureMode::ParseError(_) => Self {
                retryable: true,
                should_compress: false,
                backoff_secs: 0,
                wait_and_retry: false,
            },
            FailureMode::ToolExecError(_) => Self {
                retryable: true,
                should_compress: false,
                backoff_secs: 0,
                wait_and_retry: false,
            },
            FailureMode::ReasoningError => Self {
                retryable: true,
                should_compress: false,
                backoff_secs: 0,
                wait_and_retry: false,
            },
            FailureMode::LoopDetected => Self {
                retryable: false,
                should_compress: false,
                backoff_secs: 0,
                wait_and_retry: false,
            },
            FailureMode::ContextOverflow => Self {
                retryable: true,
                should_compress: true,
                backoff_secs: 0,
                wait_and_retry: false,
            },
            FailureMode::RateLimited => Self {
                retryable: true,
                should_compress: false,
                backoff_secs: 15,
                wait_and_retry: true,
            },
            // 環境障害: 再計画不要、待機リトライ（GLM-5.1知見）
            FailureMode::NetworkError => Self {
                retryable: true,
                should_compress: false,
                backoff_secs: 5,
                wait_and_retry: true,
            },
            FailureMode::ServerDisconnect => Self {
                retryable: true,
                should_compress: false,
                backoff_secs: 10,
                wait_and_retry: true,
            },
        }
    }
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

        if self.consecutive_failures > MAX_CONSECUTIVE_FAILURES {
            // Stage 3: 安全停止
            return RecoveryAction::ExplainAndStop(format!(
                "{}回続けて失敗しました。安全のため中断します。原因: {:?}",
                self.consecutive_failures, mode
            ));
        }

        if self.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
            // Stage 2: 再計画
            return RecoveryAction::Replan(
                "前の方法がうまくいきませんでした。目標を確認して、別の方法で計画し直してください。"
                    .to_string(),
            );
        }

        // Stage 1: 通常のリカバリ
        decide_recovery(
            &mode,
            self.consecutive_failures.saturating_sub(1),
            max_retries,
        )
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
                "前回の出力のJSONが正しくありません。正しいJSON形式で<tool_call>を書いてください。"
                    .to_string(),
            ),
            ParseErrorDetail::MissingField(field) => RecoveryAction::RetryWithFix(format!(
                "前回のツール呼び出しに '{field}' がありません。必須フィールドを含めてもう一度書いてください。"
            )),
            ParseErrorDetail::UnexpectedFormat => RecoveryAction::RetryWithTemperatureDelta,
        },
        FailureMode::ToolExecError(detail) => match detail {
            ToolErrorDetail::PermissionDenied => RecoveryAction::SuggestAlternative(
                "権限がありません。別の方法を試してください。".to_string(),
            ),
            ToolErrorDetail::CommandNotFound => RecoveryAction::SuggestAlternative(
                "コマンドが見つかりません。別のコマンドを使ってください。".to_string(),
            ),
            ToolErrorDetail::Timeout => RecoveryAction::RetryWithFix(
                "前回のコマンドがタイムアウトしました。もっと短い時間で終わる方法を試してください。"
                    .to_string(),
            ),
            ToolErrorDetail::InvalidArguments(msg) => RecoveryAction::RetryWithFix(format!(
                "引数エラー: {msg}。引数を直してもう一度試してください。"
            )),
            ToolErrorDetail::ExitCodeNonZero(code) => RecoveryAction::RetryWithFix(format!(
                "コマンドが終了コード {code} で失敗しました。エラーを確認して直してください。"
            )),
            ToolErrorDetail::Unknown(msg) => RecoveryAction::RetryWithFix(format!(
                "エラー: {msg}。別の方法を試してください。"
            )),
        },
        FailureMode::ReasoningError => RecoveryAction::RetryWithTemperatureDelta,
        FailureMode::LoopDetected => RecoveryAction::Abort(
            "同じ操作を繰り返しています。ループを止めるため中断します。".to_string(),
        ),
        FailureMode::ContextOverflow => RecoveryAction::Replan(
            "コンテキストが長すぎます。要点だけにしてもう一度試してください。".to_string(),
        ),
        FailureMode::RateLimited => RecoveryAction::RetryWithFix(
            "リクエスト制限に達しました。少し待ってからやり直します。".to_string(),
        ),
        // 通信エラーは再計画ではなくリトライ優先（GLM-5.1知見）
        FailureMode::NetworkError => RecoveryAction::RetryWithFix(
            "ネットワークエラーが起きました。接続を待ってからやり直します。".to_string(),
        ),
        FailureMode::ServerDisconnect => RecoveryAction::RetryWithFix(
            "サーバーとの接続が切れました。再接続を待ってからやり直します。".to_string(),
        ),
    }
}

/// 試行サマリー: 失敗した試行の履歴を構造化して保持（GrandCode知見）
///
/// 再計画時に「何を試し、何が失敗したか」を注入することで
/// モデルが同じアプローチを繰り返すのを防ぐ
pub struct TrialSummary {
    pub entries: Vec<TrialEntry>,
    max_entries: usize,
}

/// 個別の試行記録
pub struct TrialEntry {
    pub tool_name: String,
    /// 引数の最初の80文字
    pub args_summary: String,
    /// エラーの最初の100文字
    pub error_summary: String,
    /// イテレーション番号
    pub timestamp: usize,
}

impl TrialSummary {
    pub fn new(max: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries: max,
        }
    }

    /// 失敗した試行を記録
    pub fn record_failure(
        &mut self,
        tool_name: &str,
        args_json: &str,
        error_output: &str,
        iteration: usize,
    ) {
        let args_summary: String = args_json.chars().take(80).collect();
        let error_summary: String = error_output.chars().take(100).collect();
        self.entries.push(TrialEntry {
            tool_name: tool_name.to_string(),
            args_summary,
            error_summary,
            timestamp: iteration,
        });
        // 上限超過時は古いものを削除
        while self.entries.len() > self.max_entries {
            self.entries.remove(0);
        }
    }

    /// 再計画プロンプト用のフォーマット出力
    pub fn format_for_replan(&self) -> String {
        if self.entries.is_empty() {
            return String::new();
        }
        let mut lines =
            vec!["すでに試した方法です。同じやり方を避けて、別の方法を考えてください:".to_string()];
        for (i, entry) in self.entries.iter().enumerate() {
            lines.push(format!(
                "{}. [iter {}] {}({}) \u{2192} エラー: {}",
                i + 1,
                entry.timestamp,
                entry.tool_name,
                entry.args_summary,
                entry.error_summary,
            ));
        }
        lines.join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl Default for TrialSummary {
    fn default() -> Self {
        Self::new(10)
    }
}

/// 構造化フィードバック（NAT SelfEvaluatingAgentWithFeedback知見）
///
/// 再計画・検証時に EVALUATION / MISSING_STEPS / SUGGESTIONS の
/// 3セクション構造でフィードバックを注入し、1bitモデルの回復精度を向上させる。
/// confidence値でベスト回答追跡にも使用。
pub struct StructuredFeedback {
    pub evaluation: String,
    pub missing_steps: Vec<String>,
    pub suggestions: Vec<String>,
    /// 0.0(最低)〜1.0(最高) — 失敗数に応じて減衰
    pub confidence: f64,
}

impl StructuredFeedback {
    /// TrialSummaryから構造化フィードバックを生成
    ///
    /// confidence = max(0.0, 1.0 - 0.15 * failure_count)
    /// NAT閾値: 0.85未満でフィードバック注入、0.5はJSON解析失敗時フォールバック
    pub fn from_trial_summary(trials: &TrialSummary, task_context: &str) -> Self {
        if trials.is_empty() {
            return Self {
                evaluation: String::new(),
                missing_steps: Vec::new(),
                suggestions: Vec::new(),
                confidence: 1.0,
            };
        }

        let failure_count = trials.len() as f64;
        let confidence = (1.0 - 0.15 * failure_count).max(0.0);

        // 使用済みツールの集計
        let mut tool_counts: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for entry in &trials.entries {
            *tool_counts.entry(&entry.tool_name).or_insert(0) += 1;
        }
        let tool_list: Vec<String> = tool_counts
            .iter()
            .map(|(name, count)| format!("{}({}回失敗)", name, count))
            .collect();

        let evaluation = format!(
            "タスク「{}」で{}回の試行が失敗。使用ツール: {}",
            task_context,
            trials.len(),
            tool_list.join(", ")
        );

        // エラーパターンから未完了ステップを推定
        let mut missing = Vec::new();
        let last = trials.entries.last().unwrap();
        missing.push(format!(
            "直近の失敗: {}({}) → {}",
            last.tool_name, last.args_summary, last.error_summary
        ));

        // 同一ツール連続失敗の検出
        let mut suggestions = Vec::new();
        for (tool, count) in &tool_counts {
            if *count >= 2 {
                suggestions.push(format!(
                    "{}が{}回連続失敗 — 別のツールか別の引数を検討してください",
                    tool, count
                ));
            }
        }
        if suggestions.is_empty() {
            suggestions.push("これまでの方法を避けて、別のアプローチを試してください".to_string());
        }

        Self {
            evaluation,
            missing_steps: missing,
            suggestions,
            confidence,
        }
    }

    /// セッション注入用のフォーマット出力（NAT EVALUATION/MISSING/SUGGESTIONS構造）
    ///
    /// 空のフィードバック（confidence=1.0、失敗なし）の場合は空文字列を返す
    pub fn format_for_injection(&self) -> String {
        if self.confidence >= 1.0 && self.missing_steps.is_empty() {
            return String::new();
        }

        let mut parts = Vec::new();
        parts.push(format!("[EVALUATION]\n{}", self.evaluation));

        if !self.missing_steps.is_empty() {
            let steps: Vec<String> = self
                .missing_steps
                .iter()
                .enumerate()
                .map(|(i, s)| format!("{}. {}", i + 1, s))
                .collect();
            parts.push(format!("[MISSING STEPS]\n{}", steps.join("\n")));
        }

        if !self.suggestions.is_empty() {
            let sugs: Vec<String> = self
                .suggestions
                .iter()
                .map(|s| format!("- {}", s))
                .collect();
            parts.push(format!("[SUGGESTIONS]\n{}", sugs.join("\n")));
        }

        format!(
            "<structured-feedback>\n{}\n</structured-feedback>",
            parts.join("\n\n")
        )
    }
}

/// 環境障害フィルタ: エラー出力からサーバー/ネットワーク障害を分類（GLM-5.1知見）
///
/// サーバー障害やネットワーク障害は「モデルの能力不足」ではなく
/// 「環境の不安定さ」が原因。再計画ではなく待機・リトライが正解
pub fn classify_environment_failure(error_output: &str) -> Option<FailureMode> {
    let lower = error_output.to_lowercase();
    // サーバー障害パターン
    if lower.contains("connection refused")
        || lower.contains("timeout")
        || lower.contains("503")
        || lower.contains("502")
        || lower.contains("server")
    {
        return Some(FailureMode::ServerDisconnect);
    }
    // ネットワーク障害パターン
    if lower.contains("network") || lower.contains("dns") || lower.contains("econnreset") {
        return Some(FailureMode::NetworkError);
    }
    None
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

/// ファイル単位スタック段階エスカレーション（macOS26/Agent StuckGuard パターン）
///
/// 同一ファイルへの編集失敗をファイル単位で追跡し、段階的に回復指示を強化:
/// - 3回失敗: 「file_readで再読込→write_fileで全上書き」とnudge
/// - 6回失敗: 「このファイルを諦めて次に進め」と指示
pub struct FileStuckGuard {
    failure_counts: HashMap<String, usize>,
    nudge_threshold: usize,
    give_up_threshold: usize,
}

impl FileStuckGuard {
    pub fn new(nudge: usize, give_up: usize) -> Self {
        Self {
            failure_counts: HashMap::new(),
            nudge_threshold: nudge,
            give_up_threshold: give_up,
        }
    }

    /// ファイル編集失敗を記録
    pub fn record_file_failure(&mut self, file_path: &str) {
        *self
            .failure_counts
            .entry(file_path.to_string())
            .or_insert(0) += 1;
    }

    /// ファイル編集成功を記録（カウンタリセット）
    pub fn record_file_success(&mut self, file_path: &str) {
        self.failure_counts.remove(file_path);
    }

    /// 段階的回復指示を返す
    /// None = 通常のリトライ、Some(msg) = 注入すべきシステムメッセージ
    pub fn check_stuck(&self, file_path: &str) -> Option<FileStuckAction> {
        let count = self.failure_counts.get(file_path).copied().unwrap_or(0);
        if count >= self.give_up_threshold {
            Some(FileStuckAction::GiveUp(format!(
                "ファイル '{}' の編集が{}回続けて失敗しました。このファイルの編集はやめて、別の方法でタスクを進めてください。",
                file_path, count
            )))
        } else if count >= self.nudge_threshold {
            Some(FileStuckAction::Nudge(format!(
                "ファイル '{}' の編集が{}回失敗しています。file_readでファイル全体を読み直して、write_fileで全体を書き直してください。部分編集は使わないでください。",
                file_path, count
            )))
        } else {
            None
        }
    }

    /// 追跡中のファイル数
    pub fn tracked_files(&self) -> usize {
        self.failure_counts.len()
    }
}

impl Default for FileStuckGuard {
    fn default() -> Self {
        Self::new(3, 6)
    }
}

/// FileStuckGuard の回復アクション
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileStuckAction {
    /// 回復手順を提示（再読込→全上書き推奨）
    Nudge(String),
    /// このファイルの編集を諦めて次に進む
    GiveUp(String),
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

// ──────────────────────────────────────────────────────────────────────
// MultiFileEditCycleDetector（Step 11 — 複数ファイル間の交互編集検出）
// ──────────────────────────────────────────────────────────────────────

/// 複数ファイル間の編集サイクル検出器（macOS26/Agent ★★ 候補 A）
///
/// 過去 N ターンで編集したファイルパスを window で記録し、2-3 ファイルが
/// それぞれ 2 回以上出現すると cycle として検出する。
///
/// 既存 `LoopDetector` は同一 tool_call hash のみ検出するため、
/// 「A 編集 → B 編集 → A 編集 → B 編集」のような交互編集は検出されない。
/// 本検出器がそのギャップを埋める。
#[derive(Debug)]
pub struct MultiFileEditCycleDetector {
    recent_paths: std::collections::VecDeque<String>,
    window: usize,
    min_files: usize,
    max_files: usize,
}

impl MultiFileEditCycleDetector {
    pub fn new(window: usize) -> Self {
        Self {
            recent_paths: std::collections::VecDeque::with_capacity(window),
            window,
            min_files: 2,
            max_files: 3,
        }
    }

    /// パスを記録し、cycle 検出時 nudge 文字列を返す
    pub fn record_and_check(&mut self, path: &str) -> Option<String> {
        self.recent_paths.push_back(path.to_string());
        if self.recent_paths.len() > self.window {
            self.recent_paths.pop_front();
        }
        // 最低 4 ターン経過後に判定
        if self.recent_paths.len() < 4 {
            return None;
        }
        let mut counts: HashMap<&String, usize> = HashMap::new();
        for p in &self.recent_paths {
            *counts.entry(p).or_default() += 1;
        }
        let unique = counts.len();
        if unique < self.min_files || unique > self.max_files {
            return None;
        }
        // 全てが 2 回以上出現していることを確認
        if !counts.values().all(|&c| c >= 2) {
            return None;
        }
        let names: Vec<String> = counts
            .keys()
            .map(|p| {
                std::path::Path::new(p)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(p)
                    .to_string()
            })
            .collect();
        let nudge = format!(
            "[edit-cycle] ファイル {} を交互に編集しています — 進捗が見られません。\n\
             ステップバック: 全ファイルを再読込 → 全体計画を立てる → 一括編集してください。\n\
             まだ衝突する場合は、より大きな構造変更が必要かもしれません。",
            names.join(", ")
        );
        // 1 回 nudge を出したら window をクリア（連続 nudge 防止）
        self.recent_paths.clear();
        Some(nudge)
    }

    pub fn reset(&mut self) {
        self.recent_paths.clear();
    }

    pub fn window(&self) -> usize {
        self.window
    }
}

impl Default for MultiFileEditCycleDetector {
    fn default() -> Self {
        Self::new(6)
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

    // --- FileStuckGuard テスト ---

    #[test]
    fn test_file_stuck_guard_no_failure() {
        let guard = FileStuckGuard::default();
        assert!(guard.check_stuck("main.rs").is_none());
    }

    #[test]
    fn test_file_stuck_guard_nudge_at_3() {
        let mut guard = FileStuckGuard::default();
        for _ in 0..3 {
            guard.record_file_failure("main.rs");
        }
        let action = guard.check_stuck("main.rs");
        assert!(matches!(action, Some(FileStuckAction::Nudge(_))));
    }

    #[test]
    fn test_file_stuck_guard_give_up_at_6() {
        let mut guard = FileStuckGuard::default();
        for _ in 0..6 {
            guard.record_file_failure("main.rs");
        }
        let action = guard.check_stuck("main.rs");
        assert!(matches!(action, Some(FileStuckAction::GiveUp(_))));
    }

    #[test]
    fn test_file_stuck_guard_reset_on_success() {
        let mut guard = FileStuckGuard::default();
        guard.record_file_failure("main.rs");
        guard.record_file_failure("main.rs");
        guard.record_file_success("main.rs");
        assert!(guard.check_stuck("main.rs").is_none());
    }

    #[test]
    fn test_file_stuck_guard_tracks_per_file() {
        let mut guard = FileStuckGuard::default();
        for _ in 0..4 {
            guard.record_file_failure("a.rs");
        }
        guard.record_file_failure("b.rs");
        assert!(guard.check_stuck("a.rs").is_some());
        assert!(guard.check_stuck("b.rs").is_none());
        assert_eq!(guard.tracked_files(), 2);
    }

    // --- 構造化エラー分類テスト ---

    #[test]
    fn test_recovery_hint_context_overflow() {
        let hint = RecoveryHint::for_failure(&FailureMode::ContextOverflow);
        assert!(hint.retryable);
        assert!(hint.should_compress);
    }

    #[test]
    fn test_recovery_hint_server_disconnect() {
        let hint = RecoveryHint::for_failure(&FailureMode::ServerDisconnect);
        assert!(hint.retryable);
        // 環境障害は圧縮不要（GLM-5.1知見）
        assert!(!hint.should_compress);
        assert_eq!(hint.backoff_secs, 10);
        assert!(hint.wait_and_retry);
    }

    #[test]
    fn test_recovery_hint_rate_limited() {
        let hint = RecoveryHint::for_failure(&FailureMode::RateLimited);
        assert!(hint.retryable);
        assert!(!hint.should_compress);
        assert_eq!(hint.backoff_secs, 15);
    }

    #[test]
    fn test_recovery_hint_loop_not_retryable() {
        let hint = RecoveryHint::for_failure(&FailureMode::LoopDetected);
        assert!(!hint.retryable);
    }

    #[test]
    fn test_decide_recovery_context_overflow() {
        let action = decide_recovery(&FailureMode::ContextOverflow, 0, 3);
        assert!(matches!(action, RecoveryAction::Replan(_)));
    }

    #[test]
    fn test_decide_recovery_server_disconnect() {
        let action = decide_recovery(&FailureMode::ServerDisconnect, 0, 3);
        // 環境障害フィルタ改善後: リトライ優先（再計画ではない）
        assert!(matches!(action, RecoveryAction::RetryWithFix(_)));
    }

    // --- TrialSummary テスト（GrandCode知見） ---

    #[test]
    fn t_trial_summary_record() {
        let mut ts = TrialSummary::new(10);
        ts.record_failure(
            "shell",
            r#"{"command":"cargo build"}"#,
            "コンパイルエラー E0308",
            3,
        );
        assert_eq!(ts.len(), 1);
        let entry = &ts.entries[0];
        assert_eq!(entry.tool_name, "shell");
        assert_eq!(entry.timestamp, 3);
        assert!(!entry.args_summary.is_empty());
        assert!(!entry.error_summary.is_empty());
    }

    #[test]
    fn t_trial_summary_max_entries() {
        let mut ts = TrialSummary::new(3);
        for i in 0..5 {
            ts.record_failure("shell", &format!("arg_{i}"), &format!("error_{i}"), i);
        }
        assert_eq!(ts.len(), 3);
        // 古いものが削除され、最新3件が残る
        assert_eq!(ts.entries[0].timestamp, 2);
        assert_eq!(ts.entries[2].timestamp, 4);
    }

    #[test]
    fn t_trial_summary_format() {
        let mut ts = TrialSummary::new(10);
        ts.record_failure(
            "shell",
            r#"{"command":"cargo build"}"#,
            "コンパイルエラー E0308",
            3,
        );
        ts.record_failure("file_write", "src/main.rs", "権限拒否", 5);
        let output = ts.format_for_replan();
        assert!(output.contains("試した方法"));
        assert!(output.contains("[iter 3]"));
        assert!(output.contains("[iter 5]"));
        assert!(output.contains("shell"));
        assert!(output.contains("file_write"));
    }

    #[test]
    fn t_trial_summary_empty() {
        let ts = TrialSummary::new(10);
        assert!(ts.is_empty());
        assert_eq!(ts.len(), 0);
        let output = ts.format_for_replan();
        assert!(output.is_empty());
    }

    // --- 環境障害フィルタ テスト（GLM-5.1知見） ---

    #[test]
    fn t_classify_env_failure_server() {
        assert_eq!(
            classify_environment_failure("connection refused"),
            Some(FailureMode::ServerDisconnect)
        );
        assert_eq!(
            classify_environment_failure("HTTP 503 Service Unavailable"),
            Some(FailureMode::ServerDisconnect)
        );
        assert_eq!(
            classify_environment_failure("server error 502"),
            Some(FailureMode::ServerDisconnect)
        );
        assert_eq!(
            classify_environment_failure("request timeout after 30s"),
            Some(FailureMode::ServerDisconnect)
        );
    }

    #[test]
    fn t_classify_env_failure_network() {
        assert_eq!(
            classify_environment_failure("network unreachable"),
            Some(FailureMode::NetworkError)
        );
        assert_eq!(
            classify_environment_failure("dns resolution failed"),
            Some(FailureMode::NetworkError)
        );
        assert_eq!(
            classify_environment_failure("ECONNRESET by peer"),
            Some(FailureMode::NetworkError)
        );
    }

    #[test]
    fn t_classify_env_failure_none() {
        assert_eq!(classify_environment_failure("コンパイルエラー E0308"), None);
        assert_eq!(classify_environment_failure("file not found"), None);
        assert_eq!(classify_environment_failure("permission denied"), None);
    }

    // --- Phase 1: StructuredFeedback テスト（NAT SelfEvaluatingAgentWithFeedback知見） ---

    #[test]
    fn t_structured_feedback_from_trial_summary() {
        let mut ts = TrialSummary::new(10);
        ts.record_failure("shell", r#"{"command":"ls"}"#, "permission denied", 1);
        ts.record_failure("shell", r#"{"command":"cat /etc/shadow"}"#, "permission denied", 2);
        let fb = StructuredFeedback::from_trial_summary(&ts, "ファイル一覧を取得する");
        assert!(fb.confidence < 1.0);
        assert!(!fb.evaluation.is_empty());
        assert!(!fb.suggestions.is_empty());
    }

    #[test]
    fn t_structured_feedback_empty_trial() {
        let ts = TrialSummary::new(10);
        let fb = StructuredFeedback::from_trial_summary(&ts, "テスト");
        assert!((fb.confidence - 1.0).abs() < f64::EPSILON);
        assert!(fb.missing_steps.is_empty());
        assert!(fb.suggestions.is_empty());
    }

    #[test]
    fn t_structured_feedback_confidence_decreases_with_failures() {
        let mut ts = TrialSummary::new(10);
        let fb1 = StructuredFeedback::from_trial_summary(&ts, "タスク");
        ts.record_failure("shell", "{}", "error", 1);
        let fb2 = StructuredFeedback::from_trial_summary(&ts, "タスク");
        ts.record_failure("file_write", "{}", "error", 2);
        ts.record_failure("shell", "{}", "error", 3);
        let fb3 = StructuredFeedback::from_trial_summary(&ts, "タスク");
        assert!(fb1.confidence > fb2.confidence);
        assert!(fb2.confidence > fb3.confidence);
        assert!(fb3.confidence >= 0.0);
    }

    #[test]
    fn t_format_structured_contains_sections() {
        let mut ts = TrialSummary::new(10);
        ts.record_failure("shell", r#"{"cmd":"ls"}"#, "not found", 1);
        let fb = StructuredFeedback::from_trial_summary(&ts, "ファイルを探す");
        let output = fb.format_for_injection();
        assert!(output.contains("[EVALUATION]"));
        assert!(output.contains("[SUGGESTIONS]"));
        assert!(output.contains("<structured-feedback>"));
        assert!(output.contains("</structured-feedback>"));
    }

    #[test]
    fn t_format_structured_empty_is_empty() {
        let ts = TrialSummary::new(10);
        let fb = StructuredFeedback::from_trial_summary(&ts, "テスト");
        let output = fb.format_for_injection();
        assert!(output.is_empty());
    }

    #[test]
    fn t_structured_feedback_unique_tools_in_evaluation() {
        let mut ts = TrialSummary::new(10);
        ts.record_failure("shell", "{}", "err1", 1);
        ts.record_failure("shell", "{}", "err2", 2);
        ts.record_failure("file_read", "{}", "err3", 3);
        let fb = StructuredFeedback::from_trial_summary(&ts, "タスク");
        assert!(fb.evaluation.contains("shell"));
        assert!(fb.evaluation.contains("file_read"));
    }

    #[test]
    fn t_structured_feedback_max_confidence_clamp() {
        let ts = TrialSummary::new(10);
        let fb = StructuredFeedback::from_trial_summary(&ts, "タスク");
        assert!(fb.confidence <= 1.0);
        assert!(fb.confidence >= 0.0);
    }

    // ─── Step 11 MultiFileEditCycleDetector tests ─────────────────────

    #[test]
    fn t_cycle_detector_no_cycle_under_4_steps() {
        let mut det = MultiFileEditCycleDetector::new(6);
        assert!(det.record_and_check("a.rs").is_none());
        assert!(det.record_and_check("b.rs").is_none());
        assert!(det.record_and_check("a.rs").is_none()); // 3 turn
    }

    #[test]
    fn t_cycle_detector_detects_alternating() {
        let mut det = MultiFileEditCycleDetector::new(6);
        det.record_and_check("src/a.rs");
        det.record_and_check("src/b.rs");
        det.record_and_check("src/a.rs");
        let nudge = det.record_and_check("src/b.rs"); // 4 turn、A=2 B=2
        assert!(nudge.is_some(), "alternating A/B/A/B should be detected");
        let msg = nudge.unwrap();
        assert!(msg.contains("a.rs"));
        assert!(msg.contains("b.rs"));
    }

    #[test]
    fn t_cycle_detector_skips_too_many_files() {
        let mut det = MultiFileEditCycleDetector::new(6);
        det.record_and_check("a.rs");
        det.record_and_check("b.rs");
        det.record_and_check("c.rs");
        det.record_and_check("d.rs"); // 4 unique > max_files=3
        // 進捗ありの広範囲編集として nudge は出さない
        assert!(det.record_and_check("a.rs").is_none());
    }

    #[test]
    fn t_cycle_detector_resets_after_nudge() {
        let mut det = MultiFileEditCycleDetector::new(6);
        det.record_and_check("a.rs");
        det.record_and_check("b.rs");
        det.record_and_check("a.rs");
        let _ = det.record_and_check("b.rs"); // nudge 出る、window クリア
        // 次の判定はまた 4 ターン後まで None
        assert!(det.record_and_check("a.rs").is_none());
    }

    #[test]
    fn t_cycle_detector_three_files_each_twice() {
        let mut det = MultiFileEditCycleDetector::new(6);
        det.record_and_check("a.rs");
        det.record_and_check("b.rs");
        det.record_and_check("c.rs");
        det.record_and_check("a.rs");
        det.record_and_check("b.rs");
        let nudge = det.record_and_check("c.rs"); // 6 turn、各 2 回
        assert!(nudge.is_some(), "A/B/C/A/B/C should be detected");
    }
}
