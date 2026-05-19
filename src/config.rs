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
    #[serde(default)]
    pub experiment: ExperimentConfig,
    #[serde(default)]
    pub fallback_chain: FallbackChainSettings,
}

/// メイン推論フォールバックチェーンの設定（Step 12、opt-in）
///
/// `entries` が空ならフォールバックは無効、設定されていれば連続失敗時に
/// 順次切替する `FallbackChain` を構築する。`AdvisorSettings` の backend
/// フォールバック（advice 専用）とは独立。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FallbackChainSettings {
    /// 連続失敗 N 回で次のエントリへ切替（デフォルト 2）
    pub max_failures: Option<usize>,
    /// 項目 195: フォールバック中の連続成功 N 回でプライマリへ自動復帰
    /// 0 または未指定 = recovery 無効（既存 sticky 挙動、後方互換）
    pub recover_after_n_success: Option<usize>,
    /// プライマリ + フォールバック先のリスト（先頭がプライマリ）
    pub entries: Vec<crate::runtime::model_router::FallbackEntry>,
}

impl FallbackChainSettings {
    /// 設定値からランタイム用 `FallbackChain` を構築。
    ///
    /// `entries` が空なら `None`（フォールバック無効）。
    pub fn build_chain(&self) -> Option<crate::runtime::model_router::FallbackChain> {
        if self.entries.is_empty() {
            return None;
        }
        let threshold = self.max_failures.unwrap_or(2);
        let recover = self.recover_after_n_success.unwrap_or(0);
        Some(crate::runtime::model_router::FallbackChain::with_options(
            self.entries.clone(),
            threshold,
            recover,
        ))
    }
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
    /// 停滞時再計画プロンプト（カスタマイズ用、空文字なら組込みデフォルト）
    pub replan_prompt: String,
    /// バックエンド: "local", "http", "claude-code"（デフォルト: local）
    pub backend: String,
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
            replan_prompt: String::new(),
            backend: String::new(),
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
        use crate::runtime::model_router::{
            AdvisorConfig, DEFAULT_REPLAN_PROMPT, DEFAULT_VERIFICATION_PROMPT,
        };
        let api_key = self
            .api_key
            .clone()
            .or_else(|| Self::detect_api_key(self.api_endpoint.as_deref()));
        let verification_prompt = if self.verification_prompt.is_empty() {
            DEFAULT_VERIFICATION_PROMPT.to_string()
        } else {
            self.verification_prompt.clone()
        };
        let replan_prompt = if self.replan_prompt.is_empty() {
            DEFAULT_REPLAN_PROMPT.to_string()
        } else {
            self.replan_prompt.clone()
        };
        let backend = if self.backend.is_empty() {
            crate::runtime::model_router::AdvisorBackend::default()
        } else {
            crate::runtime::model_router::AdvisorBackend::parse_backend(&self.backend)
        };
        AdvisorConfig {
            max_uses: self.max_uses,
            calls_used: 0,
            max_advisor_tokens: self.max_advisor_tokens,
            api_endpoint: self.api_endpoint.clone(),
            api_key,
            api_model: self.api_model.clone(),
            timeout_secs: self.timeout_secs,
            verification_prompt,
            replan_prompt,
            backend,
            retry_policy: crate::runtime::model_router::RetryPolicy::default(),
            cache: std::collections::HashMap::new(),
            // 項目 210 Self-Verify default OFF (TOML 経由設定は別 PR で追加予定)
            dynamic_skip_threshold: 0.0,
            min_samples_for_skip: 5,
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
    /// プリスクリーニング有効化（少数タスクで事前評価し、明らかな悪化を早期棄却）
    #[serde(default = "default_true")]
    pub enable_prescreening: bool,
    /// プリスクリーニング棄却閾値（推定deltaがこの値未満なら早期棄却）
    #[serde(default = "default_prescreening_threshold")]
    pub prescreening_threshold: f64,
    /// ベンチマークタスク単位のタイムアウト秒数（0=無制限）
    #[serde(default = "default_task_timeout_secs")]
    pub task_timeout_secs: u64,
    /// judge gate 閾値（Phase B2、ADK rubric_based_final_response_quality_v1）
    /// `Some(0.7)` で ACCEPT に judge >= 0.7 の AND 条件を追加。`None` で従来動作。
    #[serde(default)]
    pub judge_threshold: Option<f64>,
    /// judge にかける task 数（負荷制御、デフォルト 4）
    #[serde(default = "default_judge_sample_size")]
    pub judge_sample_size: usize,
}

/// デフォルト: true
fn default_true() -> bool {
    true
}

/// デフォルト: -0.01（プリスクリーニング棄却閾値）
fn default_prescreening_threshold() -> f64 {
    -0.01
}

/// デフォルト: 300秒（5分）タスク単位タイムアウト
fn default_task_timeout_secs() -> u64 {
    300
}

/// デフォルト: 4 タスク（judge gate sample size）
fn default_judge_sample_size() -> usize {
    4
}

impl Default for ExperimentConfig {
    fn default() -> Self {
        Self {
            max_experiments: 10,
            dreamer_interval: 10,
            enable_prescreening: default_true(),
            prescreening_threshold: default_prescreening_threshold(),
            task_timeout_secs: default_task_timeout_secs(),
            judge_threshold: None,
            judge_sample_size: default_judge_sample_size(),
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

/// 推論サーバーの種別
/// 推論パラメータ（config.toml [model.inference] セクション）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InferenceParams {
    pub temperature: f64,
    pub top_p: f64,
    pub top_k: u32,
    pub min_p: f64,
    pub max_tokens: u32,
    pub repeat_penalty: f64,
}

impl Default for InferenceParams {
    fn default() -> Self {
        Self {
            temperature: 0.5,
            top_p: 0.85,
            top_k: 20,
            min_p: 0.05,
            max_tokens: 1024,
            repeat_penalty: 1.15,
        }
    }
}

impl InferenceParams {
    /// llama-server向けデフォルト（Default::defaultと同一、明示的エイリアス）
    pub fn llama_server_default() -> Self {
        Self::default()
    }

    /// MLX最適化プリセット（Ternary Bonsai向け）
    /// - temperature: 0.3（低めでツール呼び出し精度向上）
    /// - top_p: 0.9（やや広めで多様性確保）
    /// - repeat_penalty: 1.1（緩めで自然な応答）
    pub fn mlx_optimized() -> Self {
        Self {
            temperature: 0.3,
            top_p: 0.9,
            top_k: 20,
            min_p: 0.05,
            max_tokens: 1024,
            repeat_penalty: 1.1,
        }
    }

    /// Lab 用 temperature override (`BONSAI_LAB_TEMP` env 経由).
    ///
    /// `.claude/plan/lab-v22-metric-redesign.md` §3.5 = Lab cycle 内 sampling noise 排除のため、
    /// Lab 起動時のみ温度を env から強制 override する。production code (config.toml の
    /// `[model.inference] temperature`) には影響なし — env unset 時は no-op で完全後方互換。
    ///
    /// 戻り値:
    /// - `Some(prev_temp)`: override 適用、prev_temp は元の値
    /// - `None`: env unset / parse 失敗 / 範囲外 (`[0.0, 2.0]` 外)
    ///
    /// 範囲 `[0.0, 2.0]` は llama-server / mlx-lm の標準受け付け範囲。
    pub fn apply_lab_temp_override(&mut self) -> Option<f64> {
        let val = std::env::var("BONSAI_LAB_TEMP").ok()?;
        let parsed: f64 = val.parse().ok()?;
        if !(0.0..=2.0).contains(&parsed) {
            return None;
        }
        let prev = self.temperature;
        self.temperature = parsed;
        Some(prev)
    }
}

// ─── Lab Runtime Stabilization (項目 249、plan lab-runtime-stabilization.md §3) ───
//
// CCG synthesis (Codex SSE timeout root cause + Gemini Iteration Velocity) 経由で
// Lab v22 Phase A 80 min/cycle → 30 min target 達成のための 3 軸 env-gated 修正。

/// `BONSAI_LAB_LONG_SSE=1` で Lab 専用 SSE chunk timeout 60s → 180s に延長.
///
/// MLX 初トークン遅延を catch、non-stream retry + fallback chain 経路の暴走を抑止。
/// production default は 60s 維持で後方互換 (env unset 時 no-op)。
pub fn is_lab_long_sse_timeout() -> bool {
    matches!(
        std::env::var("BONSAI_LAB_LONG_SSE").as_deref(),
        Ok("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

/// `BONSAI_LAB_MLX_ONLY=1` で Lab 専用 fallback chain 無効化.
///
/// MLX primary 専用化、retry chain による 2nd backend 経由 timeout 消滅。
/// noise floor 計測の estimand 純化 (「MLX 単独」評価系)。
pub fn is_lab_mlx_only() -> bool {
    matches!(
        std::env::var("BONSAI_LAB_MLX_ONLY").as_deref(),
        Ok("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

/// `BONSAI_LAB_TASK_LIMIT=N` で Lab cycle 内 task pool 縮小 (smoke triage 用).
///
/// 戻り値: 1..=15 で `Some(N)`、それ以外 (parse 失敗 / 範囲外) で `None` → smoke 既定 15 維持。
/// 5 で smoke wall ~1/3、Lab v22 Phase A 80 → ~27 min/cycle 想定。
pub fn lab_task_limit() -> Option<usize> {
    std::env::var("BONSAI_LAB_TASK_LIMIT")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| (1..=15).contains(n))
}

/// `BONSAI_LAB_*` env を弄る test 間 cross-file mutex (項目 249 用).
#[cfg(test)]
pub(crate) static LAB_RUNTIME_ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `BONSAI_LAB_TEMP` env を弄る test 間の競合回避 (項目 226/229/233/235 同 pattern、
/// cross-file serialize)。`apply_lab_temp_override` test だけでなく、将来 Lab 起動側 test
/// が同 env を弄る際にも参照可能。test build のみコンパイル (release では dead_code)。
#[cfg(test)]
pub(crate) static LAB_TEMP_ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServerBackend {
    /// llama-server (llama.cpp, GGUF)
    #[default]
    LlamaServer,
    /// mlx-lm server (MLX, Apple Silicon最適化)
    MlxLm,
    /// bitnet.cpp (1ビット最適化カーネル、llama-server互換API)
    #[serde(rename = "bitnet")]
    BitNet,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    /// 推論バックエンド（llama-server / mlx-lm / bitnet）
    pub backend: ServerBackend,
    pub server_url: String,
    /// モデルID（例: "bonsai-8b", "ternary-bonsai-8b", "ternary-bonsai-4b"）
    pub model_id: String,
    pub context_length: u32,
    pub kv_cache_type: String,
    /// GGUFファイルパス（llama-server起動時に使用、Noneならconnect専用）
    pub gguf_path: Option<String>,
    /// SSEチャンク間タイムアウト秒数（デフォルト60秒、0で無制限）
    #[serde(default = "default_sse_timeout")]
    pub sse_chunk_timeout_secs: u64,
    /// 推論パラメータ（temperature等）
    #[serde(default)]
    pub inference: InferenceParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentSettings {
    pub soul_path: Option<std::path::PathBuf>,
    pub max_iterations: usize,
    pub max_retries: usize,
    pub shell_timeout_secs: u64,
    pub max_tools_selected: usize,
    /// ツール出力の最大文字数（超過分は切り詰め）
    pub max_tool_output_chars: usize,
    /// コンテキストに含めるツールの最大数（1bitモデルは8以下推奨）
    pub max_tools_in_context: usize,
    /// MCPツールの追加枠（ビルトインとは別枠で確保）
    pub max_mcp_tools_in_context: usize,
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
    /// 項目 179: 追加メモリブロック (Letta candidate 3 完成形)
    ///
    /// SOUL.md (label="persona") は [agent].soul_path で別途扱われる。
    /// ここでは human / scratchpad / system_state 等の追加 block を設定する。
    /// `[[memory.blocks]]` TOML セクションで複数定義可能。
    #[serde(default)]
    pub blocks: Vec<MemoryBlockConfig>,
}

/// 追加メモリブロック設定 (項目 179)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryBlockConfig {
    /// ブロック識別ラベル (例: "human", "scratchpad", "system_state")
    pub label: String,
    /// ブロック内容のファイルパス
    pub path: PathBuf,
}

fn default_sse_timeout() -> u64 {
    60
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            backend: ServerBackend::default(),
            server_url: "http://localhost:8080".to_string(),
            model_id: "bonsai-8b".to_string(),
            context_length: 16384,
            kv_cache_type: "q8_0".to_string(),
            gguf_path: None,
            sse_chunk_timeout_secs: 60,
            inference: InferenceParams::default(),
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
            max_tool_output_chars: 4000,
            max_tools_in_context: 8,
            max_mcp_tools_in_context: 3,
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
            blocks: Vec::new(),
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
        // load()はファイルが存在すればその値を返す（環境依存）
        // デフォルト値の検証は Default trait で行う
        let config = AppConfig::load().unwrap();
        // model_idが何らかの値を持つことだけ確認（環境のconfig.toml依存）
        assert!(!config.model.model_id.is_empty());
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
    fn test_model_config_ternary() {
        let toml_str = r#"
[model]
model_id = "ternary-bonsai-8b"
context_length = 65536
gguf_path = "/path/to/Ternary-Bonsai-8B.gguf"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.model.model_id, "ternary-bonsai-8b");
        assert_eq!(config.model.context_length, 65536);
        assert_eq!(
            config.model.gguf_path.as_deref(),
            Some("/path/to/Ternary-Bonsai-8B.gguf")
        );
    }

    #[test]
    fn test_experiment_config_default() {
        let config = AppConfig::default();
        assert_eq!(config.experiment.max_experiments, 10);
        assert_eq!(config.experiment.dreamer_interval, 10);
    }

    #[test]
    fn test_experiment_config_from_toml() {
        let toml_str = r#"
[experiment]
max_experiments = 20
dreamer_interval = 5
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.experiment.max_experiments, 20);
        assert_eq!(config.experiment.dreamer_interval, 5);
    }

    #[test]
    fn test_detect_api_key_no_endpoint_no_env() {
        // 環境変数を一時的にクリアしないため、戻り値は環境依存
        // 単に呼び出しが panic しないことを確認
        let _ = AdvisorSettings::detect_api_key(None);
        let _ = AdvisorSettings::detect_api_key(Some("https://example.com/v1/chat/completions"));
    }

    #[test]
    fn test_model_config_mlx_backend() {
        let toml_str = r#"
[model]
backend = "mlx-lm"
server_url = "http://localhost:8000"
model_id = "ternary-bonsai-8b"
context_length = 65536
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.model.backend, ServerBackend::MlxLm);
        assert_eq!(config.model.server_url, "http://localhost:8000");
    }

    #[test]
    fn test_model_config_default_backend_is_llama() {
        let config = AppConfig::default();
        assert_eq!(config.model.backend, ServerBackend::LlamaServer);
    }

    #[test]
    fn test_server_backend_serialize_llama() {
        let backend = ServerBackend::LlamaServer;
        let json = serde_json::to_string(&backend).unwrap();
        assert_eq!(json, r#""llama-server""#);
    }

    #[test]
    fn test_server_backend_serialize_mlx() {
        let backend = ServerBackend::MlxLm;
        let json = serde_json::to_string(&backend).unwrap();
        assert_eq!(json, r#""mlx-lm""#);
    }

    #[test]
    fn test_server_backend_deserialize_llama() {
        let backend: ServerBackend = serde_json::from_str(r#""llama-server""#).unwrap();
        assert_eq!(backend, ServerBackend::LlamaServer);
    }

    #[test]
    fn test_server_backend_deserialize_mlx() {
        let backend: ServerBackend = serde_json::from_str(r#""mlx-lm""#).unwrap();
        assert_eq!(backend, ServerBackend::MlxLm);
    }

    #[test]
    fn test_server_backend_toml_roundtrip() {
        let toml_str = r#"
[model]
backend = "llama-server"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.model.backend, ServerBackend::LlamaServer);
        let re_toml = toml::to_string_pretty(&config).unwrap();
        let re_config: AppConfig = toml::from_str(&re_toml).unwrap();
        assert_eq!(re_config.model.backend, ServerBackend::LlamaServer);
    }

    #[test]
    fn test_server_backend_toml_roundtrip_mlx() {
        let toml_str = r#"
[model]
backend = "mlx-lm"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.model.backend, ServerBackend::MlxLm);
        let re_toml = toml::to_string_pretty(&config).unwrap();
        let re_config: AppConfig = toml::from_str(&re_toml).unwrap();
        assert_eq!(re_config.model.backend, ServerBackend::MlxLm);
    }

    #[test]
    fn test_inference_params_default() {
        let params = InferenceParams::default();
        assert!((params.temperature - 0.5).abs() < f64::EPSILON);
        assert_eq!(params.top_k, 20);
        assert_eq!(params.max_tokens, 1024);
    }

    #[test]
    fn test_inference_params_from_toml() {
        let toml_str = r#"
[model]
model_id = "ternary-bonsai-8b"

[model.inference]
temperature = 0.3
top_k = 10
max_tokens = 2048
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert!((config.model.inference.temperature - 0.3).abs() < f64::EPSILON);
        assert_eq!(config.model.inference.top_k, 10);
        assert_eq!(config.model.inference.max_tokens, 2048);
        // 未指定はデフォルト
        assert!((config.model.inference.top_p - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn test_mlx_optimized_preset() {
        let params = InferenceParams::mlx_optimized();
        assert!((params.temperature - 0.3).abs() < f64::EPSILON);
        assert!((params.top_p - 0.9).abs() < f64::EPSILON);
        assert_eq!(params.top_k, 20);
        assert!((params.min_p - 0.05).abs() < f64::EPSILON);
        assert_eq!(params.max_tokens, 1024);
        assert!((params.repeat_penalty - 1.1).abs() < f64::EPSILON);
    }

    #[test]
    fn test_llama_server_default_preset() {
        // llama_server_default()はDefault::default()と同一であることを検証
        let preset = InferenceParams::llama_server_default();
        let default = InferenceParams::default();
        assert!((preset.temperature - default.temperature).abs() < f64::EPSILON);
        assert!((preset.top_p - default.top_p).abs() < f64::EPSILON);
        assert_eq!(preset.top_k, default.top_k);
        assert!((preset.min_p - default.min_p).abs() < f64::EPSILON);
        assert_eq!(preset.max_tokens, default.max_tokens);
        assert!((preset.repeat_penalty - default.repeat_penalty).abs() < f64::EPSILON);
    }

    #[test]
    fn test_experiment_config_prescreening_defaults() {
        // デフォルト値でプリスクリーニングが有効、閾値が-0.01であることを検証
        let config = AppConfig::default();
        assert!(config.experiment.enable_prescreening);
        assert!((config.experiment.prescreening_threshold - (-0.01)).abs() < f64::EPSILON);
    }

    #[test]
    fn test_experiment_config_prescreening_from_toml() {
        // TOML設定からプリスクリーニング設定を読み込めることを検証
        let toml_str = r#"
[experiment]
max_experiments = 5
enable_prescreening = false
prescreening_threshold = -0.05
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.experiment.max_experiments, 5);
        assert!(!config.experiment.enable_prescreening);
        assert!((config.experiment.prescreening_threshold - (-0.05)).abs() < f64::EPSILON);
        // 未指定フィールドはデフォルト値
        assert_eq!(config.experiment.dreamer_interval, 10);
    }

    #[test]
    fn test_mcp_config_with_url_toml() {
        // MCP HTTP transport（urlフィールド）がTOMLで正しく読み込まれることを検証
        let toml_str = r#"
[[mcp.servers]]
name = "stdio-server"
command = "node"
args = ["server.js"]

[[mcp.servers]]
name = "http-server"
command = ""
args = []
url = "http://localhost:3000/mcp"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.mcp.servers.len(), 2);
        // stdio transport
        assert_eq!(config.mcp.servers[0].name, "stdio-server");
        assert!(config.mcp.servers[0].url.is_none());
        // HTTP transport
        assert_eq!(config.mcp.servers[1].name, "http-server");
        assert_eq!(
            config.mcp.servers[1].url.as_deref(),
            Some("http://localhost:3000/mcp")
        );
    }

    #[test]
    fn test_mcp_in_full_config_toml() {
        let toml_str = r#"
[model]
backend = "mlx-lm"
server_url = "http://localhost:8000"
model_id = "ternary-bonsai-8b"
context_length = 65536

[agent]
max_iterations = 10
max_retries = 3

[advisor]
max_uses = 3
backend = "claude-code"

[[mcp.servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.mcp.servers.len(), 1, "MCP servers should be 1");
        assert_eq!(config.mcp.servers[0].name, "filesystem");
    }

    #[test]
    fn test_experiment_config_task_timeout_default() {
        let config = ExperimentConfig::default();
        assert_eq!(config.task_timeout_secs, 300);
    }

    #[test]
    fn test_experiment_config_task_timeout_from_toml() {
        let toml_str = r#"
[experiment]
task_timeout_secs = 600
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.experiment.task_timeout_secs, 600);
    }

    #[test]
    fn test_experiment_config_task_timeout_zero_means_unlimited() {
        let toml_str = r#"
[experiment]
task_timeout_secs = 0
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.experiment.task_timeout_secs, 0);
    }

    #[test]
    fn test_bitnet_backend_from_toml() {
        let toml_str = r#"
[model]
backend = "bitnet"
server_url = "http://localhost:8090"
model_id = "bitnet-3b"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.model.backend, ServerBackend::BitNet);
        assert_eq!(config.model.server_url, "http://localhost:8090");
    }

    // ─── Step 12 FallbackChainSettings tests ──────────────────────────

    #[test]
    fn t_fallback_chain_default_is_empty() {
        let config = AppConfig::default();
        assert!(config.fallback_chain.entries.is_empty());
        assert!(config.fallback_chain.build_chain().is_none());
    }

    #[test]
    fn t_fallback_chain_parse_from_toml() {
        let toml_str = r#"
[fallback_chain]
max_failures = 3

[[fallback_chain.entries]]
backend = "mlx-lm"
model_id = "ternary-bonsai-8b"
server_url = "http://localhost:8000"

[[fallback_chain.entries]]
backend = "llama-server"
model_id = "bonsai-8b-gguf"
server_url = "http://localhost:8080"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.fallback_chain.entries.len(), 2);
        assert_eq!(config.fallback_chain.max_failures, Some(3));
        assert_eq!(
            config.fallback_chain.entries[0].backend,
            ServerBackend::MlxLm
        );
        assert_eq!(
            config.fallback_chain.entries[1].backend,
            ServerBackend::LlamaServer
        );
    }

    #[test]
    fn t_fallback_chain_build_chain_uses_threshold() {
        let toml_str = r#"
[fallback_chain]
max_failures = 5

[[fallback_chain.entries]]
backend = "mlx-lm"
model_id = "primary"
server_url = "http://localhost:8000"

[[fallback_chain.entries]]
backend = "bitnet"
model_id = "fallback"
server_url = "http://localhost:8090"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        let chain = config.fallback_chain.build_chain().expect("should build");
        // 5 回未満では切替しない
        for _ in 0..4 {
            chain.record_failure();
        }
        assert_eq!(chain.current().unwrap().model_id, "primary");
        chain.record_failure(); // 5 回目で切替
        assert_eq!(chain.current().unwrap().model_id, "fallback");
    }

    // --- 項目 179: [[memory.blocks]] 設定対応テスト群 ---

    #[test]
    fn test_memory_blocks_default_empty() {
        let config = AppConfig::default();
        assert!(
            config.memory.blocks.is_empty(),
            "デフォルトでは追加 block なし"
        );
    }

    #[test]
    fn test_memory_blocks_from_toml() {
        let toml_str = r#"
[[memory.blocks]]
label = "human"
path = "/tmp/human.md"

[[memory.blocks]]
label = "scratchpad"
path = "/tmp/scratchpad.md"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.memory.blocks.len(), 2, "2 block 定義される");
        assert_eq!(config.memory.blocks[0].label, "human");
        assert_eq!(
            config.memory.blocks[0].path.to_str().unwrap(),
            "/tmp/human.md"
        );
        assert_eq!(config.memory.blocks[1].label, "scratchpad");
    }

    #[test]
    fn test_memory_blocks_backward_compat_no_section() {
        // 旧 config (memory セクションも blocks も指定なし) でパース成功
        let toml_str = r#"
[agent]
max_iterations = 20
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert!(
            config.memory.blocks.is_empty(),
            "旧 config でも blocks フィールドはデフォルト空"
        );
        assert_eq!(config.memory.max_memories, 1000, "他フィールドもデフォルト");
    }

    // ─── BONSAI_LAB_TEMP env override tests (項目 247 Phase C、plan §3.5) ───────────
    //
    // `LAB_TEMP_ENV_TEST_LOCK` で cross-file serialize、各 test 末尾で env を必ず unset
    // して隣接 test に副作用を残さない (FACTCHECK_ALL_ENV_TEST_LOCK 同 pattern)。

    #[test]
    fn t_apply_lab_temp_override_unset_returns_none() {
        let _g = LAB_TEMP_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::remove_var("BONSAI_LAB_TEMP") };
        let mut p = InferenceParams::default();
        let original = p.temperature;
        let r = p.apply_lab_temp_override();
        assert!(r.is_none(), "env unset で None 戻り");
        assert_eq!(p.temperature, original, "env unset では temperature 不変");
    }

    #[test]
    fn t_apply_lab_temp_override_valid_zero() {
        let _g = LAB_TEMP_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::set_var("BONSAI_LAB_TEMP", "0") };
        let mut p = InferenceParams::default();
        let r = p.apply_lab_temp_override();
        assert_eq!(r, Some(0.5), "default temperature 0.5 が prev として返る");
        assert_eq!(
            p.temperature, 0.0,
            "env=\"0\" で temperature=0.0 に override"
        );
        unsafe { std::env::remove_var("BONSAI_LAB_TEMP") };
    }

    #[test]
    fn t_apply_lab_temp_override_valid_decimal() {
        let _g = LAB_TEMP_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::set_var("BONSAI_LAB_TEMP", "0.3") };
        let mut p = InferenceParams::default();
        let r = p.apply_lab_temp_override();
        assert_eq!(r, Some(0.5));
        assert!(
            (p.temperature - 0.3).abs() < f64::EPSILON,
            "env=\"0.3\" で temperature=0.3 に override"
        );
        unsafe { std::env::remove_var("BONSAI_LAB_TEMP") };
    }

    #[test]
    fn t_apply_lab_temp_override_invalid_parse() {
        let _g = LAB_TEMP_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::set_var("BONSAI_LAB_TEMP", "not_a_number") };
        let mut p = InferenceParams::default();
        let original = p.temperature;
        let r = p.apply_lab_temp_override();
        assert!(r.is_none(), "parse 失敗で None");
        assert_eq!(p.temperature, original, "parse 失敗時は temperature 不変");
        unsafe { std::env::remove_var("BONSAI_LAB_TEMP") };
    }

    #[test]
    fn t_apply_lab_temp_override_out_of_range_negative() {
        let _g = LAB_TEMP_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::set_var("BONSAI_LAB_TEMP", "-1") };
        let mut p = InferenceParams::default();
        let original = p.temperature;
        let r = p.apply_lab_temp_override();
        assert!(r.is_none(), "範囲外負値で None");
        assert_eq!(p.temperature, original);
        unsafe { std::env::remove_var("BONSAI_LAB_TEMP") };
    }

    #[test]
    fn t_apply_lab_temp_override_out_of_range_high() {
        let _g = LAB_TEMP_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::set_var("BONSAI_LAB_TEMP", "3.5") };
        let mut p = InferenceParams::default();
        let original = p.temperature;
        let r = p.apply_lab_temp_override();
        assert!(r.is_none(), "範囲外 (>2.0) で None");
        assert_eq!(p.temperature, original);
        unsafe { std::env::remove_var("BONSAI_LAB_TEMP") };
    }

    // ─── Lab Runtime Stabilization (項目 249) env getter tests ─────────────────────
    //
    // `LAB_RUNTIME_ENV_TEST_LOCK` で cross-file serialize、3 env var (BONSAI_LAB_LONG_SSE /
    // BONSAI_LAB_MLX_ONLY / BONSAI_LAB_TASK_LIMIT) を保護。

    #[test]
    fn t_lab_long_sse_timeout_default_off() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::remove_var("BONSAI_LAB_LONG_SSE") };
        assert!(!is_lab_long_sse_timeout(), "env unset で long sse OFF");
    }

    #[test]
    fn t_lab_mlx_only_env_gate_active() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::set_var("BONSAI_LAB_MLX_ONLY", "1") };
        assert!(is_lab_mlx_only(), "env=\"1\" で mlx-only ON");
        unsafe { std::env::remove_var("BONSAI_LAB_MLX_ONLY") };
        assert!(!is_lab_mlx_only(), "env unset で mlx-only OFF");
    }

    #[test]
    fn t_lab_task_limit_env_parse() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::set_var("BONSAI_LAB_TASK_LIMIT", "5") };
        assert_eq!(lab_task_limit(), Some(5), "env=\"5\" で Some(5)");
        unsafe { std::env::remove_var("BONSAI_LAB_TASK_LIMIT") };
    }

    #[test]
    fn t_lab_task_limit_env_out_of_range() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // 0 (下限外)
        unsafe { std::env::set_var("BONSAI_LAB_TASK_LIMIT", "0") };
        assert_eq!(lab_task_limit(), None, "env=0 で None");
        // 16 (上限外)
        unsafe { std::env::set_var("BONSAI_LAB_TASK_LIMIT", "16") };
        assert_eq!(lab_task_limit(), None, "env=16 (15超) で None");
        // parse 失敗
        unsafe { std::env::set_var("BONSAI_LAB_TASK_LIMIT", "abc") };
        assert_eq!(lab_task_limit(), None, "parse 失敗で None");
        unsafe { std::env::remove_var("BONSAI_LAB_TASK_LIMIT") };
    }
}
