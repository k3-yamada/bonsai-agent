use std::process::Command;

use serde::{Deserialize, Serialize};

/// フック設定
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HooksConfig {
    /// ツール実行前に実行するスクリプト
    #[serde(default)]
    pub pre_tool: Vec<String>,
    /// ツール実行後に実行するスクリプト
    #[serde(default)]
    pub post_tool: Vec<String>,
    /// セッション開始時に実行するスクリプト
    #[serde(default)]
    pub session_start: Vec<String>,
    /// セッション終了時に実行するスクリプト
    #[serde(default)]
    pub session_end: Vec<String>,
}

/// フックイベント
#[derive(Debug, Clone)]
pub enum HookEvent {
    PreTool {
        tool_name: String,
        args: String,
    },
    PostTool {
        tool_name: String,
        success: bool,
        output: String,
    },
    SessionStart {
        session_id: String,
    },
    SessionEnd {
        session_id: String,
    },
}

/// フック実行エンジン
pub struct HookRunner {
    config: HooksConfig,
}

impl HookRunner {
    pub fn new(config: HooksConfig) -> Self {
        Self { config }
    }

    /// イベントに応じたフックを実行
    pub fn run(&self, event: &HookEvent) -> Vec<HookResult> {
        let scripts = match event {
            HookEvent::PreTool { .. } => &self.config.pre_tool,
            HookEvent::PostTool { .. } => &self.config.post_tool,
            HookEvent::SessionStart { .. } => &self.config.session_start,
            HookEvent::SessionEnd { .. } => &self.config.session_end,
        };

        scripts
            .iter()
            .map(|script| self.execute_hook(script, event))
            .collect()
    }

    /// スクリプトを実行し、環境変数でイベント情報を渡す
    fn execute_hook(&self, script: &str, event: &HookEvent) -> HookResult {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(script);

        // イベント情報を環境変数で渡す
        match event {
            HookEvent::PreTool { tool_name, args } => {
                cmd.env("BONSAI_HOOK", "pre_tool");
                cmd.env("BONSAI_TOOL_NAME", tool_name);
                cmd.env("BONSAI_TOOL_ARGS", args);
            }
            HookEvent::PostTool {
                tool_name,
                success,
                output,
            } => {
                cmd.env("BONSAI_HOOK", "post_tool");
                cmd.env("BONSAI_TOOL_NAME", tool_name);
                cmd.env("BONSAI_TOOL_SUCCESS", success.to_string());
                cmd.env("BONSAI_TOOL_OUTPUT", &output[..output.len().min(1000)]);
            }
            HookEvent::SessionStart { session_id } => {
                cmd.env("BONSAI_HOOK", "session_start");
                cmd.env("BONSAI_SESSION_ID", session_id);
            }
            HookEvent::SessionEnd { session_id } => {
                cmd.env("BONSAI_HOOK", "session_end");
                cmd.env("BONSAI_SESSION_ID", session_id);
            }
        }

        match cmd.output() {
            Ok(output) => HookResult {
                script: script.to_string(),
                success: output.status.success(),
                output: String::from_utf8_lossy(&output.stdout).to_string(),
                error: if output.status.success() {
                    None
                } else {
                    Some(String::from_utf8_lossy(&output.stderr).to_string())
                },
            },
            Err(e) => HookResult {
                script: script.to_string(),
                success: false,
                output: String::new(),
                error: Some(format!("フック実行失敗: {e}")),
            },
        }
    }

    pub fn has_hooks(&self, event: &HookEvent) -> bool {
        match event {
            HookEvent::PreTool { .. } => !self.config.pre_tool.is_empty(),
            HookEvent::PostTool { .. } => !self.config.post_tool.is_empty(),
            HookEvent::SessionStart { .. } => !self.config.session_start.is_empty(),
            HookEvent::SessionEnd { .. } => !self.config.session_end.is_empty(),
        }
    }
}

/// フック実行結果
#[derive(Debug, Clone)]
pub struct HookResult {
    pub script: String,
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_hooks() {
        let runner = HookRunner::new(HooksConfig::default());
        let event = HookEvent::PreTool {
            tool_name: "shell".to_string(),
            args: "{}".to_string(),
        };
        assert!(!runner.has_hooks(&event));
        assert!(runner.run(&event).is_empty());
    }

    #[test]
    fn test_pre_tool_hook() {
        let config = HooksConfig {
            pre_tool: vec!["echo pre-hook".to_string()],
            ..Default::default()
        };
        let runner = HookRunner::new(config);
        let event = HookEvent::PreTool {
            tool_name: "shell".to_string(),
            args: "{}".to_string(),
        };
        assert!(runner.has_hooks(&event));
        let results = runner.run(&event);
        assert_eq!(results.len(), 1);
        assert!(results[0].success);
        assert!(results[0].output.contains("pre-hook"));
    }

    #[test]
    fn test_post_tool_hook_with_env() {
        let config = HooksConfig {
            post_tool: vec!["echo $BONSAI_TOOL_NAME $BONSAI_TOOL_SUCCESS".to_string()],
            ..Default::default()
        };
        let runner = HookRunner::new(config);
        let event = HookEvent::PostTool {
            tool_name: "shell".to_string(),
            success: true,
            output: "result".to_string(),
        };
        let results = runner.run(&event);
        assert!(results[0].success);
        assert!(results[0].output.contains("shell"));
        assert!(results[0].output.contains("true"));
    }

    #[test]
    fn test_failing_hook() {
        let config = HooksConfig {
            pre_tool: vec!["exit 1".to_string()],
            ..Default::default()
        };
        let runner = HookRunner::new(config);
        let event = HookEvent::PreTool {
            tool_name: "test".to_string(),
            args: "{}".to_string(),
        };
        let results = runner.run(&event);
        assert!(!results[0].success);
    }

    #[test]
    fn test_multiple_hooks() {
        let config = HooksConfig {
            pre_tool: vec!["echo first".to_string(), "echo second".to_string()],
            ..Default::default()
        };
        let runner = HookRunner::new(config);
        let event = HookEvent::PreTool {
            tool_name: "test".to_string(),
            args: "{}".to_string(),
        };
        let results = runner.run(&event);
        assert_eq!(results.len(), 2);
        assert!(results[0].output.contains("first"));
        assert!(results[1].output.contains("second"));
    }

    #[test]
    fn test_session_hooks() {
        let config = HooksConfig {
            session_start: vec!["echo started".to_string()],
            session_end: vec!["echo ended".to_string()],
            ..Default::default()
        };
        let runner = HookRunner::new(config);

        let start = HookEvent::SessionStart {
            session_id: "test-123".to_string(),
        };
        assert!(runner.has_hooks(&start));

        let end = HookEvent::SessionEnd {
            session_id: "test-123".to_string(),
        };
        let results = runner.run(&end);
        assert!(results[0].output.contains("ended"));
    }

    #[test]
    fn test_hooks_config_deserialize() {
        let toml_str = r#"
pre_tool = ["echo pre"]
post_tool = ["echo post"]
"#;
        let config: HooksConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.pre_tool.len(), 1);
        assert_eq!(config.post_tool.len(), 1);
        assert!(config.session_start.is_empty());
    }
}
