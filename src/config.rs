use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::domain::model_profile::{self, InferenceDefaults, ModelProfile};

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
    #[serde(default)]
    pub sensors: SensorsConfig,
}

/// 外部知覚センサー群の設定 (フェーズ1 & フェーズ2)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SensorsConfig {
    /// センサーシステム全体の有効/無効（キルスイッチ）
    pub enabled: bool,
    /// ファイル監視（FileWatchSensor）の有効/無効
    pub file_watch: bool,
    /// アイドル検知（IdleSensor）の有効/無効
    pub idle_detection: bool,
    /// ウィンドウ/フォーカス監視（WindowChangedSensor）の有効/無効（フェーズ2、デフォルトOFF）
    pub window_focus: bool,
    /// アイドル判定までの秒数
    pub idle_threshold_secs: u64,
    /// ウィンドウ通知の最小クールダウン秒数
    pub window_cooldown_secs: u64,
    /// ウィンドウ監視の拒否アプリ/バンドル名リスト
    pub window_denylist: Vec<String>,
}

impl Default for SensorsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            file_watch: true,
            idle_detection: true,
            window_focus: false, // 安全のためデフォルトOFF
            idle_threshold_secs: 300,
            window_cooldown_secs: 30,
            window_denylist: vec![
                "1password".to_string(),
                "bitwarden".to_string(),
                "keychain".to_string(),
                "keepass".to_string(),
                "bank".to_string(),
                "login".to_string(),
                "signin".to_string(),
            ],
        }
    }
}

/// 環境変数 `BONSAI_SENSOR_WINDOW` によるウィンドウ監視の強制有効化/無効化
pub fn is_window_sensor_enabled_env() -> Option<bool> {
    std::env::var("BONSAI_SENSOR_WINDOW")
        .ok()
        .and_then(|v| match v.trim() {
            "1" | "true" | "TRUE" => Some(true),
            "0" | "false" | "FALSE" => Some(false),
            _ => None,
        })
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
    /// `#[serde(skip_serializing)]`: `--init` 等での config.toml 書き出し時に平文で
    /// 残さないため (Issue #15)。deserialize (config.toml からの読み込み) は従来どおり可能。
    #[serde(skip_serializing)]
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
            cancel: None,
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

impl From<InferenceDefaults> for InferenceParams {
    fn from(d: InferenceDefaults) -> Self {
        Self {
            temperature: d.temperature,
            top_p: d.top_p,
            top_k: d.top_k,
            min_p: d.min_p,
            max_tokens: d.max_tokens,
            repeat_penalty: d.repeat_penalty,
        }
    }
}

impl Default for InferenceParams {
    /// `model_profile::default_profile()` (= MiniCPM5-2B) の推奨初期値から導出。
    fn default() -> Self {
        model_profile::default_profile().inference.into()
    }
}

impl InferenceParams {
    /// `InferenceDefaults` から明示的に生成するコンストラクタ（`From` の別名）。
    pub fn from_defaults(d: InferenceDefaults) -> Self {
        d.into()
    }

    /// `explicit` でキー単位に明示指定されていないフィールドだけを `defaults` (profile 値)
    /// で上書きする (M-1)。明示指定されているキーは現在値を維持する。冪等 — 同じ
    /// `defaults`/`explicit` で複数回呼んでも結果は変わらない。
    pub fn apply_profile_defaults(
        &mut self,
        defaults: InferenceDefaults,
        explicit: InferenceExplicitKeys,
    ) {
        if !explicit.temperature {
            self.temperature = defaults.temperature;
        }
        if !explicit.top_p {
            self.top_p = defaults.top_p;
        }
        if !explicit.top_k {
            self.top_k = defaults.top_k;
        }
        if !explicit.min_p {
            self.min_p = defaults.min_p;
        }
        if !explicit.max_tokens {
            self.max_tokens = defaults.max_tokens;
        }
        if !explicit.repeat_penalty {
            self.repeat_penalty = defaults.repeat_penalty;
        }
    }

    /// llama-server向けデフォルト（Default::defaultと同一、明示的エイリアス）
    pub fn llama_server_default() -> Self {
        Self::default()
    }

    /// MLX最適化プリセット（legacy Ternary Bonsai 向け preset、`ternary-bonsai-8b` profile と同値）
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

    /// Lab 用 repeat_penalty override (`BONSAI_LAB_REPEAT_PENALTY` env 経由).
    ///
    /// Issue #12 Phase -1: 推論パラメータ (temp/repeat_penalty) の paired 検証を
    /// production code (config.toml の `[model.inference] repeat_penalty`) に影響させず
    /// Lab 起動時のみ強制 override するため、`apply_lab_temp_override` と同一パターンで
    /// 追加する。env unset 時は no-op で完全後方互換。
    ///
    /// 戻り値:
    /// - `Some(prev_repeat_penalty)`: override 適用、prev は元の値
    /// - `None`: env unset / parse 失敗 / 範囲外 (`[0.0, 2.0]` 外)
    ///
    /// 範囲 `[0.0, 2.0]` は `apply_lab_temp_override` と同様、llama-server の標準受け付け範囲。
    pub fn apply_lab_repeat_penalty_override(&mut self) -> Option<f64> {
        let val = std::env::var("BONSAI_LAB_REPEAT_PENALTY").ok()?;
        let parsed: f64 = val.parse().ok()?;
        if !(0.0..=2.0).contains(&parsed) {
            return None;
        }
        let prev = self.repeat_penalty;
        self.repeat_penalty = parsed;
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

/// `BONSAI_LAB_MLX_ALLOW_UNKNOWN=1` で `BONSAI_LAB_MLX_ONLY=1` 適用時に未知 model_id
/// (既知 profile に一致しない、独自 MLX repo 指定等) を opt-in で許容する (#17-1)。
///
/// 既定 OFF: 未知 model_id は `apply_lab_overrides` が `Err` を返す (クロスモデル混成防止)。
pub fn is_lab_mlx_allow_unknown() -> bool {
    matches!(
        std::env::var("BONSAI_LAB_MLX_ALLOW_UNKNOWN").as_deref(),
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

/// `BONSAI_LAB_MLX_WARMUP=1` で Lab 専用 MLX server pre-warm gate (項目 252 候補、F4 案 A).
///
/// MLX 2-bit primary の cold start latency を Lab cycle 計時前に消化、F1 (sse 180s) と併用
/// で Lab v22 paired 5h 完走を目標化。production default OFF (env unset 時 no-op)。
///
/// Phase 2 Green 実装: 既存 F2/F3 getter と同形 matches! 判定 (`1` / `true` / `yes` 系を ON).
pub fn is_lab_mlx_warmup() -> bool {
    matches!(
        std::env::var("BONSAI_LAB_MLX_WARMUP").as_deref(),
        Ok("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

/// `BONSAI_CONCEPT_SYNTHESIS=1` で概念ページ合成 (知識基盤強化 Phase 2) を有効化。
///
/// 既定 OFF (env unset = 後方互換 no-op)。概念ページは複数 source 横断の合成成果物で
/// LLM call を伴うため、daemon/手動トリガで明示有効化する。証拠ゲート (LongMemEval-S paired)
/// で recall 改善が ACCEPT されるまで default OFF を維持する。
pub fn is_concept_synthesis_enabled() -> bool {
    matches!(
        std::env::var("BONSAI_CONCEPT_SYNTHESIS").as_deref(),
        Ok("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

/// `BONSAI_CONCEPT_EVAL=1` で LongMemEval-S 証拠ゲート (知識基盤強化 Phase 4b) の concept ON arm を有効化。
///
/// 既定 OFF。ON arm は各 entry の haystack session を pseudo entry に写像して概念候補を検出し、
/// 実 LLM backend で合成した概念 memory を同一 in-memory store に追加 index する (eval-only、
/// production recall は不変)。ADR-003 paired で R@5 改善を判定するための eval scaffold。
/// `is_concept_synthesis_enabled` (production 経路) とは独立: eval ON arm はこの getter のみで制御する。
pub fn is_concept_eval_enabled() -> bool {
    matches!(
        std::env::var("BONSAI_CONCEPT_EVAL").as_deref(),
        Ok("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

/// `BONSAI_MLX_AUTO_CLAMP=1` の場合、起動時に MLX server の `/props` から `n_ctx` を取得して
/// `ModelConfig.context_length` を `min(configured, server_n_ctx)` にクランプする。
///
/// 環境変数未設定時は既存挙動を維持（副作用ゼロ）。
pub fn is_mlx_auto_clamp() -> bool {
    matches!(
        std::env::var("BONSAI_MLX_AUTO_CLAMP").as_deref(),
        Ok("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

/// `BONSAI_LAB_MLX_WARMUP_COUNT=N` で pre-warm 投入回数 override (1..=10、項目 252 候補、F4 案 A).
///
/// 戻り値: 1..=10 で `Some(N)`、parse 失敗 / 範囲外 / unset で `None` → caller 側 `unwrap_or(3)` 想定.
///
/// Phase 2 Green 実装: 既存 `lab_task_limit` と同形 parse + filter.
pub fn lab_mlx_warmup_count() -> Option<usize> {
    std::env::var("BONSAI_LAB_MLX_WARMUP_COUNT")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| (1..=10).contains(n))
}

/// 項目 252 M2 Phase 2 Green: `BONSAI_LAB_MLX_WARMUP_TIMEOUT_SECS` env で per-iter wall budget override.
///
/// plan: `.claude/plan/lab-mlx-prewarm-timeout-bound.md` 案 A (per-iter wall budget).
///
/// 動作:
/// - env unset で default 180 (F1 sse_chunk_timeout 整合、MLX 2-bit cold start 取りこぼし回避)
/// - env=N (1..=600) で N を return
/// - env=0 sentinel で 0 (caller 側で「timeout 無効化、素朴 loop」経路を選択)
/// - env=範囲外 / parse 失敗で default 180 fallback
///
/// caller (`lab_mlx_prewarm`) は戻り値 0 で素朴 loop、>0 で thread::scope + recv_timeout 経路.
pub fn lab_mlx_warmup_timeout_secs() -> u64 {
    const DEFAULT: u64 = 180;
    match std::env::var("BONSAI_LAB_MLX_WARMUP_TIMEOUT_SECS") {
        Ok(s) => match s.parse::<u64>() {
            // env=0 sentinel: 0 を passthrough (caller で素朴 loop 経路)
            Ok(0) => 0,
            // env=N (1..=600): N を return
            Ok(n) if (1..=600).contains(&n) => n,
            // range out / parse 失敗: default 180 fallback
            _ => DEFAULT,
        },
        // unset: default 180
        Err(_) => DEFAULT,
    }
}

/// B-1: MLX server idle timeout 秒数。`BONSAI_MLX_IDLE_TIMEOUT_SEC` env で指定。
///
/// 戻り値: parse 成功で N、unset / parse 失敗 / 0 で 0。
/// 0 = lifecycle supervisor 全体 OFF (既存挙動 100% 保持)。
pub fn mlx_idle_timeout_sec() -> u64 {
    std::env::var("BONSAI_MLX_IDLE_TIMEOUT_SEC")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
}

/// B-1: MLX server 起動プログラムパス。`BONSAI_MLX_SPAWN_PROGRAM` env 優先、
/// 未指定時は scripts/start-mlx-server.sh と同じ `~/.venvs/bonsai-mlx/bin/mlx-openai-server`。
pub fn mlx_spawn_program() -> String {
    std::env::var("BONSAI_MLX_SPAWN_PROGRAM").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{}/.venvs/bonsai-mlx/bin/mlx-openai-server", home)
    })
}

/// `BONSAI_LAB_*` env を弄る test 間 cross-file mutex (項目 249 用).
#[cfg(test)]
pub(crate) static LAB_RUNTIME_ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `BONSAI_LAB_TEMP` env を弄る test 間の競合回避 (項目 226/229/233/235 同 pattern、
/// cross-file serialize)。`apply_lab_temp_override` test だけでなく、将来 Lab 起動側 test
/// が同 env を弄る際にも参照可能。test build のみコンパイル (release では dead_code)。
#[cfg(test)]
pub(crate) static LAB_TEMP_ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `BONSAI_LAB_REPEAT_PENALTY` env を弄る test 間の競合回避 (Issue #12 Phase -1、
/// `LAB_TEMP_ENV_TEST_LOCK` 同 pattern)。`apply_lab_repeat_penalty_override` test だけで
/// なく、将来 Lab 起動側 test が同 env を弄る際にも参照可能。
#[cfg(test)]
pub(crate) static LAB_REPEAT_PENALTY_ENV_TEST_LOCK: std::sync::Mutex<()> =
    std::sync::Mutex::new(());

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
    /// Unsloth Desktop (OpenAI互換API、認証付き)
    #[serde(rename = "unsloth")]
    Unsloth,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    /// 推論バックエンド（llama-server / mlx-lm / bitnet / unsloth）
    pub backend: ServerBackend,
    pub server_url: String,
    /// モデルID（例: "minicpm5-2b"（既定）, "bonsai-8b"（legacy）, "ternary-bonsai-8b"（legacy）。
    /// `domain::model_profile` に登録済みプロファイルの一覧がある）
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
    /// API認証キー（Unsloth Desktop / 外部API用。None時は環境変数から自動検出）
    /// `#[serde(skip_serializing)]`: `--init` 等での config.toml 書き出し時に平文で
    /// 残さないため (Issue #15)。deserialize (config.toml からの読み込み) は従来どおり可能。
    #[serde(default, skip_serializing)]
    pub api_key: Option<String>,
    /// TOML で `context_length` / `[model.inference].<key>` が明示指定されていたかどうか
    /// (HIGH-1、M-1 でキー単位に細分化)。`Serialize`/`Deserialize` の対象外 —
    /// `AppConfig::load()` が生の `toml::Value` から `ModelExplicitKeys::from_toml` で
    /// 別途算出し設定する。`apply_profile_defaults` がユーザー明示値を profile 既定値で
    /// 上書きしないために使う。
    #[serde(skip)]
    pub explicit: ModelExplicitKeys,
}

/// `ModelConfig` の各フィールドが TOML で明示指定されていたかどうかのフラグ (HIGH-1)。
///
/// `#[serde(default)]` によるフィールド単位の穴埋めと、profile 由来の既定値適用
/// (`apply_profile_defaults`) を区別するために必要。例えば
/// `[model]\nmodel_id = "bonsai-8b"` のみの TOML では `context_length` はキー自体が
/// 存在しないため `false` になり、profile (bonsai-8b) の `default_context` で
/// 上書きしてよいと判断できる。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModelExplicitKeys {
    pub context_length: bool,
    /// `[model.inference]` の各キー単位の明示指定フラグ (M-1)。
    ///
    /// 旧実装は `inference: bool` (テーブル全体の有無のみ) だったため、
    /// `[model.inference]\ntemperature = 0.9` のように一部キーだけを明示指定した
    /// 場合に、明示していない他のキー (`top_p` 等) が serde のフィールド単位デフォルト
    /// (= `InferenceParams::default()` = 既定 profile 値) で埋まってしまい、
    /// 実際の `model_id` の profile 値とズレる不具合があった (round 2 M-1)。
    pub inference: InferenceExplicitKeys,
}

/// `[model.inference]` の各キーが TOML で明示指定されていたかどうかのフラグ (M-1)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InferenceExplicitKeys {
    pub temperature: bool,
    pub top_p: bool,
    pub top_k: bool,
    pub min_p: bool,
    pub max_tokens: bool,
    pub repeat_penalty: bool,
}

impl ModelExplicitKeys {
    /// 生の TOML (`toml::Value`) から `[model]` セクションの `context_length` キー、
    /// および `[model.inference]` セクションの各キーの有無を判定する純粋関数。
    /// 値の中身は問わずキー存在のみ見る。`[model]` / `[model.inference]` セクション
    /// 自体が無ければ該当フラグは全て `false`。
    pub fn from_toml(value: &toml::Value) -> Self {
        let model_table = value.get("model").and_then(toml::Value::as_table);
        let inference_table = model_table
            .and_then(|t| t.get("inference"))
            .and_then(toml::Value::as_table);
        let has_key = |k: &str| inference_table.is_some_and(|t| t.contains_key(k));
        Self {
            context_length: model_table.is_some_and(|t| t.contains_key("context_length")),
            inference: InferenceExplicitKeys {
                temperature: has_key("temperature"),
                top_p: has_key("top_p"),
                top_k: has_key("top_k"),
                min_p: has_key("min_p"),
                max_tokens: has_key("max_tokens"),
                repeat_penalty: has_key("repeat_penalty"),
            },
        }
    }
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
    #[serde(default)]
    pub autonomy: Option<crate::safety::autonomy::AutonomyLevel>,
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

impl ModelConfig {
    /// `ModelProfile` から `ModelConfig` を生成する（model 切替の single source of truth）。
    /// `api_key` は profile の関心事ではないため `None`（`Default::default()` 側で env から補完）。
    pub fn from_profile(p: &ModelProfile) -> Self {
        Self {
            backend: ServerBackend::default(),
            server_url: "http://localhost:8080".to_string(),
            model_id: p.id.to_string(),
            context_length: p.default_context,
            kv_cache_type: "q8_0".to_string(),
            gguf_path: None,
            sse_chunk_timeout_secs: default_sse_timeout(),
            inference: InferenceParams::from_defaults(p.inference),
            api_key: None,
            explicit: ModelExplicitKeys::default(),
        }
    }

    /// `model_id` (ロード後・`--model`/env 等での上書き解決後) に一致する `ModelProfile` を
    /// 見つけ、TOML で明示指定されていない `context_length` / `inference` を
    /// profile の推奨値で上書きする (HIGH-1)。
    ///
    /// 呼び出し順序: `model_id` を確定した直後 (Lab 系 override より前) に 1 回呼ぶこと。
    /// 戻り値: ヒットした profile。`None` なら未知モデルで何も変更しない。
    pub fn apply_profile_defaults(&mut self) -> Option<&'static ModelProfile> {
        let profile = model_profile::find_profile(&self.model_id)?;
        if !self.explicit.context_length {
            self.context_length = profile.default_context;
        }
        self.inference
            .apply_profile_defaults(profile.inference, self.explicit.inference);
        Some(profile)
    }

    /// `context_length` が `profile.native_context` を超過している場合の警告文言。
    /// 超過していなければ `None`。
    /// (呼び出し側 (main.rs / cli_diagnose.rs) が operator visibility のため出力する。
    /// `src/config.rs` は LOG-001 whitelist 外のため、ここでは stderr 出力を行わない。)
    pub fn native_context_warning(&self, profile: &ModelProfile) -> Option<String> {
        (self.context_length > profile.native_context).then(|| {
            format!(
                "context_length {} が {} の native context {} を超過",
                self.context_length, profile.display_name, profile.native_context
            )
        })
    }

    /// `apply_profile_defaults` + `native_context_warning` をまとめた便宜メソッド
    /// (main.rs の呼び出し行数を抑えるため。SIZE-001 800 行制約対応)。
    /// 出力は呼び出し側の責務 (このメソッド自体は print しない)。
    pub fn apply_profile_defaults_checked(&mut self) -> Option<String> {
        let profile = self.apply_profile_defaults()?;
        self.native_context_warning(profile)
    }
}

/// `resolve_model_id` に渡す 3 つの env override (L-2: 無名タプルではなく named struct にして
/// 呼び出し側での意味の取り違えを防ぐ)。
#[derive(Debug, Clone, Default)]
pub struct ModelIdEnvOverrides {
    /// `BONSAI_MODEL` env
    pub bonsai_model: Option<String>,
    /// `BONSAI_MODEL_ID` env
    pub bonsai_model_id: Option<String>,
    /// `UNSLOTH_MODEL` env
    pub unsloth_model: Option<String>,
}

/// `resolve_model_id` に渡す 3 つの env override をまとめて読む (R-5)。
///
/// `main.rs` の行数を抑えるためのヘルパー (SIZE-001 800 行制約対応)。
pub fn model_id_env_overrides() -> ModelIdEnvOverrides {
    ModelIdEnvOverrides {
        bonsai_model: std::env::var("BONSAI_MODEL").ok(),
        bonsai_model_id: std::env::var("BONSAI_MODEL_ID").ok(),
        unsloth_model: std::env::var("UNSLOTH_MODEL").ok(),
    }
}

/// `apply_lab_overrides` が `BONSAI_LAB_MLX_ONLY=1` を検知して適用した内容
/// (呼び出し元 `main.rs` が operator 向けログ出力で表示するための報告値。ログ出力自体は
/// main.rs 側の責務に留める、LOG-001)。
#[derive(Debug)]
pub struct MlxOnlySwitch {
    pub prev_backend: String,
    pub prev_url: String,
    pub prev_model_id: String,
    pub new_model_id: String,
    pub prev_fallback_entries: usize,
}

/// `apply_lab_overrides` の適用結果まとめ。各フィールドは該当 env が未設定/no-op なら `None`。
#[derive(Debug, Default)]
pub struct LabOverrideReport {
    /// `BONSAI_LAB_TEMP` override 適用時の (prev, new) temperature。
    pub temp_override: Option<(f64, f64)>,
    /// `BONSAI_LAB_REPEAT_PENALTY` override 適用時の (prev, new) repeat_penalty
    /// (Issue #12 Phase -1)。
    pub repeat_penalty_override: Option<(f64, f64)>,
    /// `BONSAI_LAB_LONG_SSE=1` 適用時の prev `sse_chunk_timeout_secs`。
    pub long_sse_applied: Option<u64>,
    /// `BONSAI_LAB_MLX_ONLY=1` 適用時の切替内容。
    pub mlx_only_applied: Option<MlxOnlySwitch>,
}

/// Lab (`--lab`) 起動時のみ有効な 3 種の env override
/// (`BONSAI_LAB_TEMP` / `BONSAI_LAB_LONG_SSE` / `BONSAI_LAB_MLX_ONLY`) を一括適用する。
///
/// `main.rs` の行数を抑えるためのヘルパー (SIZE-001 800 行制約対応)。呼び出し元は `cli.lab`
/// のときのみ呼び、戻り値の各 `Some` を operator 向けログ出力で表示する (このメソッド自体は
/// print しない、LOG-001: config.rs はログ出力 whitelist 対象外)。
///
/// - 項目 247 Phase C: `BONSAI_LAB_TEMP` env で temperature override.
///   `.claude/plan/lab-v22-metric-redesign.md` §3.5 — Lab cycle 内 sampling noise 排除。
///   env unset 時は no-op、completely backward compatible。
/// - Issue #12 Phase -1: `BONSAI_LAB_REPEAT_PENALTY` env で repeat_penalty override
///   (推論パラメータ paired 検証用、`BONSAI_LAB_TEMP` と同一パターン)。env unset 時は no-op。
/// - 項目 249 Phase 2 Green: Lab Runtime Stabilization (CCG synthesis 経由)
///   F1: `BONSAI_LAB_LONG_SSE=1` → SSE chunk timeout 60 → 180 で MLX 初トークン遅延 catch
/// - F2: `BONSAI_LAB_MLX_ONLY=1` → fallback_chain 無効化 + primary backend を MLX に切替
///   (項目 249 Phase 4 Smoke G-RT で fallback クリアのみでは primary llama-server を試行する
///   構造的バグを実機で検出、F2 の本来意図「MLX-only」を完全実現するため primary も切替)。
///   L-6: doc は `domain::model_profile::mlx_only_model_id` 参照。
///
/// #14-2: `mlx_only_model_id` が MLX ビルドなし profile を検出した場合は `Err` を
/// `anyhow::Error` に変換して `?` で伝播する (実装は `map_err(anyhow::anyhow!)?`、クロスモデル
/// 混成の hard error 化)。以前は既定 profile の mlx_repo へ黙って置換していたが、operator の
/// 意図しないモデル混在を招くため撤廃した。
pub fn apply_lab_overrides(app_config: &mut AppConfig) -> Result<LabOverrideReport> {
    let mut report = LabOverrideReport::default();

    if let Some(prev) = app_config.model.inference.apply_lab_temp_override() {
        report.temp_override = Some((prev, app_config.model.inference.temperature));
    }

    if let Some(prev) = app_config
        .model
        .inference
        .apply_lab_repeat_penalty_override()
    {
        report.repeat_penalty_override = Some((prev, app_config.model.inference.repeat_penalty));
    }

    if is_lab_long_sse_timeout() {
        let prev_sse = app_config.model.sse_chunk_timeout_secs;
        app_config.model.sse_chunk_timeout_secs = 180;
        report.long_sse_applied = Some(prev_sse);
    }

    if is_lab_mlx_only() {
        // #14-2: 置換先 model_id を先に解決し、Err ならここで bail する。MLX-only ブロック内の
        // 書き換え (backend / server_url / fallback_chain / model_id) より前に検証し、この
        // ブロックの部分適用を残さない。temp / SSE override は先に適用済みだが、呼び出し元
        // (main) は Err で即終了する。
        let new_model_id = model_profile::mlx_only_model_id(
            &app_config.model.model_id,
            is_lab_mlx_allow_unknown(),
        )
        .map_err(|msg| anyhow::anyhow!(msg))?;
        let prev_entries = app_config.fallback_chain.entries.len();
        app_config.fallback_chain.entries.clear();
        let prev_backend = format!("{:?}", app_config.model.backend);
        let prev_url = app_config.model.server_url.clone();
        app_config.model.backend = ServerBackend::MlxLm;
        app_config.model.server_url = "http://127.0.0.1:8000".to_string();
        let prev_model_id = std::mem::replace(&mut app_config.model.model_id, new_model_id.clone());
        report.mlx_only_applied = Some(MlxOnlySwitch {
            prev_backend,
            prev_url,
            prev_model_id,
            new_model_id,
            prev_fallback_entries: prev_entries,
        });
    }

    Ok(report)
}

impl Default for ModelConfig {
    fn default() -> Self {
        let api_key = std::env::var("UNSLOTH_API_KEY")
            .ok()
            .or_else(|| std::env::var("BONSAI_API_KEY").ok());
        Self {
            api_key,
            ..Self::from_profile(model_profile::default_profile())
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
            autonomy: None,
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
        let mut config: AppConfig = if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let mut parsed: AppConfig = toml::from_str(&content)?;
            // HIGH-1: `context_length` / `[model.inference]` の明示指定有無を生 TOML から
            // 記録する。`apply_profile_defaults` (main.rs が model_id 解決直後に呼ぶ) が
            // ユーザー明示値を profile 既定値で上書きしないために必要。
            let raw: toml::Value = toml::from_str(&content)?;
            parsed.model.explicit = ModelExplicitKeys::from_toml(&raw);
            parsed
        } else {
            Self::default()
        };
        if config.model.api_key.is_none() {
            config.model.api_key = std::env::var("UNSLOTH_API_KEY")
                .ok()
                .or_else(|| std::env::var("BONSAI_API_KEY").ok());
        }
        // M-3: config ファイルの model_id に対して profile 既定値を適用する。
        // 従来は main.rs (`--model`/env での model_id 解決後) でのみ適用していたため、
        // `longmemeval_bench` 等 main.rs を経由しない全バイナリが恩恵を受けられなかった。
        // ここで適用しても、main.rs は resolve_model_id 解決後に
        // `apply_profile_defaults_checked()` を再度呼ぶため、CLI/env で model_id が
        // 変わった場合の profile 切替は引き続き効く (explicit フラグはロード時のまま
        // 保持されるので、ファイル未指定キーだけが新 profile 値に切り替わる)。
        config.model.apply_profile_defaults();
        Ok(config)
    }

    /// デフォルト設定をファイルに書き出す（初回セットアップ用）
    pub fn save_default() -> Result<PathBuf> {
        let path = Self::config_path();
        Self::save_default_to(&path)?;
        Ok(path)
    }

    /// `save_default()` の実体。書き出し先を明示指定できるようテスト用に分離 (Issue #15)。
    ///
    /// `api_key` フィールドは `#[serde(skip_serializing)]` により内容に含まれないが、
    /// 念のためファイル権限も unix では `0o600` (owner 読み書きのみ) に揃える。
    /// 既存ファイルがある場合は上書きする (従来どおり)。
    fn save_default_to(path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(&Self::default())?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(path)?;
            std::io::Write::write_all(&mut file, content.as_bytes())?;
            // 既存ファイル (作成前から存在) の場合、`mode()` は umask 適用前の初回作成
            // 時のみ効くため、上書き時も権限を明示的に揃える。
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(path, content)?;
        }

        Ok(())
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

/// SQLite メモリ DB のパスを解決する純粋関数 (I/O なし、testability 用)。
///
/// 優先順位:
/// 1. `BONSAI_DB_PATH` env — 隔離 DB の明示指定。live 検証で production DB を
///    汚染しないための経路 (`HOME=/tmp` ハックの代替)。空白のみは未設定扱い。
/// 2. `<data_dir>/bonsai-agent/bonsai.db`
/// 3. data_dir 不明時は CWD 直下の `bonsai.db`
pub fn resolve_db_path(env_override: Option<&str>, data_dir: Option<PathBuf>) -> PathBuf {
    if let Some(raw) = env_override {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    match data_dir {
        Some(dir) => dir.join("bonsai-agent").join("bonsai.db"),
        None => PathBuf::from("bonsai.db"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用 env var RAII ガード (#17-4)。生成時に env を書き換え、`Drop` で
    /// 無条件に `remove_var` する。assert がスコープ途中で panic しても env が
    /// 確実に外れ、隣接テストへ残留しない (lock はテスト側で別途取得すること)。
    struct EnvGuard(&'static str);

    impl EnvGuard {
        /// env に `value` を設定して guard を返す。
        fn set(key: &'static str, value: &str) -> Self {
            unsafe { std::env::set_var(key, value) };
            Self(key)
        }

        /// env を未設定にしてから guard を返す (Drop 時の cleanup を保証するだけの
        /// no-op 起点、env unset を検証するテスト向け)。
        fn unset(key: &'static str) -> Self {
            unsafe { std::env::remove_var(key) };
            Self(key)
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe { std::env::remove_var(self.0) };
        }
    }

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
        assert_eq!(parsed.model.model_id, model_profile::DEFAULT_MODEL_ID);
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
        assert_eq!(
            config.model.context_length,
            model_profile::default_profile().default_context
        );
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
    fn test_advisor_to_runtime_with_cancel() {
        let settings = AdvisorSettings::default();
        let cancel = crate::cancel::CancellationToken::new();
        let runtime = settings.to_runtime().with_cancel(cancel.clone());
        assert!(runtime.cancel.is_some());
        cancel.cancel();
        assert!(runtime.cancel.as_ref().unwrap().is_cancelled());
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
    fn test_model_config_default_is_profile_derived() {
        // ModelConfig::default() は model_profile::default_profile() (= MiniCPM5-2B) 由来。
        let profile = model_profile::default_profile();
        let config = ModelConfig::default();
        assert_eq!(config.model_id, profile.id);
        assert_eq!(config.context_length, profile.default_context);
        assert!(
            (config.inference.temperature - profile.inference.temperature).abs() < f64::EPSILON
        );
    }

    #[test]
    fn test_model_config_from_profile_uses_profile_values() {
        let profile = model_profile::find_profile("ternary-bonsai-8b").expect("既知プロファイル");
        let config = ModelConfig::from_profile(profile);
        assert_eq!(config.model_id, "ternary-bonsai-8b");
        assert_eq!(config.context_length, 65536);
        assert!((config.inference.temperature - 0.3).abs() < f64::EPSILON);
    }

    // --- HIGH-1: ModelExplicitKeys::from_toml ---

    #[test]
    fn test_model_explicit_keys_from_toml_no_model_section() {
        let raw: toml::Value = toml::from_str("").unwrap();
        let explicit = ModelExplicitKeys::from_toml(&raw);
        assert!(!explicit.context_length);
        assert_eq!(explicit.inference, InferenceExplicitKeys::default());
    }

    #[test]
    fn test_model_explicit_keys_from_toml_context_length_only() {
        let raw: toml::Value = toml::from_str(
            r#"
[model]
model_id = "bonsai-8b"
context_length = 8192
"#,
        )
        .unwrap();
        let explicit = ModelExplicitKeys::from_toml(&raw);
        assert!(explicit.context_length);
        assert_eq!(explicit.inference, InferenceExplicitKeys::default());
    }

    /// M-1: `[model.inference]` の一部キーのみ明示指定した場合、そのキーだけ `true` に
    /// なる (テーブル全体の有無だけを見ていた旧実装からの回帰確認)。
    #[test]
    fn test_model_explicit_keys_from_toml_inference_section_partial_key() {
        let raw: toml::Value = toml::from_str(
            r#"
[model]
model_id = "bonsai-8b"

[model.inference]
temperature = 0.9
"#,
        )
        .unwrap();
        let explicit = ModelExplicitKeys::from_toml(&raw);
        assert!(!explicit.context_length);
        assert!(explicit.inference.temperature);
        assert!(!explicit.inference.top_p);
        assert!(!explicit.inference.top_k);
        assert!(!explicit.inference.min_p);
        assert!(!explicit.inference.max_tokens);
        assert!(!explicit.inference.repeat_penalty);
    }

    /// M-1: `[model.inference]` の全キーを明示指定した場合、全て `true` になる。
    #[test]
    fn test_model_explicit_keys_from_toml_inference_section_all_keys() {
        let raw: toml::Value = toml::from_str(
            r#"
[model]
model_id = "bonsai-8b"

[model.inference]
temperature = 0.9
top_p = 0.8
top_k = 10
min_p = 0.1
max_tokens = 512
repeat_penalty = 1.2
"#,
        )
        .unwrap();
        let explicit = ModelExplicitKeys::from_toml(&raw);
        assert!(explicit.inference.temperature);
        assert!(explicit.inference.top_p);
        assert!(explicit.inference.top_k);
        assert!(explicit.inference.min_p);
        assert!(explicit.inference.max_tokens);
        assert!(explicit.inference.repeat_penalty);
    }

    // --- HIGH-1: ModelConfig::apply_profile_defaults ---
    // qa-round1-fixes.md HIGH-1 記載のケース (a)-(e) に対応。

    /// (a) `[model]\nmodel_id = "bonsai-8b"` のみ → apply 後 context_length 16384 /
    /// temperature 0.5 / max_tokens 1024 (bonsai-8b profile 由来)。
    #[test]
    fn test_apply_profile_defaults_case_a_model_id_only() {
        let toml_str = "[model]\nmodel_id = \"bonsai-8b\"\n";
        let mut config: AppConfig = toml::from_str(toml_str).unwrap();
        let raw: toml::Value = toml::from_str(toml_str).unwrap();
        config.model.explicit = ModelExplicitKeys::from_toml(&raw);

        let profile = config.model.apply_profile_defaults();
        assert!(profile.is_some());
        assert_eq!(config.model.context_length, 16384);
        assert!((config.model.inference.temperature - 0.5).abs() < f64::EPSILON);
        assert_eq!(config.model.inference.max_tokens, 1024);
    }

    /// (b) `model_id = "bonsai-8b"` + 明示 `context_length = 8192` → 8192 維持、
    /// inference は profile 由来。
    #[test]
    fn test_apply_profile_defaults_case_b_explicit_context_length_kept() {
        let toml_str = "[model]\nmodel_id = \"bonsai-8b\"\ncontext_length = 8192\n";
        let mut config: AppConfig = toml::from_str(toml_str).unwrap();
        let raw: toml::Value = toml::from_str(toml_str).unwrap();
        config.model.explicit = ModelExplicitKeys::from_toml(&raw);

        config.model.apply_profile_defaults();
        assert_eq!(config.model.context_length, 8192, "明示値は上書きしない");
        assert!((config.model.inference.temperature - 0.5).abs() < f64::EPSILON);
    }

    /// (c) M-1: `[model.inference] temperature = 0.9` **のみ**明示 → temperature は維持、
    /// 明示されていない他キー (top_p/max_tokens/repeat_penalty) は profile (bonsai-8b) 値
    /// で埋まる (旧実装はテーブル全体の有無しか見ておらず、これらが誤って
    /// serde フィールド単位デフォルト = 既定 profile (minicpm5-2b) 値のまま残っていた)。
    #[test]
    fn test_apply_profile_defaults_case_c_partial_inference_key_explicit() {
        let toml_str =
            "[model]\nmodel_id = \"bonsai-8b\"\n\n[model.inference]\ntemperature = 0.9\n";
        let mut config: AppConfig = toml::from_str(toml_str).unwrap();
        let raw: toml::Value = toml::from_str(toml_str).unwrap();
        config.model.explicit = ModelExplicitKeys::from_toml(&raw);

        config.model.apply_profile_defaults();
        assert!(
            (config.model.inference.temperature - 0.9).abs() < f64::EPSILON,
            "明示 temperature は上書きしない"
        );
        assert!(
            (config.model.inference.top_p - 0.85).abs() < f64::EPSILON,
            "未明示 top_p は bonsai-8b profile 値"
        );
        assert_eq!(
            config.model.inference.max_tokens, 1024,
            "未明示 max_tokens は bonsai-8b profile 値"
        );
        assert!(
            (config.model.inference.repeat_penalty - 1.15).abs() < f64::EPSILON,
            "未明示 repeat_penalty は bonsai-8b profile 値"
        );
        assert_eq!(
            config.model.context_length, 16384,
            "context_length は profile 由来"
        );
    }

    /// M-1: `[model.inference]` の全キーを明示指定した場合、apply 後も全て維持される。
    #[test]
    fn test_apply_profile_defaults_all_inference_keys_explicit_kept() {
        let toml_str = r#"
[model]
model_id = "bonsai-8b"

[model.inference]
temperature = 0.9
top_p = 0.8
top_k = 10
min_p = 0.1
max_tokens = 512
repeat_penalty = 1.2
"#;
        let mut config: AppConfig = toml::from_str(toml_str).unwrap();
        let raw: toml::Value = toml::from_str(toml_str).unwrap();
        config.model.explicit = ModelExplicitKeys::from_toml(&raw);

        config.model.apply_profile_defaults();
        assert!((config.model.inference.temperature - 0.9).abs() < f64::EPSILON);
        assert!((config.model.inference.top_p - 0.8).abs() < f64::EPSILON);
        assert_eq!(config.model.inference.top_k, 10);
        assert!((config.model.inference.min_p - 0.1).abs() < f64::EPSILON);
        assert_eq!(config.model.inference.max_tokens, 512);
        assert!((config.model.inference.repeat_penalty - 1.2).abs() < f64::EPSILON);
    }

    /// M-3: 同じ model_id に対して `apply_profile_defaults` を複数回呼んでも結果は
    /// 変わらない (main.rs は `AppConfig::load()` 内での適用に加え、`resolve_model_id`
    /// 解決後に再度 `apply_profile_defaults_checked()` を呼ぶため、CLI/env で model_id
    /// が変わらない限り 2 回目の呼び出しは no-op であることが必要)。
    #[test]
    fn test_apply_profile_defaults_idempotent_for_same_model_id() {
        let toml_str = "[model]\nmodel_id = \"bonsai-8b\"\n";
        let mut config: AppConfig = toml::from_str(toml_str).unwrap();
        let raw: toml::Value = toml::from_str(toml_str).unwrap();
        config.model.explicit = ModelExplicitKeys::from_toml(&raw);

        config.model.apply_profile_defaults();
        let context_length_after_first = config.model.context_length;
        let inference_after_first = config.model.inference.clone();

        config.model.apply_profile_defaults();
        assert_eq!(config.model.context_length, context_length_after_first);
        assert!(
            (config.model.inference.temperature - inference_after_first.temperature).abs()
                < f64::EPSILON
        );
        assert!((config.model.inference.top_p - inference_after_first.top_p).abs() < f64::EPSILON);
        assert_eq!(
            config.model.inference.max_tokens,
            inference_after_first.max_tokens
        );
    }

    /// (d) 未知 model_id → 何も変わらず `None`。
    #[test]
    fn test_apply_profile_defaults_case_d_unknown_model_id_noop() {
        let mut config = ModelConfig {
            model_id: "totally-unknown-model-xyz".to_string(),
            context_length: 12345,
            ..ModelConfig::default()
        };
        let before = config.context_length;
        let profile = config.apply_profile_defaults();
        assert!(profile.is_none());
        assert_eq!(config.context_length, before);
    }

    /// (e) `--model` 相当: load 後に model_id を差し替えても profile が効く
    /// (context_length/inference の explicit フラグは model_id とは独立)。
    #[test]
    fn test_apply_profile_defaults_case_e_model_id_overridden_after_load() {
        let toml_str = "[model]\nmodel_id = \"minicpm5-2b\"\n";
        let mut config: AppConfig = toml::from_str(toml_str).unwrap();
        let raw: toml::Value = toml::from_str(toml_str).unwrap();
        config.model.explicit = ModelExplicitKeys::from_toml(&raw);

        // `--model bonsai-8b` 相当の CLI 上書き
        config.model.model_id = "bonsai-8b".to_string();
        let profile = config.model.apply_profile_defaults();
        assert_eq!(profile.map(|p| p.id), Some("bonsai-8b"));
        assert_eq!(config.model.context_length, 16384);
        assert!((config.model.inference.temperature - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_native_context_warning_none_when_within_native() {
        let config = ModelConfig::default();
        let profile = model_profile::default_profile();
        assert!(config.native_context_warning(profile).is_none());
    }

    #[test]
    fn test_native_context_warning_some_when_exceeds_native() {
        let profile = model_profile::find_profile("bonsai-8b").expect("既知プロファイル");
        let config = ModelConfig {
            context_length: profile.native_context + 1,
            ..ModelConfig::from_profile(profile)
        };
        let warning = config.native_context_warning(profile);
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("native context"));
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
    fn test_server_backend_deserialize_unsloth() {
        let backend: ServerBackend = serde_json::from_str(r#""unsloth""#).unwrap();
        assert_eq!(backend, ServerBackend::Unsloth);
    }

    #[test]
    fn test_server_backend_toml_roundtrip_unsloth() {
        let toml_str = r#"
[model]
backend = "unsloth"
server_url = "http://localhost:8000"
model_id = "Qwen3-8B-ERP-v0.1-GGUF"
api_key = "sk-unsloth-testkey"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.model.backend, ServerBackend::Unsloth);
        assert_eq!(config.model.model_id, "Qwen3-8B-ERP-v0.1-GGUF");
        assert_eq!(config.model.api_key.as_deref(), Some("sk-unsloth-testkey"));
        // Issue #15: api_key は `#[serde(skip_serializing)]` のため re-serialize 後の
        // TOML には含まれない (config.toml への平文書き出し防止)。backend 等の他フィールドは
        // 従来どおり round-trip する。
        let re_toml = toml::to_string_pretty(&config).unwrap();
        assert!(!re_toml.contains("sk-unsloth-testkey"));
        let re_config: AppConfig = toml::from_str(&re_toml).unwrap();
        assert_eq!(re_config.model.backend, ServerBackend::Unsloth);
        assert_eq!(re_config.model.api_key, None);
    }

    #[test]
    fn test_inference_params_default() {
        // 値は model_profile::default_profile() (= MiniCPM5-2B) 由来。値の直書き禁止、
        // profile 変更で自動追従させる。
        let expected = model_profile::default_profile().inference;
        let params = InferenceParams::default();
        assert!((params.temperature - expected.temperature).abs() < f64::EPSILON);
        assert_eq!(params.top_k, expected.top_k);
        assert_eq!(params.max_tokens, expected.max_tokens);
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
        // 未指定はデフォルト (= model_profile::default_profile() 由来)
        let expected_top_p = model_profile::default_profile().inference.top_p;
        assert!((config.model.inference.top_p - expected_top_p).abs() < f64::EPSILON);
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
        let _env = EnvGuard::unset("BONSAI_LAB_TEMP");
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
        let _env = EnvGuard::set("BONSAI_LAB_TEMP", "0");
        let default_temp = model_profile::default_profile().inference.temperature;
        let mut p = InferenceParams::default();
        let r = p.apply_lab_temp_override();
        assert_eq!(
            r,
            Some(default_temp),
            "default temperature が prev として返る"
        );
        assert_eq!(
            p.temperature, 0.0,
            "env=\"0\" で temperature=0.0 に override"
        );
    }

    #[test]
    fn t_apply_lab_temp_override_valid_decimal() {
        let _g = LAB_TEMP_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = EnvGuard::set("BONSAI_LAB_TEMP", "0.3");
        let default_temp = model_profile::default_profile().inference.temperature;
        let mut p = InferenceParams::default();
        let r = p.apply_lab_temp_override();
        assert_eq!(r, Some(default_temp));
        assert!(
            (p.temperature - 0.3).abs() < f64::EPSILON,
            "env=\"0.3\" で temperature=0.3 に override"
        );
    }

    #[test]
    fn t_apply_lab_temp_override_invalid_parse() {
        let _g = LAB_TEMP_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = EnvGuard::set("BONSAI_LAB_TEMP", "not_a_number");
        let mut p = InferenceParams::default();
        let original = p.temperature;
        let r = p.apply_lab_temp_override();
        assert!(r.is_none(), "parse 失敗で None");
        assert_eq!(p.temperature, original, "parse 失敗時は temperature 不変");
    }

    #[test]
    fn t_apply_lab_temp_override_out_of_range_negative() {
        let _g = LAB_TEMP_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = EnvGuard::set("BONSAI_LAB_TEMP", "-1");
        let mut p = InferenceParams::default();
        let original = p.temperature;
        let r = p.apply_lab_temp_override();
        assert!(r.is_none(), "範囲外負値で None");
        assert_eq!(p.temperature, original);
    }

    #[test]
    fn t_apply_lab_temp_override_out_of_range_high() {
        let _g = LAB_TEMP_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = EnvGuard::set("BONSAI_LAB_TEMP", "3.5");
        let mut p = InferenceParams::default();
        let original = p.temperature;
        let r = p.apply_lab_temp_override();
        assert!(r.is_none(), "範囲外 (>2.0) で None");
        assert_eq!(p.temperature, original);
    }

    // ─── BONSAI_LAB_REPEAT_PENALTY env override tests (Issue #12 Phase -1、
    // BONSAI_LAB_TEMP と同一パターン) ────────────────────────────────────────────
    //
    // `LAB_REPEAT_PENALTY_ENV_TEST_LOCK` で cross-file serialize、各 test 末尾で env を
    // 必ず unset して隣接 test に副作用を残さない。

    #[test]
    fn t_apply_lab_repeat_penalty_override_unset_returns_none() {
        let _g = LAB_REPEAT_PENALTY_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = EnvGuard::unset("BONSAI_LAB_REPEAT_PENALTY");
        let mut p = InferenceParams::default();
        let original = p.repeat_penalty;
        let r = p.apply_lab_repeat_penalty_override();
        assert!(r.is_none(), "env unset で None 戻り");
        assert_eq!(
            p.repeat_penalty, original,
            "env unset では repeat_penalty 不変"
        );
    }

    #[test]
    fn t_apply_lab_repeat_penalty_override_valid_value() {
        let _g = LAB_REPEAT_PENALTY_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = EnvGuard::set("BONSAI_LAB_REPEAT_PENALTY", "1.1");
        let default_rp = model_profile::default_profile().inference.repeat_penalty;
        let mut p = InferenceParams::default();
        let r = p.apply_lab_repeat_penalty_override();
        assert_eq!(
            r,
            Some(default_rp),
            "default repeat_penalty が prev として返る"
        );
        assert!(
            (p.repeat_penalty - 1.1).abs() < f64::EPSILON,
            "env=\"1.1\" で repeat_penalty=1.1 に override"
        );
    }

    #[test]
    fn t_apply_lab_repeat_penalty_override_invalid_parse() {
        let _g = LAB_REPEAT_PENALTY_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = EnvGuard::set("BONSAI_LAB_REPEAT_PENALTY", "not_a_number");
        let mut p = InferenceParams::default();
        let original = p.repeat_penalty;
        let r = p.apply_lab_repeat_penalty_override();
        assert!(r.is_none(), "parse 失敗で None");
        assert_eq!(
            p.repeat_penalty, original,
            "parse 失敗時は repeat_penalty 不変"
        );
    }

    #[test]
    fn t_apply_lab_repeat_penalty_override_out_of_range_negative() {
        let _g = LAB_REPEAT_PENALTY_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = EnvGuard::set("BONSAI_LAB_REPEAT_PENALTY", "-1");
        let mut p = InferenceParams::default();
        let original = p.repeat_penalty;
        let r = p.apply_lab_repeat_penalty_override();
        assert!(r.is_none(), "範囲外負値で None");
        assert_eq!(p.repeat_penalty, original);
    }

    #[test]
    fn t_apply_lab_repeat_penalty_override_out_of_range_high() {
        let _g = LAB_REPEAT_PENALTY_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = EnvGuard::set("BONSAI_LAB_REPEAT_PENALTY", "3.5");
        let mut p = InferenceParams::default();
        let original = p.repeat_penalty;
        let r = p.apply_lab_repeat_penalty_override();
        assert!(r.is_none(), "範囲外 (>2.0) で None");
        assert_eq!(p.repeat_penalty, original);
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
        let _env = EnvGuard::unset("BONSAI_LAB_LONG_SSE");
        assert!(!is_lab_long_sse_timeout(), "env unset で long sse OFF");
    }

    #[test]
    fn t_lab_mlx_only_env_gate_active() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = EnvGuard::set("BONSAI_LAB_MLX_ONLY", "1");
        assert!(is_lab_mlx_only(), "env=\"1\" で mlx-only ON");
        unsafe { std::env::remove_var("BONSAI_LAB_MLX_ONLY") };
        assert!(!is_lab_mlx_only(), "env unset で mlx-only OFF");
    }

    // #14-2: `BONSAI_LAB_MLX_ONLY=1` かつ MLX ビルドなし legacy profile (`bonsai-8b`) を
    // 指定した場合、`apply_lab_overrides` は `Err` を返す (クロスモデル混成の hard error 化)。
    #[test]
    fn t_apply_lab_overrides_mlx_only_unsupported_profile_is_error() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = EnvGuard::set("BONSAI_LAB_MLX_ONLY", "1");
        let mut app_config = AppConfig::default();
        app_config.model.model_id = "bonsai-8b".to_string();
        let result = apply_lab_overrides(&mut app_config);
        let err = result.expect_err("MLX ビルドなし profile 指定は Err");
        assert!(
            err.to_string().contains("MLX ビルドがありません"),
            "エラーメッセージに理由が含まれること: {err}"
        );
    }

    // ─── #17-3: apply_lab_overrides の Ok 経路 / Err 非変更 テスト ─────────────────

    #[test]
    fn t_apply_lab_overrides_noop_without_env() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // apply_lab_overrides は BONSAI_LAB_TEMP / BONSAI_LAB_REPEAT_PENALTY も読むため
        // LAB_TEMP_ENV_TEST_LOCK / LAB_REPEAT_PENALTY_ENV_TEST_LOCK も併せて保持する
        // (直接系 t_apply_lab_temp_override_* / t_apply_lab_repeat_penalty_override_* との
        // race 防止)。
        let _t = LAB_TEMP_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _rp = LAB_REPEAT_PENALTY_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env_temp = EnvGuard::unset("BONSAI_LAB_TEMP");
        let _env_rp = EnvGuard::unset("BONSAI_LAB_REPEAT_PENALTY");
        let _env_sse = EnvGuard::unset("BONSAI_LAB_LONG_SSE");
        let _env_mlx = EnvGuard::unset("BONSAI_LAB_MLX_ONLY");
        let _env_allow = EnvGuard::unset("BONSAI_LAB_MLX_ALLOW_UNKNOWN");

        let mut app_config = AppConfig::default();
        let prev_backend = format!("{:?}", app_config.model.backend);
        let prev_url = app_config.model.server_url.clone();
        let prev_model_id = app_config.model.model_id.clone();

        let report = apply_lab_overrides(&mut app_config).expect("env 未設定は Ok");

        assert!(
            report.temp_override.is_none(),
            "no-op で temp_override None"
        );
        assert!(
            report.repeat_penalty_override.is_none(),
            "no-op で repeat_penalty_override None"
        );
        assert!(
            report.long_sse_applied.is_none(),
            "no-op で long_sse_applied None"
        );
        assert!(
            report.mlx_only_applied.is_none(),
            "no-op で mlx_only_applied None"
        );
        assert_eq!(
            format!("{:?}", app_config.model.backend),
            prev_backend,
            "no-op で backend 不変"
        );
        assert_eq!(
            app_config.model.server_url, prev_url,
            "no-op で server_url 不変"
        );
        assert_eq!(
            app_config.model.model_id, prev_model_id,
            "no-op で model_id 不変"
        );
    }

    #[test]
    fn t_apply_lab_overrides_long_sse_sets_180() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _t = LAB_TEMP_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env_mlx = EnvGuard::unset("BONSAI_LAB_MLX_ONLY");
        let _env_temp = EnvGuard::unset("BONSAI_LAB_TEMP");
        let _env_sse = EnvGuard::set("BONSAI_LAB_LONG_SSE", "1");

        let mut app_config = AppConfig::default();
        let prev_backend = format!("{:?}", app_config.model.backend);
        let prev_model_id = app_config.model.model_id.clone();

        let report = apply_lab_overrides(&mut app_config).expect("BONSAI_LAB_LONG_SSE=1 は Ok");

        assert_eq!(
            app_config.model.sse_chunk_timeout_secs, 180,
            "BONSAI_LAB_LONG_SSE=1 で sse_chunk_timeout_secs=180"
        );
        assert!(
            report.long_sse_applied.is_some(),
            "report.long_sse_applied が Some"
        );
        assert_eq!(
            format!("{:?}", app_config.model.backend),
            prev_backend,
            "long sse のみでは backend 不変"
        );
        assert_eq!(
            app_config.model.model_id, prev_model_id,
            "long sse のみでは model_id 不変"
        );
    }

    #[test]
    fn t_apply_lab_overrides_mlx_only_switches_known_profile() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _t = LAB_TEMP_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env_temp = EnvGuard::unset("BONSAI_LAB_TEMP");
        let _env_sse = EnvGuard::unset("BONSAI_LAB_LONG_SSE");
        let _env_allow = EnvGuard::unset("BONSAI_LAB_MLX_ALLOW_UNKNOWN");
        let _env_mlx = EnvGuard::set("BONSAI_LAB_MLX_ONLY", "1");

        let mut app_config = AppConfig::default();
        app_config.model.model_id = "minicpm5-2b".to_string();

        let report = apply_lab_overrides(&mut app_config).expect("既知 profile は Ok");

        assert_eq!(format!("{:?}", app_config.model.backend), "MlxLm");
        assert_eq!(app_config.model.server_url, "http://127.0.0.1:8000");
        assert!(
            app_config.fallback_chain.entries.is_empty(),
            "MLX_ONLY で fallback_chain.entries は空"
        );
        assert_eq!(
            app_config.model.model_id,
            model_profile::default_profile().mlx_repo,
            "minicpm5-2b は default_profile().mlx_repo に一致"
        );
        let switch = report
            .mlx_only_applied
            .expect("report.mlx_only_applied が Some");
        assert_eq!(switch.prev_model_id, "minicpm5-2b");
        assert_eq!(switch.new_model_id, app_config.model.model_id);
    }

    #[test]
    fn t_apply_lab_overrides_temp_override() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _t = LAB_TEMP_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env_sse = EnvGuard::unset("BONSAI_LAB_LONG_SSE");
        let _env_mlx = EnvGuard::unset("BONSAI_LAB_MLX_ONLY");
        let _env_temp = EnvGuard::set("BONSAI_LAB_TEMP", "0");

        let mut app_config = AppConfig::default();
        let report = apply_lab_overrides(&mut app_config).expect("BONSAI_LAB_TEMP=0 は Ok");

        assert_eq!(app_config.model.inference.temperature, 0.0);
        let (prev, new) = report.temp_override.expect("report.temp_override が Some");
        assert_eq!(new, 0.0);
        assert_ne!(prev, new, "prev は override 前の値");
    }

    #[test]
    fn t_apply_lab_overrides_repeat_penalty_override() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _t = LAB_TEMP_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _rp = LAB_REPEAT_PENALTY_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env_sse = EnvGuard::unset("BONSAI_LAB_LONG_SSE");
        let _env_mlx = EnvGuard::unset("BONSAI_LAB_MLX_ONLY");
        let _env_temp = EnvGuard::unset("BONSAI_LAB_TEMP");
        let _env_rp = EnvGuard::set("BONSAI_LAB_REPEAT_PENALTY", "1.1");

        let mut app_config = AppConfig::default();
        let report =
            apply_lab_overrides(&mut app_config).expect("BONSAI_LAB_REPEAT_PENALTY=1.1 は Ok");

        assert!((app_config.model.inference.repeat_penalty - 1.1).abs() < f64::EPSILON);
        let (prev, new) = report
            .repeat_penalty_override
            .expect("report.repeat_penalty_override が Some");
        assert!((new - 1.1).abs() < f64::EPSILON);
        assert_ne!(prev, new, "prev は override 前の値");
    }

    #[test]
    fn t_apply_lab_overrides_err_leaves_mlx_block_untouched() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _t = LAB_TEMP_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env_temp = EnvGuard::unset("BONSAI_LAB_TEMP");
        let _env_sse = EnvGuard::unset("BONSAI_LAB_LONG_SSE");
        let _env_allow = EnvGuard::unset("BONSAI_LAB_MLX_ALLOW_UNKNOWN");
        let _env_mlx = EnvGuard::set("BONSAI_LAB_MLX_ONLY", "1");

        let mut app_config = AppConfig::default();
        app_config.model.model_id = "bonsai-8b".to_string();
        app_config
            .fallback_chain
            .entries
            .push(crate::runtime::model_router::FallbackEntry {
                backend: ServerBackend::Unsloth,
                server_url: "http://127.0.0.1:9000".to_string(),
                model_id: "bonsai-8b".to_string(),
            });
        let prev_backend = format!("{:?}", app_config.model.backend);
        let prev_url = app_config.model.server_url.clone();
        let prev_model_id = app_config.model.model_id.clone();
        let prev_entries = app_config.fallback_chain.entries.len();

        let result = apply_lab_overrides(&mut app_config);

        assert!(result.is_err(), "MLX ビルドなし profile は Err");
        assert_eq!(
            format!("{:?}", app_config.model.backend),
            prev_backend,
            "Err 時は backend 不変"
        );
        assert_eq!(
            app_config.model.server_url, prev_url,
            "Err 時は server_url 不変"
        );
        assert_eq!(
            app_config.model.model_id, prev_model_id,
            "Err 時は model_id 不変"
        );
        assert_eq!(
            app_config.fallback_chain.entries.len(),
            prev_entries,
            "Err 時は fallback_chain.entries 不変"
        );
    }

    // #17-1: 未知 model_id は既定で opt-in 必須。BONSAI_LAB_MLX_ALLOW_UNKNOWN=1 で許容。
    #[test]
    fn t_apply_lab_overrides_unknown_id_requires_opt_in() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _t = LAB_TEMP_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env_temp = EnvGuard::unset("BONSAI_LAB_TEMP");
        let _env_sse = EnvGuard::unset("BONSAI_LAB_LONG_SSE");
        let _env_mlx = EnvGuard::set("BONSAI_LAB_MLX_ONLY", "1");
        let _env_allow = EnvGuard::unset("BONSAI_LAB_MLX_ALLOW_UNKNOWN");

        let unknown_id = "Aratako/Qwen3-8B-ERP-v0.1-GGUF";
        let mut app_config = AppConfig::default();
        app_config.model.model_id = unknown_id.to_string();
        let result = apply_lab_overrides(&mut app_config);
        let err = result.expect_err("未知 model_id は opt-in なしで Err");
        assert!(
            err.to_string().contains("BONSAI_LAB_MLX_ALLOW_UNKNOWN"),
            "エラーメッセージに opt-in env 名が含まれること: {err}"
        );

        let _env_allow_opt_in = EnvGuard::set("BONSAI_LAB_MLX_ALLOW_UNKNOWN", "1");
        let mut app_config2 = AppConfig::default();
        app_config2.model.model_id = unknown_id.to_string();
        let report = apply_lab_overrides(&mut app_config2)
            .expect("BONSAI_LAB_MLX_ALLOW_UNKNOWN=1 で未知 id も Ok");
        assert_eq!(
            app_config2.model.model_id, unknown_id,
            "opt-in 時は未知 model_id をそのまま保持"
        );
        assert!(report.mlx_only_applied.is_some());
    }

    #[test]
    fn t_lab_task_limit_env_parse() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = EnvGuard::set("BONSAI_LAB_TASK_LIMIT", "5");
        assert_eq!(lab_task_limit(), Some(5), "env=\"5\" で Some(5)");
    }

    #[test]
    fn t_lab_task_limit_env_out_of_range() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // 0 (下限外)
        let _env = EnvGuard::set("BONSAI_LAB_TASK_LIMIT", "0");
        assert_eq!(lab_task_limit(), None, "env=0 で None");
        // 16 (上限外)
        unsafe { std::env::set_var("BONSAI_LAB_TASK_LIMIT", "16") };
        assert_eq!(lab_task_limit(), None, "env=16 (15超) で None");
        // parse 失敗
        unsafe { std::env::set_var("BONSAI_LAB_TASK_LIMIT", "abc") };
        assert_eq!(lab_task_limit(), None, "parse 失敗で None");
    }

    // ── 項目 252 候補: F4 案 A MLX pre-warm env getter test (Phase 1 Red、stub で 2 FAIL) ──
    //
    // Phase 1 Red: is_lab_mlx_warmup / lab_mlx_warmup_count は stub で false / None 返却.
    // Phase 2 Green: 既存 F2/F3 と同形 env parse + matches! + range filter で 4 test PASS.
    // Phase 3 Refactor: clippy/fmt clean、experiment.rs pre-warm 配線は別 commit (case-by-case).

    /// Phase 1 Red 核心: env=1 で true 期待、stub は常に false → FAIL.
    /// env unset 時 false (default OFF backward compat、stub でも PASS).
    #[test]
    fn t_is_lab_mlx_warmup_env_gate_active() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = EnvGuard::unset("BONSAI_LAB_MLX_WARMUP");
        assert!(
            !is_lab_mlx_warmup(),
            "env unset で MLX pre-warm OFF (default backward compat)"
        );
        unsafe { std::env::set_var("BONSAI_LAB_MLX_WARMUP", "1") };
        assert!(
            is_lab_mlx_warmup(),
            "env=\"1\" で MLX pre-warm ON (Phase 2 Green PASS 期待)"
        );
    }

    /// Phase 1 Red sanity: env unset で None (stub は常に None、PASS).
    #[test]
    fn t_lab_mlx_warmup_count_env_unset_none() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = EnvGuard::unset("BONSAI_LAB_MLX_WARMUP_COUNT");
        assert_eq!(
            lab_mlx_warmup_count(),
            None,
            "env unset で None (caller が unwrap_or(3) で default 適用)"
        );
    }

    /// Phase 1 Red 核心: env=5 で Some(5) 期待、stub は None → FAIL.
    #[test]
    fn t_lab_mlx_warmup_count_env_parse() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = EnvGuard::set("BONSAI_LAB_MLX_WARMUP_COUNT", "5");
        assert_eq!(
            lab_mlx_warmup_count(),
            Some(5),
            "env=\"5\" で Some(5) (Phase 2 Green PASS 期待)"
        );
    }

    /// Phase 1 Red sanity: range 外で None (stub は常に None、PASS).
    #[test]
    fn t_lab_mlx_warmup_count_env_out_of_range() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // 0 (下限外)
        let _env = EnvGuard::set("BONSAI_LAB_MLX_WARMUP_COUNT", "0");
        assert_eq!(lab_mlx_warmup_count(), None, "env=0 で None");
        // 11 (上限外、1..=10)
        unsafe { std::env::set_var("BONSAI_LAB_MLX_WARMUP_COUNT", "11") };
        assert_eq!(lab_mlx_warmup_count(), None, "env=11 (10超) で None");
        // parse 失敗
        unsafe { std::env::set_var("BONSAI_LAB_MLX_WARMUP_COUNT", "abc") };
        assert_eq!(lab_mlx_warmup_count(), None, "parse 失敗で None");
    }

    // ── 項目 252 M2 Phase 1 Red: lab_mlx_warmup_timeout_secs env getter 3 test ──
    //
    // Phase 1 Red: stub は常に 0 (sentinel disable) を return → 2 test FAIL.
    // Phase 2 Green: env parse + range 1..=600 + default 180 で 3 test PASS.

    /// Phase 1 Red sanity: env unset で default 180 期待、stub は 0 → FAIL.
    #[test]
    fn t_lab_mlx_warmup_timeout_secs_default_180() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = EnvGuard::unset("BONSAI_LAB_MLX_WARMUP_TIMEOUT_SECS");
        assert_eq!(
            lab_mlx_warmup_timeout_secs(),
            180,
            "env unset で default 180 = F1 sse_chunk_timeout 整合 (Phase 1 Red FAIL = stub は 0)"
        );
    }

    /// Phase 1 Red 核心: env=60 で 60 を return 期待、stub は 0 → FAIL.
    #[test]
    fn t_lab_mlx_warmup_timeout_secs_env_override() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = EnvGuard::set("BONSAI_LAB_MLX_WARMUP_TIMEOUT_SECS", "60");
        let result = lab_mlx_warmup_timeout_secs();
        assert_eq!(
            result, 60,
            "env=60 で 60 を return (Phase 1 Red FAIL = stub は 0)"
        );
    }

    /// Phase 1 Red sanity: env=0 sentinel と out-of-range fallback の挙動.
    /// stub は常に 0 を return するため env=0 sanity と range out 両方とも 0 で stub PASS、
    /// Phase 2 Green では env=0 は 0 を return (sentinel disable)、range out (601) は 180 fallback.
    #[test]
    fn t_lab_mlx_warmup_timeout_secs_sentinel_and_range() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // env=0 = sentinel disable (Phase 2 Green で 0 を return、Phase 1 Red stub も 0 で trivial PASS)
        let _env = EnvGuard::set("BONSAI_LAB_MLX_WARMUP_TIMEOUT_SECS", "0");
        assert_eq!(
            lab_mlx_warmup_timeout_secs(),
            0,
            "env=0 sentinel で 0 (timeout 無効化、Phase 1/2 共に PASS)"
        );
        // env=601 = range out → Phase 2 Green で default 180 fallback
        unsafe { std::env::set_var("BONSAI_LAB_MLX_WARMUP_TIMEOUT_SECS", "601") };
        let result_out = lab_mlx_warmup_timeout_secs();
        assert_eq!(
            result_out, 180,
            "env=601 (range out) で default 180 fallback (Phase 1 Red FAIL = stub は 0)"
        );
    }

    // ── B-1 Phase 1 Red: mlx idle supervisor config getters ──

    /// env unset / 0 で 0 (supervisor 全体 OFF)、N で N。
    #[test]
    fn t_mlx_idle_timeout_sec_env() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = EnvGuard::unset("BONSAI_MLX_IDLE_TIMEOUT_SEC");
        assert_eq!(mlx_idle_timeout_sec(), 0, "unset で 0 (feature OFF)");
        unsafe { std::env::set_var("BONSAI_MLX_IDLE_TIMEOUT_SEC", "300") };
        assert_eq!(mlx_idle_timeout_sec(), 300, "env=300 で 300");
        unsafe { std::env::set_var("BONSAI_MLX_IDLE_TIMEOUT_SEC", "abc") };
        assert_eq!(mlx_idle_timeout_sec(), 0, "parse 失敗で 0");
    }

    /// env 指定で env 値、未指定で mlx-openai-server を含むパス。
    #[test]
    fn t_mlx_spawn_program_env() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = EnvGuard::set("BONSAI_MLX_SPAWN_PROGRAM", "/custom/mlx-bin");
        assert_eq!(
            mlx_spawn_program(),
            "/custom/mlx-bin",
            "env 指定で env 値を優先"
        );
        unsafe { std::env::remove_var("BONSAI_MLX_SPAWN_PROGRAM") };
        assert!(
            mlx_spawn_program().contains("mlx-openai-server"),
            "unset で mlx-openai-server を含む default path"
        );
    }

    #[test]
    fn t_resolve_db_path_env_override_takes_precedence() {
        // BONSAI_DB_PATH 指定時は data_dir を無視してその path を直接使う。
        // live 検証で production DB を汚染しない隔離 DB 指定経路 (HOME=/tmp ハック代替)。
        let got = resolve_db_path(
            Some("/tmp/iso/x.db"),
            Some(PathBuf::from("/home/u/.local/share")),
        );
        assert_eq!(got, PathBuf::from("/tmp/iso/x.db"));
    }

    #[test]
    fn t_resolve_db_path_default_uses_data_dir() {
        let got = resolve_db_path(None, Some(PathBuf::from("/home/u/.local/share")));
        assert_eq!(
            got,
            PathBuf::from("/home/u/.local/share/bonsai-agent/bonsai.db")
        );
    }

    #[test]
    fn t_resolve_db_path_no_data_dir_falls_back_to_cwd() {
        let got = resolve_db_path(None, None);
        assert_eq!(got, PathBuf::from("bonsai.db"));
    }

    #[test]
    fn t_resolve_db_path_blank_env_is_ignored() {
        // 空白のみ env は未設定扱い (`BONSAI_DB_PATH=` の誤設定で CWD 直下に
        // 空 path で開くのを防ぐ robustness)。
        let got = resolve_db_path(Some("   "), Some(PathBuf::from("/data")));
        assert_eq!(got, PathBuf::from("/data/bonsai-agent/bonsai.db"));
    }

    #[test]
    fn t_sensors_config_defaults() {
        let cfg = SensorsConfig::default();
        assert!(cfg.enabled);
        assert!(cfg.file_watch);
        assert!(cfg.idle_detection);
        assert!(!cfg.window_focus, "window_focusは安全のためデフォルトfalse");
        assert_eq!(cfg.idle_threshold_secs, 300);
        assert_eq!(cfg.window_cooldown_secs, 30);
        assert!(cfg.window_denylist.contains(&"1password".to_string()));
    }

    #[test]
    fn t_sensors_config_env_override() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());

        let _env = EnvGuard::set("BONSAI_SENSOR_WINDOW", "1");
        assert_eq!(is_window_sensor_enabled_env(), Some(true));

        unsafe { std::env::set_var("BONSAI_SENSOR_WINDOW", "0") };
        assert_eq!(is_window_sensor_enabled_env(), Some(false));

        unsafe { std::env::remove_var("BONSAI_SENSOR_WINDOW") };
        assert_eq!(is_window_sensor_enabled_env(), None);
    }

    // --- Issue #15: `--init` の API key 平文書き出し防止 ---

    #[test]
    fn t_issue15_serialize_omits_model_and_advisor_api_key() {
        let mut config = AppConfig::default();
        config.model.api_key = Some("sk-secret-model-key".to_string());
        config.advisor.api_key = Some("sk-secret-advisor-key".to_string());

        let toml_str = toml::to_string_pretty(&config).unwrap();
        assert!(
            !toml_str.contains("api_key"),
            "api_key キー自体が serialize 結果に含まれてはならない: {toml_str}"
        );
        assert!(!toml_str.contains("sk-secret-model-key"));
        assert!(!toml_str.contains("sk-secret-advisor-key"));
    }

    #[test]
    fn t_issue15_deserialize_from_config_toml_still_reads_api_key() {
        // `#[serde(skip_serializing)]` は serialize のみ抑制する。config.toml に
        // 手動で書かれた api_key は従来どおり読み込めること。
        let toml_str = r#"
[model]
api_key = "sk-from-config-toml"

[advisor]
api_key = "sk-advisor-from-config-toml"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.model.api_key.as_deref(), Some("sk-from-config-toml"));
        assert_eq!(
            config.advisor.api_key.as_deref(),
            Some("sk-advisor-from-config-toml")
        );
    }

    #[test]
    fn t_issue15_save_default_to_does_not_leak_api_key() {
        let _g = LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = EnvGuard::set("UNSLOTH_API_KEY", "sk-test-should-not-leak");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        AppConfig::save_default_to(&path).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            !content.contains("sk-test-should-not-leak"),
            "api_key の値が config.toml に書き出されてはならない"
        );
        assert!(!content.contains("api_key"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "config.toml は 0600 で作成されること");
        }
    }
}
