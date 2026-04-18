use crate::observability::logger::{log_event, LogLevel};
use std::collections::HashMap;

use crate::agent::conversation::Message;

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
    /// セッション内キャッシュ（同一role+task_contextの重複API呼出を回避）
    /// キー: hash(role, task_context)、値: 外部APIのレスポンス本文
    /// セッションごとにクローンされるため、セッション境界で自動リセット
    #[doc(hidden)]
    pub cache: HashMap<u64, String>,
}

/// デフォルトの完了前自己検証プロンプト
pub const DEFAULT_VERIFICATION_PROMPT: &str =
    "回答前に検証: 目標を達成できていますか？不足があれば補完してください。問題なければ回答に[検証済]を含めてください。";

/// デフォルトの停滞時再計画プロンプト
pub const DEFAULT_REPLAN_PROMPT: &str =
    "停滞検出: これまでのアプローチでは進捗していません。\n<think>内で別アプローチを再計画:\n1. 失敗の根本原因\n2. 別ツール/別手順\n3. 簡潔に再開";

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

impl AdvisorRole {
    fn system_prompt(self) -> &'static str {
        match self {
            Self::Verification => {
                "あなたは1bitローカルLLMの戦略アドバイザーです。                 実行者が回答を出す前に、不足や検証ポイントを100語以内・列挙形式で提示してください。"
            }
            Self::Replan => {
                "あなたは1bitローカルLLMの戦略アドバイザーです。                 実行者が停滞しています。別アプローチを100語以内・列挙形式で:                 1. 失敗の根本原因 2. 別ツール/別手順 3. 次の具体的アクション。"
            }
        }
    }

    fn user_prompt(self, task_context: &str) -> String {
        match self {
            Self::Verification => format!("タスク: {task_context}\n\n上記の検証指示を簡潔に。"),
            Self::Replan => format!("タスク: {task_context}\n\nこれまでが停滞中。別の手段を提案してください。"),
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
            cache: HashMap::new(),
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
            let key_status = if self.api_key.is_some() { "設定済" } else { "未設定(env検出)" };
            log_event(LogLevel::Info, "advisor", &format!("リモートモード: endpoint={}, model={}, key={}, max_uses={}, timeout={}s",
                endpoint, model, key_status, self.max_uses, self.timeout_secs
            ));
        } else if self.backend == AdvisorBackend::ClaudeCode {
            log_event(LogLevel::Info, "advisor", &format!("Claude Codeモード (max_uses={}, claude -p経由)", self.max_uses));
        } else {
            log_event(LogLevel::Info, "advisor", &format!("ローカルモード (max_uses={}, 検証+再計画)", self.max_uses));
        }
    }

    /// キャッシュキーを計算（role + task_context のハッシュ）
    fn cache_key(role: AdvisorRole, task_context: &str) -> u64 {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut h = DefaultHasher::new();
        (role as u8).hash(&mut h);
        task_context.hash(&mut h);
        h.finish()
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

制約: 100語以内、列挙形式で簡潔に回答。",
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

/// macOSの利用可能RAM（バイト）を取得
#[cfg(target_os = "macos")]
pub fn get_available_ram() -> u64 {
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

    // 空きメモリの概算: 総メモリの60%を利用可能と仮定
    // （正確にはvm_statisticsを使うが、簡易実装）
    size * 60 / 100
}

#[cfg(not(target_os = "macos"))]
pub fn get_available_ram() -> u64 {
    // 非macOS: 8GBと仮定
    8 * 1024 * 1024 * 1024
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
            .push(crate::agent::conversation::Attachment::Image(vec![0xFF]));
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
        assert!(prompt.contains("回答前に検証"));
    }

    #[test]
    fn test_advisor_config_custom_verification_prompt() {
        let config = AdvisorConfig {
            verification_prompt: "カスタム検証メッセージ".to_string(),
            ..Default::default()
        };
        assert_eq!(config.build_verification_prompt(""), "カスタム検証メッセージ");
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
        original
            .cache
            .insert(0, "shared at clone time".to_string());
        let mut cloned = original.clone();
        cloned.cache.insert(1, "only in clone".to_string());
        assert!(!original.cache.contains_key(&1));
        assert!(cloned.cache.contains_key(&1));
    }
}
