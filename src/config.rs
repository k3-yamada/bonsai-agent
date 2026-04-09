use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// bonsai-agent設定ファイル
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub model: ModelConfig,
    pub agent: AgentSettings,
    pub safety: SafetyConfig,
    pub memory: MemoryConfig,
    #[serde(default)]
    pub plugins: PluginsConfig,
    #[serde(default)]
    pub mcp: McpConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginsConfig {
    #[serde(default)]
    pub tools: Vec<crate::tools::plugin::PluginToolConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: Vec<crate::tools::mcp_client::McpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    pub server_url: String,
    pub model_id: String,
    pub context_length: u32,
    pub kv_cache_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentSettings {
    pub max_iterations: usize,
    pub max_retries: usize,
    pub shell_timeout_secs: u64,
    pub max_tools_selected: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SafetyConfig {
    pub deny_paths: Vec<String>,
    pub dangerous_patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    pub max_memories: usize,
    pub decay_days: i64,
    pub skill_promotion_threshold: usize,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            server_url: "http://localhost:8080".to_string(),
            model_id: "bonsai-8b".to_string(),
            context_length: 16384,
            kv_cache_type: "q8_0".to_string(),
        }
    }
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            max_iterations: 10,
            max_retries: 3,
            shell_timeout_secs: 30,
            max_tools_selected: 5,
        }
    }
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            deny_paths: vec![
                "~/.ssh".to_string(),
                "~/.gnupg".to_string(),
                "~/.aws".to_string(),
                "/etc/shadow".to_string(),
            ],
            dangerous_patterns: vec![
                "rm -rf".to_string(),
                "sudo".to_string(),
                "chmod 777".to_string(),
            ],
        }
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            max_memories: 1000,
            decay_days: 90,
            skill_promotion_threshold: 3,
        }
    }
}

impl AppConfig {
    /// 設定ファイルを読み込む。存在しなければデフォルト値を使用。
    pub fn load() -> Result<Self> {
        let path = Self::config_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let config: AppConfig = toml::from_str(&content)?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    /// デフォルト設定をファイルに書き出す（初回セットアップ用）
    pub fn save_default() -> Result<PathBuf> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(&Self::default())?;
        std::fs::write(&path, content)?;
        Ok(path)
    }

    /// 設定ファイルのパス
    pub fn config_path() -> PathBuf {
        if let Some(config_dir) = dirs::config_dir() {
            config_dir.join("bonsai-agent").join("config.toml")
        } else {
            PathBuf::from("config.toml")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = AppConfig::default();
        assert_eq!(config.model.server_url, "http://localhost:8080");
        assert_eq!(config.agent.max_iterations, 10);
        assert_eq!(config.safety.deny_paths.len(), 4);
        assert_eq!(config.memory.max_memories, 1000);
    }

    #[test]
    fn test_serialize_deserialize() {
        let config = AppConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: AppConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.model.model_id, "bonsai-8b");
        assert_eq!(parsed.agent.max_retries, 3);
    }

    #[test]
    fn test_partial_config() {
        let toml_str = r#"
[model]
server_url = "http://localhost:9090"

[agent]
max_iterations = 20
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.model.server_url, "http://localhost:9090");
        assert_eq!(config.agent.max_iterations, 20);
        // 未指定の値はデフォルト
        assert_eq!(config.agent.max_retries, 3);
        assert_eq!(config.model.context_length, 16384);
    }

    #[test]
    fn test_load_nonexistent_returns_default() {
        // load()は存在しないファイルでもデフォルト値を返す
        let config = AppConfig::load().unwrap();
        assert_eq!(config.model.model_id, "bonsai-8b");
    }

    #[test]
    fn test_config_path() {
        let path = AppConfig::config_path();
        assert!(path.to_string_lossy().contains("bonsai-agent"));
    }
}
