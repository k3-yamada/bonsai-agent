use crate::observability::logger::{log_event, LogLevel};
use anyhow::Result;
use std::collections::HashMap;

use crate::agent::agent_loop::AgentConfig;
use crate::agent::benchmark::{BenchmarkSuite, MultiRunConfig};
use crate::agent::experiment_log::{
    AcceptedMutation, Experiment, ExperimentLog, MutationType, load_accepted_archive,
};
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
}

/// 仮説生成器: ルールベースで次の変異候補を選択
pub struct HypothesisGenerator {
    rules: Vec<PromptRuleCandidate>,
    current_index: usize,
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
        }
    }
}

impl HypothesisGenerator {
    /// 次の変異候補を生成（10サイクルローテーション）
    pub fn next_mutation(&mut self, experiment_count: usize) -> Mutation {
        // 10種のmutationをローテーション:
        // 0,1,2: プロンプトルール追加
        // 3,4: max_iterations変更
        // 5,6: max_tools_selected変更
        // 7,8: max_retries変更
        // 9: プロンプトルール追加（追加枠）
        let cycle = experiment_count % 10;

        match cycle {
            3 => Mutation {
                mutation_type: MutationType::AgentParam,
                detail: "max_iterations: 12 (+2)".into(),
                apply: MutationAction::SetMaxIterations(12),
            },
            4 => Mutation {
                mutation_type: MutationType::AgentParam,
                detail: "max_iterations: 8 (-2)".into(),
                apply: MutationAction::SetMaxIterations(8),
            },
            5 => Mutation {
                mutation_type: MutationType::AgentParam,
                detail: "max_tools_selected: 3 (-2)".into(),
                apply: MutationAction::SetMaxToolsSelected(3),
            },
            6 => Mutation {
                mutation_type: MutationType::AgentParam,
                detail: "max_tools_selected: 7 (+2)".into(),
                apply: MutationAction::SetMaxToolsSelected(7),
            },
            7 => Mutation {
                mutation_type: MutationType::AgentParam,
                detail: "max_retries: 1 (-2)".into(),
                apply: MutationAction::SetMaxRetries(1),
            },
            8 => Mutation {
                mutation_type: MutationType::AgentParam,
                detail: "max_retries: 5 (+2)".into(),
                apply: MutationAction::SetMaxRetries(5),
            },
            _ => {
                // サイクル 0,1,2,9 はプロンプトルール
                let rule = &self.rules[self.current_index % self.rules.len()];
                let mutation = Mutation {
                    mutation_type: MutationType::PromptRule,
                    detail: rule.description.clone(),
                    apply: MutationAction::AddPromptRule(rule.rule.clone()),
                };
                self.current_index += 1;
                mutation
            }
        }
    }

    /// Dreamer insightから変異候補を追加
    pub fn add_insight_mutation(&mut self, insight: &str) {
        self.rules.push(PromptRuleCandidate {
            rule: insight.to_string(),
            description: format!("insight: {}", insight.chars().take(50).collect::<String>()),
        });
    }
}

/// デフォルトのプロンプトルール候補
fn default_prompt_rules() -> Vec<PromptRuleCandidate> {
    vec![
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
        })
    }

    /// delta加重の変異優先度スコアを計算（変異選択の優先順位付けに使用）
    pub fn priority_score(&self, mutation_detail: &str) -> f64 {
        self.archive
            .iter()
            .filter(|m| {
                mutation_detail.contains(&m.detail) || m.detail.contains(mutation_detail)
            })
            .map(|m| m.delta)
            .sum::<f64>()
    }
}

/// 少数タスクで変異の効果を事前推定する（フルベンチマークの代替）
/// タスク数の半分（最大4タスク）で1回実行し、delta推定値を返す
pub fn estimate_mutation_effect(
    base_config: &AgentConfig,
    mutation: &Mutation,
    backend: &dyn LlmBackend,
    tools: &ToolRegistry,
    path_guard: &PathGuard,
    cancel: &CancellationToken,
) -> Result<f64> {
    let suite = BenchmarkSuite::default_tasks();
    // 推定用: タスク半分（最大4件）のみで1回実行
    let sample_size = (suite.tasks.len() / 2).min(4);
    let sample_tasks: Vec<_> = suite.tasks.into_iter().take(sample_size).collect();
    let sample_suite = BenchmarkSuite { tasks: sample_tasks };

    let quick_multi = MultiRunConfig {
        k: 1,
        jitter_seed: false,
    };
    let pass_threshold = 0.5;

    // ベースライン（サンプルタスクのみ）
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
        "meta_mutation",
        &format!(
            "effect estimate: {} -> delta={:+.4} ({} tasks, k=1)",
            mutation.detail, delta, sample_size,
        ),
    );
    Ok(delta)
}

/// 実験ループ設定
pub struct ExperimentLoopConfig {
    /// TSVログのパス
    pub tsv_path: Option<std::path::PathBuf>,
    /// 最大実験回数（Noneなら無限）
    pub max_experiments: Option<usize>,
    /// Dreamerレポート間隔（N実験ごと）
    pub dreamer_interval: usize,
}

impl Default for ExperimentLoopConfig {
    fn default() -> Self {
        Self {
            tsv_path: None,
            max_experiments: None,
            dreamer_interval: 10,
        }
    }
}

/// 実験ループ本体
pub fn run_experiment_loop(
    base_config: &AgentConfig,
    backend: &dyn LlmBackend,
    tools: &ToolRegistry,
    path_guard: &PathGuard,
    cancel: &CancellationToken,
    store: &MemoryStore,
    loop_config: &ExperimentLoopConfig,
) -> Result<Vec<Experiment>> {
    let suite = BenchmarkSuite::default_tasks();
    let mut generator = HypothesisGenerator::default();
    let mut experiments: Vec<Experiment> = Vec::new();
    let multi = MultiRunConfig { k: 3, jitter_seed: true };
    let pass_threshold = 0.5;

    // 1. ベースライン計測（pass^k版）
    log_event(LogLevel::Info, "lab", &format!("ベースライン計測中（k={}）...", multi.k));
    let mut baseline = suite.run_k(base_config, backend, tools, path_guard, cancel, &multi, pass_threshold)?;
    eprintln!(
        "[lab] ベースライン: score={:.4} pass@k={:.4} pass_consec={:.4} ({:.1}s)",
        baseline.composite_score(),
        baseline.composite_pass_at_k(),
        baseline.composite_pass_consecutive_k(),
        baseline.duration_secs
    );

    // メタ変異生成器の初期化（過去のACCEPTアーカイブから）
    let mut meta_gen = MetaMutationGenerator::from_db(store.conn()).unwrap_or_else(|_| {
        MetaMutationGenerator::from_archive(Vec::new())
    });
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

        // c. ベンチマーク実行（pass^k版）
        let result = suite.run_k(&modified_config, backend, tools, path_guard, cancel, &multi, pass_threshold)?;
        let snapshot = config_snapshot(&modified_config);

        // d. 評価（pass^k指標を含む）
        let exp = Experiment::from_multi_results(
            experiment_id,
            mutation.mutation_type,
            mutation.detail,
            &baseline,
            &result,
            snapshot,
        );

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
            meta_gen =
                MetaMutationGenerator::from_db(store.conn()).unwrap_or(meta_gen);
        }

        // f. ログ記録
        ExperimentLog::save_to_db(store.conn(), &exp)?;
        if let Some(tsv) = &loop_config.tsv_path {
            ExperimentLog::append_tsv(tsv, &exp)?;
        }

        experiments.push(exp);
        experiment_count += 1;

        // g. Dreamer統合（N実験ごと）
        if experiment_count % loop_config.dreamer_interval == 0
            && let Ok(report) = crate::memory::evolution::EvolutionEngine::new(store).analyze_deep(7)
        {
            for insight in &report.insights {
                generator.add_insight_mutation(insight);
                log_event(LogLevel::Info, "lab", &format!("Dreamer insight追加: {insight}"));
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
        }
    }

    #[test]
    fn test_apply_mutation_add_rule() {
        let config = make_config();
        let mutation = Mutation {
            mutation_type: MutationType::PromptRule,
            detail: "テスト".into(),
            apply: MutationAction::AddPromptRule("3. 新ルール".into()),
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
        };
        let modified = apply_mutation(&config, &mutation);
        assert_eq!(modified.max_retries, 5);
    }

    #[test]
    fn test_hypothesis_generator_rotation() {
        let mut hyp_gen = HypothesisGenerator::default();
        let m0 = hyp_gen.next_mutation(0);
        assert_eq!(m0.mutation_type, MutationType::PromptRule);
        let m3 = hyp_gen.next_mutation(3);
        assert_eq!(m3.mutation_type, MutationType::AgentParam);
        assert!(m3.detail.contains("+2"));
        let m4 = hyp_gen.next_mutation(4);
        assert_eq!(m4.mutation_type, MutationType::AgentParam);
        assert!(m4.detail.contains("-2"));
    }

    #[test]
    fn test_hypothesis_generator_cycles_rules() {
        let mut hyp_gen = HypothesisGenerator::default();
        let n_rules = hyp_gen.rules.len();
        // n_rules + 1回呼ぶとラップアラウンド
        for i in 0..=n_rules {
            let _ = hyp_gen.next_mutation(i % 3); // PromptRuleのみのcycle
        }
        // current_indexがn_rules+1になっている
        assert_eq!(hyp_gen.current_index, n_rules + 1);
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
        let mg = MetaMutationGenerator::from_archive(vec![make_accepted_mutation(
            "rule_a", 0.05,
        )]);
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
            apply: MutationAction::CompoundPromptRules(vec![
                "rule_x".into(),
                "rule_y".into(),
            ]),
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
            compound.detail.contains("insight rule")
                || compound.detail.contains("normal rule")
        );
    }

}
