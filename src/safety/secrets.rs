use regex::Regex;

/// 秘密情報フィルタ。ツール結果やLLM出力からAPI鍵/トークン/パスワードをマスクする。
pub struct SecretsFilter {
    patterns: Vec<Regex>,
}

const MASK: &str = "***REDACTED***";

impl SecretsFilter {
    /// デフォルトのパターンで初期化
    pub fn default_patterns() -> Self {
        Self::new(&[
            // API鍵/トークン/パスワード（key=value形式）
            r#"(?i)(api[_-]?key|api[_-]?secret|token|password|passwd|secret[_-]?key|access[_-]?key|auth[_-]?token)\s*[:=]\s*["']?(\S+?)["']?(?:\s|$|,|;)"#,
            // AWS Access Key（AKIA...）
            r"AKIA[0-9A-Z]{16}",
            // GitHubトークン
            r"gh[pousr]_[A-Za-z0-9_]{36,}",
            // 一般的なBearerトークン
            r"Bearer\s+[A-Za-z0-9\-._~+/]+=*",
            // SSH秘密鍵ヘッダ
            r"-----BEGIN\s+(RSA|OPENSSH|EC|DSA)\s+PRIVATE\s+KEY-----",
            // .env形式の秘密変数
            r#"(?m)^((?:DATABASE_URL|SECRET_KEY|PRIVATE_KEY|AWS_SECRET)[A-Z_]*)\s*=\s*(.+)$"#,
        ])
    }

    /// カスタムパターンで初期化
    pub fn new(patterns: &[&str]) -> Self {
        let compiled = patterns
            .iter()
            .filter_map(|p| match Regex::new(p) {
                Ok(r) => Some(r),
                Err(e) => {
                    eprintln!("秘密パターンのコンパイル失敗: {e}");
                    None
                }
            })
            .collect();
        Self { patterns: compiled }
    }

    /// テキストから秘密情報をマスクする
    pub fn redact(&self, text: &str) -> String {
        let mut result = text.to_string();
        for pattern in &self.patterns {
            result = pattern.replace_all(&result, MASK).to_string();
        }
        result
    }

    /// テキストに秘密情報が含まれているかチェック
    pub fn contains_secrets(&self, text: &str) -> bool {
        self.patterns.iter().any(|p| p.is_match(text))
    }

    /// 検出された秘密情報のリストを返す
    pub fn detect(&self, text: &str) -> Vec<String> {
        let mut found = Vec::new();
        for pattern in &self.patterns {
            for m in pattern.find_iter(text) {
                let matched = m.as_str();
                // マッチした文字列を短縮して表示（秘密値自体は隠す）
                let preview = if matched.len() > 20 {
                    format!("{}...", &matched[..20])
                } else {
                    matched.to_string()
                };
                found.push(preview);
            }
        }
        found
    }
}

impl Default for SecretsFilter {
    fn default() -> Self {
        Self::default_patterns()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_api_key() {
        let filter = SecretsFilter::default();
        let input = "API_KEY=sk-1234567890abcdef";
        let result = filter.redact(input);
        assert!(!result.contains("sk-1234567890"));
        assert!(result.contains(MASK));
    }

    #[test]
    fn test_redact_token() {
        let filter = SecretsFilter::default();
        let input = "auth_token: my-secret-token-123";
        let result = filter.redact(input);
        assert!(!result.contains("my-secret-token"));
    }

    #[test]
    fn test_redact_aws_key() {
        let filter = SecretsFilter::default();
        let input = "found key: AKIAIOSFODNN7EXAMPLE";
        let result = filter.redact(input);
        assert!(!result.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn test_redact_github_token() {
        let filter = SecretsFilter::default();
        let input = "token: ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefgh1234";
        let result = filter.redact(input);
        assert!(!result.contains("ghp_ABCDEF"));
    }

    #[test]
    fn test_redact_bearer() {
        let filter = SecretsFilter::default();
        let input = "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.test";
        let result = filter.redact(input);
        assert!(!result.contains("eyJhbGci"));
    }

    #[test]
    fn test_redact_ssh_key() {
        let filter = SecretsFilter::default();
        let input = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA";
        let result = filter.redact(input);
        assert!(!result.contains("BEGIN RSA PRIVATE KEY"));
    }

    #[test]
    fn test_redact_env_variable() {
        let filter = SecretsFilter::default();
        let input = "DATABASE_URL=postgres://user:pass@host/db";
        let result = filter.redact(input);
        assert!(!result.contains("postgres://"));
    }

    #[test]
    fn test_no_false_positive() {
        let filter = SecretsFilter::default();
        let input = "This is a normal text about APIs and authentication concepts.";
        let result = filter.redact(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_contains_secrets() {
        let filter = SecretsFilter::default();
        assert!(filter.contains_secrets("AKIAIOSFODNN7EXAMPLE"));
        assert!(!filter.contains_secrets("hello world"));
    }

    #[test]
    fn test_detect() {
        let filter = SecretsFilter::default();
        let found = filter.detect("key AKIAIOSFODNN7EXAMPLE here");
        assert!(!found.is_empty());
    }

    #[test]
    fn test_multiple_secrets() {
        let filter = SecretsFilter::default();
        let input = "AKIAIOSFODNN7EXAMPLE and ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefgh1234";
        let result = filter.redact(input);
        assert!(!result.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(!result.contains("ghp_ABCDEF"));
    }

    #[test]
    fn test_custom_patterns() {
        let filter = SecretsFilter::new(&[r"custom_secret_\d+"]);
        assert!(filter.contains_secrets("found custom_secret_12345"));
        assert!(!filter.contains_secrets("normal text"));
    }
}
