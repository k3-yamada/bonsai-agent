use crate::observability::logger::{LogLevel, log_event};
use anyhow::Result;
use std::collections::HashMap;

use crate::agent::agent_loop::AgentConfig;
use crate::agent::benchmark::{BenchmarkSuite, MultiRunBenchmarkResult, MultiRunConfig};
use crate::agent::experiment_log::{
    AcceptedMutation, Experiment, ExperimentLog, MutationTheme, MutationType, load_accepted_archive,
};
use crate::agent::judge::{HttpAdvisorJudge, LlmJudge, RubricScore};
use crate::agent::validate::PathGuard;
use crate::cancel::CancellationToken;
use crate::memory::store::MemoryStore;
use crate::runtime::inference::LlmBackend;
use crate::tools::ToolRegistry;

/// 変異候補
#[derive(Debug, Clone)]
pub struct Mutation {
    pub mutation_type: MutationType,
    pub detail: String,
    pub apply: MutationAction,
    /// 変異テーマ（1 iteration 1 theme）
    pub theme: MutationTheme,
}

/// 変異の具体的な操作
#[derive(Debug, Clone)]
pub enum MutationAction {
    /// システムプロンプトにルールを追加
    AddPromptRule(String),
    /// システムプロンプトからルールを削除（部分一致）
    RemovePromptRule(String),
    /// max_iterationsを変更
    SetMaxIterations(usize),
    /// max_retriesを変更
    SetMaxRetries(usize),
    /// 動的ツール選択数を変更
    SetMaxToolsSelected(usize),
    /// 複数のプロンプトルールを同時適用（メタ変異用）
    CompoundPromptRules(Vec<String>),
    /// InferenceParamsの温度を変更
    SetTemperature(f64),
    /// ツール出力サイズ上限を変更
    SetMaxToolOutputChars(usize),
}

/// 仮説生成器: ルールベースで次の変異候補を選択
pub struct HypothesisGenerator {
    rules: Vec<PromptRuleCandidate>,
    current_index: usize,
    /// 試行済み変異detailのセット（重複回避）
    tried: std::collections::HashSet<String>,
}

/// プロンプトルール候補
#[derive(Debug, Clone)]
struct PromptRuleCandidate {
    rule: String,
    description: String,
}

impl Default for HypothesisGenerator {
    fn default() -> Self {
        Self {
            rules: default_prompt_rules(),
            current_index: 0,
            tried: std::collections::HashSet::new(),
        }
    }
}

/// パラメータ変異定義（コード重複削減）
struct ParamMutation {
    detail: &'static str,
    action: MutationAction,
}

/// 全パラメータ変異候補（拡張版: 16種）
fn param_mutations() -> Vec<ParamMutation> {
    vec![
        ParamMutation { detail: "max_iterations: 12 (+2)", action: MutationAction::SetMaxIterations(12) },
        ParamMutation { detail: "max_iterations: 8 (-2)", action: MutationAction::SetMaxIterations(8) },
        ParamMutation { detail: "max_iterations: 15 (+5)", action: MutationAction::SetMaxIterations(15) },
        ParamMutation { detail: "max_tools_selected: 3 (-2)", action: MutationAction::SetMaxToolsSelected(3) },
        ParamMutation { detail: "max_tools_selected: 7 (+2)", action: MutationAction::SetMaxToolsSelected(7) },
        ParamMutation { detail: "max_tools_selected: 4 (-1)", action: MutationAction::SetMaxToolsSelected(4) },
        ParamMutation { detail: "max_retries: 1 (-2)", action: MutationAction::SetMaxRetries(1) },
        ParamMutation { detail: "max_retries: 5 (+2)", action: MutationAction::SetMaxRetries(5) },
        ParamMutation { detail: "max_retries: 4 (+1)", action: MutationAction::SetMaxRetries(4) },
        ParamMutation { detail: "temperature: 0.2 (超精密)", action: MutationAction::SetTemperature(0.2) },
        ParamMutation { detail: "temperature: 0.5 (低め)", action: MutationAction::SetTemperature(0.5) },
        ParamMutation { detail: "temperature: 0.7 (バランス)", action: MutationAction::SetTemperature(0.7) },
        ParamMutation { detail: "temperature: 0.9 (探索的)", action: MutationAction::SetTemperature(0.9) },
        ParamMutation { detail: "max_tool_output_chars: 2000 (コンパクト)", action: MutationAction::SetMaxToolOutputChars(2000) },
        ParamMutation { detail: "max_tool_output_chars: 6000 (増量)", action: MutationAction::SetMaxToolOutputChars(6000) },
        ParamMutation { detail: "max_tool_output_chars: 8000 (大容量)", action: MutationAction::SetMaxToolOutputChars(8000) },
    ]
}

impl HypothesisGenerator {
    /// 過去の実験ログから試行済みセットを構築
    pub fn with_tried_details(mut self, details: impl IntoIterator<Item = String>) -> Self {
        self.tried.extend(details);
        self
    }

    /// 次の変異候補を生成（適応型: 試行済みをスキップ）
    pub fn next_mutation(&mut self, experiment_count: usize) -> Mutation {
        let params = param_mutations();
        let total_slots = self.rules.len() + params.len();
        // ルール→パラメータの順でローテーション
        let base_slot = experiment_count % total_slots;

        // 試行済みスキップ（最大total_slots回試行して見つからなければそのまま返す）
        for offset in 0..total_slots {
            let slot = (base_slot + offset) % total_slots;
            let mutation = if slot < self.rules.len() {
                let rule = &self.rules[slot];
                let theme = MutationTheme::from_cycle(slot % 4);
                Mutation {
                    mutation_type: MutationType::PromptRule,
                    detail: rule.description.clone(),
                    apply: MutationAction::AddPromptRule(rule.rule.clone()),
                    theme,
                }
            } else {
                let pi = slot - self.rules.len();
                let p = &params[pi];
                let theme = MutationTheme::from_cycle((pi % 10) + 4);
                Mutation {
                    mutation_type: MutationType::AgentParam,
                    detail: p.detail.to_string(),
                    apply: p.action.clone(),
                    theme,
                }
            };

            if !self.tried.contains(&mutation.detail) {
                self.tried.insert(mutation.detail.clone());
                return mutation;
            }
        }

        // 全候補試行済みの場合、current_indexベースでルール返却
        let rule = &self.rules[self.current_index % self.rules.len()];
        self.current_index += 1;
        Mutation {
            mutation_type: MutationType::PromptRule,
            detail: rule.description.clone(),
            apply: MutationAction::AddPromptRule(rule.rule.clone()),
            theme: MutationTheme::Precision,
        }
    }

    /// Dreamer insightから変異候補を追加
    pub fn add_insight_mutation(&mut self, insight: &str) {
        self.rules.push(PromptRuleCandidate {
            rule: insight.to_string(),
            description: format!("insight: {}", insight.chars().take(50).collect::<String>()),
        });
    }

    /// 失敗理由コンテキストから逆向き変異候補を生成（NAT oracle feedback知見）
    pub fn add_worst_reasoning_insights(&mut self, worst: &[(String, f64)]) {
        for (detail, delta) in worst.iter().take(3) {
            let counter_rule = format!(
                "前回の変異({})がdelta={:.4}で悪化。この方向を避け逆のアプローチを試す",
                detail.chars().take(40).collect::<String>(),
                delta
            );
            self.add_insight_mutation(&counter_rule);
        }
    }
}

/// デフォルトのプロンプトルール候補（20種: 多様なエージェント行動制御）
fn default_prompt_rules() -> Vec<PromptRuleCandidate> {
    vec![
        // --- 精度向上系 ---
        PromptRuleCandidate {
            rule: "10. ツール呼び出しの前に必ず <think> タグで考える".into(),
            description: "ツール使用前に思考を強制".into(),
        },
        PromptRuleCandidate {
            rule: "10. エラーが発生したら原因を <think> で分析してから次の行動を決める".into(),
            description: "エラー分析の強制".into(),
        },
        PromptRuleCandidate {
            rule: "10. ツール結果が期待と違う場合、別のツールを試す".into(),
            description: "フォールバック戦略".into(),
        },
        PromptRuleCandidate {
            rule: "10. ファイル操作の前にパスの存在を確認する".into(),
            description: "ファイル存在確認の強制".into(),
        },
        PromptRuleCandidate {
            rule: "10. 回答を出す前にファイルの内容を確認する".into(),
            description: "回答前の事実確認".into(),
        },
        PromptRuleCandidate {
            rule: "10. 複数のツールが使える場合、最も単純なツールを選ぶ".into(),
            description: "最小限ツール選択".into(),
        },
        PromptRuleCandidate {
            rule: "10. タスクが曖昧な場合、小さなテストで仮説を検証する".into(),
            description: "仮説検証アプローチ".into(),
        },
        PromptRuleCandidate {
            rule: "10. 前のステップの結果を要約してから次のステップに進む".into(),
            description: "段階的要約".into(),
        },
        // --- 効率化系 ---
        PromptRuleCandidate {
            rule: "10. 1つのツール呼び出しで十分な情報が得られたら、追加の呼び出しを控える".into(),
            description: "冗長ツール呼び出し抑制".into(),
        },
        PromptRuleCandidate {
            rule: "10. 回答は簡潔に、必要最小限の情報だけを含める".into(),
            description: "簡潔回答の強制".into(),
        },
        PromptRuleCandidate {
            rule: "10. ツール引数は正確に指定し、省略せず完全な値を渡す".into(),
            description: "ツール引数の正確性".into(),
        },
        // --- ロバスト性系 ---
        PromptRuleCandidate {
            rule: "10. ツールが失敗した場合、同じツールを2回まで再試行してから別の方法を試す".into(),
            description: "リトライ上限付き再試行".into(),
        },
        PromptRuleCandidate {
            rule: "10. ファイル編集前に必ず現在の内容を読み取る".into(),
            description: "編集前読み取り強制".into(),
        },
        PromptRuleCandidate {
            rule: "10. shell コマンドのタイムアウトを想定し、長時間コマンドは避ける".into(),
            description: "コマンドタイムアウト意識".into(),
        },
        // --- 探索系 ---
        PromptRuleCandidate {
            rule: "10. 答えが不明な場合、まず関連ファイルを検索してから回答を組み立てる".into(),
            description: "検索優先アプローチ".into(),
        },
        PromptRuleCandidate {
            rule: "10. 数値計算が必要な場合、shell で計算してから回答する".into(),
            description: "計算ツール活用".into(),
        },
        PromptRuleCandidate {
            rule: "10. コード生成時は、まず完成形をイメージしてから書き始める".into(),
            description: "完成形イメージ先行".into(),
        },
        // --- 構造化思考系 ---
        PromptRuleCandidate {
            rule: "10. タスクを受け取ったら、まず3つ以下のサブタスクに分解する".into(),
            description: "タスク分解の強制".into(),
        },
        PromptRuleCandidate {
            rule: "10. 最終回答の前に、タスクの要件を全て満たしたか確認する".into(),
            description: "完了条件チェック".into(),
        },
        PromptRuleCandidate {
            rule: "10. 推測で回答せず、確認できる情報はツールで確認する".into(),
            description: "推測回避・事実確認".into(),
        },
    ]
}

/// 変異をAgentConfigに適用
pub fn apply_mutation(base_config: &AgentConfig, mutation: &Mutation) -> AgentConfig {
    let mut config = AgentConfig {
        max_iterations: base_config.max_iterations,
        max_retries: base_config.max_retries,
        max_tools_selected: base_config.max_tools_selected,
        system_prompt: base_config.system_prompt.clone(),
        advisor: base_config.advisor.clone(),
        auto_checkpoint: base_config.auto_checkpoint,
        max_tool_output_chars: base_config.max_tool_output_chars,
        max_tools_in_context: base_config.max_tools_in_context,
        max_mcp_tools_in_context: base_config.max_mcp_tools_in_context,
        base_inference: base_config.base_inference.clone(),
        task_timeout: base_config.task_timeout,
    };

    match &mutation.apply {
        MutationAction::AddPromptRule(rule) => {
            config.system_prompt.push('\n');
            config.system_prompt.push_str(rule);
        }
        MutationAction::RemovePromptRule(pattern) => {
            let lines: Vec<&str> = config
                .system_prompt
                .lines()
                .filter(|l| !l.contains(pattern))
                .collect();
            config.system_prompt = lines.join("\n");
        }
        MutationAction::SetMaxIterations(n) => {
            config.max_iterations = *n;
        }
        MutationAction::SetMaxRetries(n) => {
            config.max_retries = *n;
        }
        MutationAction::SetMaxToolsSelected(n) => {
            config.max_tools_selected = *n;
        }
        MutationAction::CompoundPromptRules(rules) => {
            for rule in rules {
                config.system_prompt.push('\n');
                config.system_prompt.push_str(rule);
            }
        }
        MutationAction::SetTemperature(t) => {
            config.base_inference.temperature = *t;
        }
        MutationAction::SetMaxToolOutputChars(n) => {
            config.max_tool_output_chars = *n;
        }
    }

    config
}

/// 設定のスナップショットをHashMapに変換（実験ログ用）
pub fn config_snapshot(config: &AgentConfig) -> HashMap<String, String> {
    HashMap::from([
        ("max_iterations".into(), config.max_iterations.to_string()),
        ("max_retries".into(), config.max_retries.to_string()),
        (
            "max_tools_selected".into(),
            config.max_tools_selected.to_string(),
        ),
        (
            "system_prompt_len".into(),
            config.system_prompt.len().to_string(),
        ),
    ])
}

/// Hyperagentsメタ変異生成器: 過去のACCEPT変異を組み合わせた複合変異を生成
pub struct MetaMutationGenerator {
    /// ACCEPTされた変異アーカイブ
    archive: Vec<AcceptedMutation>,
}

impl MetaMutationGenerator {
    /// DBからACCEPTアーカイブを読み込んで初期化
    pub fn from_db(conn: &rusqlite::Connection) -> Result<Self> {
        let archive = load_accepted_archive(conn)?;
        Ok(Self { archive })
    }

    /// アーカイブを直接指定して初期化（テスト用）
    pub fn from_archive(archive: Vec<AcceptedMutation>) -> Self {
        Self { archive }
    }

    /// アーカイブ内のACCEPT変異数
    pub fn archive_len(&self) -> usize {
        self.archive.len()
    }

    /// メタ変異が生成可能か（PromptRule/PromptHint型が2件以上必要）
    pub fn can_generate(&self) -> bool {
        self.prompt_rule_mutations().len() >= 2
    }

    /// PromptRule型のACCEPT変異のみ抽出
    fn prompt_rule_mutations(&self) -> Vec<&AcceptedMutation> {
        self.archive
            .iter()
            .filter(|m| {
                m.mutation_type == MutationType::PromptRule
                    || m.mutation_type == MutationType::PromptHint
            })
            .collect()
    }

    /// 複合メタ変異を生成: delta上位のPromptRule変異を組み合わせる
    /// cycle_indexでペア選択をローテーション
    pub fn generate_compound(&self, cycle_index: usize) -> Option<Mutation> {
        let mut candidates = self.prompt_rule_mutations();
        if candidates.len() < 2 {
            return None;
        }

        // delta降順ソート（効果の高い変異を優先）
        candidates.sort_by(|a, b| {
            b.delta
                .partial_cmp(&a.delta)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // cycle_indexに基づきペア選択（最良+ローテーション）
        let first = 0;
        let second = 1 + (cycle_index % (candidates.len() - 1));

        let rules: Vec<String> = [candidates[first], candidates[second]]
            .iter()
            .map(|m| m.detail.clone())
            .collect();

        let detail = format!(
            "meta compound: [{}] + [{}] (delta: {:+.4}, {:+.4})",
            candidates[first].detail,
            candidates[second].detail,
            candidates[first].delta,
            candidates[second].delta,
        );

        Some(Mutation {
            mutation_type: MutationType::MetaMutation,
            detail,
            apply: MutationAction::CompoundPromptRules(rules),
            theme: MutationTheme::Precision,
        })
    }

    /// delta加重の変異優先度スコアを計算（変異選択の優先順位付けに使用）
    pub fn priority_score(&self, mutation_detail: &str) -> f64 {
        self.archive
            .iter()
            .filter(|m| mutation_detail.contains(&m.detail) || m.detail.contains(mutation_detail))
            .map(|m| m.delta)
            .sum::<f64>()
    }
}

/// 少数タスクで変異の効果を事前推定する（事前計算済みベースラインスコアは使わず、
/// サンプルタスク上で独自にベースラインを計測して比較する）
/// タスク数の半分（最大4タスク）で1回実行し、delta推定値を返す
pub fn estimate_mutation_effect(
    base_config: &AgentConfig,
    mutation: &Mutation,
    backend: &dyn LlmBackend,
    tools: &ToolRegistry,
    path_guard: &PathGuard,
    cancel: &CancellationToken,
) -> Result<f64> {
    // 旧APIラッパー: ベースラインスコア0.0を渡すが、内部でサンプル独自計測するため無関係
    estimate_mutation_effect_with_baseline(
        base_config, mutation, 0.0, backend, tools, path_guard, cancel,
    )
}

/// 少数タスクで変異の効果を事前推定する（プリスクリーニング用）
/// サンプルタスク上でベースラインと変異後の両方を計測し、delta推定値を返す。
/// `_baseline_score`引数は互換性のために受け取るが、サンプルタスクの特性が
/// フルスイートと異なる可能性があるため、サンプル独自のベースラインを計測する。
pub fn estimate_mutation_effect_with_baseline(
    base_config: &AgentConfig,
    mutation: &Mutation,
    _baseline_score: f64,
    backend: &dyn LlmBackend,
    tools: &ToolRegistry,
    path_guard: &PathGuard,
    cancel: &CancellationToken,
) -> Result<f64> {
    let suite = BenchmarkSuite::default_tasks();
    // 推定用: タスク半分（最大4件）のみで1回実行
    let sample_size = (suite.tasks.len() / 2).min(4);
    let sample_tasks: Vec<_> = suite.tasks.into_iter().take(sample_size).collect();
    let sample_suite = BenchmarkSuite {
        tasks: sample_tasks,
    };

    let quick_multi = MultiRunConfig {
        k: 1,
        jitter_seed: false,
    };
    let pass_threshold = 0.5;

    // サンプルタスク上でベースライン計測（フルスイートのスコアとは異なる可能性がある）
    let baseline = sample_suite.run_k(
        base_config,
        backend,
        tools,
        path_guard,
        cancel,
        &quick_multi,
        pass_threshold,
    )?;

    // 変異適用後（サンプルタスクのみ）
    let modified_config = apply_mutation(base_config, mutation);
    let experiment = sample_suite.run_k(
        &modified_config,
        backend,
        tools,
        path_guard,
        cancel,
        &quick_multi,
        pass_threshold,
    )?;

    let delta = experiment.composite_score() - baseline.composite_score();
    log_event(
        LogLevel::Info,
        "lab",
        &format!(
            "pre-screen estimate: {} -> delta={:+.4} ({} tasks, k=1)",
            mutation.detail, delta, sample_size,
        ),
    );
    Ok(delta)
}

/// DBから過去の試行済み変異detailをロード（重複回避用）
fn load_tried_details(conn: &rusqlite::Connection) -> Vec<String> {
    let sql = "SELECT DISTINCT mutation_detail FROM experiments WHERE accepted = 0";
    conn.prepare(sql)
        .and_then(|mut stmt| {
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            Ok(rows.filter_map(|r| r.ok()).collect())
        })
        .unwrap_or_default()
}

/// Lab停滞検出器（NAT check_adaptive_triggers知見）
///
/// 3条件で停滞を検出し、Dreamer早期起動のトリガーを提供:
/// - Stagnation: ベストスコア不変N回以上
/// - VarianceCollapse: 直近delta分散が閾値未満
pub struct LabStagnationDetector {
    best_score: f64,
    best_unchanged_count: usize,
    recent_deltas: std::collections::VecDeque<f64>,
    stagnation_threshold: usize,
    variance_collapse_threshold: f64,
    window_size: usize,
}

/// 停滞トリガーの種類
#[derive(Debug, Clone, PartialEq)]
pub enum LabTrigger {
    None,
    Stagnation,
    VarianceCollapse,
}

impl LabStagnationDetector {
    pub fn new(stagnation_threshold: usize, variance_collapse_threshold: f64) -> Self {
        Self {
            best_score: f64::NEG_INFINITY,
            best_unchanged_count: 0,
            recent_deltas: std::collections::VecDeque::new(),
            stagnation_threshold,
            variance_collapse_threshold,
            window_size: 5,
        }
    }

    /// 実験結果を記録し、トリガーを判定
    pub fn record_and_check(&mut self, delta: f64, experiment_score: f64) -> LabTrigger {
        // delta履歴を更新
        self.recent_deltas.push_back(delta);
        if self.recent_deltas.len() > self.window_size {
            self.recent_deltas.pop_front();
        }

        // ベストスコア更新チェック
        if experiment_score > self.best_score {
            self.best_score = experiment_score;
            self.best_unchanged_count = 0;
        } else {
            self.best_unchanged_count += 1;
        }

        // 1. Stagnation: ベスト不変N回以上（NAT閾値: delta < 0.001）
        if self.best_unchanged_count >= self.stagnation_threshold {
            return LabTrigger::Stagnation;
        }

        // 2. VarianceCollapse: 直近deltas分散が閾値未満
        if self.recent_deltas.len() >= self.window_size {
            let mean = self.recent_deltas.iter().sum::<f64>() / self.recent_deltas.len() as f64;
            let variance = self.recent_deltas.iter().map(|d| (d - mean).powi(2)).sum::<f64>()
                / self.recent_deltas.len() as f64;
            if variance < self.variance_collapse_threshold {
                return LabTrigger::VarianceCollapse;
            }
        }

        LabTrigger::None
    }

    pub fn reset(&mut self) {
        self.best_unchanged_count = 0;
        self.recent_deltas.clear();
    }
}

impl Default for LabStagnationDetector {
    fn default() -> Self {
        Self::new(3, 0.001)
    }
}

/// 実験ループ設定
pub struct ExperimentLoopConfig {
    /// TSVログのパス
    pub tsv_path: Option<std::path::PathBuf>,
    /// 最大実験回数（Noneなら無限）
    pub max_experiments: Option<usize>,
    /// Dreamerレポート間隔（N実験ごと）
    pub dreamer_interval: usize,
    /// プリスクリーニング有効化（少数タスクで事前評価し、明らかな悪化を早期棄却）
    pub enable_prescreening: bool,
    /// プリスクリーニング棄却閾値（推定deltaがこの値未満なら早期棄却）
    pub prescreening_threshold: f64,
    /// タスク単位タイムアウト秒数（0=無制限）
    pub task_timeout_secs: u64,
    /// judge gate 閾値（Phase B2: ADK rubric_based_final_response_quality_v1）
    /// `Some(0.7)` で有効化、`None` で従来動作（delta > 0 のみで ACCEPT）
    pub judge_threshold: Option<f64>,
    /// judge にかける task 数（負荷制御、デフォルト 4）
    pub judge_sample_size: usize,
}

impl Default for ExperimentLoopConfig {
    fn default() -> Self {
        Self {
            tsv_path: None,
            max_experiments: None,
            dreamer_interval: 10,
            enable_prescreening: true,
            prescreening_threshold: -0.01,
            task_timeout_secs: 300,
            judge_threshold: None,
            judge_sample_size: 4,
        }
    }
}

/// 実験ループ本体
/// REJECT実験から最悪タスクの失敗パターンを抽出（NAT extract_worst_reasoning知見）
///
/// 直近N件のREJECT実験のdeltaでソートし、最も悪化した変異+deltaを返す。
/// worst_n=5（NAT閾値）
pub fn extract_worst_reasoning(experiments: &[Experiment], worst_n: usize) -> Vec<(String, f64)> {
    let mut rejects: Vec<_> = experiments
        .iter()
        .filter(|e| !e.accepted && !e.prescreened)
        .map(|e| (e.mutation_detail.clone(), e.delta))
        .collect();
    rejects.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    rejects.truncate(worst_n);
    rejects
}

/// Lab 自己改善ループ本体
///
/// # ADK Phase D 評価結果（項目166）
///
/// Workflow primitive trait（`SequentialAgent`/`ParallelAgent`/`LoopAgent`）への
/// 形式化は YAGNI 判定で **見送り**（加重 1.0/17.0 < 閾値 12.0）。
/// 詳細は `.claude/plan/phase-d-evaluation.md` を参照。
///
/// 再評価トリガー:
/// 1. 複合 Workflow 要件発生（リサーチ→コーダー→レビュアー等の固定パイプライン）
/// 2. Lab pass^k 改善天井（v13/v14 で 3 サイクル全 REJECT）
/// 3. ADK 2.0+ で primitive 標準スキーマが公開
pub fn run_experiment_loop(
    base_config: &AgentConfig,
    backend: &dyn LlmBackend,
    tools: &ToolRegistry,
    path_guard: &PathGuard,
    cancel: &CancellationToken,
    store: &MemoryStore,
    loop_config: &ExperimentLoopConfig,
) -> Result<Vec<Experiment>> {
    // BONSAI_LAB_SMOKE=1 で smoke タスク（5 件）に切替（dev iteration 用）
    let suite = if std::env::var("BONSAI_LAB_SMOKE")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
    {
        log_event(
            LogLevel::Info,
            "lab",
            "BONSAI_LAB_SMOKE=1 → smoke_tasks() 使用（5 タスク）",
        );
        BenchmarkSuite::smoke_tasks()
    } else {
        BenchmarkSuite::default_tasks()
    };
    // 過去の試行済み変異detailをDBからロードし、重複回避
    let tried_details = load_tried_details(store.conn());
    let tried_count = tried_details.len();
    let mut generator = HypothesisGenerator::default().with_tried_details(tried_details);
    if tried_count > 0 {
        log_event(
            LogLevel::Info,
            "lab",
            &format!("過去の試行済み変異: {}件（重複スキップ対象）", tried_count),
        );
    }
    let mut experiments: Vec<Experiment> = Vec::new();
    let multi = MultiRunConfig {
        k: 3,
        jitter_seed: true,
    };
    let pass_threshold = 0.5;

    // 1. ベースライン計測（pass^k版）
    log_event(
        LogLevel::Info,
        "lab",
        &format!("ベースライン計測中（k={}）...", multi.k),
    );
    let mut baseline = suite.run_k(
        base_config,
        backend,
        tools,
        path_guard,
        cancel,
        &multi,
        pass_threshold,
    )?;
    eprintln!(
        "[lab] ベースライン: score={:.4} pass@k={:.4} pass_consec={:.4} ({:.1}s)",
        baseline.composite_score(),
        baseline.composite_pass_at_k(),
        baseline.composite_pass_consecutive_k(),
        baseline.duration_secs
    );

    // メタ変異生成器の初期化（過去のACCEPTアーカイブから）
    let mut meta_gen = MetaMutationGenerator::from_db(store.conn())
        .unwrap_or_else(|_| MetaMutationGenerator::from_archive(Vec::new()));
    if meta_gen.archive_len() > 0 {
        log_event(
            LogLevel::Info,
            "lab",
            &format!(
                "meta mutation generator: {} accepted mutations in archive",
                meta_gen.archive_len()
            ),
        );
    }

    // 停滞検出器（NAT知見、項目141: Dreamer早期起動トリガー）
    let mut stagnation_detector = LabStagnationDetector::default();

    // 2. 実験ループ
    let mut experiment_count = 0;
    let mut meta_cycle = 0;
    loop {
        if cancel.is_cancelled() {
            log_event(LogLevel::Warn, "lab", "キャンセルされました");
            break;
        }

        if let Some(max) = loop_config.max_experiments
            && experiment_count >= max
        {
            log_event(LogLevel::Info, "lab", &format!("最大実験回数({max})に到達"));
            break;
        }

        // a. 仮説生成（5回に1回メタ変異を試行）
        let mutation = if experiment_count % 5 == 4
            && meta_gen.can_generate()
            && let Some(meta_mutation) = meta_gen.generate_compound(meta_cycle)
        {
            meta_cycle += 1;
            meta_mutation
        } else {
            generator.next_mutation(experiment_count)
        };
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let experiment_id = format!("exp_{ts}_{:04}", experiment_count);
        eprintln!(
            "[lab] 実験 {experiment_id}: {} — {}",
            mutation.mutation_type.as_str(),
            mutation.detail
        );

        // b. 変異適用
        let modified_config = apply_mutation(base_config, &mutation);

        // b2. プリスクリーニング: 少数タスクで事前評価し、明らかな悪化を早期棄却
        if loop_config.enable_prescreening {
            let estimated_delta = estimate_mutation_effect_with_baseline(
                base_config,
                &mutation,
                baseline.composite_score(),
                backend,
                tools,
                path_guard,
                cancel,
            )?;
            if estimated_delta < loop_config.prescreening_threshold {
                eprintln!(
                    "[lab] pre-screen REJECT: {} (estimated delta={:+.4})",
                    mutation.detail, estimated_delta
                );
                let snapshot = config_snapshot(&modified_config);
                let exp = Experiment {
                    experiment_id: experiment_id.clone(),
                    mutation_type: mutation.mutation_type,
                    mutation_detail: mutation.detail,
                    baseline_score: baseline.composite_score(),
                    experiment_score: baseline.composite_score() + estimated_delta,
                    delta: estimated_delta,
                    accepted: false,
                    duration_secs: 0.0,
                    config_snapshot: snapshot,
                    pass_at_k: None,
                    pass_consecutive_k: None,
                    score_variance: None,
                    prescreened: true,
                };
                ExperimentLog::save_to_db(store.conn(), &exp)?;
                if let Some(tsv) = &loop_config.tsv_path {
                    ExperimentLog::append_tsv(tsv, &exp)?;
                }
                experiments.push(exp);
                experiment_count += 1;
                continue;
            }
            eprintln!(
                "[lab] pre-screen PASS: {} (estimated delta={:+.4})",
                mutation.detail, estimated_delta
            );
        }

        // c. ベンチマーク実行（pass^k版）
        let result = suite.run_k(
            &modified_config,
            backend,
            tools,
            path_guard,
            cancel,
            &multi,
            pass_threshold,
        )?;
        let snapshot = config_snapshot(&modified_config);

        // d. 評価（pass^k指標を含む）
        let mut exp = Experiment::from_multi_results(
            experiment_id,
            mutation.mutation_type,
            mutation.detail,
            &baseline,
            &result,
            snapshot,
        );

        // d-1. judge gate（Phase B2、opt-in）— delta > 0 の experiment のみ judge にかけ、
        // mean_composite が threshold 未満なら ACCEPT を REJECT に格下げ。
        // 設計判断: ベースラインは judge にかけない（コスト 2 倍回避、delta > 0 で baseline 越えは保証）。
        if let Some(threshold) = loop_config.judge_threshold
            && exp.accepted
        {
            let task_descs: HashMap<String, String> = suite
                .tasks
                .iter()
                .map(|t| (t.id.clone(), t.input.clone()))
                .collect();
            let mut judge_advisor = base_config.advisor.clone();
            let mut judge = HttpAdvisorJudge::new(&mut judge_advisor);
            match judge_gate_check(
                &mut judge,
                &result,
                threshold,
                loop_config.judge_sample_size,
                &task_descs,
            ) {
                Ok(outcome) => {
                    if !outcome.passed {
                        log_event(
                            LogLevel::Info,
                            "lab",
                            &format!(
                                "judge gate REJECT: mean_composite={:.4} < threshold={:.4} (judged {} tasks)",
                                outcome.mean_composite,
                                threshold,
                                outcome.scores.len()
                            ),
                        );
                        exp.accepted = false;
                    } else {
                        log_event(
                            LogLevel::Info,
                            "lab",
                            &format!(
                                "judge gate PASS: mean_composite={:.4} ≥ threshold={:.4} (judged {} tasks)",
                                outcome.mean_composite,
                                threshold,
                                outcome.scores.len()
                            ),
                        );
                    }
                }
                Err(e) => {
                    log_event(
                        LogLevel::Warn,
                        "lab",
                        &format!("judge gate failed (fail-open, ACCEPT 維持): {e}"),
                    );
                    // fail-open: judge 失敗時は exp.accepted を変えない
                }
            }
        }

        eprintln!(
            "[lab]   score={:.4} pass@k={:.4} consec={:.4} (delta: {:+.4}) → {}",
            exp.experiment_score,
            exp.pass_at_k.unwrap_or(0.0),
            exp.pass_consecutive_k.unwrap_or(0.0),
            exp.delta,
            if exp.accepted { "ACCEPT" } else { "REJECT" }
        );

        // e. accept/reject + メタ変異アーカイブ更新
        if exp.accepted {
            baseline = result;
            // 新しいACCEPTをメタ変異アーカイブに追加
            meta_gen = MetaMutationGenerator::from_db(store.conn()).unwrap_or(meta_gen);
        }

        // f. ログ記録
        ExperimentLog::save_to_db(store.conn(), &exp)?;
        if let Some(tsv) = &loop_config.tsv_path {
            ExperimentLog::append_tsv(tsv, &exp)?;
        }

        experiments.push(exp);
        experiment_count += 1;

        // g. 停滞検出+Dreamer早期起動（NAT知見、項目141）
        let last_exp = experiments.last().unwrap();
        let trigger = stagnation_detector.record_and_check(last_exp.delta, last_exp.experiment_score);
        if trigger != LabTrigger::None {
            log_event(
                LogLevel::Info,
                "lab",
                &format!("停滞検出: {trigger:?} → Dreamer早期起動+oracle feedback注入"),
            );
            // oracle feedback: REJECT失敗パターンから逆向き変異候補を生成（NAT知見、項目140）
            let worst = extract_worst_reasoning(&experiments, 5);
            if !worst.is_empty() {
                generator.add_worst_reasoning_insights(&worst);
                log_event(
                    LogLevel::Info,
                    "lab",
                    &format!("oracle feedback: {}件の失敗パターンから逆向き変異追加", worst.len()),
                );
            }
            // Dreamer早期起動
            if let Ok(report) =
                crate::memory::evolution::EvolutionEngine::new(store).analyze_deep(7)
            {
                for insight in &report.insights {
                    generator.add_insight_mutation(insight);
                    log_event(
                        LogLevel::Info,
                        "lab",
                        &format!("Dreamer early insight: {insight}"),
                    );
                }
            }
            stagnation_detector.reset();
        }

        // h. Dreamer定期統合（N実験ごと）
        if experiment_count % loop_config.dreamer_interval == 0
            && let Ok(report) =
                crate::memory::evolution::EvolutionEngine::new(store).analyze_deep(7)
        {
            for insight in &report.insights {
                generator.add_insight_mutation(insight);
                log_event(
                    LogLevel::Info,
                    "lab",
                    &format!("Dreamer insight追加: {insight}"),
                );
            }
            for skill in &report.skill_promotions {
                log_event(LogLevel::Info, "lab", &format!("スキル自動昇格: {skill}"));
            }
        }
    }

    let total = experiments.len();
    let accepted = experiments.iter().filter(|e| e.accepted).count();
    eprintln!(
        "[lab] 完了: {total}実験, {accepted}承認 ({:.0}%)",
        if total > 0 {
            accepted as f64 / total as f64 * 100.0
        } else {
            0.0
        }
    );

    Ok(experiments)
}

/// judge gate の判定結果（B2: ACCEPT 判定の補助シグナル）
///
/// `passed` は `mean_composite >= threshold` を満たすかどうか。
/// `scores` は judge を実際にかけた task の RubricScore（sample_size 件）。
#[derive(Debug, Clone)]
pub struct JudgeGateOutcome {
    pub passed: bool,
    pub mean_composite: f64,
    pub scores: Vec<RubricScore>,
}

/// experiment 結果に judge を適用し、threshold を満たすか判定する
///
/// - `result.task_scores` の先頭 `sample_size` 件を judge にかける
/// - `last_response` / `last_trajectory` が None のタスクはスキップ（judge 不可）
/// - judge が Err を返したタスクは fail-open（warn ログ + 当該 task は scores に含めない）
/// - 全 score の `composite()` 平均が `threshold` 以上なら `passed=true`
/// - judge にかけられた task が 0 件なら `passed=true`（fail-open）
pub fn judge_gate_check(
    judge: &mut dyn LlmJudge,
    result: &MultiRunBenchmarkResult,
    threshold: f64,
    sample_size: usize,
    task_descriptions: &HashMap<String, String>,
) -> Result<JudgeGateOutcome> {
    let mut scores: Vec<RubricScore> = Vec::new();
    let mut judged = 0usize;

    for task_score in result.task_scores.iter() {
        if judged >= sample_size {
            break;
        }
        // last_response が無い task は judge にかけられない（skip）
        let Some(response) = task_score.last_response.as_ref() else {
            continue;
        };
        let trajectory: &[String] = task_score
            .last_trajectory
            .as_deref()
            .unwrap_or(&[] as &[String]);
        let description = task_descriptions
            .get(&task_score.task_id)
            .cloned()
            .unwrap_or_else(|| task_score.task_id.clone());

        judged += 1;
        match judge.evaluate(&description, response, trajectory) {
            Ok(score) => scores.push(score),
            Err(e) => {
                log_event(
                    LogLevel::Warn,
                    "judge",
                    &format!("judge_gate evaluate failed (task={}): {e}", task_score.task_id),
                );
                // fail-open: scores に積まない
            }
        }
    }

    // composite 平均（fail-open: judge にかけられなかった or 全 err なら mean=0、passed=true）
    let (mean_composite, passed) = if scores.is_empty() {
        (0.0, true)
    } else {
        let mean: f64 =
            scores.iter().map(|s| s.composite()).sum::<f64>() / scores.len() as f64;
        (mean, mean >= threshold)
    };

    Ok(JudgeGateOutcome {
        passed,
        mean_composite,
        scores,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> AgentConfig {
        AgentConfig {
            max_iterations: 10,
            max_retries: 3,
            max_tools_selected: 5,
            system_prompt: "test prompt\n1. ルール1\n2. ルール2".into(),
            advisor: Default::default(),
            auto_checkpoint: false,
            max_tool_output_chars: 4000,
            max_tools_in_context: 8,
            max_mcp_tools_in_context: 3,
            base_inference: crate::config::InferenceParams::default(),
            task_timeout: None,
        }
    }

    #[test]
    fn test_apply_mutation_add_rule() {
        let config = make_config();
        let mutation = Mutation {
            mutation_type: MutationType::PromptRule,
            detail: "テスト".into(),
            apply: MutationAction::AddPromptRule("3. 新ルール".into()),
            theme: MutationTheme::Precision,
        };
        let modified = apply_mutation(&config, &mutation);
        assert!(modified.system_prompt.contains("3. 新ルール"));
        assert_eq!(modified.max_iterations, config.max_iterations);
    }

    #[test]
    fn test_apply_mutation_remove_rule() {
        let config = make_config();
        let mutation = Mutation {
            mutation_type: MutationType::PromptRule,
            detail: "テスト".into(),
            apply: MutationAction::RemovePromptRule("ルール1".into()),
            theme: MutationTheme::Precision,
        };
        let modified = apply_mutation(&config, &mutation);
        assert!(!modified.system_prompt.contains("ルール1"));
        assert!(modified.system_prompt.contains("ルール2"));
    }

    #[test]
    fn test_apply_mutation_set_iterations() {
        let config = make_config();
        let mutation = Mutation {
            mutation_type: MutationType::AgentParam,
            detail: "テスト".into(),
            apply: MutationAction::SetMaxIterations(15),
            theme: MutationTheme::Precision,
        };
        let modified = apply_mutation(&config, &mutation);
        assert_eq!(modified.max_iterations, 15);
        assert_eq!(modified.system_prompt, config.system_prompt);
    }

    #[test]
    fn test_apply_mutation_set_retries() {
        let config = make_config();
        let mutation = Mutation {
            mutation_type: MutationType::AgentParam,
            detail: "テスト".into(),
            apply: MutationAction::SetMaxRetries(5),
            theme: MutationTheme::Precision,
        };
        let modified = apply_mutation(&config, &mutation);
        assert_eq!(modified.max_retries, 5);
    }

    #[test]
    fn test_hypothesis_generator_rotation() {
        let mut hyp_gen = HypothesisGenerator::default();
        // 最初はプロンプトルール（slot 0〜19）
        let m0 = hyp_gen.next_mutation(0);
        assert_eq!(m0.mutation_type, MutationType::PromptRule);
        // slot 20以降はパラメータ変異
        let mut hyp_gen2 = HypothesisGenerator::default();
        let n_rules = hyp_gen2.rules.len();
        let m_param = hyp_gen2.next_mutation(n_rules);
        assert_eq!(m_param.mutation_type, MutationType::AgentParam);
    }

    #[test]
    fn test_hypothesis_generator_skips_tried() {
        let mut hyp_gen = HypothesisGenerator::default();
        let m0 = hyp_gen.next_mutation(0);
        let m0_detail = m0.detail.clone();
        // 同じdetailをtriedに入れて再生成
        let mut hyp_gen2 = HypothesisGenerator::default()
            .with_tried_details(vec![m0_detail.clone()]);
        let m0_retry = hyp_gen2.next_mutation(0);
        // スキップされて別の変異が返る
        assert_ne!(m0_retry.detail, m0_detail);
    }

    #[test]
    fn test_add_insight_mutation() {
        let mut hyp_gen = HypothesisGenerator::default();
        let initial_count = hyp_gen.rules.len();
        hyp_gen.add_insight_mutation("新しい洞察: ツールの前に考えるべし");
        assert_eq!(hyp_gen.rules.len(), initial_count + 1);
        assert!(hyp_gen.rules.last().unwrap().rule.contains("新しい洞察"));
    }

    #[test]
    fn test_config_snapshot() {
        let config = make_config();
        let snap = config_snapshot(&config);
        assert_eq!(snap.get("max_iterations").unwrap(), "10");
        assert_eq!(snap.get("max_retries").unwrap(), "3");
        assert!(snap.contains_key("system_prompt_len"));
    }

    #[test]
    fn test_default_prompt_rules_non_empty() {
        let rules = default_prompt_rules();
        assert!(!rules.is_empty());
        for r in &rules {
            assert!(!r.rule.is_empty());
            assert!(!r.description.is_empty());
        }
    }

    #[test]
    fn test_experiment_loop_config_default() {
        let config = ExperimentLoopConfig::default();
        assert!(config.tsv_path.is_none());
        assert!(config.max_experiments.is_none());
        assert_eq!(config.dreamer_interval, 10);
    }

    #[test]
    fn test_apply_mutation_preserves_base() {
        let config = make_config();
        let original_prompt = config.system_prompt.clone();
        let mutation = Mutation {
            mutation_type: MutationType::PromptRule,
            detail: "テスト".into(),
            apply: MutationAction::AddPromptRule("新ルール".into()),
            theme: MutationTheme::Precision,
        };
        let _ = apply_mutation(&config, &mutation);
        // 元のconfigは変更されていない
        assert_eq!(config.system_prompt, original_prompt);
    }

    // --- メタ変異テスト ---

    fn make_accepted_mutation(detail: &str, delta: f64) -> AcceptedMutation {
        AcceptedMutation {
            mutation_type: MutationType::PromptRule,
            detail: detail.into(),
            delta,
            baseline_score: 0.8,
            timestamp: 1000,
        }
    }

    #[test]
    fn test_meta_generator_cannot_generate_with_zero() {
        let mg = MetaMutationGenerator::from_archive(Vec::new());
        assert!(!mg.can_generate());
        assert_eq!(mg.archive_len(), 0);
    }

    #[test]
    fn test_meta_generator_cannot_generate_with_one() {
        let mg = MetaMutationGenerator::from_archive(vec![make_accepted_mutation("rule_a", 0.05)]);
        assert!(!mg.can_generate());
    }

    #[test]
    fn test_meta_generator_can_generate_with_two() {
        let mg = MetaMutationGenerator::from_archive(vec![
            make_accepted_mutation("rule_a", 0.05),
            make_accepted_mutation("rule_b", 0.03),
        ]);
        assert!(mg.can_generate());
        assert_eq!(mg.archive_len(), 2);
    }

    #[test]
    fn test_meta_generator_compound_combines_top_delta() {
        let mg = MetaMutationGenerator::from_archive(vec![
            make_accepted_mutation("low_effect", 0.01),
            make_accepted_mutation("mid_effect", 0.05),
            make_accepted_mutation("high_effect", 0.10),
        ]);
        let compound = mg.generate_compound(0).unwrap();
        assert_eq!(compound.mutation_type, MutationType::MetaMutation);
        // 最もdeltaが高い「high_effect」が必ず含まれる
        assert!(compound.detail.contains("high_effect"));
    }

    #[test]
    fn test_meta_generator_compound_rotation() {
        let mg = MetaMutationGenerator::from_archive(vec![
            make_accepted_mutation("rule_a", 0.10),
            make_accepted_mutation("rule_b", 0.05),
            make_accepted_mutation("rule_c", 0.03),
        ]);
        let c0 = mg.generate_compound(0).unwrap();
        let c1 = mg.generate_compound(1).unwrap();
        // cycle_indexでペア相手がローテーション
        assert_ne!(c0.detail, c1.detail);
    }

    #[test]
    fn test_meta_generator_compound_returns_none_insufficient() {
        // AgentParam型のみの場合、PromptRule型が2件未満→None
        let mg = MetaMutationGenerator::from_archive(vec![
            AcceptedMutation {
                mutation_type: MutationType::AgentParam,
                detail: "max_iterations: 12".into(),
                delta: 0.05,
                baseline_score: 0.8,
                timestamp: 1000,
            },
            AcceptedMutation {
                mutation_type: MutationType::PromptRule,
                detail: "single rule".into(),
                delta: 0.03,
                baseline_score: 0.8,
                timestamp: 1001,
            },
        ]);
        // PromptRule型が1件のみなのでNone
        assert!(mg.generate_compound(0).is_none());
    }

    #[test]
    fn test_apply_mutation_compound_rules() {
        let config = make_config();
        let mutation = Mutation {
            mutation_type: MutationType::MetaMutation,
            detail: "compound test".into(),
            apply: MutationAction::CompoundPromptRules(vec!["rule_x".into(), "rule_y".into()]),
            theme: MutationTheme::Precision,
        };
        let modified = apply_mutation(&config, &mutation);
        assert!(modified.system_prompt.contains("rule_x"));
        assert!(modified.system_prompt.contains("rule_y"));
        // 元のプロンプトも保持
        assert!(modified.system_prompt.contains("test prompt"));
    }

    #[test]
    fn test_priority_score_matching() {
        let mg = MetaMutationGenerator::from_archive(vec![
            make_accepted_mutation("force thinking", 0.05),
            make_accepted_mutation("fallback strategy", 0.03),
        ]);
        // 部分一致でdeltaを集計
        let score = mg.priority_score("force thinking");
        assert!((score - 0.05).abs() < 0.001);
    }

    #[test]
    fn test_priority_score_no_match() {
        let mg = MetaMutationGenerator::from_archive(vec![make_accepted_mutation(
            "force thinking",
            0.05,
        )]);
        let score = mg.priority_score("completely unrelated");
        assert!((score).abs() < f64::EPSILON);
    }

    #[test]
    fn test_meta_generator_prompt_hint_included() {
        // PromptHint型もメタ変異のPromptRule候補として扱われる
        let mg = MetaMutationGenerator::from_archive(vec![
            AcceptedMutation {
                mutation_type: MutationType::PromptHint,
                detail: "insight rule".into(),
                delta: 0.04,
                baseline_score: 0.8,
                timestamp: 1000,
            },
            make_accepted_mutation("normal rule", 0.06),
        ]);
        assert!(mg.can_generate());
        let compound = mg.generate_compound(0).unwrap();
        assert!(
            compound.detail.contains("insight rule") || compound.detail.contains("normal rule")
        );
    }

    // --- プリスクリーニングテスト ---

    #[test]
    fn test_experiment_loop_config_prescreening_defaults() {
        let config = ExperimentLoopConfig::default();
        assert!(config.enable_prescreening, "プリスクリーニングはデフォルト有効");
        assert!(
            (config.prescreening_threshold - (-0.01)).abs() < f64::EPSILON,
            "閾値のデフォルトは-0.01"
        );
    }

    #[test]
    fn test_prescreening_threshold_rejects_negative_delta() {
        // 推定deltaが閾値を下回る場合、棄却されるべき
        let threshold = -0.01;
        let estimated_delta = -0.05;
        assert!(
            estimated_delta < threshold,
            "大きな悪化は閾値を下回る"
        );
    }

    #[test]
    fn test_prescreening_threshold_passes_positive_delta() {
        // 推定deltaが閾値以上の場合、通過するべき
        let threshold = -0.01;
        let estimated_delta = 0.02;
        assert!(
            estimated_delta >= threshold,
            "改善は閾値以上で通過"
        );
        // 閾値ちょうどの場合も通過
        let estimated_delta_border = -0.01;
        assert!(
            !(estimated_delta_border < threshold),
            "閾値ちょうどは通過（<で判定するため）"
        );
    }

    // --- 新変異カテゴリテスト ---

    #[test]
    fn test_new_mutation_actions_apply() {
        let config = make_config();

        // SetTemperature: 温度変更が反映される
        let temp_mutation = Mutation {
            mutation_type: MutationType::AgentParam,
            detail: "temperature変更".into(),
            apply: MutationAction::SetTemperature(0.9),
            theme: MutationTheme::Exploration,
        };
        let modified = apply_mutation(&config, &temp_mutation);
        assert!(
            (modified.base_inference.temperature - 0.9).abs() < f64::EPSILON,
            "温度が0.9に設定される"
        );
        // 他のパラメータは変更されない
        assert_eq!(modified.max_iterations, config.max_iterations);
        assert_eq!(modified.max_tool_output_chars, config.max_tool_output_chars);

        // SetMaxToolOutputChars: 出力サイズ上限変更が反映される
        let output_mutation = Mutation {
            mutation_type: MutationType::AgentParam,
            detail: "出力サイズ変更".into(),
            apply: MutationAction::SetMaxToolOutputChars(2000),
            theme: MutationTheme::Efficiency,
        };
        let modified2 = apply_mutation(&config, &output_mutation);
        assert_eq!(modified2.max_tool_output_chars, 2000);
        // 他のパラメータは変更されない
        assert_eq!(modified2.max_iterations, config.max_iterations);
        assert!(
            (modified2.base_inference.temperature - config.base_inference.temperature).abs()
                < f64::EPSILON
        );
    }

    #[test]
    fn test_adaptive_cycle_rotation() {
        let mut hyp_gen = HypothesisGenerator::default();
        let n_rules = hyp_gen.rules.len();
        let n_params = param_mutations().len();
        let total = n_rules + n_params;

        // 最初のn_rules個はPromptRule
        for i in 0..n_rules {
            let m = hyp_gen.next_mutation(i);
            assert_eq!(
                m.mutation_type,
                MutationType::PromptRule,
                "slot {i}はPromptRule"
            );
        }

        // 次のn_params個はAgentParam
        let mut hyp_gen2 = HypothesisGenerator::default();
        for i in n_rules..total {
            let m = hyp_gen2.next_mutation(i);
            assert_eq!(
                m.mutation_type,
                MutationType::AgentParam,
                "slot {i}はAgentParam"
            );
        }

        // totalでラップ
        let mut hyp_gen3 = HypothesisGenerator::default();
        let m_wrap = hyp_gen3.next_mutation(total);
        assert_eq!(m_wrap.mutation_type, MutationType::PromptRule);
    }

    #[test]
    fn test_default_rules_expanded() {
        let rules = default_prompt_rules();
        // 20候補に拡張
        assert_eq!(rules.len(), 20, "プロンプトルール候補は20件");

        let descriptions: Vec<&str> = rules.iter().map(|r| r.description.as_str()).collect();
        // 既存ルール保持
        assert!(descriptions.contains(&"ツール使用前に思考を強制"));
        assert!(descriptions.contains(&"エラー分析の強制"));
        assert!(descriptions.contains(&"フォールバック戦略"));
        // 新規ルール存在
        assert!(descriptions.contains(&"冗長ツール呼び出し抑制"));
        assert!(descriptions.contains(&"タスク分解の強制"));
        assert!(descriptions.contains(&"推測回避・事実確認"));
    }

    #[test]
    fn test_param_mutations_count() {
        let params = param_mutations();
        assert_eq!(params.len(), 16, "パラメータ変異候補は16件");
    }

    // --- Phase 3: Oracle Feedback テスト（NAT extract_worst_reasoning知見） ---

    #[test]
    fn t_extract_worst_reasoning_empty() {
        let experiments: Vec<Experiment> = vec![];
        let worst = extract_worst_reasoning(&experiments, 5);
        assert!(worst.is_empty());
    }

    #[test]
    fn t_extract_worst_reasoning_filters_rejects() {
        use std::collections::HashMap;
        let experiments = vec![
            Experiment {
                experiment_id: "e1".into(), mutation_type: MutationType::PromptRule,
                mutation_detail: "rule_a".into(), baseline_score: 0.8,
                experiment_score: 0.75, delta: -0.05, accepted: false,
                duration_secs: 10.0, config_snapshot: HashMap::new(),
                pass_at_k: None, pass_consecutive_k: None, score_variance: None,
                prescreened: false,
            },
            Experiment {
                experiment_id: "e2".into(), mutation_type: MutationType::PromptRule,
                mutation_detail: "rule_b".into(), baseline_score: 0.8,
                experiment_score: 0.82, delta: 0.02, accepted: true,
                duration_secs: 10.0, config_snapshot: HashMap::new(),
                pass_at_k: None, pass_consecutive_k: None, score_variance: None,
                prescreened: false,
            },
            Experiment {
                experiment_id: "e3".into(), mutation_type: MutationType::AgentParam,
                mutation_detail: "param_x".into(), baseline_score: 0.8,
                experiment_score: 0.7, delta: -0.10, accepted: false,
                duration_secs: 10.0, config_snapshot: HashMap::new(),
                pass_at_k: None, pass_consecutive_k: None, score_variance: None,
                prescreened: false,
            },
        ];
        let worst = extract_worst_reasoning(&experiments, 5);
        assert_eq!(worst.len(), 2, "REJECT2件のみ");
        assert_eq!(worst[0].0, "param_x", "最悪delta順");
        assert!(worst[0].1 < worst[1].1);
    }

    #[test]
    fn t_extract_worst_reasoning_truncates() {
        use std::collections::HashMap;
        let experiments: Vec<Experiment> = (0..10)
            .map(|i| Experiment {
                experiment_id: format!("e{i}"), mutation_type: MutationType::PromptRule,
                mutation_detail: format!("rule_{i}"), baseline_score: 0.8,
                experiment_score: 0.8 - (i as f64 * 0.01), delta: -(i as f64 * 0.01),
                accepted: false, duration_secs: 10.0, config_snapshot: HashMap::new(),
                pass_at_k: None, pass_consecutive_k: None, score_variance: None,
                prescreened: false,
            })
            .collect();
        let worst = extract_worst_reasoning(&experiments, 3);
        assert_eq!(worst.len(), 3, "worst_n=3で切り詰め");
    }

    // --- Phase 4: LabStagnationDetector テスト（NAT adaptive triggers知見） ---

    #[test]
    fn t_stagnation_detector_no_trigger() {
        let mut det = LabStagnationDetector::default();
        let t = det.record_and_check(-0.01, 0.80);
        assert_eq!(t, LabTrigger::None);
    }

    #[test]
    fn t_stagnation_detector_stagnation() {
        let mut det = LabStagnationDetector::new(3, 0.001);
        det.record_and_check(-0.01, 0.80); // sets best=0.80, unchanged=0
        det.record_and_check(-0.02, 0.78); // unchanged=1
        det.record_and_check(-0.03, 0.77); // unchanged=2
        let t = det.record_and_check(-0.01, 0.76); // unchanged=3 -> trigger
        assert_eq!(t, LabTrigger::Stagnation, "4th non-improvement triggers at threshold=3");
    }

    #[test]
    fn t_stagnation_detector_reset_on_improvement() {
        let mut det = LabStagnationDetector::new(3, 0.001);
        det.record_and_check(-0.01, 0.80);
        det.record_and_check(-0.02, 0.78);
        det.record_and_check(0.05, 0.85); // improvement resets
        let t = det.record_and_check(-0.01, 0.84);
        assert_eq!(t, LabTrigger::None, "reset after improvement");
    }

    #[test]
    fn t_stagnation_detector_variance_collapse() {
        let mut det = LabStagnationDetector::new(100, 0.001); // high stagnation to avoid
        // 5 nearly identical deltas
        for _ in 0..5 {
            det.record_and_check(-0.01, 0.79);
        }
        let t = det.record_and_check(-0.01, 0.78);
        assert_eq!(t, LabTrigger::VarianceCollapse);
    }

    #[test]
    fn t_stagnation_detector_reset() {
        let mut det = LabStagnationDetector::default();
        det.record_and_check(-0.01, 0.80);
        det.record_and_check(-0.02, 0.78);
        det.reset();
        let t = det.record_and_check(-0.01, 0.77);
        assert_eq!(t, LabTrigger::None, "reset clears state");
    }

    #[test]
    fn t_add_worst_reasoning_insights() {
        let mut hypo = HypothesisGenerator::default();
        let worst = vec![
            ("rule_a".to_string(), -0.05),
            ("rule_b".to_string(), -0.03),
        ];
        hypo.add_worst_reasoning_insights(&worst);
        // 20 rules + 2 insights = 22 rules, 22 + 16 params = 38 total
        // slot 20 and 21 should be insight-derived
        let m20 = hypo.next_mutation(20);
        assert!(
            m20.detail.contains("insight:"),
            "slot 20 should be insight: {}",
            m20.detail
        );
    }

    // ===== Phase B2: judge_gate_check tests (TDD Red) =====

    /// テスト用 judge: 事前にスクリプトされた RubricScore をキューから順に返す
    struct ScriptedJudge {
        responses: std::collections::VecDeque<Result<RubricScore>>,
        call_count: usize,
    }

    impl ScriptedJudge {
        fn new(scores: Vec<RubricScore>) -> Self {
            Self {
                responses: scores.into_iter().map(Ok).collect(),
                call_count: 0,
            }
        }

        fn with_results(results: Vec<Result<RubricScore>>) -> Self {
            Self {
                responses: results.into_iter().collect(),
                call_count: 0,
            }
        }
    }

    impl LlmJudge for ScriptedJudge {
        fn evaluate(
            &mut self,
            _task_description: &str,
            _response: &str,
            _trajectory: &[String],
        ) -> Result<RubricScore> {
            self.call_count += 1;
            self.responses
                .pop_front()
                .unwrap_or_else(|| anyhow::bail!("ScriptedJudge: queue empty"))
        }
    }

    fn make_score(comp: f64, corr: f64, reas: f64) -> RubricScore {
        RubricScore {
            completeness: comp,
            correctness: corr,
            reasoning_quality: reas,
            raw_judge_response: "scripted".into(),
        }
    }

    fn make_task_score(task_id: &str, with_run: bool) -> crate::agent::benchmark::MultiRunTaskScore {
        let s = crate::agent::benchmark::MultiRunTaskScore::from_scores(
            task_id.into(),
            vec![1.0, 1.0, 1.0],
            0.5,
        );
        if with_run {
            s.with_last_run("dummy response".into(), vec!["tool_a".into()])
        } else {
            s
        }
    }

    fn make_descs(ids: &[&str]) -> HashMap<String, String> {
        ids.iter()
            .map(|id| ((*id).to_string(), format!("desc for {id}")))
            .collect()
    }

    #[test]
    fn test_judge_gate_passes_when_mean_above_threshold() {
        // composite = 0.4*0.9 + 0.4*0.9 + 0.2*0.8 = 0.88 (high score, passes 0.7)
        let mut judge = ScriptedJudge::new(vec![make_score(0.9, 0.9, 0.8); 2]);
        let result = MultiRunBenchmarkResult {
            task_scores: vec![make_task_score("t1", true), make_task_score("t2", true)],
            duration_secs: 1.0,
        };
        let descs = make_descs(&["t1", "t2"]);

        let outcome = judge_gate_check(&mut judge, &result, 0.7, 2, &descs).unwrap();
        assert!(outcome.passed, "high judge scores should pass threshold");
        assert!(
            outcome.mean_composite > 0.7,
            "mean_composite={}",
            outcome.mean_composite
        );
        assert_eq!(outcome.scores.len(), 2);
    }

    #[test]
    fn test_judge_gate_rejects_when_mean_below_threshold() {
        // composite = 0.4*0.5 + 0.4*0.5 + 0.2*0.5 = 0.5 (below 0.7)
        let mut judge = ScriptedJudge::new(vec![make_score(0.5, 0.5, 0.5); 2]);
        let result = MultiRunBenchmarkResult {
            task_scores: vec![make_task_score("t1", true), make_task_score("t2", true)],
            duration_secs: 1.0,
        };
        let descs = make_descs(&["t1", "t2"]);

        let outcome = judge_gate_check(&mut judge, &result, 0.7, 2, &descs).unwrap();
        assert!(!outcome.passed, "low judge scores should fail threshold");
        assert!(
            outcome.mean_composite < 0.7,
            "mean_composite={}",
            outcome.mean_composite
        );
    }

    #[test]
    fn test_judge_gate_skips_tasks_without_last_run() {
        // last_response が None の task は judge にかけられない
        let mut judge = ScriptedJudge::new(vec![make_score(0.9, 0.9, 0.9)]);
        let result = MultiRunBenchmarkResult {
            task_scores: vec![
                make_task_score("t1", false), // skipped
                make_task_score("t2", true),  // judged
            ],
            duration_secs: 1.0,
        };
        let descs = make_descs(&["t1", "t2"]);

        let outcome = judge_gate_check(&mut judge, &result, 0.7, 2, &descs).unwrap();
        assert_eq!(judge.call_count, 1, "only t2 should be judged");
        assert_eq!(outcome.scores.len(), 1);
    }

    #[test]
    fn test_judge_gate_fail_open_on_judge_error() {
        // judge が Err を返した task はスキップ、scores が空なら passed=true
        let mut judge =
            ScriptedJudge::with_results(vec![Err(anyhow::anyhow!("judge backend down"))]);
        let result = MultiRunBenchmarkResult {
            task_scores: vec![make_task_score("t1", true)],
            duration_secs: 1.0,
        };
        let descs = make_descs(&["t1"]);

        let outcome = judge_gate_check(&mut judge, &result, 0.7, 1, &descs).unwrap();
        assert!(
            outcome.passed,
            "all-error judge should fail-open (passed=true)"
        );
        assert_eq!(outcome.scores.len(), 0);
    }

    #[test]
    fn test_judge_gate_respects_sample_size() {
        // sample_size=1 なら 1 task のみ judge にかける
        let mut judge = ScriptedJudge::new(vec![make_score(0.9, 0.9, 0.9); 5]);
        let result = MultiRunBenchmarkResult {
            task_scores: (0..5)
                .map(|i| make_task_score(&format!("t{i}"), true))
                .collect(),
            duration_secs: 1.0,
        };
        let descs = make_descs(&["t0", "t1", "t2", "t3", "t4"]);

        let outcome = judge_gate_check(&mut judge, &result, 0.7, 1, &descs).unwrap();
        assert_eq!(judge.call_count, 1, "sample_size=1 should judge 1 task");
        assert_eq!(outcome.scores.len(), 1);
    }
}
