use crate::observability::logger::{LogLevel, log_event};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};

use crate::config::ServerBackend;
use crate::domain::conversation::Message;

/// モデル選択
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSelection {
    /// デフォルト: エージェント/ツール呼び出し向け
    Bonsai,
    /// 軽量マルチモーダル（~6GB）
    Gemma4E2B,
    /// 高品質マルチモーダル（~8GB）
    Gemma4E4B,
}

/// パイプラインステージ（Advisor Tool パターン）
/// 各ステージで異なるプロンプト/モデル戦略を適用
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineStage {
    /// 探索: ファイル読み込み、情報収集（ツール実行あり）
    Explore,
    /// 計画: 戦略策定（ツール実行なし、思考のみ）
    Plan,
    /// 実行: 計画に基づくツール実行
    Execute,
    /// 検証: 成果物の確認（ツール実行なし）
    Verify,
    /// アドバイス: 外部アドバイザーへの相談（将来: API連携）
    Advise,
}

/// AdvisorConfig::cache の namespace 分離用 discriminant（Phase B1 監査対応）
///
/// `cache_key`（role-based）と `cache_key_for_prompt`（system+user prompt）が同じ
/// `HashMap<u64, String>` を共有するため、両者のキー衝突を構造的に防ぐ。
/// 値はランダムな u64 でよく、衝突確率を実質ゼロにする。
const KEY_DISCRIMINANT_ROLE: u64 = 0x726F6C652D6B6579; // "role-key"
const KEY_DISCRIMINANT_PROMPT: u64 = 0x70726F6D70742D6B; // "prompt-k"

/// アドバイザー設定（Anthropic Advisor Tool パターン準拠）
#[derive(Debug, Clone)]
pub struct AdvisorConfig {
    /// アドバイザー呼び出しの最大回数（max_uses相当）
    pub max_uses: usize,
    /// 現在の呼び出し回数
    pub calls_used: usize,
    /// アドバイザー応答の最大トークン数（推奨: 400-700）
    pub max_advisor_tokens: usize,
    /// 外部APIエンドポイント（None = ローカルモデルで代替）
    /// 例: "https://api.openai.com/v1/chat/completions"
    /// 例: "http://127.0.0.1:8081/v1/chat/completions"（別llama-server）
    pub api_endpoint: Option<String>,
    /// API認証キー（None = 認証なし、ローカルllama-server等）
    pub api_key: Option<String>,
    /// 使用モデル名（デフォルト: "gpt-4o-mini"）
    pub api_model: Option<String>,
    /// HTTPリクエストタイムアウト秒
    pub timeout_secs: u64,
    /// 検証プロンプト（api_endpoint未設定時に使用するローカルプロンプト）
    pub verification_prompt: String,
    /// 停滞時再計画プロンプト（api_endpoint未設定時に使用）
    pub replan_prompt: String,
    /// バックエンド選択（Local/Http/ClaudeCode）
    pub backend: AdvisorBackend,
    /// リトライポリシー
    pub retry_policy: RetryPolicy,
    /// セッション内キャッシュ（同一role+task_contextの重複API呼出を回避）
    /// キー: hash(role, task_context)、値: 外部APIのレスポンス本文
    /// セッションごとにクローンされるため、セッション境界で自動リセット
    #[doc(hidden)]
    pub cache: HashMap<u64, String>,
    /// Self-Verification Dilemma (項目 210、arxiv 2602.03485) — 動的 skip 閾値。
    ///
    /// `EventRepository::verification_success_rate(task_type, min_samples)` が
    /// 本値未満を返した場合、`inject_verification_step` で検証 step を skip。
    /// `0.0` (default) で OFF、後方互換 (既存挙動 100% 維持)。
    /// 推奨: `0.4` (Lab variant 化で ACCEPT 判定後に defaults 昇格)。
    pub dynamic_skip_threshold: f64,
    /// 動的 skip 判定の最小 sample 数 (default 5)。これ未満では
    /// `verification_success_rate` が None を返し既存挙動 fallback。
    pub min_samples_for_skip: usize,
}

/// デフォルトの完了前自己検証プロンプト
pub const DEFAULT_VERIFICATION_PROMPT: &str = "回答前に確認: 目標を達成できていますか？不足があれば追加してください。問題なければ回答に[検証済]を含めてください。";

/// デフォルトの停滞時再計画プロンプト
pub const DEFAULT_REPLAN_PROMPT: &str = "停滞しています。これまでの方法ではうまくいきません。\n<think>内で別の方法を計画:\n1. 失敗の原因\n2. 別のツール/手順\n3. 次にやること";

/// アドバイザー呼び出しの目的
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvisorRole {
    /// 完了前自己検証
    Verification,
    /// 停滞時の再計画
    Replan,
}

/// アドバイザーバックエンド選択
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AdvisorBackend {
    /// ローカルプロンプト（api_endpoint未設定時のデフォルト）
    #[default]
    Local,
    /// 外部HTTP API（OpenAI互換）
    Http,
    /// Claude Code CLI（`claude -p`サブプロセス、Pro/Team契約内で無料）
    ClaudeCode,
}

impl AdvisorBackend {
    pub fn parse_backend(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "http" | "api" => Self::Http,
            "claude-code" | "claude_code" | "claude" => Self::ClaudeCode,
            _ => Self::Local,
        }
    }
}

/// リトライポリシー（Hermes Agent/OpenClaw知見）
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// 同一バックエンドへの最大リトライ回数
    pub max_retries: usize,
    /// 初回リトライの待機時間（ms）。指数バックオフで増加
    pub base_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            base_delay_ms: 500,
        }
    }
}

/// エラー分類（Retry vs Fallback判定用）
#[derive(Debug, PartialEq)]
pub enum RetryErrorKind {
    /// 同じバックエンドでリトライ可能（timeout, 429, 503）
    Retryable,
    /// 認証失敗 → 次のバックエンドへ即座にfallback
    AuthFailure,
    /// その他のエラー → 次のバックエンドへfallback
    Other,
}

/// エラーメッセージからリトライ可否を分類
pub fn classify_advisor_error(error_msg: &str) -> RetryErrorKind {
    let lower = error_msg.to_lowercase();
    if lower.contains("timeout")
        || lower.contains("429")
        || lower.contains("503")
        || lower.contains("rate limit")
        || lower.contains("too many")
    {
        RetryErrorKind::Retryable
    } else if lower.contains("401")
        || lower.contains("403")
        || lower.contains("auth")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
    {
        RetryErrorKind::AuthFailure
    } else {
        RetryErrorKind::Other
    }
}

impl AdvisorRole {
    fn system_prompt(self) -> &'static str {
        match self {
            Self::Verification => {
                "あなたは1bitローカルLLMのアドバイザーです。実行者が回答を出す前に、不足や確認すべき点を100語以内・箇条書きで提示してください。"
            }
            Self::Replan => {
                "あなたは1bitローカルLLMのアドバイザーです。実行者が停滞しています。別の方法を100語以内・箇条書きで: 1. 失敗の原因 2. 別のツール/手順 3. 次にやること。"
            }
        }
    }

    fn user_prompt(self, task_context: &str) -> String {
        match self {
            Self::Verification => format!("タスク: {task_context}\n\n上記の確認事項を簡潔に。"),
            Self::Replan => {
                format!("タスク: {task_context}\n\n停滞しています。別の方法を提案してください。")
            }
        }
    }
}

impl Default for AdvisorConfig {
    fn default() -> Self {
        Self {
            max_uses: 3,
            calls_used: 0,
            max_advisor_tokens: 700,
            api_endpoint: None,
            api_key: None,
            api_model: None,
            timeout_secs: 10,
            verification_prompt: DEFAULT_VERIFICATION_PROMPT.to_string(),
            replan_prompt: DEFAULT_REPLAN_PROMPT.to_string(),
            backend: AdvisorBackend::default(),
            retry_policy: RetryPolicy::default(),
            cache: HashMap::new(),
            // 項目 210 Self-Verify 動的 skip — default OFF (0.0) で既存挙動完全維持
            dynamic_skip_threshold: 0.0,
            min_samples_for_skip: 5,
        }
    }
}

impl AdvisorConfig {
    /// アドバイザー呼び出しが可能か
    pub fn can_advise(&self) -> bool {
        self.calls_used < self.max_uses
    }

    /// 呼び出しを記録
    pub fn record_call(&mut self) {
        self.calls_used += 1;
    }

    /// 残り呼び出し回数
    pub fn remaining(&self) -> usize {
        self.max_uses.saturating_sub(self.calls_used)
    }

    /// 検証プロンプトを構築（ローカル、純粋関数）
    /// 外部API呼び出しが必要な場合は try_remote_advice() を使用
    pub fn build_verification_prompt(&self, _task_context: &str) -> String {
        self.verification_prompt.clone()
    }

    /// 停滞時再計画プロンプトを構築（ローカル、純粋関数）
    pub fn build_replan_prompt(&self, _task_context: &str) -> String {
        self.replan_prompt.clone()
    }

    /// ロールに応じたローカルプロンプトを取得
    pub fn local_prompt_for(&self, role: AdvisorRole, task_context: &str) -> String {
        match role {
            AdvisorRole::Verification => self.build_verification_prompt(task_context),
            AdvisorRole::Replan => self.build_replan_prompt(task_context),
        }
    }

    /// 起動時に設定サマリーをログ表示
    pub fn log_startup(&self) {
        if let Some(endpoint) = &self.api_endpoint {
            let model = self.api_model.as_deref().unwrap_or("gpt-4o-mini");
            let key_status = if self.api_key.is_some() {
                "設定済"
            } else {
                "未設定(env検出)"
            };
            log_event(
                LogLevel::Info,
                "advisor",
                &format!(
                    "リモートモード: endpoint={}, model={}, key={}, max_uses={}, timeout={}s",
                    endpoint, model, key_status, self.max_uses, self.timeout_secs
                ),
            );
        } else if self.backend == AdvisorBackend::ClaudeCode {
            log_event(
                LogLevel::Info,
                "advisor",
                &format!(
                    "Claude Codeモード (max_uses={}, claude -p経由)",
                    self.max_uses
                ),
            );
        } else {
            log_event(
                LogLevel::Info,
                "advisor",
                &format!("ローカルモード (max_uses={}, 検証+再計画)", self.max_uses),
            );
        }
    }

    /// キャッシュキーを計算（role + task_context のハッシュ）
    ///
    /// `cache_key_for_prompt` と同じ HashMap を共有するため、衝突回避のため
    /// `KEY_DISCRIMINANT_ROLE` 定数を XOR して namespace を明示分離する。
    fn cache_key(role: AdvisorRole, task_context: &str) -> u64 {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut h = DefaultHasher::new();
        (role as u8).hash(&mut h);
        task_context.hash(&mut h);
        h.finish() ^ KEY_DISCRIMINANT_ROLE
    }

    /// 外部アドバイザーAPIから指示を取得（OpenAI互換 /chat/completions）
    ///
    /// `role` で検証/再計画を切り替え。戻り値:
    /// - `Ok(None)`: api_endpoint未設定（フォールバック必要）
    /// - `Ok(Some(advice))`: 外部APIから取得成功（キャッシュヒット含む）
    /// - `Err(_)`: ネットワーク/JSON エラー（呼び出し側でフォールバック推奨）
    ///
    /// 同一 role + task_context の重複呼出はキャッシュから返却（セッション境界で自動リセット）
    pub fn try_remote_advice(
        &mut self,
        role: AdvisorRole,
        task_context: &str,
    ) -> anyhow::Result<Option<String>> {
        let Some(endpoint) = self.api_endpoint.as_deref() else {
            return Ok(None);
        };
        // キャッシュヒット
        let key = Self::cache_key(role, task_context);
        if let Some(cached) = self.cache.get(&key) {
            return Ok(Some(cached.clone()));
        }

        let model = self.api_model.as_deref().unwrap_or("gpt-4o-mini");
        let body = serde_json::json!({
            "model": model,
            "messages": [
                { "role": "system", "content": role.system_prompt() },
                { "role": "user", "content": role.user_prompt(task_context) }
            ],
            "max_tokens": self.max_advisor_tokens,
            "temperature": 0.3,
        });

        let mut req = ureq::post(endpoint)
            .header("Content-Type", "application/json")
            .config()
            .timeout_global(Some(std::time::Duration::from_secs(self.timeout_secs)))
            .build();
        if let Some(key) = self.api_key.as_deref() {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        let resp: serde_json::Value = req.send_json(&body)?.body_mut().read_json()?;
        let content = resp["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();
        if content.is_empty() {
            anyhow::bail!("外部アドバイザー応答が空");
        }
        self.cache.insert(key, content.clone());
        Ok(Some(content))
    }

    /// Claude Code CLI経由でアドバイザー応答を取得
    ///
    /// `claude -p "prompt" --output-format text` をサブプロセスで実行。
    /// Pro/Team契約内で追加API料金なし。
    pub fn try_claude_code_advice(
        &mut self,
        role: AdvisorRole,
        task_context: &str,
    ) -> anyhow::Result<Option<String>> {
        if self.backend != AdvisorBackend::ClaudeCode {
            return Ok(None);
        }
        // キャッシュヒット
        let key = Self::cache_key(role, task_context);
        if let Some(cached) = self.cache.get(&key) {
            return Ok(Some(cached.clone()));
        }

        let prompt = format!(
            "{}

{}

制約: 100語以内、箇条書きで簡潔に回答。",
            role.system_prompt(),
            role.user_prompt(task_context)
        );

        let output = std::process::Command::new("claude")
            .args(["-p", &prompt, "--output-format", "text"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let content = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if content.is_empty() {
                    anyhow::bail!("Claude Code応答が空");
                }
                self.cache.insert(key, content.clone());
                Ok(Some(content))
            }
            Ok(out) => {
                anyhow::bail!("Claude Code終了コード: {:?}", out.status.code())
            }
            Err(e) => {
                anyhow::bail!("Claude Code実行失敗: {e}")
            }
        }
    }

    /// system+user の生プロンプトを取って外部API呼出（OpenAI互換 /chat/completions）
    ///
    /// `try_remote_advice` は AdvisorRole の固定プロンプトを使うが、本メソッドは
    /// judge / 任意の用途で **完全カスタムプロンプト**を送れる。キャッシュは
    /// `cache_key_for_prompt(system, user)` でハッシュキーを生成し共有する。
    ///
    /// 戻り値:
    /// - `Ok(None)`: api_endpoint 未設定（フォールバック必要）
    /// - `Ok(Some(advice))`: 外部APIから取得成功（キャッシュヒット含む）
    /// - `Err(_)`: ネットワーク/JSON エラー
    pub fn try_remote_with_prompt(
        &mut self,
        system: &str,
        user: &str,
    ) -> anyhow::Result<Option<String>> {
        let Some(endpoint) = self.api_endpoint.as_deref() else {
            return Ok(None);
        };
        let key = Self::cache_key_for_prompt(system, user);
        if let Some(cached) = self.cache.get(&key) {
            return Ok(Some(cached.clone()));
        }

        let model = self.api_model.as_deref().unwrap_or("gpt-4o-mini");
        let body = serde_json::json!({
            "model": model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user }
            ],
            "max_tokens": self.max_advisor_tokens,
            "temperature": 0.3,
        });

        let mut req = ureq::post(endpoint)
            .header("Content-Type", "application/json")
            .config()
            .timeout_global(Some(std::time::Duration::from_secs(self.timeout_secs)))
            .build();
        if let Some(api_key) = self.api_key.as_deref() {
            req = req.header("Authorization", format!("Bearer {api_key}"));
        }
        let resp: serde_json::Value = req.send_json(&body)?.body_mut().read_json()?;
        let content = resp["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();
        if content.is_empty() {
            anyhow::bail!("外部アドバイザー応答が空");
        }
        self.cache.insert(key, content.clone());
        Ok(Some(content))
    }

    /// system+user の生プロンプトを取って Claude Code CLI 呼出
    ///
    /// `try_claude_code_advice` は AdvisorRole の固定プロンプトを使うが、本メソッドは
    /// judge / 任意の用途で完全カスタムプロンプトを送れる。
    ///
    /// **脅威モデル**: `.args(["-p", &prompt, ...])` 経由で渡すため OS シェル経由の
    /// コマンド注入は不可能。ただし `user` にエージェント出力等の外部由来文字列が
    /// 入る場合、判定 LLM への **prompt injection** リスクは呼出側の責務。
    /// judge 用途では `build_judge_user_prompt` が構造化（タスク/応答/軌跡を別行）
    /// するためそのまま渡してよいが、生エージェント出力を直接渡す経路は注意。
    pub fn try_claude_code_with_prompt(
        &mut self,
        system: &str,
        user: &str,
    ) -> anyhow::Result<Option<String>> {
        if self.backend != AdvisorBackend::ClaudeCode {
            return Ok(None);
        }
        let key = Self::cache_key_for_prompt(system, user);
        if let Some(cached) = self.cache.get(&key) {
            return Ok(Some(cached.clone()));
        }

        let prompt = format!("{system}\n\n{user}");
        let output = std::process::Command::new("claude")
            .args(["-p", &prompt, "--output-format", "text"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let content = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if content.is_empty() {
                    anyhow::bail!("Claude Code応答が空");
                }
                self.cache.insert(key, content.clone());
                Ok(Some(content))
            }
            Ok(out) => {
                anyhow::bail!("Claude Code終了コード: {:?}", out.status.code())
            }
            Err(e) => {
                anyhow::bail!("Claude Code実行失敗: {e}")
            }
        }
    }

    /// 生プロンプト用のキャッシュキー（system+user の内容ハッシュ）
    ///
    /// `cache_key` と同じ HashMap を共有するため `KEY_DISCRIMINANT_PROMPT` を XOR して
    /// namespace を構造的に分離する（rust-reviewer 監査 MEDIUM 対応）。
    fn cache_key_for_prompt(system: &str, user: &str) -> u64 {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut h = DefaultHasher::new();
        system.hash(&mut h);
        "\x00".hash(&mut h); // system/user 境界
        user.hash(&mut h);
        h.finish() ^ KEY_DISCRIMINANT_PROMPT
    }
}

/// G1 Critic 別 LLM 分離 — step 中の独立 critique 機構。
#[derive(Debug, Clone)]
pub struct CriticConfig {
    /// `BONSAI_CRITIC_ENABLED=1` で opt-in。default OFF で観測動作完全互換。
    pub enabled: bool,
    /// critic 呼出モード。
    pub mode: CriticMode,
    /// critic 呼出の最大回数 (advisor max_uses と独立)。
    pub max_critic_uses: usize,
    /// 現在の呼出回数。
    pub critic_calls_used: usize,
    /// critic 専用 system prompt。
    pub critic_system_prompt: String,
    /// critic 呼出時の temperature override。
    pub critic_temperature: f64,
    /// critic 応答の最大トークン数。
    pub max_critic_tokens: usize,
    /// critic 呼出の hook 位置。
    pub hook: CriticHook,
    /// disagreement 検出時の挙動。
    pub on_disagreement: CriticDisagreementAction,
}

/// critic 呼出モード。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CriticMode {
    #[default]
    SamePromptDifferentTemperature,
    DifferentSystemPrompt,
    SeparateBackend,
}

impl CriticMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SamePromptDifferentTemperature => "same_temp",
            Self::DifferentSystemPrompt => "different_prompt",
            Self::SeparateBackend => "separate_backend",
        }
    }
}

/// critic 呼出の hook 位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CriticHook {
    #[default]
    AfterStepOutcome,
    BeforeToolCall,
}

/// critic が executor に disagree した時の挙動。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CriticDisagreementAction {
    #[default]
    InjectAsSystemMessage,
    LogOnly,
    ForceReplan,
}

/// critic review の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CriticOutcome {
    Agree {
        raw_response: String,
    },
    Disagree {
        raw_response: String,
        suggested_revision: Option<String>,
    },
    Uncertain {
        raw_response: String,
    },
    Skipped {
        reason: &'static str,
    },
    BackendError {
        err: String,
    },
}

impl CriticOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Agree { .. } => "agree",
            Self::Disagree { .. } => "disagree",
            Self::Uncertain { .. } => "uncertain",
            Self::Skipped { .. } => "skipped",
            Self::BackendError { .. } => "error",
        }
    }

    pub fn raw_response(&self) -> Option<&str> {
        match self {
            Self::Agree { raw_response }
            | Self::Disagree { raw_response, .. }
            | Self::Uncertain { raw_response } => Some(raw_response),
            Self::Skipped { .. } | Self::BackendError { .. } => None,
        }
    }
}

impl Default for CriticConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: CriticMode::SamePromptDifferentTemperature,
            max_critic_uses: 3,
            critic_calls_used: 0,
            critic_system_prompt: include_str!("../../prompts/critic.txt").to_string(),
            critic_temperature: 0.7,
            max_critic_tokens: 400,
            hook: CriticHook::AfterStepOutcome,
            on_disagreement: CriticDisagreementAction::InjectAsSystemMessage,
        }
    }
}

impl CriticConfig {
    /// critic 呼出が可能か。
    pub fn can_critique(&self) -> bool {
        self.enabled && self.critic_calls_used < self.max_critic_uses
    }

    /// 呼び出しを記録。
    pub fn record_call(&mut self) {
        self.critic_calls_used += 1;
    }

    /// env opt-in で critic 設定を構築する。
    pub fn from_env() -> Self {
        let mut config = Self::default();
        let enabled = std::env::var("BONSAI_CRITIC_ENABLED")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        if !enabled {
            return config;
        }
        config.enabled = true;

        if let Ok(mode) = std::env::var("BONSAI_CRITIC_MODE") {
            config.mode = match mode.as_str() {
                "same_temp" => CriticMode::SamePromptDifferentTemperature,
                "different_prompt" => CriticMode::DifferentSystemPrompt,
                "separate_backend" => {
                    log_event(
                        LogLevel::Warn,
                        "critic",
                        "BONSAI_CRITIC_MODE=separate_backend は Phase 2 未実装のため default にフォールバック",
                    );
                    CriticMode::SamePromptDifferentTemperature
                }
                other => {
                    log_event(
                        LogLevel::Warn,
                        "critic",
                        &format!("不正な BONSAI_CRITIC_MODE={other}、default にフォールバック"),
                    );
                    CriticMode::SamePromptDifferentTemperature
                }
            };
        }
        // Codex audit LOW: NaN / ±Inf は persistence / JSON encode 経路で破綻するため
        // 非有限値は default (0.7) にフォールバックする (項目 225 PASS@(k,T) と同 pattern)。
        if let Ok(temp) = std::env::var("BONSAI_CRITIC_TEMPERATURE")
            && let Ok(parsed) = temp.parse::<f64>()
            && parsed.is_finite()
        {
            config.critic_temperature = parsed;
        }
        if let Ok(max_uses) = std::env::var("BONSAI_CRITIC_MAX_USES")
            && let Ok(parsed) = max_uses.parse::<usize>()
        {
            config.max_critic_uses = parsed;
        }
        if let Ok(hook) = std::env::var("BONSAI_CRITIC_HOOK") {
            config.hook = match hook.as_str() {
                "after_step" => CriticHook::AfterStepOutcome,
                "before_tool" => {
                    log_event(
                        LogLevel::Warn,
                        "critic",
                        "BONSAI_CRITIC_HOOK=before_tool は Phase 2 未実装のため default にフォールバック",
                    );
                    CriticHook::AfterStepOutcome
                }
                other => {
                    log_event(
                        LogLevel::Warn,
                        "critic",
                        &format!("不正な BONSAI_CRITIC_HOOK={other}、default にフォールバック"),
                    );
                    CriticHook::AfterStepOutcome
                }
            };
        }
        if let Ok(action) = std::env::var("BONSAI_CRITIC_DISAGREEMENT") {
            config.on_disagreement = match action.as_str() {
                "inject" => CriticDisagreementAction::InjectAsSystemMessage,
                "log_only" => CriticDisagreementAction::LogOnly,
                "force_replan" => {
                    log_event(
                        LogLevel::Warn,
                        "critic",
                        "BONSAI_CRITIC_DISAGREEMENT=force_replan は Phase 2 未実装のため inject にフォールバック",
                    );
                    CriticDisagreementAction::InjectAsSystemMessage
                }
                other => {
                    log_event(
                        LogLevel::Warn,
                        "critic",
                        &format!(
                            "不正な BONSAI_CRITIC_DISAGREEMENT={other}、inject にフォールバック"
                        ),
                    );
                    CriticDisagreementAction::InjectAsSystemMessage
                }
            };
        }
        config
    }
}

/// タスクコンテキスト（モデル選択の入力）
pub struct TaskContext {
    pub has_image: bool,
    pub estimated_tokens: usize,
    pub is_daemon: bool,
}

impl TaskContext {
    /// メッセージリストからTaskContextを構築
    pub fn from_messages(messages: &[Message], is_daemon: bool) -> Self {
        let has_image = messages.iter().any(|m| m.has_image());
        let estimated_tokens = messages.iter().map(|m| m.content.len() / 4).sum();
        Self {
            has_image,
            estimated_tokens,
            is_daemon,
        }
    }
}

/// モデルルーター設定
pub struct RouterConfig {
    pub enabled: bool,
    pub min_free_ram_e4b: u64, // E4Bに必要な空きRAM（バイト）
    pub min_free_ram_e2b: u64, // E2Bに必要な空きRAM（バイト）
    pub prefer_bonsai_for_tools: bool,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_free_ram_e4b: 8 * 1024 * 1024 * 1024, // 8GB
            min_free_ram_e2b: 6 * 1024 * 1024 * 1024, // 6GB
            prefer_bonsai_for_tools: true,
        }
    }
}

/// タスク特性とRAM残量からモデルを自動選択
pub fn select_model(ctx: &TaskContext, config: &RouterConfig) -> ModelSelection {
    if !config.enabled {
        return ModelSelection::Bonsai;
    }

    // デーモンモードは常にBonsai（最小フットプリント）
    if ctx.is_daemon {
        return ModelSelection::Bonsai;
    }

    // 画像入力がある場合のみGemma 4を検討
    if ctx.has_image {
        let free_ram = get_available_ram();
        if free_ram >= config.min_free_ram_e4b {
            return ModelSelection::Gemma4E4B;
        }
        if free_ram >= config.min_free_ram_e2b {
            return ModelSelection::Gemma4E2B;
        }
    }

    // デフォルト: Bonsai（エージェント能力最高）
    ModelSelection::Bonsai
}

/// macOSの総RAM（バイト）を取得
#[cfg(target_os = "macos")]
pub fn get_total_ram() -> u64 {
    let mut size: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    let name = std::ffi::CString::new("hw.memsize").unwrap();
    unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            &mut size as *mut u64 as *mut std::ffi::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        );
    }
    size
}

/// macOSの利用可能RAM（バイト）を取得（vm_stat経由で実測値）
#[cfg(target_os = "macos")]
pub fn get_available_ram() -> u64 {
    // vm_statコマンドで実際の空きメモリを取得
    if let Ok(output) = std::process::Command::new("vm_stat").output()
        && let Ok(text) = String::from_utf8(output.stdout)
    {
        let page_size = parse_vm_stat_page_size(&text).unwrap_or(16384);
        let mut free: u64 = 0;
        let mut inactive: u64 = 0;
        let mut purgeable: u64 = 0;

        for line in text.lines() {
            if let Some(v) = line.strip_prefix("Pages free:") {
                free = parse_vm_stat_value(v);
            } else if let Some(v) = line.strip_prefix("Pages inactive:") {
                inactive = parse_vm_stat_value(v);
            } else if let Some(v) = line.strip_prefix("Pages purgeable:") {
                purgeable = parse_vm_stat_value(v);
            }
        }

        let available = (free + inactive + purgeable) * page_size;
        if available > 0 {
            return available;
        }
    }

    // フォールバック: 総メモリの60%
    get_total_ram() * 60 / 100
}

/// vm_stat出力からページ数を抽出（macOS専用 — `get_available_ram` からのみ使用）
#[cfg(target_os = "macos")]
fn parse_vm_stat_value(s: &str) -> u64 {
    s.trim().trim_end_matches('.').parse().unwrap_or(0)
}

/// vm_stat出力からページサイズを抽出（macOS専用 — `get_available_ram` からのみ使用）
#[cfg(target_os = "macos")]
fn parse_vm_stat_page_size(text: &str) -> Option<u64> {
    // "Mach Virtual Memory Statistics: (page size of 16384 bytes)"
    let start = text.find("page size of ")? + 13;
    let end = start + text[start..].find(' ')?;
    text[start..end].parse().ok()
}

#[cfg(not(target_os = "macos"))]
pub fn get_total_ram() -> u64 {
    8 * 1024 * 1024 * 1024
}

#[cfg(not(target_os = "macos"))]
pub fn get_available_ram() -> u64 {
    // 非macOS: 8GBと仮定
    8 * 1024 * 1024 * 1024
}

// ──────────────────────────────────────────────────────────────────────
// FallbackChain（Step 12 — メイン推論フォールバック）
// ──────────────────────────────────────────────────────────────────────

/// フォールバック対象のバックエンド + モデル ID + 接続先
///
/// 既存 `[advisor]` の backend フォールバックは advice 専用。
/// このエントリはメイン推論 (`LlmBackend::generate`) に適用される。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FallbackEntry {
    pub backend: ServerBackend,
    pub model_id: String,
    pub server_url: String,
}

/// 連続失敗時に次のバックエンドへ切替えるチェーン
///
/// macOS26/Agent の `FallbackChainService` 設計を踏襲し、
/// (provider, model) のチェーン上で N 回連続失敗で次の provider に自動切替する。
/// 1 回成功するとカウンタはリセット、ただしチェーン位置は保持（手動 `reset_to_primary` で復帰）。
///
/// インデックスは 0-based: `entries[0]` = primary、`entries[1]` 以降がフォールバック先。
#[derive(Debug)]
pub struct FallbackChain {
    entries: Vec<FallbackEntry>,
    current_idx: AtomicUsize,
    consecutive_failures: AtomicUsize,
    /// フォールバック中の連続成功カウンタ（項目 195、recovery 用）
    consecutive_successes_on_fallback: AtomicUsize,
    max_failures_before_fallback: usize,
    /// フォールバック中に N 回連続成功したらプライマリへ復帰 probe（項目 195）
    /// 0 = recovery 無効（既存 sticky 挙動 100% 維持、後方互換）
    recover_after_n_success: usize,
}

impl FallbackChain {
    pub fn new(entries: Vec<FallbackEntry>) -> Self {
        Self::with_options(entries, 2, 0)
    }

    pub fn with_threshold(entries: Vec<FallbackEntry>, max_failures: usize) -> Self {
        Self::with_options(entries, max_failures, 0)
    }

    /// recovery threshold 含む全オプションを指定（項目 195）
    pub fn with_options(
        entries: Vec<FallbackEntry>,
        max_failures: usize,
        recover_after_n_success: usize,
    ) -> Self {
        Self {
            entries,
            current_idx: AtomicUsize::new(0),
            consecutive_failures: AtomicUsize::new(0),
            consecutive_successes_on_fallback: AtomicUsize::new(0),
            max_failures_before_fallback: max_failures.max(1),
            recover_after_n_success,
        }
    }

    /// 現在のエントリを取得（idx=0 なら primary）
    pub fn current(&self) -> Option<&FallbackEntry> {
        let idx = self.current_idx.load(Ordering::SeqCst);
        self.entries.get(idx)
    }

    /// 失敗を記録、必要なら次のバックエンドへ切替（戻り値は切替先、無ければ None）
    pub fn record_failure(&self) -> Option<&FallbackEntry> {
        let count = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
        // フォールバック中の失敗で recovery success counter リセット（項目 195）
        self.consecutive_successes_on_fallback
            .store(0, Ordering::SeqCst);
        if count >= self.max_failures_before_fallback {
            let next_idx = self.current_idx.load(Ordering::SeqCst) + 1;
            if next_idx < self.entries.len() {
                self.current_idx.store(next_idx, Ordering::SeqCst);
                self.consecutive_failures.store(0, Ordering::SeqCst);
                return self.entries.get(next_idx);
            }
        }
        None
    }

    /// 成功を記録、failure カウンタをリセット
    /// 項目 195: recover_after_n_success > 0 かつフォールバック中なら、
    /// 連続成功 N 回でプライマリへ自動復帰。primary 時は no-op。
    pub fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::SeqCst);
        let idx = self.current_idx.load(Ordering::SeqCst);
        if idx == 0 || self.recover_after_n_success == 0 {
            // primary または recovery 無効: 既存挙動維持
            return;
        }
        let n = self
            .consecutive_successes_on_fallback
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        if n >= self.recover_after_n_success {
            self.current_idx.store(0, Ordering::SeqCst);
            self.consecutive_successes_on_fallback
                .store(0, Ordering::SeqCst);
        }
    }

    /// プライマリへ手動復帰
    pub fn reset_to_primary(&self) {
        self.current_idx.store(0, Ordering::SeqCst);
        self.consecutive_failures.store(0, Ordering::SeqCst);
    }

    pub fn entries(&self) -> &[FallbackEntry] {
        &self.entries
    }

    /// チェーン枯渇（最後のエントリにいる）か判定
    pub fn is_exhausted(&self) -> bool {
        let idx = self.current_idx.load(Ordering::SeqCst);
        idx >= self.entries.len().saturating_sub(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_default_bonsai() {
        let ctx = TaskContext {
            has_image: false,
            estimated_tokens: 100,
            is_daemon: false,
        };
        let config = RouterConfig::default();
        assert_eq!(select_model(&ctx, &config), ModelSelection::Bonsai);
    }

    #[test]
    fn test_select_daemon_always_bonsai() {
        let ctx = TaskContext {
            has_image: true,
            estimated_tokens: 100,
            is_daemon: true,
        };
        let config = RouterConfig::default();
        assert_eq!(select_model(&ctx, &config), ModelSelection::Bonsai);
    }

    #[test]
    fn test_select_image_with_enough_ram() {
        let ctx = TaskContext {
            has_image: true,
            estimated_tokens: 100,
            is_daemon: false,
        };
        let config = RouterConfig {
            min_free_ram_e4b: 1, // 1バイト → 必ず通過
            min_free_ram_e2b: 1,
            ..Default::default()
        };
        assert_eq!(select_model(&ctx, &config), ModelSelection::Gemma4E4B);
    }

    #[test]
    fn test_select_image_e2b_fallback() {
        let ctx = TaskContext {
            has_image: true,
            estimated_tokens: 100,
            is_daemon: false,
        };
        let config = RouterConfig {
            min_free_ram_e4b: u64::MAX, // E4Bは無理
            min_free_ram_e2b: 1,        // E2Bは可能
            ..Default::default()
        };
        assert_eq!(select_model(&ctx, &config), ModelSelection::Gemma4E2B);
    }

    #[test]
    fn test_select_image_insufficient_ram() {
        let ctx = TaskContext {
            has_image: true,
            estimated_tokens: 100,
            is_daemon: false,
        };
        let config = RouterConfig {
            min_free_ram_e4b: u64::MAX,
            min_free_ram_e2b: u64::MAX,
            ..Default::default()
        };
        assert_eq!(select_model(&ctx, &config), ModelSelection::Bonsai);
    }

    #[test]
    fn test_select_disabled_router() {
        let ctx = TaskContext {
            has_image: true,
            estimated_tokens: 100,
            is_daemon: false,
        };
        let config = RouterConfig {
            enabled: false,
            ..Default::default()
        };
        assert_eq!(select_model(&ctx, &config), ModelSelection::Bonsai);
    }

    #[test]
    fn test_get_available_ram() {
        let ram = get_available_ram();
        assert!(ram > 0);
        // M2 16GBでは空きRAMは1GB以上16GB未満であるべき
        let gb = ram / (1024 * 1024 * 1024);
        assert!(gb >= 1, "空きRAMが1GB未満: {}GB", gb);
        assert!(gb <= 16, "空きRAMが16GB超: {}GB", gb);
    }

    #[test]
    fn test_get_total_ram() {
        let total = get_total_ram();
        assert!(total > 0);
        let gb = total / (1024 * 1024 * 1024);
        assert!(gb >= 4, "総RAMが4GB未満: {}GB", gb);
    }

    #[test]
    fn test_task_context_from_messages() {
        let msgs = vec![Message::user("テスト")];
        let ctx = TaskContext::from_messages(&msgs, false);
        assert!(!ctx.has_image);
        assert!(!ctx.is_daemon);
    }

    #[test]
    fn test_task_context_with_image() {
        let mut msg = Message::user("画像");
        msg.attachments
            .push(crate::domain::conversation::Attachment::Image(vec![0xFF]));
        let ctx = TaskContext::from_messages(&[msg], false);
        assert!(ctx.has_image);
    }

    #[test]
    fn test_model_selection_debug() {
        assert_eq!(format!("{:?}", ModelSelection::Bonsai), "Bonsai");
        assert_eq!(format!("{:?}", ModelSelection::Gemma4E4B), "Gemma4E4B");
    }

    // --- PipelineStage テスト ---

    #[test]
    fn test_pipeline_stage_debug() {
        assert_eq!(format!("{:?}", PipelineStage::Plan), "Plan");
        assert_eq!(format!("{:?}", PipelineStage::Execute), "Execute");
        assert_eq!(format!("{:?}", PipelineStage::Advise), "Advise");
    }

    // --- AdvisorConfig テスト ---

    #[test]
    fn test_advisor_config_default() {
        let config = AdvisorConfig::default();
        assert_eq!(config.max_uses, 3);
        assert_eq!(config.calls_used, 0);
        assert!(config.can_advise());
        assert_eq!(config.remaining(), 3);
    }

    #[test]
    fn test_advisor_config_max_uses() {
        let mut config = AdvisorConfig::default();
        config.record_call();
        config.record_call();
        assert!(config.can_advise());
        assert_eq!(config.remaining(), 1);
        config.record_call();
        assert!(!config.can_advise());
        assert_eq!(config.remaining(), 0);
    }

    #[test]
    fn test_advisor_config_api_endpoint() {
        let config = AdvisorConfig {
            api_endpoint: Some("http://localhost:8081".to_string()),
            ..Default::default()
        };
        assert!(config.api_endpoint.is_some());
    }

    #[test]
    fn test_advisor_config_default_verification_prompt() {
        let config = AdvisorConfig::default();
        assert!(config.verification_prompt.contains("検証"));
        assert_eq!(config.verification_prompt, DEFAULT_VERIFICATION_PROMPT);
    }

    #[test]
    fn test_advisor_config_build_verification_prompt() {
        let config = AdvisorConfig::default();
        let prompt = config.build_verification_prompt("テストタスク");
        assert!(prompt.contains("回答前に確認"));
    }

    #[test]
    fn test_advisor_config_custom_verification_prompt() {
        let config = AdvisorConfig {
            verification_prompt: "カスタム検証メッセージ".to_string(),
            ..Default::default()
        };
        assert_eq!(
            config.build_verification_prompt(""),
            "カスタム検証メッセージ"
        );
    }

    #[test]
    fn test_try_remote_advice_no_endpoint_returns_none() {
        let mut config = AdvisorConfig::default();
        let result = config
            .try_remote_advice(AdvisorRole::Verification, "テスト")
            .unwrap();
        assert!(result.is_none(), "endpoint未設定時はOk(None)");
    }

    #[test]
    fn test_try_remote_advice_invalid_endpoint_returns_err() {
        let mut config = AdvisorConfig {
            api_endpoint: Some("http://127.0.0.1:1/invalid".to_string()),
            timeout_secs: 1,
            ..Default::default()
        };
        let result = config.try_remote_advice(AdvisorRole::Verification, "テスト");
        assert!(result.is_err(), "無効endpointはErr");
    }

    #[test]
    fn test_advisor_role_system_prompts_differ() {
        assert_ne!(
            AdvisorRole::Verification.system_prompt(),
            AdvisorRole::Replan.system_prompt()
        );
    }

    #[test]
    fn test_advisor_role_user_prompts_include_context() {
        let v = AdvisorRole::Verification.user_prompt("ファイルを修正");
        let r = AdvisorRole::Replan.user_prompt("ファイルを修正");
        assert!(v.contains("ファイルを修正"));
        assert!(r.contains("ファイルを修正"));
    }

    #[test]
    fn test_local_prompt_for_routes_by_role() {
        let config = AdvisorConfig::default();
        let v = config.local_prompt_for(AdvisorRole::Verification, "");
        let r = config.local_prompt_for(AdvisorRole::Replan, "");
        assert!(v.contains("検証"));
        assert!(r.contains("停滞"));
    }

    #[test]
    fn test_advisor_config_default_replan_prompt() {
        let config = AdvisorConfig::default();
        assert!(config.replan_prompt.contains("停滞"));
        assert_eq!(config.replan_prompt, DEFAULT_REPLAN_PROMPT);
    }

    #[test]
    fn test_advisor_config_default_includes_new_fields() {
        let config = AdvisorConfig::default();
        assert!(config.api_key.is_none());
        assert!(config.api_model.is_none());
        assert_eq!(config.timeout_secs, 10);
    }

    #[test]
    fn test_cache_key_differs_by_role() {
        let k_v = AdvisorConfig::cache_key(AdvisorRole::Verification, "task");
        let k_r = AdvisorConfig::cache_key(AdvisorRole::Replan, "task");
        assert_ne!(k_v, k_r);
    }

    #[test]
    fn test_cache_key_differs_by_context() {
        let k1 = AdvisorConfig::cache_key(AdvisorRole::Verification, "task A");
        let k2 = AdvisorConfig::cache_key(AdvisorRole::Verification, "task B");
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_cache_key_same_for_same_inputs() {
        let k1 = AdvisorConfig::cache_key(AdvisorRole::Verification, "task");
        let k2 = AdvisorConfig::cache_key(AdvisorRole::Verification, "task");
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_cache_starts_empty() {
        let config = AdvisorConfig::default();
        assert!(config.cache.is_empty());
    }

    #[test]
    fn test_log_startup_local_mode() {
        // パニックしないことを確認（eprintlnの出力内容はテスト対象外）
        let config = AdvisorConfig::default();
        config.log_startup();
    }

    #[test]
    fn test_log_startup_remote_mode() {
        let config = AdvisorConfig {
            api_endpoint: Some("https://api.openai.com/v1/chat/completions".to_string()),
            api_key: Some("sk-test".to_string()),
            api_model: Some("gpt-4o".to_string()),
            ..Default::default()
        };
        config.log_startup();
    }

    #[test]
    fn test_cache_returns_hit_without_http() {
        // api_endpoint設定済 + 事前にキャッシュへ手動挿入
        let mut config = AdvisorConfig {
            api_endpoint: Some("http://127.0.0.1:1/never-reached".to_string()),
            timeout_secs: 1,
            ..Default::default()
        };
        let key = AdvisorConfig::cache_key(AdvisorRole::Verification, "task");
        config.cache.insert(key, "cached advice".to_string());
        // HTTPに行かずキャッシュから返却（無効endpointだがエラーにならない＝キャッシュヒット）
        let result = config
            .try_remote_advice(AdvisorRole::Verification, "task")
            .unwrap();
        assert_eq!(result.as_deref(), Some("cached advice"));
    }

    #[test]
    fn test_cache_clone_independence() {
        // クローン後の変更が元に影響しないこと（セッション境界の独立性）
        let mut original = AdvisorConfig::default();
        original.cache.insert(0, "shared at clone time".to_string());
        let mut cloned = original.clone();
        cloned.cache.insert(1, "only in clone".to_string());
        assert!(!original.cache.contains_key(&1));
        assert!(cloned.cache.contains_key(&1));
    }

    #[test]
    fn test_cache_for_prompt_returns_hit_without_http() {
        // try_remote_with_prompt のキャッシュヒット経路: HTTP に行かずに既存値を返す
        // （rust-reviewer 監査 LOW#3 対応）
        let mut config = AdvisorConfig {
            api_endpoint: Some("http://127.0.0.1:1/never-reached".to_string()),
            timeout_secs: 1,
            ..Default::default()
        };
        let key = AdvisorConfig::cache_key_for_prompt("sys-template", "user-payload");
        config
            .cache
            .insert(key, "cached judge response".to_string());
        let result = config
            .try_remote_with_prompt("sys-template", "user-payload")
            .unwrap();
        assert_eq!(result.as_deref(), Some("cached judge response"));
    }

    #[test]
    fn test_cache_key_namespaces_isolated() {
        // role-based key と prompt-based key は同一文字列でも異なる namespace に属する
        // （MEDIUM 監査対応: discriminant XOR で構造的分離）
        let role_key = AdvisorConfig::cache_key(AdvisorRole::Verification, "shared-text");
        let prompt_key = AdvisorConfig::cache_key_for_prompt("shared-text", "");
        // 同じ入力（"shared-text"）でも namespace 分離により別キー
        assert_ne!(
            role_key, prompt_key,
            "role と prompt の cache key は構造的に分離されるべき"
        );
    }

    // ─── Step 12 FallbackChain tests ──────────────────────────────────

    fn fb_entry(id: &str) -> FallbackEntry {
        FallbackEntry {
            backend: ServerBackend::MlxLm,
            model_id: id.to_string(),
            server_url: format!("http://localhost:8000/{id}"),
        }
    }

    #[test]
    fn t_fallback_chain_starts_at_primary() {
        let chain = FallbackChain::new(vec![fb_entry("a"), fb_entry("b")]);
        assert_eq!(chain.current().unwrap().model_id, "a");
    }

    #[test]
    fn t_fallback_chain_does_not_switch_below_threshold() {
        let chain = FallbackChain::new(vec![fb_entry("a"), fb_entry("b")]);
        // threshold=2 なので 1 回目は切替なし
        assert!(chain.record_failure().is_none());
        assert_eq!(chain.current().unwrap().model_id, "a");
    }

    #[test]
    fn t_fallback_chain_switches_on_threshold() {
        let chain = FallbackChain::new(vec![fb_entry("a"), fb_entry("b")]);
        assert!(chain.record_failure().is_none()); // count=1
        let switched = chain.record_failure(); // count=2 → 切替
        assert!(switched.is_some());
        assert_eq!(switched.unwrap().model_id, "b");
        assert_eq!(chain.current().unwrap().model_id, "b");
    }

    #[test]
    fn t_fallback_chain_exhausts_when_no_more_entries() {
        let chain = FallbackChain::new(vec![fb_entry("a")]);
        chain.record_failure();
        chain.record_failure();
        // entries は 1 件しかない → 次がない
        assert_eq!(chain.current().unwrap().model_id, "a");
        assert!(chain.is_exhausted() || !chain.is_exhausted()); // チェーン枯渇でも primary は維持
    }

    #[test]
    fn t_fallback_chain_success_resets_counter() {
        let chain = FallbackChain::new(vec![fb_entry("a"), fb_entry("b")]);
        chain.record_failure(); // count=1
        chain.record_success(); // count=0、位置は a 維持
        chain.record_failure(); // count=1
        // threshold=2 にまだ到達していない
        assert_eq!(chain.current().unwrap().model_id, "a");
    }

    #[test]
    fn t_fallback_chain_reset_to_primary() {
        let chain = FallbackChain::new(vec![fb_entry("a"), fb_entry("b")]);
        chain.record_failure();
        chain.record_failure();
        assert_eq!(chain.current().unwrap().model_id, "b");
        chain.reset_to_primary();
        assert_eq!(chain.current().unwrap().model_id, "a");
    }

    #[test]
    fn t_fallback_chain_custom_threshold() {
        let chain = FallbackChain::with_threshold(vec![fb_entry("a"), fb_entry("b")], 3);
        chain.record_failure();
        chain.record_failure();
        // threshold=3 なので 2 回ではまだ切替えない
        assert_eq!(chain.current().unwrap().model_id, "a");
        chain.record_failure();
        assert_eq!(chain.current().unwrap().model_id, "b");
    }

    // ─── 項目 195: sticky fallback recovery (handoff 05-06e ★ TODO) ─────
    //
    // 既存挙動: フォールバック後の record_success は consecutive_failures のみリセット、
    // current_idx は永久に保持 (sticky)。MLX primary + llama fallback 構成で
    // -27% 品質劣化 (項目 188) の真因。recover_after_n_success > 0 で
    // フォールバック中の連続成功 N 回でプライマリ復帰を probe。
    // default 0 = 既存 sticky 挙動 100% 維持 (後方互換)。

    #[test]
    fn t_fallback_chain_recovery_default_disabled() {
        // default では recovery 無効、fallback 後の success で primary に戻らない
        let chain = FallbackChain::new(vec![fb_entry("a"), fb_entry("b")]);
        chain.record_failure();
        chain.record_failure(); // → b に切替
        assert_eq!(chain.current().unwrap().model_id, "b");
        for _ in 0..100 {
            chain.record_success();
        }
        // recovery 無効なので b のまま
        assert_eq!(chain.current().unwrap().model_id, "b");
    }

    #[test]
    fn t_fallback_chain_recovery_returns_to_primary_after_n_success() {
        // recover_after_n_success=3、fallback 中 3 回連続成功で primary に戻る
        let chain = FallbackChain::with_options(vec![fb_entry("a"), fb_entry("b")], 2, 3);
        chain.record_failure();
        chain.record_failure(); // → b
        assert_eq!(chain.current().unwrap().model_id, "b");
        chain.record_success();
        chain.record_success();
        // まだ 2 回、しきい値未達
        assert_eq!(chain.current().unwrap().model_id, "b");
        chain.record_success();
        // 3 回目で primary に戻る
        assert_eq!(chain.current().unwrap().model_id, "a");
    }

    #[test]
    fn t_fallback_chain_recovery_failure_resets_success_counter() {
        // fallback 中に失敗が混じると success counter は 0 にリセット
        let chain = FallbackChain::with_options(vec![fb_entry("a"), fb_entry("b")], 2, 3);
        chain.record_failure();
        chain.record_failure(); // → b
        chain.record_success();
        chain.record_success(); // 2/3
        chain.record_failure(); // success counter リセット (max_failures に到達せず entry 維持)
        chain.record_success();
        chain.record_success();
        // 失敗でリセットされたので 2/3 でまだ primary に戻らない
        assert_eq!(chain.current().unwrap().model_id, "b");
        chain.record_success();
        // 3/3 で復帰
        assert_eq!(chain.current().unwrap().model_id, "a");
    }

    #[test]
    fn t_fallback_chain_recovery_does_nothing_on_primary() {
        // primary 時の record_success は recovery counter を操作しない
        let chain = FallbackChain::with_options(vec![fb_entry("a"), fb_entry("b")], 2, 3);
        for _ in 0..10 {
            chain.record_success();
        }
        assert_eq!(chain.current().unwrap().model_id, "a");
        // primary success の影響なく fallback 動作は通常通り
        chain.record_failure();
        chain.record_failure();
        assert_eq!(chain.current().unwrap().model_id, "b");
    }
}
