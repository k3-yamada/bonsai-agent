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
                .args(["commit", "-m", &format!("bonsai: snapshot before edit {}", file_path.file_name().unwrap_or_default().to_string_lossy()), "--allow-empty"])
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
                    if !current.contains(old_text) {
                        return Ok(ToolResult {
                            output: format!("置換対象テキストがファイル内に見つかりません: {path}"),
                            success: false,
                        });
                    }
                    let updated = current.replacen(old_text, new_text, 1);
                    match std::fs::write(path, &updated) {
                        Ok(()) => Ok(ToolResult {
                            output: format!("差分適用しました: {path}"),
                            success: true,
                        }),
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
        let path = format!("/tmp/bonsai-test-nested-{}/sub/file.txt", uuid::Uuid::new_v4());
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
        let result = tool
            .call(serde_json::json!({"path": "/tmp/x"}))
            .unwrap();
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
}
