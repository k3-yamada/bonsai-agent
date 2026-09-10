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

    /// キャンセルトークンを注入する。Tool traitのシグネチャは変えず、
    /// 構築時にResourceLimitsへ載せる（Issue #22 AC-2-5）。
    pub fn with_cancel(mut self, cancel: crate::cancel::CancellationToken) -> Self {
        self.limits.cancel = Some(cancel);
        self
    }

    pub fn with_path_guard(mut self, guard: PathGuard) -> Self {
        self.sandbox = Box::new(NativeSandbox::new(guard.deny_paths()));
        self.path_guard = Some(guard);
        self
    }

    pub fn new_with_guard(guard: PathGuard) -> Self {
        Self::new().with_path_guard(guard)
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
                    ..Default::default()
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
            // ユーザー取消 (Ctrl+C) を後段 (apply_tool_result) に伝える (Issue #22 qa MEDIUM-2)。
            // success は維持したまま (result.success() が既に !cancelled を含む)。
            cancelled: result.cancelled,
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
    fn t_shell_cancel_marks_result_failed() {
        let cancel = crate::cancel::CancellationToken::new();
        cancel.cancel();
        let tool = ShellTool::new().with_cancel(cancel);
        let result = tool
            .call(serde_json::json!({"command": "echo test123"}))
            .unwrap();
        assert!(!result.success);
    }

    /// Issue #22 qa-reviewer MEDIUM-2: 取消は `success: false` を維持しつつ、
    /// `ToolResult.cancelled` に伝わること（`apply_tool_result` が学習信号記録を
    /// スキップする判断材料になる）。
    #[test]
    fn t_shell_cancel_propagates_cancelled_flag() {
        let cancel = crate::cancel::CancellationToken::new();
        cancel.cancel();
        let tool = ShellTool::new().with_cancel(cancel);
        let result = tool
            .call(serde_json::json!({"command": "echo test123"}))
            .unwrap();
        assert!(!result.success);
        assert!(result.cancelled, "取消時はcancelled=trueが伝わるべき");
    }

    /// path_guard による拒否は「取消」ではなく通常の失敗であり、
    /// `cancelled` は false のままであるべき（取消と権限拒否の混同防止）。
    #[test]
    fn t_shell_path_guard_denial_is_not_cancelled() {
        let guard = PathGuard::new(vec![".env".to_string()]);
        let tool = ShellTool::new().with_path_guard(guard);
        let result = tool
            .call(serde_json::json!({"command": "cat .env"}))
            .unwrap();
        assert!(!result.success);
        assert!(
            !result.cancelled,
            "path_guard拒否はcancelledではなく通常の失敗であるべき"
        );
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
