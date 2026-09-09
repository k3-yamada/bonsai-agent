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

    /// 設定されているアクセス禁止パスの参照を返す
    pub fn deny_paths(&self) -> &[String] {
        &self.deny_paths
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

    /// シェルコマンド内のトークン（引数、リダイレクト先、パス表記）を走査し、
    /// アクセス禁止パスに抵触するものを検出してリストとして返却する
    pub fn find_denied_paths_in_command(&self, command: &str) -> Vec<String> {
        let tokens = extract_shell_tokens(command);
        let mut denied = Vec::new();

        for token in tokens {
            let candidates = extract_candidates_from_token(&token);
            for cand in candidates {
                if self.is_denied(&cand) && !denied.contains(&cand) {
                    denied.push(cand);
                }
            }
        }
        denied
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

/// シェルコマンドから主要なトークン（コマンド名、引数、リダイレクト先など）を抽出
fn extract_shell_tokens(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut chars = command.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
            }
            _ if in_single_quote || in_double_quote => {
                current.push(c);
            }
            ' ' | '\t' | '\r' | '\n' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            '|' | ';' | '&' | '(' | ')' | '`' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            '>' | '<' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                if chars.peek() == Some(&'>') {
                    chars.next();
                }
            }
            _ => {
                current.push(c);
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// ANSI-C クォート ($'...') のエスケープシーケンスを展開
fn unescape_ansic(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('a') => out.push('\x07'),
                Some('b') => out.push('\x08'),
                Some('e') | Some('E') => out.push('\x1B'),
                Some('f') => out.push('\x0C'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('v') => out.push('\x0B'),
                Some('\\') => out.push('\\'),
                Some('\'') => out.push('\''),
                Some('"') => out.push('"'),
                Some('x') => {
                    // \xHH 16進数バイト
                    let mut hex = String::new();
                    for _ in 0..2 {
                        if let Some(&h) = chars.peek() {
                            if h.is_ascii_hexdigit() {
                                hex.push(h);
                                chars.next();
                            } else {
                                break;
                            }
                        }
                    }
                    if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                        out.push(byte as char);
                    }
                }
                Some(oct) if ('0'..='7').contains(&oct) => {
                    // \OOO 8進数バイト
                    let mut octal = String::from(oct);
                    for _ in 0..2 {
                        if let Some(&o) = chars.peek() {
                            if ('0'..='7').contains(&o) {
                                octal.push(o);
                                chars.next();
                            } else {
                                break;
                            }
                        }
                    }
                    if let Ok(byte) = u8::from_str_radix(&octal, 8) {
                        out.push(byte as char);
                    }
                }
                Some(other) => {
                    out.push(other);
                }
                None => {
                    out.push('\\');
                }
            }
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// 単一トークンからパス照合候補を抽出（クォート除去、フラグ右辺、空白サブトークン、ANSI-C展開）
fn extract_candidates_from_token(token: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let cleaned = token.trim_matches(['\'', '"', '(', ')', '[', ']', '{', '}', ';', ',', '`']);
    if !cleaned.is_empty() {
        candidates.push(cleaned.to_string());
    }

    // ANSI-C quoting ($'/path' or $"...") のプレフィックス '$' 除去およびエスケープ解除
    if cleaned.starts_with('$') {
        let stripped = cleaned
            .trim_start_matches('$')
            .trim_matches(['\'', '"', '`']);
        if !stripped.is_empty() {
            if !candidates.contains(&stripped.to_string()) {
                candidates.push(stripped.to_string());
            }
            if stripped.contains('\\')
                && let Some(unescaped) = unescape_ansic(stripped)
                && !unescaped.is_empty()
                && !candidates.contains(&unescaped)
            {
                candidates.push(unescaped);
            }
        }
    }

    // --key=value や VAR=value の右辺
    if let Some((_, val)) = cleaned.split_once('=') {
        let val_cleaned =
            val.trim_matches(['\'', '"', '(', ')', '[', ']', '{', '}', ';', ',', '`']);
        if !val_cleaned.is_empty() && !candidates.contains(&val_cleaned.to_string()) {
            candidates.push(val_cleaned.to_string());
        }
        if val_cleaned.starts_with('$') {
            let stripped = val_cleaned
                .trim_start_matches('$')
                .trim_matches(['\'', '"', '`']);
            if !stripped.is_empty() && !candidates.contains(&stripped.to_string()) {
                candidates.push(stripped.to_string());
            }
            if stripped.contains('\\')
                && let Some(unescaped) = unescape_ansic(stripped)
                && !unescaped.is_empty()
                && !candidates.contains(&unescaped)
            {
                candidates.push(unescaped);
            }
        }
    }

    // 引用符内の空白区切りサブトークン (e.g. "cat /etc/shadow" -> "/etc/shadow")
    if token.contains(' ') || token.contains('\t') {
        for word in token.split_whitespace() {
            let word_cleaned =
                word.trim_matches(['\'', '"', '(', ')', '[', ']', '{', '}', ';', ',', '`']);
            if !word_cleaned.is_empty() && !candidates.contains(&word_cleaned.to_string()) {
                candidates.push(word_cleaned.to_string());
            }
            if word_cleaned.starts_with('$') {
                let stripped = word_cleaned
                    .trim_start_matches('$')
                    .trim_matches(['\'', '"', '`']);
                if !stripped.is_empty() && !candidates.contains(&stripped.to_string()) {
                    candidates.push(stripped.to_string());
                }
            }
        }
    }

    // 引用符で囲まれた部分文字列 (e.g. open('/etc/shadow') -> /etc/shadow)
    if token.contains('\'') || token.contains('"') || token.contains('`') {
        for part in token.split(['\'', '"', '`']) {
            let part_cleaned =
                part.trim_matches(['\'', '"', '(', ')', '[', ']', '{', '}', ';', ',', '`']);
            if !part_cleaned.is_empty() && !candidates.contains(&part_cleaned.to_string()) {
                candidates.push(part_cleaned.to_string());
            }
            if part_cleaned.starts_with('$') {
                let stripped = part_cleaned
                    .trim_start_matches('$')
                    .trim_matches(['\'', '"', '`']);
                if !stripped.is_empty() && !candidates.contains(&stripped.to_string()) {
                    candidates.push(stripped.to_string());
                }
            }
        }
    }

    candidates
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

    #[test]
    fn test_find_denied_paths_in_command() {
        let guard = PathGuard::default_deny_list();

        // 直接のコマンド引数
        let denied = guard.find_denied_paths_in_command("cat /etc/shadow");
        assert_eq!(denied, vec!["/etc/shadow"]);

        // 複数コマンド・パイプ
        let denied = guard.find_denied_paths_in_command("echo hello | grep foo ~/.ssh/id_rsa");
        assert_eq!(denied, vec!["~/.ssh/id_rsa"]);

        // リダイレクト先
        let denied = guard.find_denied_paths_in_command("echo 'secret' > .env.local");
        assert_eq!(denied, vec![".env.local"]);

        // クォート付きパス
        let denied = guard.find_denied_paths_in_command("tail -n 5 \"/etc/passwd\"");
        assert_eq!(denied, vec!["/etc/passwd"]);

        // フラグ指定 (--config=.env)
        let denied = guard.find_denied_paths_in_command("app --config=.env");
        assert_eq!(denied, vec![".env"]);

        // リダイレクト追記 (>>)
        let denied = guard.find_denied_paths_in_command("echo 'new_key=val' >> .env");
        assert_eq!(denied, vec![".env"]);

        // パストラバーサル
        let denied = guard.find_denied_paths_in_command("cat ../../../../../../../etc/passwd");
        assert_eq!(denied, vec!["../../../../../../../etc/passwd"]);

        // 環境変数代入プレフィックス
        let denied = guard.find_denied_paths_in_command("CONF=/etc/shadow run_service");
        assert_eq!(denied, vec!["/etc/shadow"]);

        // コマンド置換 $(...)
        let denied = guard.find_denied_paths_in_command("echo $(cat /etc/shadow)");
        assert_eq!(denied, vec!["/etc/shadow"]);

        // バッククォートによるコマンド置換 `...`
        let denied = guard.find_denied_paths_in_command("cat `echo /etc/shadow`");
        assert_eq!(denied, vec!["/etc/shadow"]);
        let denied = guard.find_denied_paths_in_command("`cat /etc/shadow`");
        assert_eq!(denied, vec!["/etc/shadow"]);
        let denied = guard.find_denied_paths_in_command("echo `cat .env`");
        assert_eq!(denied, vec![".env"]);

        // ANSI-C quoting ($'/path' または $"...")
        let denied = guard.find_denied_paths_in_command("cat $'/etc/shadow'");
        assert_eq!(denied, vec!["/etc/shadow"]);
        let denied = guard.find_denied_paths_in_command("cat $'/etc/\\x73hadow'");
        assert_eq!(denied, vec!["/etc/shadow"]);
        let denied = guard.find_denied_paths_in_command("head -n 1 $\".env\"");
        assert_eq!(denied, vec![".env"]);

        // クォート分割連結
        let denied = guard.find_denied_paths_in_command("cat /etc/sha\"\"dow");
        assert_eq!(denied, vec!["/etc/shadow"]);

        // 安全なコマンド
        let safe = guard.find_denied_paths_in_command("ls -la src/ && cargo check");
        assert!(safe.is_empty());
    }

    #[test]
    fn test_unescape_ansic() {
        assert_eq!(
            unescape_ansic("/etc/\\x73hadow"),
            Some("/etc/shadow".to_string())
        );
        assert_eq!(
            unescape_ansic("foo\\nbar\\t"),
            Some("foo\nbar\t".to_string())
        );
        assert_eq!(
            unescape_ansic("/etc/\\163hadow"),
            Some("/etc/shadow".to_string())
        );
    }
}
