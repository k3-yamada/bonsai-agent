use anyhow::Result;

use crate::tools::permission::Permission;
use crate::tools::sandbox::{DirectSandbox, ResourceLimits, Sandbox};
use crate::tools::{Tool, ToolResult};

/// シェルコマンド実行ツール
pub struct ShellTool {
    sandbox: Box<dyn Sandbox>,
    limits: ResourceLimits,
}

impl ShellTool {
    pub fn new() -> Self {
        Self {
            sandbox: Box::new(DirectSandbox),
            limits: ResourceLimits::default(),
        }
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.limits.timeout = std::time::Duration::from_secs(secs);
        self
    }
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "シェルコマンドを実行する。commandパラメータにコマンド文字列を指定。"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "実行するシェルコマンド"
                }
            },
            "required": ["command"]
        })
    }

    fn permission(&self) -> Permission {
        Permission::Confirm
    }

    fn call(&self, args: serde_json::Value) -> Result<ToolResult> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("'command' パラメータが必要です"))?;

        let result = self.sandbox.execute("sh", &["-c", command], &self.limits)?;

        let output = if result.stdout.is_empty() {
            result.stderr.clone()
        } else if result.stderr.is_empty() {
            result.stdout.clone()
        } else {
            format!("{}\n[stderr] {}", result.stdout, result.stderr)
        };

        Ok(ToolResult {
            output,
            success: result.success(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_echo() {
        let tool = ShellTool::new();
        let result = tool
            .call(serde_json::json!({"command": "echo test123"}))
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("test123"));
    }

    #[test]
    fn test_shell_missing_command() {
        let tool = ShellTool::new();
        let result = tool.call(serde_json::json!({}));
        assert!(result.is_err());
    }

    #[test]
    fn test_shell_failing_command() {
        let tool = ShellTool::new();
        let result = tool
            .call(serde_json::json!({"command": "exit 1"}))
            .unwrap();
        assert!(!result.success);
    }

    #[test]
    fn test_shell_metadata() {
        let tool = ShellTool::new();
        assert_eq!(tool.name(), "shell");
        assert_eq!(tool.permission(), Permission::Confirm);
    }

    #[test]
    fn test_shell_with_timeout() {
        let tool = ShellTool::new().with_timeout(5);
        assert_eq!(tool.limits.timeout.as_secs(), 5);
    }

    #[test]
    fn test_shell_pwd() {
        let tool = ShellTool::new();
        let result = tool
            .call(serde_json::json!({"command": "pwd"}))
            .unwrap();
        assert!(result.success);
        assert!(result.output.starts_with('/'));
    }
}
