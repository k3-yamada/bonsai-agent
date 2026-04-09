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
    pub min_free_ram_e4b: u64,   // E4Bに必要な空きRAM（バイト）
    pub min_free_ram_e2b: u64,   // E2Bに必要な空きRAM（バイト）
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
        msg.attachments.push(crate::agent::conversation::Attachment::Image(vec![0xFF]));
        let ctx = TaskContext::from_messages(&[msg], false);
        assert!(ctx.has_image);
    }

    #[test]
    fn test_model_selection_debug() {
        assert_eq!(format!("{:?}", ModelSelection::Bonsai), "Bonsai");
        assert_eq!(format!("{:?}", ModelSelection::Gemma4E4B), "Gemma4E4B");
    }
}
