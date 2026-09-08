use anyhow::Result;

use crate::safety::path_guard::PathGuard;
use crate::tools::ToolResult;
use crate::tools::permission::Permission;
use crate::tools::sandbox::{NativeSandbox, ResourceLimits, Sandbox};
use crate::tools::typed::TypedTool;
use schemars::JsonSchema;
use serde::Deserialize;

/// シェルコマンド実行ツール
pub struct ShellTool {
    sandbox: Box<dyn Sandbox>,
    limits: ResourceLimits,
    path_guard: Option<PathGuard>,
}

impl ShellTool {
    pub fn new() -> Self {
        Self {
            sandbox: Box::new(NativeSandbox::default()),
            limits: ResourceLimits::default(),
            path_guard: None,
        }
    }

    pub fn with_sandbox(mut self, sandbox: Box<dyn Sandbox>) -> Self {
        self.sandbox = sandbox;
        self
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.limits.timeout = std::time::Duration::from_secs(secs);
        self
    }

    pub fn with_path_guard(mut self, guard: PathGuard) -> Self {
        self.path_guard = Some(guard);
        self
    }
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct ShellArgs {
    /// 実行するシェルコマンド
    command: String,
}

impl TypedTool for ShellTool {
    type Args = ShellArgs;
    const NAME: &'static str = "shell";
    const DESCRIPTION: &'static str = super::descriptions::SHELL;
    const PERMISSION: Permission = Permission::Confirm;

    fn execute(&self, args: ShellArgs) -> Result<ToolResult> {
        let command = &args.command;

        // PathGuard による防衛（Defense in Depth）
        if let Some(guard) = &self.path_guard {
            let denied = guard.find_denied_paths_in_command(command);
            if !denied.is_empty() {
                return Ok(ToolResult {
                    output: format!(
                        "アクセスが拒否されたパスへの操作が含まれています: {}",
                        denied.join(", ")
                    ),
                    success: false,
                });
            }
        }

        let result = self.sandbox.execute_script(command, &self.limits)?;

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
    use crate::tools::Tool;

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
        let result = tool.call(serde_json::json!({"command": "exit 1"})).unwrap();
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
        let result = tool.call(serde_json::json!({"command": "pwd"})).unwrap();
        assert!(result.success);
        assert!(result.output.starts_with('/'));
    }

    #[test]
    fn test_shell_blocked_by_path_guard() {
        let guard = PathGuard::new(vec![".env".to_string()]);
        let tool = ShellTool::new().with_path_guard(guard);
        let result = tool
            .call(serde_json::json!({"command": "cat .env"}))
            .unwrap();
        assert!(!result.success);
        assert!(
            result
                .output
                .contains("アクセスが拒否されたパスへの操作が含まれています")
        );
        assert!(result.output.contains(".env"));
    }
}
