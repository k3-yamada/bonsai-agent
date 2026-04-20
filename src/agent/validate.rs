use crate::agent::conversation::ToolCall;
use regex::Regex;
use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

/// バリデーション結果
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub is_valid: bool,
    pub issues: Vec<ValidationIssue>,
}

#[derive(Debug, Clone)]
pub struct ValidationIssue {
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Severity {
    /// ブロック — 実行不可
    Block,
    /// 警告 — ユーザー確認が必要
    Warn,
}

/// 危険コマンドパターン（コンパイル時に一度だけ初期化）
static DANGEROUS_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"rm\s+-rf\s",
        r"\bsudo\b",
        r"chmod\s+777\b",
        r"\bmkfs\b",
        r">\s*/dev/",
        r"\bdd\b.*\bof=/dev/",
        r":\(\)\s*\{\s*:\|:\s*&\s*\}\s*;", // フォーク爆弾
    ]
    .iter()
    .map(|p| Regex::new(p).expect("危険パターンのコンパイルに失敗"))
    .collect()
});

/// パスガード: アクセス禁止パスのリスト
pub struct PathGuard {
    deny_paths: Vec<String>,
}

impl PathGuard {
    pub fn new(deny_paths: Vec<String>) -> Self {
        Self { deny_paths }
    }

    pub fn default_deny_list() -> Self {
        Self::new(vec![
            "~/.ssh".to_string(),
            "~/.gnupg".to_string(),
            "~/.aws".to_string(),
            "/etc/shadow".to_string(),
            "/etc/passwd".to_string(),
        ])
    }

    /// パスがdenyリストに含まれるかチェック
    pub fn is_denied(&self, path: &str) -> bool {
        let expanded = expand_tilde(path);
        let check_path = Path::new(&expanded);

        for deny in &self.deny_paths {
            let expanded_deny = expand_tilde(deny);
            let deny_path = Path::new(&expanded_deny);
            if check_path.starts_with(deny_path) {
                return true;
            }
        }
        false
    }
}

/// `~` をホームディレクトリに展開
fn expand_tilde(path: &str) -> String {
    if path.starts_with("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return format!("{}{}", home.to_string_lossy(), &path[1..]);
    }
    path.to_string()
}


/// 編集距離ベースの類似ツール名提案（OpenCode知見: Invalidツールハンドラ）
#[cfg_attr(not(test), allow(dead_code))]
fn suggest_similar_tool(name: &str, known: &HashSet<String>) -> Option<String> {
    let mut best: Option<(String, usize)> = None;
    for tool_name in known {
        let dist = edit_distance(name, tool_name);
        // 距離がツール名長の半分以下なら候補
        if dist <= name.len() / 2 + 1
            && best.as_ref().is_none_or(|(_, d)| dist < *d)
        {
            best = Some((tool_name.clone(), dist));
        }
    }
    best.map(|(name, _)| name)
}

/// 簡易Levenshtein距離
#[allow(clippy::needless_range_loop)]
#[cfg_attr(not(test), allow(dead_code))]
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (m, n) = (a.len(), b.len());
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in 0..=m { dp[i][0] = i; }
    for j in 0..=n { dp[0][j] = j; }
    for i in 1..=m {
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            dp[i][j] = (dp[i - 1][j] + 1).min(dp[i][j - 1] + 1).min(dp[i - 1][j - 1] + cost);
        }
    }
    dp[m][n]
}

/// ツール呼び出しをバリデーションする
pub fn validate_tool_call(
    call: &ToolCall,
    known_tools: &HashSet<String>,
    path_guard: &PathGuard,
    dangerous_patterns: Option<&[Regex]>,
) -> ValidationResult {
    let mut issues = Vec::new();
    let patterns = dangerous_patterns.unwrap_or(&DANGEROUS_PATTERNS);

    // 1. ツール名がレジストリに存在するか
    if !known_tools.contains(&call.name) {
        issues.push(ValidationIssue {
            severity: Severity::Block,
            message: format!("不明なツール: '{}'", call.name),
        });
    }

    // 2. 引数内のパスをチェック
    check_paths_in_value(&call.arguments, path_guard, &mut issues);

    // 3. 危険パターンの検出
    check_dangerous_patterns(&call.arguments, patterns, &mut issues);

    let is_valid = !issues.iter().any(|i| i.severity == Severity::Block);

    ValidationResult { is_valid, issues }
}

/// JSON値内のパス文字列をチェック
fn check_paths_in_value(
    value: &serde_json::Value,
    guard: &PathGuard,
    issues: &mut Vec<ValidationIssue>,
) {
    match value {
        serde_json::Value::String(s) => {
            if (s.starts_with('/') || s.starts_with("~/")) && guard.is_denied(s) {
                issues.push(ValidationIssue {
                    severity: Severity::Block,
                    message: format!("禁止パスへのアクセス: '{s}'"),
                });
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values() {
                check_paths_in_value(v, guard, issues);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                check_paths_in_value(v, guard, issues);
            }
        }
        _ => {}
    }
}

/// 危険なコマンドパターンの検出
fn check_dangerous_patterns(
    value: &serde_json::Value,
    patterns: &[Regex],
    issues: &mut Vec<ValidationIssue>,
) {
    match value {
        serde_json::Value::String(s) => {
            for pattern in patterns {
                if pattern.is_match(s) {
                    issues.push(ValidationIssue {
                        severity: Severity::Warn,
                        message: format!("危険なコマンドパターン検出: '{s}'"),
                    });
                }
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values() {
                check_dangerous_patterns(v, patterns, issues);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                check_dangerous_patterns(v, patterns, issues);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_tools() -> HashSet<String> {
        ["shell", "file_read", "file_write", "memory_search"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn test_guard() -> PathGuard {
        PathGuard::default_deny_list()
    }

    // テスト1: 正常なツール呼び出し
    #[test]
    fn test_valid_tool_call() {
        let call = ToolCall {
            name: "shell".to_string(),
            arguments: serde_json::json!({"command": "ls -la"}),
        };
        let result = validate_tool_call(&call, &test_tools(), &test_guard(), None);
        assert!(result.is_valid);
        assert!(result.issues.is_empty());
    }

    // テスト2: 不明なツール名
    #[test]
    fn test_unknown_tool() {
        let call = ToolCall {
            name: "hack_system".to_string(),
            arguments: serde_json::json!({}),
        };
        let result = validate_tool_call(&call, &test_tools(), &test_guard(), None);
        assert!(!result.is_valid);
        assert_eq!(result.issues.len(), 1);
        assert_eq!(result.issues[0].severity, Severity::Block);
        assert!(result.issues[0].message.contains("不明なツール"));
    }

    // テスト3: 禁止パスへのアクセス
    #[test]
    fn test_denied_path() {
        let call = ToolCall {
            name: "file_read".to_string(),
            arguments: serde_json::json!({"path": "~/.ssh/id_rsa"}),
        };
        let result = validate_tool_call(&call, &test_tools(), &test_guard(), None);
        assert!(!result.is_valid);
        assert!(result.issues.iter().any(|i| i.message.contains("禁止パス")));
    }

    // テスト4: 危険コマンド（rm -rf）
    #[test]
    fn test_dangerous_rm_rf() {
        let call = ToolCall {
            name: "shell".to_string(),
            arguments: serde_json::json!({"command": "rm -rf /"}),
        };
        let result = validate_tool_call(&call, &test_tools(), &test_guard(), None);
        // 危険パターンはWarn（ツール自体は有効なのでis_valid=true、ただし警告あり）
        assert!(result.is_valid);
        assert!(result.issues.iter().any(|i| i.severity == Severity::Warn));
    }

    // テスト5: 危険コマンド（sudo）
    #[test]
    fn test_dangerous_sudo() {
        let call = ToolCall {
            name: "shell".to_string(),
            arguments: serde_json::json!({"command": "sudo apt install vim"}),
        };
        let result = validate_tool_call(&call, &test_tools(), &test_guard(), None);
        assert!(
            result
                .issues
                .iter()
                .any(|i| i.message.contains("危険なコマンドパターン"))
        );
    }

    // テスト6: 安全なパス
    #[test]
    fn test_safe_path() {
        let call = ToolCall {
            name: "file_read".to_string(),
            arguments: serde_json::json!({"path": "/tmp/test.txt"}),
        };
        let result = validate_tool_call(&call, &test_tools(), &test_guard(), None);
        assert!(result.is_valid);
        assert!(result.issues.is_empty());
    }

    // テスト7: ネストされたJSON内のパスチェック
    #[test]
    fn test_nested_path_check() {
        let call = ToolCall {
            name: "shell".to_string(),
            arguments: serde_json::json!({
                "options": {
                    "files": ["~/.ssh/config", "/tmp/ok.txt"]
                }
            }),
        };
        let result = validate_tool_call(&call, &test_tools(), &test_guard(), None);
        assert!(!result.is_valid);
        assert!(result.issues.iter().any(|i| i.message.contains(".ssh")));
    }

    // テスト8: 複数の問題を同時検出
    #[test]
    fn test_multiple_issues() {
        let call = ToolCall {
            name: "unknown_tool".to_string(),
            arguments: serde_json::json!({"command": "sudo rm -rf ~/"}),
        };
        let result = validate_tool_call(&call, &test_tools(), &test_guard(), None);
        assert!(!result.is_valid);
        // 不明ツール(Block) + sudo(Warn) + rm -rf(Warn) = 3件
        assert!(result.issues.len() >= 2);
    }

    // テスト9: PathGuardの~展開
    #[test]
    fn test_path_guard_tilde_expansion() {
        let guard = PathGuard::default_deny_list();
        assert!(guard.is_denied("~/.ssh/id_rsa"));
        assert!(guard.is_denied("~/.aws/credentials"));
        assert!(!guard.is_denied("~/Documents/test.txt"));
        assert!(!guard.is_denied("/tmp/test"));
    }

    // テスト10: フォーク爆弾の検出
    #[test]
    fn test_fork_bomb_detection() {
        let call = ToolCall {
            name: "shell".to_string(),
            arguments: serde_json::json!({"command": ":(){ :|:& };"}),
        };
        let result = validate_tool_call(&call, &test_tools(), &test_guard(), None);
        assert!(result.issues.iter().any(|i| i.severity == Severity::Warn));
    }

    #[test]
    fn test_suggest_similar_tool() {
        let tools: HashSet<String> = ["file_read", "file_write", "shell", "git"]
            .iter().map(|s| s.to_string()).collect();
        // typo: file_rea → file_read
        assert_eq!(suggest_similar_tool("file_rea", &tools), Some("file_read".to_string()));
        // typo: shel → shell
        assert_eq!(suggest_similar_tool("shel", &tools), Some("shell".to_string()));
        // 全く違う名前 → None
        assert_eq!(suggest_similar_tool("completely_different_very_long_name", &tools), None);
    }

    #[test]
    fn test_edit_distance() {
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("same", "same"), 0);
    }

}
