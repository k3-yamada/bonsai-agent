use anyhow::Result;
use std::collections::HashMap;


use crate::agent::agent_loop::AgentConfig;
use crate::agent::benchmark::BenchmarkSuite;
use crate::agent::experiment_log::{Experiment, ExperimentLog, MutationType};
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
    /// max_tools_selectedを変更（AgentConfigにはないが将来用）
    SetMaxRetries(usize),
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
    /// 次の変異候補を生成（ラウンドロビン）
    pub fn next_mutation(&mut self, experiment_count: usize) -> Mutation {
        // 3種のmutationをローテーション:
        // 0,1,2: プロンプトルール追加
        // 3: max_iterations増加
        // 4: max_iterations減少
        let cycle = experiment_count % 5;

        match cycle {
            3 => Mutation {
                mutation_type: MutationType::AgentParam,
                detail: "max_iterations: +2".into(),
                apply: MutationAction::SetMaxIterations(12),
            },
            4 => Mutation {
                mutation_type: MutationType::AgentParam,
                detail: "max_iterations: -2".into(),
                apply: MutationAction::SetMaxIterations(8),
            },
            _ => {
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
            description: format!("insight: {}", &insight[..insight.len().min(50)]),
        });
    }
}

/// デフォルトのプロンプトルール候補
fn default_prompt_rules() -> Vec<PromptRuleCandidate> {
    vec![
        PromptRuleCandidate {
            rule: "9. ツール呼び出しの前に必ず <think> タグで考える".into(),
            description: "ツール使用前に思考を強制".into(),
        },
        PromptRuleCandidate {
            rule: "9. 複数ステップが必要な場合、まず計画を <think> に書いてから実行する".into(),
            description: "マルチステップ計画の強制".into(),
        },
        PromptRuleCandidate {
            rule: "9. エラーが発生したら原因を <think> で分析してから次の行動を決める".into(),
            description: "エラー分析の強制".into(),
        },
        PromptRuleCandidate {
            rule: "9. ツール結果が期待と違う場合、別のツールを試す".into(),
            description: "フォールバック戦略".into(),
        },
        PromptRuleCandidate {
            rule: "9. ファイル操作の前にパスの存在を確認する".into(),
            description: "ファイル存在確認の強制".into(),
        },
    ]
}

/// 変異をAgentConfigに適用
pub fn apply_mutation(base_config: &AgentConfig, mutation: &Mutation) -> AgentConfig {
    let mut config = AgentConfig {
        max_iterations: base_config.max_iterations,
        max_retries: base_config.max_retries,
        system_prompt: base_config.system_prompt.clone(),
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
    }

    config
}

/// 設定のスナップショットをHashMapに変換（実験ログ用）
pub fn config_snapshot(config: &AgentConfig) -> HashMap<String, String> {
    HashMap::from([
        ("max_iterations".into(), config.max_iterations.to_string()),
        ("max_retries".into(), config.max_retries.to_string()),
        (
            "system_prompt_len".into(),
            config.system_prompt.len().to_string(),
        ),
    ])
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

    // 1. ベースライン計測
    eprintln!("[lab] ベースライン計測中...");
    let mut baseline = suite.run(base_config, backend, tools, path_guard, cancel)?;
    eprintln!(
        "[lab] ベースライン: {:.4} ({:.1}s)",
        baseline.composite_score(),
        baseline.duration_secs
    );

    // 2. 実験ループ
    let mut experiment_count = 0;
    loop {
        if cancel.is_cancelled() {
            eprintln!("[lab] キャンセルされました");
            break;
        }

        if let Some(max) = loop_config.max_experiments
            && experiment_count >= max {
                eprintln!("[lab] 最大実験回数({max})に到達");
                break;
        }

        // a. 仮説生成
        let mutation = generator.next_mutation(experiment_count);
        let experiment_id = format!("exp_{:04}", experiment_count);
        eprintln!(
            "[lab] 実験 {experiment_id}: {} — {}",
            mutation.mutation_type.as_str(),
            mutation.detail
        );

        // b. 変異適用
        let modified_config = apply_mutation(base_config, &mutation);

        // c. ベンチマーク実行
        let result = suite.run(&modified_config, backend, tools, path_guard, cancel)?;
        let snapshot = config_snapshot(&modified_config);

        // d. 評価
        let exp = Experiment::from_results(
            experiment_id,
            mutation.mutation_type,
            mutation.detail,
            &baseline,
            &result,
            snapshot,
        );

        eprintln!(
            "[lab]   score: {:.4} (delta: {:+.4}) → {}",
            exp.experiment_score,
            exp.delta,
            if exp.accepted { "ACCEPT" } else { "REJECT" }
        );

        // e. accept/reject
        if exp.accepted {
            baseline = result;
        }

        // f. ログ記録
        ExperimentLog::save_to_db(store.conn(), &exp)?;
        if let Some(tsv) = &loop_config.tsv_path { ExperimentLog::append_tsv(tsv, &exp)?; }

        experiments.push(exp);
        experiment_count += 1;

        // g. Dreamer統合（N実験ごと）
        if experiment_count % loop_config.dreamer_interval == 0
            && let Ok(report) = crate::memory::dreams::Dreamer::new(store.conn()).generate_report(7)
        {
            for insight in &report.insights {
                generator.add_insight_mutation(insight);
                eprintln!("[lab] Dreamer insight追加: {insight}");
            }
        }
    }

    let total = experiments.len();
    let accepted = experiments.iter().filter(|e| e.accepted).count();
    eprintln!("[lab] 完了: {total}実験, {accepted}承認 ({:.0}%)", if total > 0 { accepted as f64 / total as f64 * 100.0 } else { 0.0 });

    Ok(experiments)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> AgentConfig {
        AgentConfig {
            max_iterations: 10,
            max_retries: 3,
            system_prompt: "test prompt\n1. ルール1\n2. ルール2".into(),
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
}
