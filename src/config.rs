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
    #[serde(default)]
    pub hooks: crate::tools::hooks::HooksConfig,
    #[serde(default)]
    pub advisor: AdvisorSettings,
}

/// アドバイザー設定（config.toml向け、AdvisorConfig::default()ベース）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AdvisorSettings {
    /// 完了前自己検証の最大呼出回数
    pub max_uses: usize,
    /// アドバイザー応答の最大トークン数
    pub max_advisor_tokens: usize,
    /// 外部APIエンドポイント（None = ローカル検証プロンプトのみ）
    pub api_endpoint: Option<String>,
    /// API認証キー（指定なし時は env から自動検出）
    pub api_key: Option<String>,
    /// 使用モデル名
    pub api_model: Option<String>,
    /// HTTPタイムアウト秒
    pub timeout_secs: u64,
    /// 検証プロンプト（カスタマイズ用、空文字なら組込みデフォルト）
    pub verification_prompt: String,
}

impl Default for AdvisorSettings {
    fn default() -> Self {
        Self {
            max_uses: 3,
            max_advisor_tokens: 700,
            api_endpoint: None,
            api_key: None,
            api_model: None,
            timeout_secs: 10,
            verification_prompt: String::new(), // 空 = ランタイムでDEFAULTを使用
        }
    }
}

impl AdvisorSettings {
    /// 実行時用 AdvisorConfig に変換（環境変数からAPIキー自動検出）
    ///
    /// API キー解決順序:
    /// 1. config.toml の api_key
    /// 2. endpoint URL に基づく環境変数（openai → OPENAI_API_KEY、anthropic → ANTHROPIC_API_KEY）
    /// 3. OPENAI_API_KEY → ANTHROPIC_API_KEY（汎用フォールバック）
    pub fn to_runtime(&self) -> crate::runtime::model_router::AdvisorConfig {
        use crate::runtime::model_router::{AdvisorConfig, DEFAULT_VERIFICATION_PROMPT};
        let api_key = self.api_key.clone().or_else(|| Self::detect_api_key(self.api_endpoint.as_deref()));
        let prompt = if self.verification_prompt.is_empty() {
            DEFAULT_VERIFICATION_PROMPT.to_string()
        } else {
            self.verification_prompt.clone()
        };
        AdvisorConfig {
            max_uses: self.max_uses,
            calls_used: 0,
            max_advisor_tokens: self.max_advisor_tokens,
            api_endpoint: self.api_endpoint.clone(),
            api_key,
            api_model: self.api_model.clone(),
            timeout_secs: self.timeout_secs,
            verification_prompt: prompt,
        }
    }

    /// エンドポイントURLから環境変数を推定して取得
    fn detect_api_key(endpoint: Option<&str>) -> Option<String> {
        let endpoint_lower = endpoint.map(|e| e.to_lowercase()).unwrap_or_default();
        // ベンダー固有の優先順位
        if endpoint_lower.contains("openai") {
            return std::env::var("OPENAI_API_KEY").ok();
        }
        if endpoint_lower.contains("anthropic") {
            return std::env::var("ANTHROPIC_API_KEY").ok();
        }
        // 汎用フォールバック: OPENAI 優先
        std::env::var("OPENAI_API_KEY")
            .ok()
            .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExperimentConfig {
    pub max_experiments: usize,
    pub dreamer_interval: usize,
}

impl Default for ExperimentConfig {
    fn default() -> Self {
        Self {
            max_experiments: 10,
            dreamer_interval: 10,
        }
    }
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
    pub soul_path: Option<std::path::PathBuf>,
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
            soul_path: None,
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

    #[test]
    fn test_soul_path_default_none() {
        let config = AppConfig::default();
        assert!(config.agent.soul_path.is_none());
    }

    #[test]
    fn test_soul_path_from_toml() {
        let toml_str = r#"
[agent]
soul_path = "/tmp/SOUL.md"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.agent.soul_path.as_ref().unwrap().to_str().unwrap(),
            "/tmp/SOUL.md"
        );
    }

    #[test]
    fn test_advisor_default() {
        let config = AppConfig::default();
        assert_eq!(config.advisor.max_uses, 3);
        assert!(config.advisor.api_endpoint.is_none());
        assert_eq!(config.advisor.timeout_secs, 10);
    }

    #[test]
    fn test_advisor_from_toml() {
        let toml_str = r#"
[advisor]
api_endpoint = "https://api.openai.com/v1/chat/completions"
api_model = "gpt-4o-mini"
max_uses = 5
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.advisor.api_endpoint.as_deref().unwrap(),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(config.advisor.api_model.as_deref().unwrap(), "gpt-4o-mini");
        assert_eq!(config.advisor.max_uses, 5);
    }

    #[test]
    fn test_advisor_to_runtime_uses_default_prompt_when_empty() {
        let settings = AdvisorSettings {
            verification_prompt: String::new(),
            ..Default::default()
        };
        let runtime = settings.to_runtime();
        assert!(runtime.verification_prompt.contains("検証"));
    }

    #[test]
    fn test_advisor_to_runtime_preserves_custom_prompt() {
        let settings = AdvisorSettings {
            verification_prompt: "カスタム".to_string(),
            ..Default::default()
        };
        let runtime = settings.to_runtime();
        assert_eq!(runtime.verification_prompt, "カスタム");
    }

    #[test]
    fn test_advisor_to_runtime_explicit_api_key_takes_precedence() {
        let settings = AdvisorSettings {
            api_endpoint: Some("https://api.openai.com/v1/chat/completions".to_string()),
            api_key: Some("sk-explicit-key".to_string()),
            ..Default::default()
        };
        let runtime = settings.to_runtime();
        assert_eq!(runtime.api_key.as_deref(), Some("sk-explicit-key"));
    }

    #[test]
    fn test_detect_api_key_no_endpoint_no_env() {
        // 環境変数を一時的にクリアしないため、戻り値は環境依存
        // 単に呼び出しが panic しないことを確認
        let _ = AdvisorSettings::detect_api_key(None);
        let _ = AdvisorSettings::detect_api_key(Some("https://example.com/v1/chat/completions"));
    }
}
