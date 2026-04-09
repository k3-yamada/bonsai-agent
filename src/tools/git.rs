use std::process::Command;

use anyhow::Result;

use crate::tools::permission::Permission;
use crate::tools::{Tool, ToolResult};

/// Git操作ツール
pub struct GitTool;

impl GitTool {
    fn run_git(args: &[&str]) -> Result<ToolResult> {
        let output = Command::new("git")
            .args(args)
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        let result_text = if stdout.is_empty() && !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() && !stderr.is_empty() {
            format!("{stdout}\n[stderr] {stderr}")
        } else {
            stdout
        };

        Ok(ToolResult {
            output: result_text,
            success: output.status.success(),
        })
    }
}

impl Tool for GitTool {
    fn name(&self) -> &str {
        "git"
    }

    fn description(&self) -> &str {
        "Gitリポジトリを操作する。subcommandパラメータにstatus/diff/log/commit/add/branchを指定。commitにはmessageパラメータも必要。"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "subcommand": {
                    "type": "string",
                    "enum": ["status", "diff", "log", "commit", "add", "branch"],
                    "description": "Gitサブコマンド"
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "追加引数"
                },
                "message": {
                    "type": "string",
                    "description": "コミットメッセージ（commitサブコマンド用）"
                }
            },
            "required": ["subcommand"]
        })
    }

    fn permission(&self) -> Permission {
        // 読み取り系はAutoだが、commit等の書き込み系はConfirm
        // ここではConfirmをデフォルトにし、agent_loopで読み取り系はバリデーションで許可
        Permission::Confirm
    }

    fn call(&self, args: serde_json::Value) -> Result<ToolResult> {
        let subcommand = args
            .get("subcommand")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("'subcommand' パラメータが必要です"))?;

        let extra_args: Vec<String> = args
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        match subcommand {
            "status" => Self::run_git(&["status", "--short"]),
            "diff" => {
                let mut git_args = vec!["diff"];
                let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
                git_args.extend(extra_refs);
                Self::run_git(&git_args)
            }
            "log" => {
                let mut git_args = vec!["log", "--oneline", "-20"];
                let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
                git_args.extend(extra_refs);
                Self::run_git(&git_args)
            }
            "commit" => {
                let message = args
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("bonsai: auto commit");
                Self::run_git(&["commit", "-m", message])
            }
            "add" => {
                let files: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
                if files.is_empty() {
                    Self::run_git(&["add", "-A"])
                } else {
                    let mut git_args = vec!["add"];
                    git_args.extend(files);
                    Self::run_git(&git_args)
                }
            }
            "branch" => Self::run_git(&["branch", "-a"]),
            _ => Ok(ToolResult {
                output: format!("不明なサブコマンド: {subcommand}"),
                success: false,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_status() {
        let tool = GitTool;
        let result = tool
            .call(serde_json::json!({"subcommand": "status"}))
            .unwrap();
        // gitリポジトリ内で実行されるはず
        assert!(result.success || !result.output.is_empty());
    }

    #[test]
    fn test_git_log() {
        let tool = GitTool;
        let result = tool
            .call(serde_json::json!({"subcommand": "log"}))
            .unwrap();
        assert!(result.success);
    }

    #[test]
    fn test_git_diff() {
        let tool = GitTool;
        let result = tool
            .call(serde_json::json!({"subcommand": "diff"}))
            .unwrap();
        assert!(result.success);
    }

    #[test]
    fn test_git_branch() {
        let tool = GitTool;
        let result = tool
            .call(serde_json::json!({"subcommand": "branch"}))
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("master") || result.output.contains("main"));
    }

    #[test]
    fn test_git_unknown_subcommand() {
        let tool = GitTool;
        let result = tool
            .call(serde_json::json!({"subcommand": "unknown"}))
            .unwrap();
        assert!(!result.success);
    }

    #[test]
    fn test_git_missing_subcommand() {
        let tool = GitTool;
        let result = tool.call(serde_json::json!({}));
        assert!(result.is_err());
    }

    #[test]
    fn test_git_metadata() {
        let tool = GitTool;
        assert_eq!(tool.name(), "git");
        assert_eq!(tool.permission(), Permission::Confirm);
    }
}
