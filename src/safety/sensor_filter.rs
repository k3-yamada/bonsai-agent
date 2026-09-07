//! 外部知覚センサー用プライバシーフィルタ
//!
//! ウィンドウタイトルやファイルパスに含まれるパスワード、機微情報、
//! 認証情報（.ssh, .env, keychain等）を検知し、統合記憶への混入を事前に遮断する。

use std::path::Path;

pub struct PrivacyFilter {
    blocked_path_keywords: Vec<String>,
    blocked_title_keywords: Vec<String>,
}

impl PrivacyFilter {
    pub fn new() -> Self {
        Self {
            blocked_path_keywords: vec![
                ".env".to_string(),
                ".ssh".to_string(),
                ".gnupg".to_string(),
                "id_rsa".to_string(),
                "id_ed25519".to_string(),
                "credentials".to_string(),
                "secrets".to_string(),
                "keychain".to_string(),
                "token".to_string(),
                "password".to_string(),
            ],
            blocked_title_keywords: vec![
                "1password".to_string(),
                "bitwarden".to_string(),
                "keychain".to_string(),
                "keepass".to_string(),
                "login".to_string(),
                "signin".to_string(),
                "bank".to_string(),
                "password".to_string(),
            ],
        }
    }

    /// ファイルパスが安全かどうかを判定。機微キーワードを含む場合は false を返す。
    pub fn is_path_safe(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy().to_lowercase();
        for keyword in &self.blocked_path_keywords {
            if path_str.contains(keyword) {
                return false;
            }
        }
        true
    }

    /// ウィンドウタイトルが安全かどうかを判定。
    pub fn is_window_title_safe(&self, title: &str) -> bool {
        let title_lower = title.to_lowercase();
        for keyword in &self.blocked_title_keywords {
            if title_lower.contains(keyword) {
                return false;
            }
        }
        true
    }
}

impl Default for PrivacyFilter {
    fn default() -> Self {
        Self::new()
    }
}

/// アプリ名・バンドル識別子ベースの除外リスト (AppDenylist)
#[derive(Debug, Clone)]
pub struct AppDenylist {
    blocked_apps: Vec<String>,
}

impl AppDenylist {
    pub fn new(custom_denylist: &[String]) -> Self {
        let mut list = vec![
            "1password".to_string(),
            "bitwarden".to_string(),
            "keychain".to_string(),
            "keepass".to_string(),
            "bank".to_string(),
            "com.agilebits.onepassword".to_string(),
            "com.bitwarden.desktop".to_string(),
            "com.apple.keychainaccess".to_string(),
        ];
        for item in custom_denylist {
            let lower = item.trim().to_lowercase();
            if !lower.is_empty() && !list.contains(&lower) {
                list.push(lower);
            }
        }
        Self { blocked_apps: list }
    }

    /// 指定されたアプリ名またはバンドルIDがブロック対象かどうかを判定
    pub fn is_blocked(&self, app_identifier: &str) -> bool {
        let lower = app_identifier.trim().to_lowercase();
        for blocked in &self.blocked_apps {
            if lower.contains(blocked) {
                return true;
            }
        }
        false
    }
}

impl Default for AppDenylist {
    fn default() -> Self {
        Self::new(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_privacy_filter_blocks_sensitive_paths() {
        let filter = PrivacyFilter::new();

        assert!(!filter.is_path_safe(&PathBuf::from("/Users/alice/.ssh/id_rsa")));
        assert!(!filter.is_path_safe(&PathBuf::from("/project/.env")));
        assert!(!filter.is_path_safe(&PathBuf::from("/project/.env.local")));
        assert!(!filter.is_path_safe(&PathBuf::from("/secrets/tokens.json")));
        assert!(!filter.is_path_safe(&PathBuf::from("/data/credentials.xml")));

        // 安全なパス
        assert!(filter.is_path_safe(&PathBuf::from("/Users/alice/documents/diary/entry.md")));
        assert!(filter.is_path_safe(&PathBuf::from("/src/main.rs")));
    }

    #[test]
    fn test_privacy_filter_blocks_sensitive_window_titles() {
        let filter = PrivacyFilter::new();

        assert!(!filter.is_window_title_safe("1Password - Vault"));
        assert!(!filter.is_window_title_safe("Bitwarden Password Manager"));
        assert!(!filter.is_window_title_safe("Online Banking Sign In"));

        // 安全なタイトル
        assert!(filter.is_window_title_safe("Visual Studio Code - main.rs"));
        assert!(filter.is_window_title_safe("Terminal - zsh"));
    }

    #[test]
    fn test_app_denylist_blocks_sensitive_apps() {
        let denylist = AppDenylist::default();

        assert!(denylist.is_blocked("1Password 7"));
        assert!(denylist.is_blocked("com.agilebits.onepassword7"));
        assert!(denylist.is_blocked("Bitwarden"));
        assert!(denylist.is_blocked("Apple Keychain Access"));
        assert!(denylist.is_blocked("Chase Bank"));

        // 安全なアプリ
        assert!(!denylist.is_blocked("com.apple.Terminal"));
        assert!(!denylist.is_blocked("com.microsoft.VSCode"));
        assert!(!denylist.is_blocked("Google Chrome"));
        assert!(!denylist.is_blocked("Slack"));
    }

    #[test]
    fn test_app_denylist_with_custom_items() {
        let custom = vec!["custom_secret_app".to_string()];
        let denylist = AppDenylist::new(&custom);

        assert!(denylist.is_blocked("custom_secret_app"));
        assert!(denylist.is_blocked("1Password"));
        assert!(!denylist.is_blocked("Terminal"));
    }
}
