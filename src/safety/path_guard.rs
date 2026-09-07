use std::path::{Component, Path, PathBuf};

/// アクセス禁止パスのリストを管理するガード
#[derive(Debug, Clone)]
pub struct PathGuard {
    deny_paths: Vec<String>,
}

impl PathGuard {
    pub fn new(deny_paths: Vec<String>) -> Self {
        Self { deny_paths }
    }

    /// デフォルトのアクセス禁止パスリスト
    pub fn default_deny_list() -> Self {
        Self::new(vec![
            "~/.ssh".to_string(),
            "~/.gnupg".to_string(),
            "~/.aws".to_string(),
            "/etc/shadow".to_string(),
            "/etc/passwd".to_string(),
            ".env".to_string(),
            ".env.local".to_string(),
            ".env.production".to_string(),
            "id_rsa".to_string(),
            "id_ed25519".to_string(),
        ])
    }

    /// 指定されたパスがアクセス禁止対象かチェックする
    ///
    /// 既存のファイル/ディレクトリは `canonicalize()` を通してシンボリックリンクや `..` を解決して照合する。
    /// 未存在のパスの場合は論理正規化（`..` や `.` の解消）およびプレフィックス・ファイル名一致で検査する。
    pub fn is_denied(&self, path_str: &str) -> bool {
        let expanded = expand_tilde(path_str);
        let path = Path::new(&expanded);

        // 1. 実ファイルが存在する場合は canonicalize して実体パスで照合
        if let Ok(canonical) = path.canonicalize()
            && self.matches_denied_resolved(&canonical)
        {
            return true;
        }

        // 2. カレントディレクトリを前置した絶対パスを生成
        let absolute_path = if path.is_absolute() {
            path.to_path_buf()
        } else if let Ok(cwd) = std::env::current_dir() {
            cwd.join(path)
        } else {
            path.to_path_buf()
        };

        // 3. 絶対パスの実体解決 (存在する場合)
        if let Ok(canonical) = absolute_path.canonicalize()
            && self.matches_denied_resolved(&canonical)
        {
            return true;
        }

        // 4. 論理パス（正規化絶対パス）で照合
        let normalized = normalize_path(&absolute_path);
        if self.matches_denied_resolved(&normalized) {
            return true;
        }

        // 5. 元の論理パスでも照合
        let orig_normalized = normalize_path(path);
        if self.matches_denied_resolved(&orig_normalized) {
            return true;
        }

        // 6. ファイル名単体での重要機密一致（例: ".env", "id_rsa"）
        if let Some(file_name) = path.file_name().and_then(|f| f.to_str()) {
            for deny in &self.deny_paths {
                if !deny.contains('/') && file_name == deny.as_str() {
                    return true;
                }
                if deny.starts_with(".env") && file_name.starts_with(".env") {
                    return true;
                }
            }
        }

        false
    }

    fn matches_denied_resolved(&self, check_path: &Path) -> bool {
        for deny in &self.deny_paths {
            let expanded_deny = expand_tilde(deny);
            let deny_path = Path::new(&expanded_deny);
            let norm_deny = normalize_path(deny_path);
            let canon_deny = deny_path.canonicalize().ok();

            // 候補となる deny パス一覧 (論理パス + 実体パス)
            let candidates = [Some(&norm_deny), canon_deny.as_ref()];

            for candidate in candidates.into_iter().flatten() {
                // ディレクトリ階層としての前方一致チェック
                if check_path.starts_with(candidate) {
                    return true;
                }

                // 完全一致
                if check_path == candidate {
                    return true;
                }

                // ファイル名完全一致チェック（単体ファイル名が deny リストに含まれる場合）
                if let Some(deny_name) = candidate.file_name().and_then(|f| f.to_str())
                    && let Some(check_name) = check_path.file_name().and_then(|f| f.to_str())
                    && deny_name == check_name
                    && (!deny.contains('/') || check_path == candidate)
                {
                    return true;
                }
            }
        }
        false
    }
}

/// `~` をホームディレクトリに展開
pub fn expand_tilde(path: &str) -> String {
    if (path == "~" || path.starts_with("~/"))
        && let Some(home) = std::env::var_os("HOME")
    {
        let home_str = home.to_string_lossy();
        if path == "~" {
            return home_str.to_string();
        }
        return format!("{}{}", home_str, &path[1..]);
    }
    path.to_string()
}

/// パスから `.` や `..` を論理的に解決して正規化
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => normalized.push(Component::Prefix(p)),
            Component::RootDir => normalized.push(Component::RootDir),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(c) => normalized.push(c),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_deny_list_ssh() {
        let guard = PathGuard::default_deny_list();
        let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/test".to_string());
        assert!(guard.is_denied("~/.ssh/id_rsa"));
        assert!(guard.is_denied(&format!("{}/.ssh/config", home)));
    }

    #[test]
    fn test_default_deny_list_etc_shadow() {
        let guard = PathGuard::default_deny_list();
        assert!(guard.is_denied("/etc/shadow"));
        assert!(guard.is_denied("/etc/passwd"));
        assert!(!guard.is_denied("/etc/hosts"));
    }

    #[test]
    fn test_dotenv_denied() {
        let guard = PathGuard::default_deny_list();
        assert!(guard.is_denied(".env"));
        assert!(guard.is_denied("./.env"));
        assert!(guard.is_denied("/path/to/.env"));
        assert!(guard.is_denied("/path/to/.env.local"));
        assert!(guard.is_denied("/path/to/.env.production"));
    }

    #[test]
    fn test_path_traversal_denied() {
        let guard = PathGuard::default_deny_list();
        // ルートまで確実に抜けるトラバーサル
        assert!(guard.is_denied("../../../../../../../etc/passwd"));
        assert!(guard.is_denied("./sub/../../../../../../etc/passwd"));
        assert!(guard.is_denied("/var/log/../../etc/shadow"));
    }

    #[test]
    fn test_safe_path_allowed() {
        let guard = PathGuard::default_deny_list();
        assert!(!guard.is_denied("src/main.rs"));
        assert!(!guard.is_denied("/tmp/safe.txt"));
    }
}
