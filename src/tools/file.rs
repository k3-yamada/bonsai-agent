use std::path::Path;

use anyhow::Result;

use crate::tools::permission::Permission;
use crate::tools::{Tool, ToolResult};

/// ファイル読み取りツール
pub struct FileReadTool;

impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        "ファイルの内容を読み取る。pathパラメータにファイルパスを指定。"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "読み取るファイルのパス" }
            },
            "required": ["path"]
        })
    }

    fn permission(&self) -> Permission {
        Permission::Auto
    }

    fn call(&self, args: serde_json::Value) -> Result<ToolResult> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("'path' パラメータが必要です"))?;

        match std::fs::read_to_string(path) {
            Ok(content) => Ok(ToolResult {
                output: content,
                success: true,
            }),
            Err(e) => Ok(ToolResult {
                output: format!("ファイル読み取りエラー: {e}"),
                success: false,
            }),
        }
    }
}

/// ファイル書き込みツール（全文置換 + search/replace差分適用）
pub struct FileWriteTool;

impl FileWriteTool {
    /// git管理下であれば書き込み前にコミット
    fn git_snapshot(path: &str) -> Option<()> {
        let file_path = Path::new(path);
        if !file_path.exists() {
            return None;
        }

        // gitリポジトリかチェック
        let status = std::process::Command::new("git")
            .args(["rev-parse", "--is-inside-work-tree"])
            .current_dir(file_path.parent().unwrap_or(Path::new(".")))
            .output();

        if let Ok(out) = status
            && out.status.success()
        {
            // 変更があればスナップショットコミット
            let _ = std::process::Command::new("git")
                .args(["add", path])
                .output();
            let _ = std::process::Command::new("git")
                .args([
                    "commit",
                    "-m",
                    &format!(
                        "bonsai: snapshot before edit {}",
                        file_path.file_name().unwrap_or_default().to_string_lossy()
                    ),
                    "--allow-empty",
                ])
                .output();
            return Some(());
        }
        None
    }
}

impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        "file_write"
    }

    fn description(&self) -> &str {
        "ファイルに書き込む。全文置換(content)またはsearch/replace差分適用(old_text/new_text)。git管理下では自動スナップショット。"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "書き込み先のファイルパス" },
                "content": { "type": "string", "description": "全文置換する内容（old_text/new_textと排他）" },
                "old_text": { "type": "string", "description": "置換対象のテキスト" },
                "new_text": { "type": "string", "description": "置換後のテキスト" }
            },
            "required": ["path"]
        })
    }

    fn permission(&self) -> Permission {
        Permission::Confirm
    }

    fn call(&self, args: serde_json::Value) -> Result<ToolResult> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("'path' パラメータが必要です"))?;

        // git-first: 書き込み前にスナップショット
        Self::git_snapshot(path);

        if let Some(content) = args.get("content").and_then(|v| v.as_str()) {
            // 全文置換
            if let Some(parent) = Path::new(path).parent() {
                std::fs::create_dir_all(parent).ok();
            }
            match std::fs::write(path, content) {
                Ok(()) => Ok(ToolResult {
                    output: format!("ファイルを書き込みました: {path}"),
                    success: true,
                }),
                Err(e) => Ok(ToolResult {
                    output: format!("書き込みエラー: {e}"),
                    success: false,
                }),
            }
        } else if let (Some(old_text), Some(new_text)) = (
            args.get("old_text").and_then(|v| v.as_str()),
            args.get("new_text").and_then(|v| v.as_str()),
        ) {
            // search/replace差分適用
            match std::fs::read_to_string(path) {
                Ok(current) => {
                    let (updated, warning) = if current.contains(old_text) {
                        (current.replacen(old_text, new_text, 1), None)
                    } else if let Some((fuzzy_result, msg)) =
                        fuzzy_find_replace(&current, old_text, new_text)
                    {
                        (fuzzy_result, Some(msg))
                    } else {
                        return Ok(ToolResult {
                            output: format!("置換対象テキストがファイル内に見つかりません: {path}"),
                            success: false,
                        });
                    };
                    match std::fs::write(path, &updated) {
                        Ok(()) => {
                            let msg = if let Some(w) = warning {
                                format!("差分適用しました（{w}）: {path}")
                            } else {
                                format!("差分適用しました: {path}")
                            };
                            Ok(ToolResult {
                                output: msg,
                                success: true,
                            })
                        }
                        Err(e) => Ok(ToolResult {
                            output: format!("書き込みエラー: {e}"),
                            success: false,
                        }),
                    }
                }
                Err(e) => Ok(ToolResult {
                    output: format!("ファイル読み取りエラー: {e}"),
                    success: false,
                }),
            }
        } else {
            Ok(ToolResult {
                output: "'content' または 'old_text'+'new_text' のいずれかが必要です".to_string(),
                success: false,
            })
        }
    }
}


/// 空白を正規化（連続空白→単一スペース、先頭末尾trim）
fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// fuzzyマッチで置換を試みる。完全一致失敗時のフォールバック。
/// 成功時は (置換後テキスト, 警告メッセージ) を返す。
fn fuzzy_find_replace(content: &str, old_text: &str, new_text: &str) -> Option<(String, String)> {
    let norm_content = normalize_whitespace(content);
    let norm_old = normalize_whitespace(old_text);

    if norm_old.is_empty() {
        return None;
    }

    // 空白正規化一致で検索
    if norm_content.contains(&norm_old) {
        // 元テキスト内で対応する範囲を探す
        // 行単位で比較して一致範囲を特定
        let content_lines: Vec<&str> = content.lines().collect();
        let old_lines: Vec<&str> = old_text.lines().collect();

        if old_lines.is_empty() {
            return None;
        }

        // 先頭行の空白正規化版で位置を特定
        let norm_first = normalize_whitespace(old_lines[0]);
        for (i, cl) in content_lines.iter().enumerate() {
            if normalize_whitespace(cl) == norm_first
                && i + old_lines.len() <= content_lines.len()
            {
                // 全行が空白正規化で一致するか確認
                let all_match = old_lines.iter().enumerate().all(|(j, ol)| {
                    normalize_whitespace(content_lines[i + j]) == normalize_whitespace(ol)
                });
                if all_match {
                    // 元の行を置換
                    let mut result_lines = Vec::new();
                    result_lines.extend_from_slice(&content_lines[..i]);
                    for new_line in new_text.lines() {
                        result_lines.push(new_line);
                    }
                    result_lines.extend_from_slice(&content_lines[i + old_lines.len()..]);
                    let result = result_lines.join("
");
                    // 元のファイルが末尾改行ありならそれも保持
                    let result = if content.ends_with('\n') && !result.ends_with('\n') {
                        result + "\n"
                    } else {
                        result
                    };
                    return Some((
                        result,
                        "模糊一致で置換しました（空白の差異）".to_string(),
                    ));
                }
            }
        }
    }

    // trim一致: old_textの前後空白をtrimして再試行
    let trimmed_old = old_text.trim();
    if trimmed_old != old_text && content.contains(trimmed_old) {
        // 曖昧さチェック: 1箇所のみ
        if content.matches(trimmed_old).count() == 1 {
            let updated = content.replacen(trimmed_old, new_text.trim(), 1);
            return Some((
                updated,
                "模糊一致で置換しました（先頭/末尾の空白差異）".to_string(),
            ));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_path(name: &str) -> String {
        format!("/tmp/bonsai-test-{}-{}", name, uuid::Uuid::new_v4())
    }

    // FileReadTool
    #[test]
    fn test_read_existing_file() {
        let path = temp_path("read");
        fs::write(&path, "hello world").unwrap();

        let tool = FileReadTool;
        let result = tool.call(serde_json::json!({"path": path})).unwrap();
        assert!(result.success);
        assert_eq!(result.output, "hello world");

        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_read_nonexistent_file() {
        let tool = FileReadTool;
        let result = tool
            .call(serde_json::json!({"path": "/tmp/nonexistent-bonsai-xyz"}))
            .unwrap();
        assert!(!result.success);
        assert!(result.output.contains("エラー"));
    }

    #[test]
    fn test_read_missing_param() {
        let tool = FileReadTool;
        let result = tool.call(serde_json::json!({}));
        assert!(result.is_err());
    }

    // FileWriteTool — 全文置換
    #[test]
    fn test_write_full_content() {
        let path = temp_path("write-full");
        let tool = FileWriteTool;

        let result = tool
            .call(serde_json::json!({"path": &path, "content": "new content"}))
            .unwrap();
        assert!(result.success);
        assert_eq!(fs::read_to_string(&path).unwrap(), "new content");

        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_write_creates_parent_dirs() {
        let path = format!(
            "/tmp/bonsai-test-nested-{}/sub/file.txt",
            uuid::Uuid::new_v4()
        );
        let tool = FileWriteTool;

        let result = tool
            .call(serde_json::json!({"path": &path, "content": "test"}))
            .unwrap();
        assert!(result.success);

        // cleanup
        if let Some(parent) = Path::new(&path).parent() {
            fs::remove_dir_all(parent.parent().unwrap()).ok();
        }
    }

    // FileWriteTool — 差分適用
    #[test]
    fn test_write_search_replace() {
        let path = temp_path("write-diff");
        fs::write(&path, "hello world").unwrap();

        let tool = FileWriteTool;
        let result = tool
            .call(serde_json::json!({
                "path": &path,
                "old_text": "world",
                "new_text": "rust"
            }))
            .unwrap();
        assert!(result.success);
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello rust");

        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_write_search_replace_not_found() {
        let path = temp_path("write-notfound");
        fs::write(&path, "hello world").unwrap();

        let tool = FileWriteTool;
        let result = tool
            .call(serde_json::json!({
                "path": &path,
                "old_text": "xyz",
                "new_text": "abc"
            }))
            .unwrap();
        assert!(!result.success);
        assert!(result.output.contains("見つかりません"));

        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_write_no_content_params() {
        let tool = FileWriteTool;
        let result = tool.call(serde_json::json!({"path": "/tmp/x"})).unwrap();
        assert!(!result.success);
    }

    // メタデータ
    #[test]
    fn test_file_read_permission() {
        assert_eq!(FileReadTool.permission(), Permission::Auto);
    }

    #[test]
    fn test_file_write_permission() {
        assert_eq!(FileWriteTool.permission(), Permission::Confirm);
    }

    // --- fuzzyマッチテスト ---

    #[test]
    fn test_normalize_whitespace() {
        assert_eq!(normalize_whitespace("  hello   world  "), "hello world");
        assert_eq!(normalize_whitespace("a\n  b"), "a b");
    }

    #[test]
    fn test_fuzzy_replace_whitespace_difference() {
        let content = "fn main() {\n    println!("hello");\n}";
        let old_text = "fn main() {\n  println!("hello");\n}"; // インデント違い
        let new_text = "fn main() {\n    println!("world");\n}";
        let result = fuzzy_find_replace(content, old_text, new_text);
        assert!(result.is_some());
        let (replaced, warning) = result.unwrap();
        assert!(replaced.contains("world"));
        assert!(warning.contains("模糊一致"));
    }

    #[test]
    fn test_fuzzy_replace_trailing_whitespace() {
        let content = "hello world";
        let old_text = "  hello world  "; // 前後に空白
        let new_text = "hello rust";
        let result = fuzzy_find_replace(content, old_text, new_text);
        assert!(result.is_some());
    }

    #[test]
    fn test_fuzzy_replace_exact_still_preferred() {
        let path = temp_path("fuzzy-exact");
        fs::write(&path, "hello world").unwrap();
        let tool = FileWriteTool;
        let result = tool
            .call(serde_json::json!({
                "path": &path,
                "old_text": "hello",
                "new_text": "greet"
            }))
            .unwrap();
        assert!(result.success);
        // 完全一致の場合は「模糊一致」を含まない
        assert!(!result.output.contains("模糊一致"));
        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_fuzzy_replace_not_found_returns_none() {
        let content = "hello world";
        let old_text = "completely different text";
        let result = fuzzy_find_replace(content, old_text, "new");
        assert!(result.is_none());
    }
}
