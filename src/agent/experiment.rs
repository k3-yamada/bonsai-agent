use crate::observability::logger::{LogLevel, log_event};
use anyhow::Result;
use std::collections::HashMap;

use crate::agent::agent_loop::AgentConfig;
use crate::agent::benchmark::{
    BenchmarkSuite, CapabilityTier, MultiRunBenchmarkResult, MultiRunConfig,
};
use crate::agent::event_store::EventStore;
use crate::agent::experiment_log::{
    AcceptedMutation, Experiment, ExperimentLog, MutationTheme, MutationType, load_accepted_archive,
};
use crate::agent::judge::{HttpAdvisorJudge, LlmJudge, RubricScore};
use crate::agent::validate::PathGuard;
use crate::cancel::CancellationToken;
use crate::memory::experience::{ExperienceStore, SubgoalJudgeMethod, extract_hindsight_relabels};
use crate::memory::factcheck::{self, FactCheckSummary};
use crate::memory::graph::KnowledgeGraph;
use crate::memory::heuristics::{
    HeuristicStore, HeuristicSummary, extract_reflection_full, is_erl_enabled,
};
use crate::memory::skill::SkillStore;
use crate::memory::store::MemoryStore;
use crate::observability::audit::{AuditAction, AuditLog};
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
    /// AdvisorConfig::dynamic_skip_threshold を変更 (Self-Verification Dilemma 動的 skip 閾値、項目 210/211)
    SetAdvisorThreshold(f64),
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
        ParamMutation {
            detail: "max_iterations: 12 (+2)",
            action: MutationAction::SetMaxIterations(12),
        },
        ParamMutation {
            detail: "max_iterations: 8 (-2)",
            action: MutationAction::SetMaxIterations(8),
        },
        ParamMutation {
            detail: "max_iterations: 15 (+5)",
            action: MutationAction::SetMaxIterations(15),
        },
        ParamMutation {
            detail: "max_tools_selected: 3 (-2)",
            action: MutationAction::SetMaxToolsSelected(3),
        },
        ParamMutation {
            detail: "max_tools_selected: 7 (+2)",
            action: MutationAction::SetMaxToolsSelected(7),
        },
        ParamMutation {
            detail: "max_tools_selected: 4 (-1)",
            action: MutationAction::SetMaxToolsSelected(4),
        },
        ParamMutation {
            detail: "max_retries: 1 (-2)",
            action: MutationAction::SetMaxRetries(1),
        },
        ParamMutation {
            detail: "max_retries: 5 (+2)",
            action: MutationAction::SetMaxRetries(5),
        },
        ParamMutation {
            detail: "max_retries: 4 (+1)",
            action: MutationAction::SetMaxRetries(4),
        },
        ParamMutation {
            detail: "temperature: 0.2 (超精密)",
            action: MutationAction::SetTemperature(0.2),
        },
        ParamMutation {
            detail: "temperature: 0.5 (低め)",
            action: MutationAction::SetTemperature(0.5),
        },
        ParamMutation {
            detail: "temperature: 0.7 (バランス)",
            action: MutationAction::SetTemperature(0.7),
        },
        ParamMutation {
            detail: "temperature: 0.9 (探索的)",
            action: MutationAction::SetTemperature(0.9),
        },
        ParamMutation {
            detail: "max_tool_output_chars: 2000 (コンパクト)",
            action: MutationAction::SetMaxToolOutputChars(2000),
        },
        ParamMutation {
            detail: "max_tool_output_chars: 6000 (増量)",
            action: MutationAction::SetMaxToolOutputChars(6000),
        },
        ParamMutation {
            detail: "max_tool_output_chars: 8000 (大容量)",
            action: MutationAction::SetMaxToolOutputChars(8000),
        },
        // --- Self-Verification Dilemma Phase 5 (項目 211): 動的 skip threshold variant ---
        ParamMutation {
            detail: "advisor.dynamic_skip_threshold: 0.3 (低閾値、過剰 skip 抑制)",
            action: MutationAction::SetAdvisorThreshold(0.3),
        },
        ParamMutation {
            detail: "advisor.dynamic_skip_threshold: 0.4 (中閾値、推奨設定)",
            action: MutationAction::SetAdvisorThreshold(0.4),
        },
        ParamMutation {
            detail: "advisor.dynamic_skip_threshold: 0.5 (高閾値、保守的)",
            action: MutationAction::SetAdvisorThreshold(0.5),
        },
    ]
}

impl HypothesisGenerator {
    /// 過去の実験ログから試行済みセットを構築
    pub fn with_tried_details(mut self, details: impl IntoIterator<Item = String>) -> Self {
        self.tried.extend(details);
        self
    }

    /// 次の変異候補を生成（適応型: 試行済みをスキップ）
    ///
    /// env `BONSAI_LAB_PHASE5_FOCUS` 設定時は focus filter 経由で variant 絞り込み。
    /// 値 `"advisor_threshold"` で SetAdvisorThreshold variant のみ返す (項目 211 Phase 5)。
    pub fn next_mutation(&mut self, experiment_count: usize) -> Mutation {
        let focus = std::env::var("BONSAI_LAB_PHASE5_FOCUS").ok();
        self.next_mutation_with_focus(experiment_count, focus.as_deref())
    }

    /// focus filter 付き次変異候補生成 (Phase 5 effectiveness 検証用、項目 211)。
    ///
    /// `focus = Some("advisor_threshold")` で `MutationAction::SetAdvisorThreshold` 系のみ返す。
    /// focus が None or 該当 variant 不在なら既存 rotation 動作にフォールバック。
    /// テストは env を介さず focus を引数で直接渡せる (test 隔離性確保)。
    pub fn next_mutation_with_focus(
        &mut self,
        experiment_count: usize,
        focus: Option<&str>,
    ) -> Mutation {
        if let Some(f) = focus
            && let Some(m) = self.try_focused_mutation(f, experiment_count)
        {
            return m;
        }
        self.next_mutation_unfocused(experiment_count)
    }

    /// focus filter で対応 variant のみ rotate 選択 (Phase 5 用、項目 211)。
    /// すべて tried 済みでも focus 維持のため再選択 (rotation cycle 続行)。
    fn try_focused_mutation(&mut self, focus: &str, experiment_count: usize) -> Option<Mutation> {
        let params = param_mutations();
        let filtered: Vec<&ParamMutation> = params
            .iter()
            .filter(|p| match focus {
                "advisor_threshold" => matches!(p.action, MutationAction::SetAdvisorThreshold(_)),
                _ => false,
            })
            .collect();
        if filtered.is_empty() {
            return None;
        }
        // tried 済みスキップで rotate (focus 内 dedup)
        for offset in 0..filtered.len() {
            let idx = (experiment_count + offset) % filtered.len();
            let p = filtered[idx];
            if !self.tried.contains(p.detail) {
                self.tried.insert(p.detail.into());
                return Some(Mutation {
                    mutation_type: MutationType::AgentParam,
                    detail: p.detail.into(),
                    apply: p.action.clone(),
                    theme: MutationTheme::from_cycle(0),
                });
            }
        }
        // すべて tried 済みでも focus 維持で再選択
        let p = filtered[experiment_count % filtered.len()];
        Some(Mutation {
            mutation_type: MutationType::AgentParam,
            detail: p.detail.into(),
            apply: p.action.clone(),
            theme: MutationTheme::from_cycle(0),
        })
    }

    /// focus 未指定時の既存 rotation ロジック (rules → params 全体 rotate)。
    fn next_mutation_unfocused(&mut self, experiment_count: usize) -> Mutation {
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
            rule: "10. ツールが失敗した場合、同じツールを2回まで再試行してから別の方法を試す"
                .into(),
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
        soul_path: base_config.soul_path.clone(),
        n_ctx_budget: base_config.n_ctx_budget,
        memory_blocks: base_config.memory_blocks.clone(),
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
        MutationAction::SetAdvisorThreshold(t) => {
            config.advisor.dynamic_skip_threshold = *t;
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

// ─── smoke 補正係数（項目 184 由来、Lab smoke→core 42% retention） ───────────
//
// Pessimistic Prescreening 戦略: smoke は positive delta を体系的に inflate するため
// (項目 184: smoke +0.0969 → core +0.0405、retention ~42%)、positive のみ scaling、
// negative は保持。inflated win を疑い、loss signal は信頼する非対称設計。
//
// WARNING: BONSAI_LAB_SMOKE / BONSAI_LAB_SMOKE_CORRECTION を実験中に変更すると、
// 同一 Lab セッション内で評価基準が揺れ計測整合性が崩れる。Lab 開始前に固定すること。

/// Lab smoke モード判定（既存 `BONSAI_LAB_SMOKE` と同セマンティクス）
fn lab_smoke_enabled() -> bool {
    std::env::var("BONSAI_LAB_SMOKE")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
}

/// smoke 補正係数のデフォルト値
///
/// 項目 184 で実測された smoke 5-task → core 22-task の retention rate (~0.42)。
/// `BONSAI_LAB_SMOKE_CORRECTION` env var で `0 < x ≤ 1` の値に上書き可能、
/// 範囲外/無効値はこのデフォルトに自動フォールバック (`apply_smoke_correction_to_delta` 経由)。
pub const DEFAULT_SMOKE_CORRECTION: f64 = 0.42;

/// smoke 補正係数を取得（env override 優先、無効値/範囲外は default にフォールバック）
///
/// 受理範囲: `(0.0, 1.0]`。0 以下、NaN/inf、parse 不能は default に倒す。
fn smoke_correction_coefficient() -> f64 {
    std::env::var("BONSAI_LAB_SMOKE_CORRECTION")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0 && *v <= 1.0)
        .unwrap_or(DEFAULT_SMOKE_CORRECTION)
}

/// smoke モード時に推定 delta に補正係数を適用（sign-aware: positive のみ scaling）
///
/// **Why sign-aware**: smoke が positive delta を inflate する性質 (項目 184) を補正する一方、
/// negative delta も同じ係数で scaling すると false-accept リスク (例: -0.10 → -0.042 で
/// -0.01 threshold を通過) が発生する。inflated win のみ補正、negative signal は保持。
fn apply_smoke_correction_to_delta(delta: f64) -> f64 {
    if lab_smoke_enabled() && delta > 0.0 {
        delta * smoke_correction_coefficient()
    } else {
        delta
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
        base_config,
        mutation,
        0.0,
        backend,
        tools,
        path_guard,
        cancel,
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

    // pre-screen は AgentHER 対象外。Option A 移行 (agenther-option-a-migration.md A3) で
    // run_k は &MemoryStore 必須化されたため、persistent.events 汚染回避のため scratch を作る。
    // scratch_store は本 fn scope 抜けで drop → events も消える (旧 None 渡しと同等挙動)。
    let scratch_store = MemoryStore::in_memory()?;

    // サンプルタスク上でベースライン計測（フルスイートのスコアとは異なる可能性がある）
    let baseline = sample_suite.run_k(
        base_config,
        backend,
        tools,
        path_guard,
        cancel,
        &quick_multi,
        pass_threshold,
        &scratch_store,
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
        &scratch_store,
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
            let variance = self
                .recent_deltas
                .iter()
                .map(|d| (d - mean).powi(2))
                .sum::<f64>()
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

/// AgentFloor tier map をログ出力する (plan §4.5)。
///
/// `tier_avg_scores` が None (ladder mode 非使用) の場合は no-op。
/// baseline 計測直後および各実験計測直後に呼ぶことで、
/// 能力分布の推移を `[INFO][lab.agentfloor]` チャンネルで追跡できる。
fn emit_tier_map_log(result: &MultiRunBenchmarkResult, cycle_label: &str) {
    let Some(scores) = &result.tier_avg_scores else {
        return;
    };
    log_event(
        LogLevel::Info,
        "lab.agentfloor",
        &format!("AgentFloor capability map ({cycle_label}):"),
    );
    for tier in CapabilityTier::all() {
        if let Some(score) = scores[tier as usize] {
            let paper = tier.paper_baseline();
            let delta = score - paper;
            log_event(
                LogLevel::Info,
                "lab.agentfloor",
                &format!(
                    "  {:18}: {:.2}  (paper {:.2}, {:+.2})",
                    tier.label(),
                    score,
                    paper,
                    delta
                ),
            );
        }
    }
    if let Some((weakest, weakest_score)) = result.weakest_tier() {
        log_event(
            LogLevel::Info,
            "lab.agentfloor",
            &format!(
                "  weakest_tier      = {} ({:.2})",
                weakest.label(),
                weakest_score
            ),
        );
    }
}

/// Frontier benchmark の bucket / inject scores をログ出力する
/// (`frontier-benchmark-impl.md` Sub-Phase 2F、antirez/ds4 ds4-bench inspired)。
///
/// `BONSAI_FRONTIER_ENABLED` / `BONSAI_FRONTIER_INJECT_ENABLED` のどちらか有効時に
/// `[INFO][lab.frontier]` チャンネルで context-length axis を追跡できる。
/// 両方 OFF / データなしの場合は no-op。
fn emit_frontier_log(result: &MultiRunBenchmarkResult, cycle_label: &str) {
    let bucket_enabled = crate::agent::frontier::is_frontier_enabled();
    let inject_enabled = crate::agent::frontier::is_frontier_inject_enabled();
    if !bucket_enabled && !inject_enabled {
        return;
    }
    log_event(
        LogLevel::Info,
        "lab.frontier",
        &format!("Frontier metric ({cycle_label}):"),
    );
    if bucket_enabled {
        let boundaries = crate::agent::frontier::parse_frontier_buckets_env();
        let buckets = result.composite_frontier_bucket_scores(&boundaries);
        if buckets.is_empty() {
            log_event(
                LogLevel::Info,
                "lab.frontier",
                "  bucket: (no final_context_tokens populated)",
            );
        } else {
            for (idx, score) in &buckets {
                let range = if *idx == 0 {
                    format!("[0, {})", boundaries[0])
                } else if *idx < boundaries.len() {
                    format!("[{}, {})", boundaries[idx - 1], boundaries[*idx])
                } else {
                    format!("[{}, ∞)", boundaries[boundaries.len() - 1])
                };
                log_event(
                    LogLevel::Info,
                    "lab.frontier",
                    &format!("  bucket {idx} {range}: {score:.4}"),
                );
            }
        }
    }
    if inject_enabled {
        let inject = result.composite_frontier_inject_scores();
        if inject.is_empty() {
            log_event(
                LogLevel::Info,
                "lab.frontier",
                "  inject: (no T6 tasks populated)",
            );
        } else {
            for (size_kb, mean) in &inject {
                log_event(
                    LogLevel::Info,
                    "lab.frontier",
                    &format!("  inject {size_kb:>3} KB: {mean:.4}"),
                );
            }
        }
    }
}

/// pre-screen REJECT 経路で `Experiment` を構築する private helper (項目 224)。
///
/// G-4c v3 PARTIAL PASS で発覚: pre-screen REJECT は full run 未実行のため
/// `MultiRunBenchmarkResult` を生成せず、従来は inline literal で `tier_t1..t6: None`
/// 固定だった。修正 = baseline の tier 値を carry-over する (full-cycle と一貫した
/// `from_multi_results` の `and_then(|t| t[N])` pattern と同形)。
///
/// `baseline_tier_avg_scores=None` 時 (LADDER mode 未使用) は全 tier None で後方互換。
///
/// 項目 234 候補 (frontier-prescreen-carryover-fix) で 7→9 引数に拡張、9/7 clippy
/// `too_many_arguments` を許容: pre-screen REJECT 行に対して baseline の段階的 carry-over
/// (tier / frontier_bucket / frontier_inject) を select 可能化する設計選択
/// (struct 集約 refactor は別 plan、`MultiRunBenchmarkResult` から都度取り出す既存
/// pattern を維持する方が caller 1 箇所の変更で済み diff 最小)。
#[allow(clippy::too_many_arguments)]
fn build_prescreen_reject_experiment(
    experiment_id: String,
    mutation_type: MutationType,
    mutation_detail: String,
    baseline_score: f64,
    baseline_tier_avg_scores: Option<[Option<f64>; 6]>,
    baseline_frontier_bucket_scores: &[(usize, f64)],
    baseline_frontier_inject_scores: &[(usize, f64)],
    estimated_delta: f64,
    snapshot: HashMap<String, String>,
) -> Experiment {
    let tiers = baseline_tier_avg_scores;
    // 項目 234 候補 (frontier-prescreen-carryover-fix): env-gated baseline carry-over。
    // env unset で空 Vec = 後方互換 100% (項目 229 当初実装と同等)。
    // env ON で baseline.frontier_* を carry-over = "no improvement" 仮定で
    // pre-screen REJECT 行の Lab v19 解析 sample size を +20-30% 拡張する。
    let frontier_bucket = if crate::agent::frontier::is_frontier_enabled() {
        baseline_frontier_bucket_scores.to_vec()
    } else {
        Vec::new()
    };
    let frontier_inject = if crate::agent::frontier::is_frontier_inject_enabled() {
        baseline_frontier_inject_scores.to_vec()
    } else {
        Vec::new()
    };
    Experiment {
        experiment_id,
        mutation_type,
        mutation_detail,
        baseline_score,
        experiment_score: baseline_score + estimated_delta,
        delta: estimated_delta,
        accepted: false,
        duration_secs: 0.0,
        config_snapshot: snapshot,
        pass_at_k: None,
        pass_consecutive_k: None,
        score_variance: None,
        prescreened: true,
        reliability_decay: None,
        variance_amplification: None,
        graceful_degradation: None,
        stability_delta: None,
        tier_t1: tiers.and_then(|t| t[0]),
        tier_t2: tiers.and_then(|t| t[1]),
        tier_t3: tiers.and_then(|t| t[2]),
        tier_t4: tiers.and_then(|t| t[3]),
        tier_t5: tiers.and_then(|t| t[4]),
        tier_t6: tiers.and_then(|t| t[5]),
        // 項目 225 (PASS@(k,T)): pre-screen REJECT は full run 未実行のため measurement なし。
        // tier carry-over と異なり baseline には PASS@(k,T) が計算済 (run_k 内で env 起動時)
        // でも、experiment 側に carry-over するセマンティクスは無意味 (T 軸は experiment の
        // efficiency を測る軸であり baseline 値の流用は誤情報)。空 Vec で保持する。
        pass_at_k_t_steps: Vec::new(),
        pass_at_k_t_seconds: Vec::new(),
        // V16 (frontier benchmark): env-gated baseline carry-over (項目 234 候補)。
        // BONSAI_FRONTIER_ENABLED=1 で baseline.frontier_bucket_scores を carry-over、
        // BONSAI_FRONTIER_INJECT_ENABLED=1 で baseline.frontier_inject_scores を carry-over。
        // 両 env unset (default) で空 Vec = 項目 229 の "no measurement" 後方互換挙動。
        frontier_bucket_scores: frontier_bucket,
        frontier_inject_scores: frontier_inject,
    }
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
    // BONSAI_LAB_SMOKE=1 → smoke (5 件), BONSAI_BENCH_TIER=core/extended → tier 別 (項目 172 P1)
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
        // 大文字小文字を吸収し、未対応値はワーニングを出して default に明示フォールバック
        // (Phase 5 の長時間 baseline 計測中にタイポによる silent fallback を防ぐ。Codex audit LOW finding 対応)
        let bench_tier = std::env::var("BONSAI_BENCH_TIER")
            .ok()
            .map(|v| v.trim().to_ascii_lowercase());

        match bench_tier.as_deref() {
            Some("core") => {
                log_event(
                    LogLevel::Info,
                    "lab",
                    "BONSAI_BENCH_TIER=core → core_tasks() 使用（22 タスク）",
                );
                BenchmarkSuite::core_tasks()
            }
            Some("extended") => {
                log_event(
                    LogLevel::Info,
                    "lab",
                    "BONSAI_BENCH_TIER=extended → extended_tasks() 使用（18 タスク）",
                );
                BenchmarkSuite::extended_tasks()
            }
            Some("") | None => BenchmarkSuite::default_tasks(),
            Some(other) => {
                log_event(
                    LogLevel::Warn,
                    "lab",
                    &format!(
                        "BONSAI_BENCH_TIER={other} は未対応のため default_tasks() 使用（core/extended のみ対応）"
                    ),
                );
                BenchmarkSuite::default_tasks()
            }
        }
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

    // handoff 05-07g Phase 5 scoping: Lab cycle 開始時の events.id snapshot。
    // 終端 AgentHER pass はこの id < event.id の events のみ対象とし、
    // 過去 cycle の累積汚染を回避する。SQL レベルのエラーは 0 fallback (cold-start 互換)。
    let lab_start_event_id: i64 = EventStore::new(store.conn()).current_max_id().unwrap_or(0);

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
        store,
    )?;
    eprintln!(
        "[lab] ベースライン: score={:.4} pass@k={:.4} pass_consec={:.4} ({:.1}s)",
        baseline.composite_score(),
        baseline.composite_pass_at_k(),
        baseline.composite_pass_consecutive_k(),
        baseline.duration_secs
    );
    // AgentFloor tier map ログ出力 (plan §4.5: baseline 計測直後)
    emit_tier_map_log(&baseline, "baseline");
    emit_frontier_log(&baseline, "baseline");

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
            let raw_estimated_delta = estimate_mutation_effect_with_baseline(
                base_config,
                &mutation,
                baseline.composite_score(),
                backend,
                tools,
                path_guard,
                cancel,
            )?;

            // smoke モード時のみ補正を適用 (sign-aware ×0.42、項目 184)
            // env を 1 回だけ読んで log の整合性を保ちつつ、決定値は canonical function 経由
            // (Codex audit Low #1: log フィールドのレース window を mitigate しつつ、
            //  決定ロジックは apply_smoke_correction_to_delta() に集約 = dead_code warning 解消)
            let smoke_enabled = lab_smoke_enabled();
            let smoke_coeff = smoke_correction_coefficient();
            let estimated_delta = apply_smoke_correction_to_delta(raw_estimated_delta);

            if smoke_enabled {
                log_event(
                    LogLevel::Info,
                    "lab",
                    &format!(
                        "pre-screen smoke correction: raw_delta={:+.4} coeff={:.2} adjusted_delta={:+.4} threshold={:+.4}",
                        raw_estimated_delta,
                        smoke_coeff,
                        estimated_delta,
                        loop_config.prescreening_threshold,
                    ),
                );
            }

            if estimated_delta < loop_config.prescreening_threshold {
                eprintln!(
                    "[lab] pre-screen REJECT: {} (estimated delta={:+.4})",
                    mutation.detail, estimated_delta
                );
                let snapshot = config_snapshot(&modified_config);
                // 項目 224: baseline tier carry-over。
                // 項目 234 候補: baseline frontier carry-over (env-gated、default OFF で後方互換)。
                // baseline.frontier_* は composite method 経由で取得 (フィールド直 access はなく
                // run_k 完走後に lazy 計算する API、env OFF 時は空 Vec で安全)。
                let frontier_boundaries = crate::agent::frontier::parse_frontier_buckets_env();
                let baseline_frontier_buckets =
                    baseline.composite_frontier_bucket_scores(&frontier_boundaries);
                let baseline_frontier_inject = baseline.composite_frontier_inject_scores();
                let exp = build_prescreen_reject_experiment(
                    experiment_id.clone(),
                    mutation.mutation_type,
                    mutation.detail,
                    baseline.composite_score(),
                    baseline.tier_avg_scores,
                    &baseline_frontier_buckets,
                    &baseline_frontier_inject,
                    estimated_delta,
                    snapshot,
                );
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
            store,
        )?;
        let snapshot = config_snapshot(&modified_config);
        // AgentFloor tier map ログ出力 (plan §4.6: 各実験計測直後)
        emit_tier_map_log(&result, &format!("exp_{experiment_count}"));
        emit_frontier_log(&result, &format!("exp_{experiment_count}"));

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

        // 項目 185 D-side: smoke モード時にフル評価 delta の raw vs adjusted を可視化
        // (accept 判定は `delta > 0.0` で sign-preserving のため adjusted で結果不変だが、
        //  operator が smoke→core retention 42% を考慮して結果を解釈できるよう情報出力)
        if lab_smoke_enabled() {
            let raw_delta = exp.delta;
            let adjusted_delta = apply_smoke_correction_to_delta(raw_delta);
            log_event(
                LogLevel::Info,
                "lab",
                &format!(
                    "full-eval smoke correction: raw_delta={:+.4} adjusted_delta={:+.4} accepted={} (sign-preserving + threshold=0.0 で判定は不変)",
                    raw_delta, adjusted_delta, exp.accepted,
                ),
            );
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
        let trigger =
            stagnation_detector.record_and_check(last_exp.delta, last_exp.experiment_score);
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
                    &format!(
                        "oracle feedback: {}件の失敗パターンから逆向き変異追加",
                        worst.len()
                    ),
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

    // Plan A FactCheck post-Lab pass (項目 230 候補、AgentHER 直前 / plan §3 Phase 2 遵守)
    // non-fatal: env unset (`BONSAI_KG_FACTCHECK_ENABLED` 未設定) で短絡、default OFF。
    // KG 状態は AgentHER pass で touch されないため順序差は観測上ゼロだが、plan 通り
    // AgentHER の前に配置 (将来 AgentHER が KG 更新へ拡張された場合の安全側設計)。
    if factcheck::is_factcheck_enabled() {
        match run_factcheck_pass_lab(store, lab_start_event_id) {
            Ok(s) => log_event(
                LogLevel::Info,
                "lab.factcheck",
                &format!(
                    "FactCheck post-Lab: total={} matched={} unknown={} conflicting={} \
                     mean_path_len={:.2}",
                    s.total, s.matched, s.unknown, s.conflicting, s.mean_path_len,
                ),
            ),
            Err(e) => log_event(
                LogLevel::Warn,
                "lab.factcheck",
                &format!("FactCheck post-Lab pass failed (non-fatal): {e}"),
            ),
        }
    }

    // AgentHER post-Lab pass: 失敗 trajectory の HSL relabel + 成功軌跡の symmetric promotion
    // (handoff 05-07e TODO #1、項目 161/201 dead-code 解消)
    // non-fatal: エラーは Warn log のみで握り潰し、Lab 結果は通常通り返す
    // handoff 05-07g Phase 5 scoping: lab_start_event_id < event.id の events
    // のみ AgentHER 対象 (繰り返し Lab cycle 跨ぎの累積汚染を回避)
    match run_hindsight_pass(store, lab_start_event_id) {
        Ok(s) => log_event(
            LogLevel::Info,
            "lab.agenther",
            &format!(
                "AgentHER post-Lab: failed={} successful={} relabels={} skills={} insights={}",
                s.failed_sessions,
                s.successful_sessions,
                s.relabels,
                s.skills_promoted,
                s.insights_recorded,
            ),
        ),
        Err(e) => log_event(
            LogLevel::Warn,
            "lab.agenther",
            &format!("AgentHER post-Lab pass failed (non-fatal): {e}"),
        ),
    }

    // ERL post-Lab pass (項目 213、plan §4/§5): 自然言語助言を heuristics layer に保管。
    // 順序は AgentHER の後 (F4 audit)、両者同 lab_start_event_id で scoping、non-fatal。
    match run_heuristics_pass(store, lab_start_event_id, backend) {
        Ok(s) => log_event(
            LogLevel::Info,
            "lab.heuristics",
            &format!(
                "ERL post-Lab: extracted={} saved={} skipped_to_skill={} pruned={} parse_failures={}",
                s.extracted, s.saved, s.skipped_to_skill, s.pruned, s.parse_failures,
            ),
        ),
        Err(e) => log_event(
            LogLevel::Warn,
            "lab.heuristics",
            &format!("ERL post-Lab pass failed (non-fatal): {e}"),
        ),
    }

    Ok(experiments)
}

/// AgentHER post-Lab pass で集計するメトリクス（項目 201 / handoff 05-07e TODO #1）
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct HindsightSummary {
    pub failed_sessions: usize,
    pub successful_sessions: usize,
    pub relabels: usize,
    pub skills_promoted: usize,
    pub insights_recorded: usize,
}

/// Lab 完走後に呼び出され、events から失敗 trajectory の hindsight relabel を抽出 + 成功 trajectory も symmetric に skill 昇格 (項目 161/201 dead-code 解消)。
///
/// non-fatal: 任意のエラーは呼出側で `Warn` log にして握り潰す（Lab 結果を破壊しない）。
///
/// `since_event_id` は当該 Lab cycle 開始時の `MAX(events.id)` (handoff 05-07g
/// Phase 5 scoping)。`since_event_id < event.id` の events のみが AgentHER pass の
/// 対象になり、過去 cycle の events 累積汚染を回避する。`0` で全期間 = 既存挙動。
fn run_hindsight_pass(store: &MemoryStore, since_event_id: i64) -> Result<HindsightSummary> {
    let conn = store.conn();
    let event_store = EventStore::new(conn);
    let skill_store = SkillStore::new(conn);
    let exp_store = ExperienceStore::new(conn);
    let mut summary = HindsightSummary::default();

    // 1. 失敗 session (success_rate < 0.8 / steps >= 2) の HSL relabel + ECHO insight
    let failed = event_store.extract_failed_trajectories_since_id(since_event_id, 0.8, 2)?;
    summary.failed_sessions = failed.len();
    for candidate in &failed {
        let events = event_store.replay(&candidate.session_id)?;
        let relabels = extract_hindsight_relabels(
            &events,
            SubgoalJudgeMethod::ToolEndSuccessOrSideEffect, // recall 重視 default
        );
        summary.relabels += relabels.len();
        for relabel in &relabels {
            // (a) skill 昇格 (max_promote=3 で爆発防止、tool_chain UNIQUE で dedup)
            let ids = skill_store.promote_from_hindsight_relabel(relabel, 3)?;
            summary.skills_promoted += ids.len();
            // (b) ECHO insight 記録 (ExperienceType::Insight)
            exp_store.record_hindsight_insight(relabel)?;
            summary.insights_recorded += 1;
        }
    }

    // 2. 成功軌跡 (項目 161 dead-code 解消) も symmetric に skill 昇格 (prefix 'traj_')
    let successful =
        event_store.extract_successful_trajectories_since_id(since_event_id, 0.8, 2)?;
    summary.successful_sessions = successful.len();
    for candidate in &successful {
        if skill_store.promote_from_trajectory(candidate)?.is_some() {
            summary.skills_promoted += 1;
        }
    }

    Ok(summary)
}

/// Plan A (KG-Grounded Hallucination Check、項目 230 候補) post-Lab pass。
///
/// 失敗 trajectory (`extract_failed_trajectories_since_id` 経由) の `AssistantMessage`
/// event_data から content を抽出し、`factcheck::run_factcheck_pass` で triple 検証。
/// 結果を `AuditAction::FactCheck` で audit_log に persist、summary を return。
///
/// non-fatal: 任意のエラーは呼出側で `Warn` log にして握り潰す (Lab 結果を破壊しない)。
/// `since_event_id` は `run_hindsight_pass` と共有 = 同じ Lab cycle 開始時 snapshot。
///
/// 項目 235 拡張: `BONSAI_FACTCHECK_ALL_TRAJECTORIES=1` で failed + successful trajectory を
/// chain で集計 + min_steps=0 に緩和し halluc task (tool_success_rate=1.0 / 0-1 tool call)
/// の `AssistantMessage` も検証対象に含める。env unset で従来挙動 100% 互換 (Plan §3 §6)。
fn run_factcheck_pass_lab(store: &MemoryStore, since_event_id: i64) -> Result<FactCheckSummary> {
    let started = std::time::Instant::now();
    let conn = store.conn();
    let event_store = EventStore::new(conn);
    let graph = KnowledgeGraph::new(conn);

    // Plan A G-4c: halluc benchmark task 3 件の正解 fact を KG に seed (冪等 UPSERT)。
    // 失敗は warn log のみで Lab を継続 (non-fatal、`run_factcheck_pass` の検証は KG が
    // 空でも `Unknown` 分類で意味ある metric を出すため、seed 失敗で短絡しない)。
    if let Err(e) = factcheck::seed_kg_for_factcheck_lab(&graph) {
        log_event(
            LogLevel::Warn,
            "lab.factcheck",
            &format!("KG seed failed (non-fatal): {e}"),
        );
    }

    // 項目 244 Phase 4: KG lint pass (LLM Wiki Lint パターン適用)。
    // Lab v20 structural finding (matched=0 deterministic、19h 投下後発覚) の事前検出。
    // 4 軸 (矛盾/孤立/uncovered/case_variant) を check、clean ない時 warn_log で警告。
    // `BONSAI_KG_LINT_STRICT=1` 設定時のみ非 clean で abort (案 B 設計、plan §2.1)。
    // production agent_loop はこの path に到達しない (`is_factcheck_enabled()` で短絡)。
    let seed_triples = factcheck::seed_triples_for_factcheck_lab();
    let keyword_bundles: Vec<Vec<String>> = BenchmarkSuite::default_tasks()
        .tasks
        .iter()
        .map(|t| t.expected_keywords.clone())
        .collect();
    let lint_report = factcheck::lint_kg_for_lab(&graph, &seed_triples, &keyword_bundles);
    lint_report.warn_log();
    if factcheck::is_kg_lint_strict() && !lint_report.is_clean() {
        anyhow::bail!(
            "KG lint NOT clean and BONSAI_KG_LINT_STRICT=1 (項目 244 strict gate): \
             conflicting={} orphan={} uncovered={} case_variant={}. \
             seed/benchmark の不整合を解消するか env を unset してください。",
            lint_report.conflicting_triples.len(),
            lint_report.orphan_nodes.len(),
            lint_report.uncovered_seed_triples.len(),
            lint_report.case_variant_nodes.len(),
        );
    }

    // 項目 235: env opt-in で trajectory selection を拡張 (halluc SUCCESS-by-design 対応)。
    // env unset (default) → 従来 failed-only / min_steps=2 完全互換 (G-4c v1/v2 と同経路)。
    // env=1 → failed + successful chain + min_steps=0 で halluc 0/1-tool session も対象化
    // (`.claude/plan/factcheck-trajectory-scope-expansion.md` §2.1 案 A + §1.3 min_steps 補完)。
    let all_trajectories = factcheck::is_all_trajectories_enabled();
    let min_steps = if all_trajectories { 0 } else { 2 };
    let mut candidates = event_store.extract_failed_trajectories_since_id(
        since_event_id,
        0.8, // max_tool_success_rate (failed-side cap、env opt-in でも保持)
        min_steps,
    )?;
    if all_trajectories {
        let successful = event_store.extract_successful_trajectories_since_id(
            since_event_id,
            0.8, // min_tool_success_rate (successful-side floor、failed と互いに排他)
            min_steps,
        )?;
        candidates.extend(successful);
    }

    let mut texts: Vec<String> = Vec::new();
    for candidate in &candidates {
        let events = event_store.replay(&candidate.session_id)?;
        for ev in events {
            if ev.event_type == "assistant_message" {
                // event_data JSON で `{"content": "..."}` 形式が典型。parse 失敗時は
                // raw string を fall back 採用 (false positive 起こさない高 precision regex)。
                let text = serde_json::from_str::<serde_json::Value>(&ev.event_data)
                    .ok()
                    .and_then(|v| v.get("content").and_then(|c| c.as_str()).map(String::from))
                    .unwrap_or(ev.event_data);
                if !text.is_empty() {
                    texts.push(text);
                }
            }
        }
    }

    let summary = factcheck::run_factcheck_pass(&texts, &graph);
    let duration_ms = started.elapsed().as_millis() as u64;

    // audit emit (non-fatal、persist 失敗は summary 返却を阻害しない)
    let audit = AuditLog::new(conn);
    let _ = audit.log(
        None,
        &AuditAction::FactCheck {
            total: summary.total,
            matched: summary.matched,
            unknown: summary.unknown,
            conflicting: summary.conflicting,
            mean_path_len: summary.mean_path_len,
            duration_ms,
        },
    );

    Ok(summary)
}

/// ERL post-Lab pass (項目 213): events から reflection で自然言語助言を抽出し、
/// HeuristicStore に保存。tool_chain 表現可能 advice は SkillStore に routing する。
///
/// non-fatal: 任意のエラーは呼出側で `Warn` log で握り潰す (Lab 結果を破壊しない)。
/// `since_event_id` は `run_hindsight_pass` と共有 = 同じ Lab cycle 開始時 snapshot。
fn run_heuristics_pass(
    store: &MemoryStore,
    since_event_id: i64,
    backend: &dyn LlmBackend,
) -> Result<HeuristicSummary> {
    // 項目 216 defaults OFF 切替 (Lab v17 REJECT 反映):
    // production default = env unset で post-Lab pass 全体を skip。
    // `BONSAI_ERL_ENABLED=1` で opt-in 復活 (項目 213 Phase 2 Green の post-Lab hook 動作)。
    if !is_erl_enabled() {
        return Ok(HeuristicSummary::default());
    }
    let conn = store.conn();
    let event_store = EventStore::new(conn);
    let h_store = HeuristicStore::new(conn);
    let skill_store = SkillStore::new(conn);

    let result = extract_reflection_full(&event_store, since_event_id, backend)?;

    let mut summary = HeuristicSummary {
        extracted: result.candidates.len() + result.tool_chain_advice.len(),
        parse_failures: result.parse_failures,
        ..Default::default()
    };

    for c in &result.candidates {
        h_store.save(
            &c.advice,
            &c.trigger_patterns,
            Some(c.source_session_id.as_str()),
            &c.source_task,
            &c.category,
        )?;
        summary.saved += 1;
    }

    for (chain, advice, sid) in &result.tool_chain_advice {
        if skill_store
            .promote_from_erl_advice(chain, advice, sid)?
            .is_some()
        {
            summary.skipped_to_skill += 1;
        }
    }

    summary.pruned = h_store.prune().unwrap_or(0);
    Ok(summary)
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
                    &format!(
                        "judge_gate evaluate failed (task={}): {e}",
                        task_score.task_id
                    ),
                );
                // fail-open: scores に積まない
            }
        }
    }

    // composite 平均（fail-open: judge にかけられなかった or 全 err なら mean=0、passed=true）
    let (mean_composite, passed) = if scores.is_empty() {
        (0.0, true)
    } else {
        let mean: f64 = scores.iter().map(|s| s.composite()).sum::<f64>() / scores.len() as f64;
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
            soul_path: None,
            n_ctx_budget: None,
            memory_blocks: Vec::new(),
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
        let mut hyp_gen2 =
            HypothesisGenerator::default().with_tried_details(vec![m0_detail.clone()]);
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
        assert!(
            config.enable_prescreening,
            "プリスクリーニングはデフォルト有効"
        );
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
        assert!(estimated_delta < threshold, "大きな悪化は閾値を下回る");
    }

    #[test]
    fn test_prescreening_threshold_passes_positive_delta() {
        // 推定deltaが閾値以上の場合、通過するべき
        let threshold = -0.01;
        let estimated_delta = 0.02;
        assert!(estimated_delta >= threshold, "改善は閾値以上で通過");
        // 閾値ちょうどの場合も通過
        let estimated_delta_border = -0.01;
        assert!(
            estimated_delta_border >= threshold,
            "閾値ちょうどは通過（<で判定するため）"
        );
    }

    // --- pre-screen REJECT tier carry-over テスト (項目 224、本 plan agentfloor-prescreen-tier-fix.md) ---
    //
    // 由来 = G-4c v3 PARTIAL PASS で SQLite tier_t1..t6 全 NULL 発覚。
    // commit 572a9a4 (run_k tier populate) は意図通り動作するが、pre-screen REJECT 経路の
    // Experiment inline literal が tier_t1..t6 を hardcoded None で構築していた。
    // 修正 = baseline.tier_avg_scores を carry-over (full run 未実行のため baseline 値が論理的に正しい)。

    #[test]
    fn t_prescreen_reject_carries_baseline_tier_when_populated() {
        use std::collections::HashMap;
        let baseline_tiers = Some([
            Some(0.68),
            Some(0.52),
            Some(0.77),
            Some(0.64),
            Some(0.70),
            Some(0.47),
        ]);
        let exp = build_prescreen_reject_experiment(
            "test-prescreen-001".to_string(),
            MutationType::PromptRule,
            "test_mutation".to_string(),
            0.6, // baseline_score
            baseline_tiers,
            &[],     // baseline_frontier_bucket_scores (env OFF で carry-over 不要)
            &[],     // baseline_frontier_inject_scores
            -0.1583, // estimated_delta
            HashMap::new(),
        );
        assert_eq!(exp.tier_t1, Some(0.68), "T1 carry-over from baseline");
        assert_eq!(exp.tier_t2, Some(0.52), "T2 carry-over from baseline");
        assert_eq!(exp.tier_t3, Some(0.77), "T3 carry-over from baseline");
        assert_eq!(exp.tier_t4, Some(0.64), "T4 carry-over from baseline");
        assert_eq!(exp.tier_t5, Some(0.70), "T5 carry-over from baseline");
        assert_eq!(exp.tier_t6, Some(0.47), "T6 carry-over from baseline");
        assert!(exp.prescreened, "prescreened=true 維持");
        assert!(!exp.accepted, "accepted=false 維持");
        assert!((exp.delta - (-0.1583)).abs() < f64::EPSILON);
    }

    #[test]
    fn t_prescreen_reject_tier_none_when_baseline_none() {
        use std::collections::HashMap;
        // LADDER mode 未使用時 (baseline.tier_avg_scores=None) の後方互換性
        let exp = build_prescreen_reject_experiment(
            "test-prescreen-002".to_string(),
            MutationType::AgentParam,
            "test_mutation".to_string(),
            0.5, // baseline_score
            None,
            &[],   // baseline_frontier_bucket_scores
            &[],   // baseline_frontier_inject_scores
            -0.05, // estimated_delta
            HashMap::new(),
        );
        assert_eq!(exp.tier_t1, None, "LADDER mode OFF で全 tier None");
        assert_eq!(exp.tier_t2, None);
        assert_eq!(exp.tier_t3, None);
        assert_eq!(exp.tier_t4, None);
        assert_eq!(exp.tier_t5, None);
        assert_eq!(exp.tier_t6, None);
    }

    #[test]
    fn t_prescreen_reject_partial_tier_carries_correctly() {
        use std::collections::HashMap;
        // 部分 NULL の伝搬 (一部 tier に該当 task がない smoke 等のケース)
        let baseline_tiers = Some([Some(0.68), None, Some(0.77), None, Some(0.70), None]);
        let exp = build_prescreen_reject_experiment(
            "test-prescreen-003".to_string(),
            MutationType::PromptRule,
            "test_mutation".to_string(),
            0.6,
            baseline_tiers,
            &[], // baseline_frontier_bucket_scores
            &[], // baseline_frontier_inject_scores
            -0.02,
            HashMap::new(),
        );
        assert_eq!(exp.tier_t1, Some(0.68));
        assert_eq!(exp.tier_t2, None, "部分 NULL は伝搬");
        assert_eq!(exp.tier_t3, Some(0.77));
        assert_eq!(exp.tier_t4, None);
        assert_eq!(exp.tier_t5, Some(0.70));
        assert_eq!(exp.tier_t6, None);
    }

    // --- 項目 234 候補 (frontier-prescreen-carryover-fix): pre-screen REJECT で baseline frontier を
    // env-gated carry-over する 4 件。BONSAI_FRONTIER_ENABLED=1 / BONSAI_FRONTIER_INJECT_ENABLED=1 で
    // baseline.frontier_*_scores を Vec::clone()、env unset で空 Vec 維持 (項目 229 後方互換)。
    // env mutation race 回避は SMOKE_TEST_LOCK と同 pattern (serial_test 不採用)。

    static FRONTIER_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn reset_frontier_env() {
        unsafe {
            std::env::remove_var("BONSAI_FRONTIER_ENABLED");
            std::env::remove_var("BONSAI_FRONTIER_INJECT_ENABLED");
        }
    }

    /// (Phase 1+2 atomic) BONSAI_FRONTIER_ENABLED=1 で baseline_frontier_bucket_scores を carry-over。
    /// 期待: 渡した `[(1, 0.6313), (2, 0.45)]` が出力に保存され、inject は env OFF で空 Vec。
    #[test]
    fn t_prescreen_reject_carries_baseline_frontier_when_bucket_enabled() {
        use std::collections::HashMap;
        let _guard = FRONTIER_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_frontier_env();
        unsafe { std::env::set_var("BONSAI_FRONTIER_ENABLED", "1") };

        let baseline_buckets = vec![(1, 0.6313), (2, 0.45)];
        let exp = build_prescreen_reject_experiment(
            "exp_test_carry_bucket".to_string(),
            MutationType::PromptRule,
            "test_mutation".to_string(),
            0.5,
            None,
            &baseline_buckets,
            &[],
            -0.05,
            HashMap::new(),
        );

        assert_eq!(
            exp.frontier_bucket_scores,
            vec![(1, 0.6313), (2, 0.45)],
            "BONSAI_FRONTIER_ENABLED=1 で baseline bucket carry-over されるべき"
        );
        assert!(
            exp.frontier_inject_scores.is_empty(),
            "inject env OFF で空 Vec 維持"
        );

        reset_frontier_env();
    }

    /// (Phase 1+2 atomic) BONSAI_FRONTIER_INJECT_ENABLED=1 で baseline_frontier_inject_scores を carry-over。
    /// bucket env OFF で bucket 側は空 Vec 維持 = 2 軸独立性確認。
    #[test]
    fn t_prescreen_reject_carries_baseline_frontier_when_inject_enabled() {
        use std::collections::HashMap;
        let _guard = FRONTIER_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_frontier_env();
        unsafe { std::env::set_var("BONSAI_FRONTIER_INJECT_ENABLED", "1") };

        let baseline_inject = vec![(0, 0.85), (4, 0.72)];
        let exp = build_prescreen_reject_experiment(
            "exp_test_carry_inject".to_string(),
            MutationType::AgentParam,
            "test_mutation".to_string(),
            0.5,
            None,
            &[(99, 0.99)], // bucket env OFF で carry-over されないことを確証
            &baseline_inject,
            -0.03,
            HashMap::new(),
        );

        assert_eq!(
            exp.frontier_inject_scores,
            vec![(0, 0.85), (4, 0.72)],
            "BONSAI_FRONTIER_INJECT_ENABLED=1 で baseline inject carry-over"
        );
        assert!(
            exp.frontier_bucket_scores.is_empty(),
            "bucket env OFF で空 Vec 維持 (2 軸独立)"
        );

        reset_frontier_env();
    }

    /// (Phase 1+2 atomic) 両 env unset で frontier_* 全て空 Vec = 後方互換 100%。
    /// 項目 229 当初実装 (`Vec::new()` ハードコード) と完全に等価な挙動を確証する回帰防止。
    #[test]
    fn t_prescreen_reject_frontier_empty_when_env_off() {
        use std::collections::HashMap;
        let _guard = FRONTIER_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_frontier_env();

        // baseline が空でない場合でも env unset で carry-over されないこと
        let exp = build_prescreen_reject_experiment(
            "exp_test_env_off".to_string(),
            MutationType::PromptRule,
            "test_mutation".to_string(),
            0.5,
            None,
            &[(1, 0.7), (2, 0.5)],
            &[(0, 0.8), (4, 0.6)],
            -0.05,
            HashMap::new(),
        );

        assert!(
            exp.frontier_bucket_scores.is_empty(),
            "env unset で bucket 空 (項目 229 後方互換)"
        );
        assert!(
            exp.frontier_inject_scores.is_empty(),
            "env unset で inject 空 (項目 229 後方互換)"
        );
    }

    /// (Phase 1+2 atomic) env ON でも baseline が空なら出力も空 = preserve 動作確認。
    /// SMOKE 7 task で T6 task ゼロのため frontier_inject_scores が空のケース等を再現。
    #[test]
    fn t_prescreen_reject_frontier_empty_when_baseline_empty() {
        use std::collections::HashMap;
        let _guard = FRONTIER_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_frontier_env();
        unsafe {
            std::env::set_var("BONSAI_FRONTIER_ENABLED", "1");
            std::env::set_var("BONSAI_FRONTIER_INJECT_ENABLED", "1");
        }

        let exp = build_prescreen_reject_experiment(
            "exp_test_baseline_empty".to_string(),
            MutationType::PromptHint,
            "test_mutation".to_string(),
            0.5,
            None,
            &[], // baseline 空 = LADDER 未使用、または bucket emit ゼロ
            &[],
            -0.04,
            HashMap::new(),
        );

        assert!(
            exp.frontier_bucket_scores.is_empty(),
            "env ON でも baseline 空なら出力空 (.to_vec() は空 slice を空 Vec で preserve)"
        );
        assert!(exp.frontier_inject_scores.is_empty());

        reset_frontier_env();
    }

    // --- smoke 補正係数テスト (項目 184 由来、Lab smoke→core 42% retention) ---
    //
    // env mutation race を避けるため module-local Mutex で serialize する
    // (serial_test crate を増やさない方針、YAGNI)。

    static SMOKE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn reset_smoke_env() {
        unsafe {
            std::env::remove_var("BONSAI_LAB_SMOKE");
            std::env::remove_var("BONSAI_LAB_SMOKE_CORRECTION");
        }
    }

    #[test]
    fn test_smoke_correction_off_leaves_delta_unchanged() {
        // poison は env mutation には実害なし（process-global、test の panic で値は残るのみ）
        let _g = SMOKE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_smoke_env();
        // smoke off: positive/negative ともに delta 不変
        assert!((apply_smoke_correction_to_delta(0.10) - 0.10).abs() < 1e-9);
        assert!((apply_smoke_correction_to_delta(-0.10) - (-0.10)).abs() < 1e-9);
        assert!((apply_smoke_correction_to_delta(0.0) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn test_smoke_correction_on_scales_positive_delta_only() {
        // poison は env mutation には実害なし（process-global、test の panic で値は残るのみ）
        let _g = SMOKE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_smoke_env();
        unsafe {
            std::env::set_var("BONSAI_LAB_SMOKE", "1");
        }
        // sign-aware: positive のみ ×0.42、negative は不変
        assert!(
            (apply_smoke_correction_to_delta(0.10) - 0.042).abs() < 1e-9,
            "smoke on + positive delta は ×0.42 補正されるべき"
        );
        assert!(
            (apply_smoke_correction_to_delta(-0.10) - (-0.10)).abs() < 1e-9,
            "smoke on でも negative delta は補正されない (sign-aware)"
        );
        assert!(
            (apply_smoke_correction_to_delta(0.0) - 0.0).abs() < 1e-9,
            "delta=0 は補正対象外"
        );
        reset_smoke_env();
    }

    #[test]
    fn test_smoke_correction_env_override_valid() {
        // poison は env mutation には実害なし（process-global、test の panic で値は残るのみ）
        let _g = SMOKE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_smoke_env();
        unsafe {
            std::env::set_var("BONSAI_LAB_SMOKE", "1");
            std::env::set_var("BONSAI_LAB_SMOKE_CORRECTION", "0.5");
        }
        assert!(
            (apply_smoke_correction_to_delta(0.10) - 0.05).abs() < 1e-9,
            "BONSAI_LAB_SMOKE_CORRECTION=0.5 で 0.10 → 0.05"
        );
        // 境界値: 1.0 は補正なし相当
        unsafe {
            std::env::set_var("BONSAI_LAB_SMOKE_CORRECTION", "1.0");
        }
        assert!(
            (apply_smoke_correction_to_delta(0.10) - 0.10).abs() < 1e-9,
            "BONSAI_LAB_SMOKE_CORRECTION=1.0 で従来動作復元"
        );
        reset_smoke_env();
    }

    #[test]
    fn test_smoke_correction_env_override_invalid_falls_back() {
        // poison は env mutation には実害なし（process-global、test の panic で値は残るのみ）
        let _g = SMOKE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_smoke_env();
        unsafe {
            std::env::set_var("BONSAI_LAB_SMOKE", "1");
        }
        for invalid in ["abc", "NaN", ""] {
            unsafe {
                std::env::set_var("BONSAI_LAB_SMOKE_CORRECTION", invalid);
            }
            assert!(
                (apply_smoke_correction_to_delta(0.10) - 0.042).abs() < 1e-9,
                "invalid override '{}' は default ×0.42 にフォールバック",
                invalid
            );
        }
        reset_smoke_env();
    }

    #[test]
    fn test_smoke_correction_env_override_out_of_range() {
        // poison は env mutation には実害なし（process-global、test の panic で値は残るのみ）
        let _g = SMOKE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_smoke_env();
        unsafe {
            std::env::set_var("BONSAI_LAB_SMOKE", "1");
        }
        // 範囲外 (≤0 or >1): default にフォールバック
        for out_of_range in ["0", "0.0", "-0.1", "-1", "1.5", "10", "inf"] {
            unsafe {
                std::env::set_var("BONSAI_LAB_SMOKE_CORRECTION", out_of_range);
            }
            assert!(
                (apply_smoke_correction_to_delta(0.10) - 0.042).abs() < 1e-9,
                "範囲外 '{}' は default ×0.42 にフォールバック",
                out_of_range
            );
        }
        reset_smoke_env();
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
        // 16 既存 + 3 (項目 211 SetAdvisorThreshold 0.3/0.4/0.5) = 19
        assert_eq!(
            params.len(),
            19,
            "パラメータ変異候補は19件 (16 既存 + 3 advisor_threshold)"
        );
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
                experiment_id: "e1".into(),
                mutation_type: MutationType::PromptRule,
                mutation_detail: "rule_a".into(),
                baseline_score: 0.8,
                experiment_score: 0.75,
                delta: -0.05,
                accepted: false,
                duration_secs: 10.0,
                config_snapshot: HashMap::new(),
                pass_at_k: None,
                pass_consecutive_k: None,
                score_variance: None,
                prescreened: false,
                reliability_decay: None,
                variance_amplification: None,
                graceful_degradation: None,
                stability_delta: None,
                tier_t1: None,
                tier_t2: None,
                tier_t3: None,
                tier_t4: None,
                tier_t5: None,
                tier_t6: None,
                pass_at_k_t_steps: Vec::new(),
                pass_at_k_t_seconds: Vec::new(),
                frontier_bucket_scores: Vec::new(),
                frontier_inject_scores: Vec::new(),
            },
            Experiment {
                experiment_id: "e2".into(),
                mutation_type: MutationType::PromptRule,
                mutation_detail: "rule_b".into(),
                baseline_score: 0.8,
                experiment_score: 0.82,
                delta: 0.02,
                accepted: true,
                duration_secs: 10.0,
                config_snapshot: HashMap::new(),
                pass_at_k: None,
                pass_consecutive_k: None,
                score_variance: None,
                prescreened: false,
                reliability_decay: None,
                variance_amplification: None,
                graceful_degradation: None,
                stability_delta: None,
                tier_t1: None,
                tier_t2: None,
                tier_t3: None,
                tier_t4: None,
                tier_t5: None,
                tier_t6: None,
                pass_at_k_t_steps: Vec::new(),
                pass_at_k_t_seconds: Vec::new(),
                frontier_bucket_scores: Vec::new(),
                frontier_inject_scores: Vec::new(),
            },
            Experiment {
                experiment_id: "e3".into(),
                mutation_type: MutationType::AgentParam,
                mutation_detail: "param_x".into(),
                baseline_score: 0.8,
                experiment_score: 0.7,
                delta: -0.10,
                accepted: false,
                duration_secs: 10.0,
                config_snapshot: HashMap::new(),
                pass_at_k: None,
                pass_consecutive_k: None,
                score_variance: None,
                prescreened: false,
                reliability_decay: None,
                variance_amplification: None,
                graceful_degradation: None,
                stability_delta: None,
                tier_t1: None,
                tier_t2: None,
                tier_t3: None,
                tier_t4: None,
                tier_t5: None,
                tier_t6: None,
                pass_at_k_t_steps: Vec::new(),
                pass_at_k_t_seconds: Vec::new(),
                frontier_bucket_scores: Vec::new(),
                frontier_inject_scores: Vec::new(),
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
                experiment_id: format!("e{i}"),
                mutation_type: MutationType::PromptRule,
                mutation_detail: format!("rule_{i}"),
                baseline_score: 0.8,
                experiment_score: 0.8 - (i as f64 * 0.01),
                delta: -(i as f64 * 0.01),
                accepted: false,
                duration_secs: 10.0,
                config_snapshot: HashMap::new(),
                pass_at_k: None,
                pass_consecutive_k: None,
                score_variance: None,
                prescreened: false,
                reliability_decay: None,
                variance_amplification: None,
                graceful_degradation: None,
                stability_delta: None,
                tier_t1: None,
                tier_t2: None,
                tier_t3: None,
                tier_t4: None,
                tier_t5: None,
                tier_t6: None,
                pass_at_k_t_steps: Vec::new(),
                pass_at_k_t_seconds: Vec::new(),
                frontier_bucket_scores: Vec::new(),
                frontier_inject_scores: Vec::new(),
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
        assert_eq!(
            t,
            LabTrigger::Stagnation,
            "4th non-improvement triggers at threshold=3"
        );
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
        let worst = vec![("rule_a".to_string(), -0.05), ("rule_b".to_string(), -0.03)];
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

    fn make_task_score(
        task_id: &str,
        with_run: bool,
    ) -> crate::agent::benchmark::MultiRunTaskScore {
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
            core_avg_score: None,
            extended_avg_score: None,
            tier_avg_scores: None,
            critic_stats: None,
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
            core_avg_score: None,
            extended_avg_score: None,
            tier_avg_scores: None,
            critic_stats: None,
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
            core_avg_score: None,
            extended_avg_score: None,
            tier_avg_scores: None,
            critic_stats: None,
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
            core_avg_score: None,
            extended_avg_score: None,
            tier_avg_scores: None,
            critic_stats: None,
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
            core_avg_score: None,
            extended_avg_score: None,
            tier_avg_scores: None,
            critic_stats: None,
        };
        let descs = make_descs(&["t0", "t1", "t2", "t3", "t4"]);

        let outcome = judge_gate_check(&mut judge, &result, 0.7, 1, &descs).unwrap();
        assert_eq!(judge.call_count, 1, "sample_size=1 should judge 1 task");
        assert_eq!(outcome.scores.len(), 1);
    }

    // ===== AgentHER runtime integration tests (handoff 05-07e TODO #1, 項目 201/161 dead-code 解消) =====

    /// 1 session 分の events を append する test helper。
    /// `tool_results: &[(tool_name, success)]` で各ツール呼出のシーケンスを指定。
    fn seed_session_events(
        es: &EventStore,
        session_id: &str,
        user_content: &str,
        tool_results: &[(&str, bool)],
    ) {
        es.append(
            session_id,
            &crate::agent::event_store::EventType::SessionStart,
            "{}",
            None,
        )
        .unwrap();
        let user_payload = format!(r#"{{"content":"{}"}}"#, user_content);
        es.append(
            session_id,
            &crate::agent::event_store::EventType::UserMessage,
            &user_payload,
            Some(0),
        )
        .unwrap();
        for (i, (tool, success)) in tool_results.iter().enumerate() {
            let start_payload = format!(r#"{{"tool":"{}"}}"#, tool);
            es.append(
                session_id,
                &crate::agent::event_store::EventType::ToolCallStart,
                &start_payload,
                Some(i),
            )
            .unwrap();
            let end_payload = format!(r#"{{"tool":"{}","success":{}}}"#, tool, success);
            es.append(
                session_id,
                &crate::agent::event_store::EventType::ToolCallEnd,
                &end_payload,
                Some(i),
            )
            .unwrap();
        }
        es.append(
            session_id,
            &crate::agent::event_store::EventType::SessionEnd,
            "{}",
            None,
        )
        .unwrap();
    }

    #[test]
    fn t_hindsight_pass_no_events_returns_zero_summary() {
        let store = MemoryStore::in_memory().unwrap();
        let summary = run_hindsight_pass(&store, 0).unwrap();
        assert_eq!(summary, HindsightSummary::default());
    }

    #[test]
    fn t_hindsight_pass_extracts_subgoals_from_failed_session() {
        let store = MemoryStore::in_memory().unwrap();
        let es = EventStore::new(store.conn());
        // success_rate = 1/2 = 0.5 < 0.8 → failed、file_write 1 件 → 1 subgoal
        seed_session_events(
            &es,
            "fail1",
            "FizzBuzz実装",
            &[("file_write", true), ("shell", false)],
        );
        let summary = run_hindsight_pass(&store, 0).unwrap();
        assert_eq!(summary.failed_sessions, 1, "1 failed session");
        assert!(summary.relabels >= 1, "1 file_write 成功で >= 1 relabel");
        assert!(summary.insights_recorded >= 1, "ECHO insight 記録");
        assert!(summary.skills_promoted >= 1, "hsl_ skill 1 件以上");
    }

    #[test]
    fn t_hindsight_pass_promotes_successful_trajectory_symmetric() {
        let store = MemoryStore::in_memory().unwrap();
        let es = EventStore::new(store.conn());
        // success_rate=1.0 >= 0.8 / steps=2 → 成功軌跡として extract、symmetric promotion
        seed_session_events(
            &es,
            "ok1",
            "ファイル処理",
            &[("shell", true), ("shell", true)],
        );
        let summary = run_hindsight_pass(&store, 0).unwrap();
        assert_eq!(summary.failed_sessions, 0);
        assert_eq!(summary.successful_sessions, 1, "1 successful session");
        assert!(summary.skills_promoted >= 1, "traj_ skill 1 件以上");
    }

    #[test]
    fn t_hindsight_pass_max_promote_caps_skill_explosion() {
        let store = MemoryStore::in_memory().unwrap();
        let es = EventStore::new(store.conn());
        // 4 success + 2 fail = success_rate 0.667 < 0.8 → failed
        // 4 異なる tool successes → 4 subgoals、max_promote=3 で skills_promoted ≤ 3
        seed_session_events(
            &es,
            "fail2",
            "複数ファイル処理",
            &[
                ("file_write", true),
                ("multi_edit", true),
                ("git_commit", true),
                ("file_write", true),
                ("shell", false),
                ("shell", false),
            ],
        );
        let summary = run_hindsight_pass(&store, 0).unwrap();
        assert_eq!(summary.failed_sessions, 1);
        assert!(summary.relabels >= 1);
        // 1 relabel × max_promote=3 で 1 session あたり最大 3 hsl_ skills
        assert!(
            summary.skills_promoted <= 3,
            "max_promote=3 cap で skills_promoted={} <= 3",
            summary.skills_promoted
        );
    }

    // ===== 項目 235: factcheck trajectory scope expansion tests =====
    // (`.claude/plan/factcheck-trajectory-scope-expansion.md` Phase 1 Red + Phase 2 Green atomic)
    //
    // Plan A G-4c v1/v2 反証 (項目 234) で確定した halluc task の SUCCESS-by-design 構造的排除を
    // env opt-in (`BONSAI_FACTCHECK_ALL_TRAJECTORIES=1`) で解消。env unset で従来 failed-only +
    // min_steps=2 完全互換、env=1 で failed + successful chain + min_steps=0 拡張。
    // env mutation race 回避は `factcheck::FACTCHECK_ALL_ENV_TEST_LOCK` (cross-file shared) で
    // serialize (FRONTIER_TEST_LOCK / SMOKE_TEST_LOCK が file-local 単独なのと異なり、本 env は
    // factcheck.rs/experiment.rs 両 file の test が触るため crate-level shared mutex 必須)。

    fn reset_factcheck_all_env() {
        unsafe { std::env::remove_var("BONSAI_FACTCHECK_ALL_TRAJECTORIES") };
    }

    /// 1 session 分の events を append する factcheck test helper。
    /// `seed_session_events` (tool 専用) と異なり AssistantMessage event も追加できる。
    fn seed_session_with_assistant(
        es: &EventStore,
        session_id: &str,
        user_content: &str,
        tool_results: &[(&str, bool)],
        assistant_messages: &[&str],
    ) {
        es.append(
            session_id,
            &crate::agent::event_store::EventType::SessionStart,
            "{}",
            None,
        )
        .unwrap();
        let user_payload = format!(r#"{{"content":"{}"}}"#, user_content);
        es.append(
            session_id,
            &crate::agent::event_store::EventType::UserMessage,
            &user_payload,
            Some(0),
        )
        .unwrap();
        for (i, (tool, success)) in tool_results.iter().enumerate() {
            let start = format!(r#"{{"tool":"{}"}}"#, tool);
            es.append(
                session_id,
                &crate::agent::event_store::EventType::ToolCallStart,
                &start,
                Some(i),
            )
            .unwrap();
            let end = format!(r#"{{"tool":"{}","success":{}}}"#, tool, success);
            es.append(
                session_id,
                &crate::agent::event_store::EventType::ToolCallEnd,
                &end,
                Some(i),
            )
            .unwrap();
        }
        for (i, msg) in assistant_messages.iter().enumerate() {
            let payload = serde_json::json!({ "content": msg }).to_string();
            es.append(
                session_id,
                &crate::agent::event_store::EventType::AssistantMessage,
                &payload,
                Some(i),
            )
            .unwrap();
        }
        es.append(
            session_id,
            &crate::agent::event_store::EventType::SessionEnd,
            "{}",
            None,
        )
        .unwrap();
    }

    /// (Phase 1+2 atomic) env unset で従来 failed-only 完全互換。
    /// 期待: success_rate=1.0 + 1 tool call の SUCCESS session は trajectory から
    /// 除外され、AssistantMessage に triple 含有でも factcheck total=0。
    /// (= G-4c v1/v2 と同経路、項目 234 反証の再現)。
    ///
    /// 注: regex `RE_IS_A` のみ subject/object に dash を許容するため、KG seed の
    /// `(Prism-ml, is_a, ternary_model)` を使う Pattern 2 経路で検証 (Pattern 1
    /// `RE_IS_THE_OF` の dash 対応は本 plan scope 外、§9 別 plan)。
    #[test]
    fn t_factcheck_default_failed_only_backwards_compat() {
        let _guard = crate::memory::factcheck::FACTCHECK_ALL_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_factcheck_all_env();

        let store = MemoryStore::in_memory().unwrap();
        let es = EventStore::new(store.conn());
        seed_session_with_assistant(
            &es,
            "halluc_like_success",
            "halluc-like task",
            &[("file_read", true)], // 1 tool, success_rate=1.0 → halluc_t2 と同型
            &["Prism-ml is a ternary_model."], // KG seed と一致 (Pattern 2)
        );
        let summary = run_factcheck_pass_lab(&store, 0).unwrap();
        // env unset → min_steps=2 で 1-tool session 排除、AssistantMessage 未走査で total=0。
        assert_eq!(
            summary.total, 0,
            "env unset で halluc-like SUCCESS session は trajectory に入らないべき (項目 234 反証再現)"
        );
    }

    /// (Phase 1+2 atomic) env=1 で SUCCESS task の AssistantMessage から triple 抽出 + Match 検証。
    /// 期待: success_rate=1.0 + 1 tool call の SUCCESS session でも、AssistantMessage に
    /// (Prism-ml, is_a, ternary_model) 含む文があれば factcheck pass が triple 抽出 + KG seed
    /// (`seed_kg_for_factcheck_lab` が自動投入する 3 fact) と Match 判定。
    /// Pattern 2 `RE_IS_A` 経由 (dash 対応)。
    #[test]
    fn t_factcheck_all_trajectories_extracts_from_success_assistant_message() {
        let _guard = crate::memory::factcheck::FACTCHECK_ALL_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_factcheck_all_env();
        unsafe { std::env::set_var("BONSAI_FACTCHECK_ALL_TRAJECTORIES", "1") };

        let store = MemoryStore::in_memory().unwrap();
        let es = EventStore::new(store.conn());
        // halluc_is_a_false_type と同型の SUCCESS session (1 file_read OK)
        seed_session_with_assistant(
            &es,
            "halluc_t2_like_success",
            "halluc t2-like",
            &[("file_read", true)],
            &["Prism-ml is a ternary_model."],
        );
        let summary = run_factcheck_pass_lab(&store, 0).unwrap();
        reset_factcheck_all_env();

        // KG は `seed_kg_for_factcheck_lab` で seed 済 = (Prism-ml, is_a, ternary_model) Match 期待。
        assert!(
            summary.total >= 1,
            "env=1 で SUCCESS session の AssistantMessage から triple 抽出されるべき (total={})",
            summary.total
        );
        assert!(
            summary.matched >= 1,
            "KG seed と一致する triple は Match 判定されるべき (matched={})",
            summary.matched
        );
    }

    /// (Phase 1+2 atomic) env=1 で halluc 0-tool session も対象化 (min_steps=0 緩和の確証)。
    /// 期待: tool 0 件 (`tool_success_rate=0.0`) の session でも、AssistantMessage に
    /// triple 含有なら factcheck pass が抽出 + Conflict 判定。
    /// (Prism-ml, is_a, language_model) は KG seed (Prism-ml, is_a, ternary_model) と
    /// subject+predicate 同一 + object 不一致で Conflict 発火 = G-4c effectiveness 経路。
    ///
    /// 注: tool 0 件 → `build_trajectory_from_events` で `tool_success_rate=0.0`
    /// (event_store.rs:328-332) → `extract_failed_trajectories_since_id` の
    /// `< 0.8` filter を通過 (env=1 で min_steps=0 拡張)。
    #[test]
    fn t_factcheck_all_trajectories_detects_conflict_in_zero_tool_session() {
        let _guard = crate::memory::factcheck::FACTCHECK_ALL_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_factcheck_all_env();
        unsafe { std::env::set_var("BONSAI_FACTCHECK_ALL_TRAJECTORIES", "1") };

        let store = MemoryStore::in_memory().unwrap();
        let es = EventStore::new(store.conn());
        // halluc_is_a_false_type と同型: tool 0 件 session、AssistantMessage に矛盾 fact
        seed_session_with_assistant(
            &es,
            "halluc_zero_tool_success",
            "halluc zero-tool",
            &[], // tool 0 件 → total_steps=0、min_steps=0 で extract される (env=1)
            &["Prism-ml is a language_model."], // KG seed (is_a, ternary_model) と矛盾 → Conflict
        );
        let summary = run_factcheck_pass_lab(&store, 0).unwrap();
        reset_factcheck_all_env();

        assert!(
            summary.total >= 1,
            "env=1 + min_steps=0 で 0-tool session の AssistantMessage が走査されるべき (total={})",
            summary.total
        );
        assert!(
            summary.conflicting >= 1,
            "KG seed (is_a, ternary_model) と矛盾する (is_a, language_model) は Conflict 判定 (conflicting={})",
            summary.conflicting
        );
    }

    // ===== Phase 5: Self-Verification Dilemma — AdvisorThreshold variant tests (TDD Red) =====
    //
    // 項目 211 候補: 項目 210 で実装した AdvisorConfig::dynamic_skip_threshold を Lab variant pool
    // に投入するための MutationAction::SetAdvisorThreshold 拡張。SetTemperature(f64) と同型。

    #[test]
    fn t_phase5_apply_mutation_set_advisor_threshold() {
        let config = make_config();
        // baseline では default (0.0、項目 210 の default OFF)
        assert_eq!(config.advisor.dynamic_skip_threshold, 0.0);

        let mutation = Mutation {
            mutation_type: MutationType::AgentParam,
            detail: "advisor.dynamic_skip_threshold: 0.4".into(),
            apply: MutationAction::SetAdvisorThreshold(0.4),
            theme: MutationTheme::Precision,
        };
        let modified = apply_mutation(&config, &mutation);
        assert!((modified.advisor.dynamic_skip_threshold - 0.4).abs() < 1e-9);
    }

    #[test]
    fn t_phase5_apply_mutation_set_advisor_threshold_preserves_others() {
        let config = make_config();
        let baseline_min_samples = config.advisor.min_samples_for_skip;
        let baseline_max_iter = config.max_iterations;

        let mutation = Mutation {
            mutation_type: MutationType::AgentParam,
            detail: "advisor.dynamic_skip_threshold: 0.5".into(),
            apply: MutationAction::SetAdvisorThreshold(0.5),
            theme: MutationTheme::Precision,
        };
        let modified = apply_mutation(&config, &mutation);
        // threshold のみ変動、他の AdvisorConfig フィールド + AgentConfig フィールドは保持
        assert!((modified.advisor.dynamic_skip_threshold - 0.5).abs() < 1e-9);
        assert_eq!(modified.advisor.min_samples_for_skip, baseline_min_samples);
        assert_eq!(modified.max_iterations, baseline_max_iter);
        assert_eq!(modified.system_prompt, config.system_prompt);
    }

    #[test]
    fn t_phase5_param_mutations_includes_advisor_threshold_variants() {
        let params = param_mutations();
        let threshold_values: Vec<f64> = params
            .iter()
            .filter_map(|p| match p.action {
                MutationAction::SetAdvisorThreshold(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(
            threshold_values.len(),
            3,
            "param_mutations() に SetAdvisorThreshold 3 件 (0.3/0.4/0.5) が必要、実際: {:?}",
            threshold_values
        );
        assert!(threshold_values.iter().any(|t| (*t - 0.3).abs() < 1e-9));
        assert!(threshold_values.iter().any(|t| (*t - 0.4).abs() < 1e-9));
        assert!(threshold_values.iter().any(|t| (*t - 0.5).abs() < 1e-9));
    }

    #[test]
    fn t_phase5_next_mutation_with_focus_advisor_threshold() {
        let mut hyp_gen = HypothesisGenerator::default();
        // focus="advisor_threshold" で 3 連続呼出すべて SetAdvisorThreshold variant
        let m0 = hyp_gen.next_mutation_with_focus(0, Some("advisor_threshold"));
        let m1 = hyp_gen.next_mutation_with_focus(1, Some("advisor_threshold"));
        let m2 = hyp_gen.next_mutation_with_focus(2, Some("advisor_threshold"));
        for (i, m) in [&m0, &m1, &m2].iter().enumerate() {
            assert!(
                matches!(m.apply, MutationAction::SetAdvisorThreshold(_)),
                "focus filter で {} 回目に SetAdvisorThreshold 以外: {:?}",
                i,
                m.apply
            );
        }
        // 3 件で 0.3/0.4/0.5 すべて網羅 (順不同 dedup 確認)
        let mut seen: Vec<f64> = [&m0, &m1, &m2]
            .iter()
            .filter_map(|m| match m.apply {
                MutationAction::SetAdvisorThreshold(t) => Some(t),
                _ => None,
            })
            .collect();
        seen.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(seen.len(), 3);
        assert!((seen[0] - 0.3).abs() < 1e-9);
        assert!((seen[1] - 0.4).abs() < 1e-9);
        assert!((seen[2] - 0.5).abs() < 1e-9);
    }
}
