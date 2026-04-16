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
    pub api_endpoint: Option<String>,
    /// 検証プロンプト（将来: api_endpoint設定時に外部アドバイザーへ差し替え可能）
    pub verification_prompt: String,
}

/// デフォルトの完了前自己検証プロンプト
pub const DEFAULT_VERIFICATION_PROMPT: &str =
    "回答前に検証: 目標を達成できていますか？不足があれば補完してください。問題なければ回答に[検証済]を含めてください。";

impl Default for AdvisorConfig {
    fn default() -> Self {
        Self {
            max_uses: 3,
            calls_used: 0,
            max_advisor_tokens: 700,
            api_endpoint: None,
            verification_prompt: DEFAULT_VERIFICATION_PROMPT.to_string(),
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

    /// 検証プロンプトを構築
    /// 将来: api_endpoint が設定されている場合は外部アドバイザーへの問い合わせ結果を返す想定
    pub fn build_verification_prompt(&self, _task_context: &str) -> String {
        // TODO: api_endpoint が Some の場合は HTTP POST で外部アドバイザーに問い合わせ
        self.verification_prompt.clone()
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
}
