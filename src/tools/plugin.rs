use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::tools::permission::Permission;
use crate::tools::sandbox::{DirectSandbox, ResourceLimits, Sandbox};
use crate::tools::{Tool, ToolResult};

/// TOML設定から定義されるカスタムツール
///
/// config.toml例:
/// ```toml
/// [[plugins.tools]]
/// name = "weather"
/// command = "curl -s 'wttr.in/{location}?format=3'"
/// description = "指定した都市の天気を取得する"
/// permission = "auto"
/// [plugins.tools.parameters]
/// location = { type = "string", description = "都市名" }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginToolConfig {
    pub name: String,
    pub command: String,
    pub description: String,
    #[serde(default = "default_permission")]
    pub permission: String,
    #[serde(default)]
    pub parameters: HashMap<String, ParameterDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterDef {
    #[serde(rename = "type", default = "default_string_type")]
    pub param_type: String,
    #[serde(default)]
    pub description: String,
}

fn default_permission() -> String {
    "confirm".to_string()
}

fn default_string_type() -> String {
    "string".to_string()
}

/// プラグインツール（コマンドラッパー）
pub struct PluginTool {
    config: PluginToolConfig,
    sandbox: Box<dyn Sandbox>,
}

impl PluginTool {
    pub fn from_config(config: PluginToolConfig) -> Self {
        Self {
            config,
            sandbox: Box::new(DirectSandbox),
        }
    }

    /// コマンドテンプレート内の {param_name} を引数値で置換
    fn expand_command(&self, args: &serde_json::Value) -> String {
        let mut cmd = self.config.command.clone();
        if let Some(obj) = args.as_object() {
            for (key, value) in obj {
                let placeholder = format!("{{{key}}}");
                let replacement = match value {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                cmd = cmd.replace(&placeholder, &replacement);
            }
        }
        cmd
    }
}

impl Tool for PluginTool {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn description(&self) -> &str {
        &self.config.description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        for (name, def) in &self.config.parameters {
            properties.insert(
                name.clone(),
                serde_json::json!({
                    "type": def.param_type,
                    "description": def.description,
                }),
            );
            required.push(serde_json::Value::String(name.clone()));
        }

        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required,
        })
    }

    fn permission(&self) -> Permission {
        match self.config.permission.as_str() {
            "auto" => Permission::Auto,
            "deny" => Permission::Deny,
            _ => Permission::Confirm,
        }
    }

    fn call(&self, args: serde_json::Value) -> Result<ToolResult> {
        let expanded = self.expand_command(&args);
        let result = self.sandbox.execute(
            "sh",
            &["-c", &expanded],
            &ResourceLimits::default(),
        )?;

        let success = result.success();
        let output = if result.stdout.is_empty() {
            result.stderr
        } else {
            result.stdout
        };
        Ok(ToolResult { output, success })
    }
}

/// TOML設定からプラグインツールのリストを作成
pub fn load_plugin_tools(configs: &[PluginToolConfig]) -> Vec<Box<dyn Tool>> {
    configs
        .iter()
        .map(|c| Box::new(PluginTool::from_config(c.clone())) as Box<dyn Tool>)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> PluginToolConfig {
        PluginToolConfig {
            name: "echo_plugin".to_string(),
            command: "echo {message}".to_string(),
            description: "メッセージをエコーする".to_string(),
            permission: "auto".to_string(),
            parameters: {
                let mut m = HashMap::new();
                m.insert(
                    "message".to_string(),
                    ParameterDef {
                        param_type: "string".to_string(),
                        description: "表示するメッセージ".to_string(),
                    },
                );
                m
            },
        }
    }

    #[test]
    fn test_plugin_tool_metadata() {
        let tool = PluginTool::from_config(test_config());
        assert_eq!(tool.name(), "echo_plugin");
        assert_eq!(tool.permission(), Permission::Auto);
    }

    #[test]
    fn test_expand_command() {
        let tool = PluginTool::from_config(test_config());
        let expanded = tool.expand_command(&serde_json::json!({"message": "hello world"}));
        assert_eq!(expanded, "echo hello world");
    }

    #[test]
    fn test_expand_command_multiple_params() {
        let config = PluginToolConfig {
            name: "test".to_string(),
            command: "curl {url} -o {output}".to_string(),
            description: "test".to_string(),
            permission: "auto".to_string(),
            parameters: HashMap::new(),
        };
        let tool = PluginTool::from_config(config);
        let expanded = tool.expand_command(&serde_json::json!({
            "url": "https://example.com",
            "output": "file.txt"
        }));
        assert_eq!(expanded, "curl https://example.com -o file.txt");
    }

    #[test]
    fn test_plugin_tool_call() {
        let tool = PluginTool::from_config(test_config());
        let result = tool
            .call(serde_json::json!({"message": "test123"}))
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("test123"));
    }

    #[test]
    fn test_plugin_permission_confirm() {
        let mut config = test_config();
        config.permission = "confirm".to_string();
        let tool = PluginTool::from_config(config);
        assert_eq!(tool.permission(), Permission::Confirm);
    }

    #[test]
    fn test_plugin_permission_deny() {
        let mut config = test_config();
        config.permission = "deny".to_string();
        let tool = PluginTool::from_config(config);
        assert_eq!(tool.permission(), Permission::Deny);
    }

    #[test]
    fn test_parameters_schema() {
        let tool = PluginTool::from_config(test_config());
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["message"].is_object());
    }

    #[test]
    fn test_load_plugin_tools() {
        let configs = vec![test_config()];
        let tools = load_plugin_tools(&configs);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "echo_plugin");
    }

    #[test]
    fn test_toml_deserialization() {
        let toml_str = r#"
name = "weather"
command = "curl -s 'wttr.in/{location}?format=3'"
description = "天気を取得する"
permission = "auto"

[parameters]
location = { type = "string", description = "都市名" }
"#;
        let config: PluginToolConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.name, "weather");
        assert_eq!(config.parameters.len(), 1);
    }
}
