use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::agent::agent_loop::{AgentConfig, AgentLoopResult, run_agent_loop};
use crate::agent::validate::PathGuard;
use crate::cancel::CancellationToken;
use crate::memory::store::MemoryStore;
use crate::runtime::inference::LlmBackend;
use crate::tools::ToolRegistry;

/// ベンチマークタスクのカテゴリ
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskCategory {
    ToolUse,
    Reasoning,
    MultiStep,
    ErrorRecovery,
    ToolSelection,
}

/// 単一のベンチマークタスク定義
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkTask {
    pub id: String,
    pub name: String,
    pub input: String,
    pub expected_tools: Vec<String>,
    pub expected_keywords: Vec<String>,
    pub max_iterations: usize,
    pub category: TaskCategory,
}

/// タスク実行結果のスコア
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskScore {
    pub task_id: String,
    pub completed: bool,
    pub correct_tools: f64,
    pub keyword_hits: f64,
    pub iterations_used: usize,
    pub iteration_budget: usize,
}

impl TaskScore {
    /// タスクスコアを計算（0.0-1.0）
    /// 40% 完了, 30% ツール正確性, 20% キーワード一致, 10% イテレーション効率
    pub fn score(&self) -> f64 {
        let completed_score = if self.completed { 1.0 } else { 0.0 };
        let efficiency = if self.iteration_budget > 0 {
            1.0 - (self.iterations_used as f64 / self.iteration_budget as f64)
        } else {
            0.0
        };
        let efficiency_clamped = efficiency.max(0.0);

        0.4 * completed_score
            + 0.3 * self.correct_tools
            + 0.2 * self.keyword_hits
            + 0.1 * efficiency_clamped
    }
}

/// ベンチマーク全体の結果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResult {
    pub task_scores: Vec<TaskScore>,
    pub duration_secs: f64,
}

impl BenchmarkResult {
    /// 全タスクの平均スコア（0.0-1.0）
    pub fn composite_score(&self) -> f64 {
        if self.task_scores.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.task_scores.iter().map(|s| s.score()).sum();
        sum / self.task_scores.len() as f64
    }
}

/// 複数回実行の設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiRunConfig {
    /// 各タスクの実行回数（default 3）
    pub k: usize,
    /// 実行間のシステムプロンプト微小変動用シード（決定論的出力を回避）
    pub jitter_seed: bool,
}

impl Default for MultiRunConfig {
    fn default() -> Self {
        Self {
            k: 3,
            jitter_seed: true,
        }
    }
}

/// 複数回実行時の単一タスクスコア
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiRunTaskScore {
    pub task_id: String,
    /// k回中の成功割合（score > pass_threshold のラン数 / k）
    pub pass_at_k: f64,
    /// 最長連続成功 / k（p^n 指標）
    pub pass_consecutive_k: f64,
    /// 平均スコア
    pub mean_score: f64,
    /// スコアの分散
    pub variance: f64,
    /// 個別スコア
    pub individual_scores: Vec<f64>,
}

impl MultiRunTaskScore {
    /// 個別スコア列からMultiRunTaskScoreを計算
    pub fn from_scores(task_id: String, scores: Vec<f64>, pass_threshold: f64) -> Self {
        let k = scores.len();
        if k == 0 {
            return Self {
                task_id,
                pass_at_k: 0.0,
                pass_consecutive_k: 0.0,
                mean_score: 0.0,
                variance: 0.0,
                individual_scores: vec![],
            };
        }

        let passes: Vec<bool> = scores.iter().map(|s| *s >= pass_threshold).collect();
        let pass_at_k = passes.iter().filter(|p| **p).count() as f64 / k as f64;

        // 最長連続成功
        let mut max_streak = 0usize;
        let mut current_streak = 0usize;
        for &passed in &passes {
            if passed {
                current_streak += 1;
                max_streak = max_streak.max(current_streak);
            } else {
                current_streak = 0;
            }
        }
        let pass_consecutive_k = max_streak as f64 / k as f64;

        let mean_score = scores.iter().sum::<f64>() / k as f64;
        let variance = if k > 1 {
            scores
                .iter()
                .map(|s| (s - mean_score).powi(2))
                .sum::<f64>()
                / (k - 1) as f64
        } else {
            0.0
        };

        Self {
            task_id,
            pass_at_k,
            pass_consecutive_k,
            mean_score,
            variance,
            individual_scores: scores,
        }
    }
}

/// 複数回実行のベンチマーク全体結果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiRunBenchmarkResult {
    pub task_scores: Vec<MultiRunTaskScore>,
    pub duration_secs: f64,
}

impl MultiRunBenchmarkResult {
    /// 全タスクの平均pass_at_k
    pub fn composite_pass_at_k(&self) -> f64 {
        if self.task_scores.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.task_scores.iter().map(|s| s.pass_at_k).sum();
        sum / self.task_scores.len() as f64
    }

    /// 全タスクの平均pass_consecutive_k
    pub fn composite_pass_consecutive_k(&self) -> f64 {
        if self.task_scores.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.task_scores.iter().map(|s| s.pass_consecutive_k).sum();
        sum / self.task_scores.len() as f64
    }

    /// 全タスクの平均mean_score（既存composite_scoreと互換）
    pub fn composite_score(&self) -> f64 {
        if self.task_scores.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.task_scores.iter().map(|s| s.mean_score).sum();
        sum / self.task_scores.len() as f64
    }

    /// 全タスクの平均variance
    pub fn mean_variance(&self) -> f64 {
        if self.task_scores.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.task_scores.iter().map(|s| s.variance).sum();
        sum / self.task_scores.len() as f64
    }
}

/// ベンチマークスイート
pub struct BenchmarkSuite {
    pub tasks: Vec<BenchmarkTask>,
}

impl BenchmarkSuite {
    /// デフォルトのベンチマークタスクセット（8タスク）
    pub fn default_tasks() -> Self {
        Self {
            tasks: vec![
                BenchmarkTask {
                    id: "file_read_simple".into(),
                    name: "ファイル読み取り".into(),
                    input: "README.mdの内容を教えて".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec!["README".into()],
                    max_iterations: 3,
                    category: TaskCategory::ToolUse,
                },
                BenchmarkTask {
                    id: "shell_ls".into(),
                    name: "ファイル一覧".into(),
                    input: "このディレクトリのファイル一覧を表示して".into(),
                    expected_tools: vec!["shell".into()],
                    expected_keywords: vec!["src".into()],
                    max_iterations: 3,
                    category: TaskCategory::ToolUse,
                },
                BenchmarkTask {
                    id: "git_status".into(),
                    name: "Git状態確認".into(),
                    input: "Gitの状態を確認して".into(),
                    expected_tools: vec!["git".into()],
                    expected_keywords: vec!["branch".into()],
                    max_iterations: 3,
                    category: TaskCategory::ToolUse,
                },
                BenchmarkTask {
                    id: "multi_step_write_read".into(),
                    name: "書き込み→読み返し".into(),
                    input: "hello.txtに'Hello World'と書いて、それを読み返して".into(),
                    expected_tools: vec!["file_write".into(), "file_read".into()],
                    expected_keywords: vec!["Hello World".into()],
                    max_iterations: 5,
                    category: TaskCategory::MultiStep,
                },
                BenchmarkTask {
                    id: "reasoning_calc".into(),
                    name: "計算推論".into(),
                    input: "2の10乗はいくつですか".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["1024".into()],
                    max_iterations: 3,
                    category: TaskCategory::Reasoning,
                },
                BenchmarkTask {
                    id: "error_recovery".into(),
                    name: "エラー回復".into(),
                    input: "存在しないファイル /tmp/bonsai_nonexistent_test.txt を読んで".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec!["存在".into()],
                    max_iterations: 3,
                    category: TaskCategory::ErrorRecovery,
                },
                BenchmarkTask {
                    id: "tool_selection_git".into(),
                    name: "ツール選択".into(),
                    input: "このプロジェクトのGitログを見せて".into(),
                    expected_tools: vec!["git".into()],
                    expected_keywords: vec!["commit".into()],
                    max_iterations: 3,
                    category: TaskCategory::ToolSelection,
                },
                BenchmarkTask {
                    id: "direct_answer".into(),
                    name: "直接回答".into(),
                    input: "Rustのマスコットの名前は？".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["Ferris".into()],
                    max_iterations: 2,
                    category: TaskCategory::Reasoning,
                },
            ],
        }
    }

    /// 各タスクをk回実行してpass^k指標を計算
    #[allow(clippy::too_many_arguments)]
    pub fn run_k(
        &self,
        config: &AgentConfig,
        backend: &dyn LlmBackend,
        tools: &ToolRegistry,
        path_guard: &PathGuard,
        cancel: &CancellationToken,
        multi: &MultiRunConfig,
        pass_threshold: f64,
    ) -> Result<MultiRunBenchmarkResult> {
        let start = std::time::Instant::now();
        let mut task_scores = Vec::new();

        for task in &self.tasks {
            if cancel.is_cancelled() {
                break;
            }

            let mut scores = Vec::new();
            for run_idx in 0..multi.k {
                if cancel.is_cancelled() {
                    break;
                }

                // jitter_seed時はプロンプトに実行番号を付加してキャッシュ回避
                let system_prompt = if multi.jitter_seed {
                    format!("{}\n<!-- run:{run_idx} -->", config.system_prompt)
                } else {
                    config.system_prompt.clone()
                };

                let task_config = AgentConfig {
                    max_iterations: task.max_iterations,
                    max_retries: config.max_retries,
                    max_tools_selected: config.max_tools_selected,
                    system_prompt,
                    advisor: config.advisor.clone(),
                    auto_checkpoint: false, // ベンチマークではCP不要
                };

                let store = MemoryStore::in_memory()?;
                let result = run_agent_loop(
                    &task.input,
                    backend,
                    tools,
                    path_guard,
                    &task_config,
                    cancel,
                    Some(&store),
                );

                let score = match result {
                    Ok(ref loop_result) => evaluate_task_response(task, loop_result).score(),
                    Err(_) => 0.0,
                };
                scores.push(score);
            }

            task_scores.push(MultiRunTaskScore::from_scores(
                task.id.clone(),
                scores,
                pass_threshold,
            ));
        }

        Ok(MultiRunBenchmarkResult {
            task_scores,
            duration_secs: start.elapsed().as_secs_f64(),
        })
    }

    /// ベンチマークスイートを実行し結果を返す
    pub fn run(
        &self,
        config: &AgentConfig,
        backend: &dyn LlmBackend,
        tools: &ToolRegistry,
        path_guard: &PathGuard,
        cancel: &CancellationToken,
    ) -> Result<BenchmarkResult> {
        let start = std::time::Instant::now();
        let mut task_scores = Vec::new();

        for task in &self.tasks {
            if cancel.is_cancelled() {
                break;
            }

            let task_config = AgentConfig {
                max_iterations: task.max_iterations,
                max_retries: config.max_retries,
                max_tools_selected: config.max_tools_selected,
                system_prompt: config.system_prompt.clone(),
                advisor: config.advisor.clone(),
                auto_checkpoint: false,
            };

            let store = MemoryStore::in_memory()?;
            let result = run_agent_loop(
                &task.input,
                backend,
                tools,
                path_guard,
                &task_config,
                cancel,
                Some(&store),
            );

            let score = match result {
                Ok(ref loop_result) => evaluate_task_response(task, loop_result),
                Err(_) => TaskScore {
                    task_id: task.id.clone(),
                    completed: false,
                    correct_tools: 0.0,
                    keyword_hits: 0.0,
                    iterations_used: task.max_iterations,
                    iteration_budget: task.max_iterations,
                },
            };

            task_scores.push(score);
        }

        Ok(BenchmarkResult {
            task_scores,
            duration_secs: start.elapsed().as_secs_f64(),
        })
    }
}

/// タスクのレスポンスを評価してスコアを生成
fn evaluate_task_response(task: &BenchmarkTask, result: &AgentLoopResult) -> TaskScore {
    let response = &result.answer;
    let keyword_hits = if task.expected_keywords.is_empty() {
        1.0
    } else {
        let hits = task
            .expected_keywords
            .iter()
            .filter(|kw| response.contains(kw.as_str()))
            .count();
        hits as f64 / task.expected_keywords.len() as f64
    };

    // ツール正確性: 実際に呼ばれたツールと期待ツールを照合
    let correct_tools = if task.expected_tools.is_empty() {
        1.0 // ツール不要タスクではツール正確性は常に満点
    } else {
        let matched = task
            .expected_tools
            .iter()
            .filter(|t| result.tools_called.iter().any(|c| c == *t))
            .count();
        matched as f64 / task.expected_tools.len() as f64
    };

    TaskScore {
        task_id: task.id.clone(),
        completed: !result.answer.starts_with("[中断]"),
        correct_tools,
        keyword_hits,
        iterations_used: result.iterations_used,
        iteration_budget: task.max_iterations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_score_perfect() {
        let score = TaskScore {
            task_id: "test".into(),
            completed: true,
            correct_tools: 1.0,
            keyword_hits: 1.0,
            iterations_used: 0,
            iteration_budget: 3,
        };
        assert!((score.score() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_task_score_zero() {
        let score = TaskScore {
            task_id: "test".into(),
            completed: false,
            correct_tools: 0.0,
            keyword_hits: 0.0,
            iterations_used: 3,
            iteration_budget: 3,
        };
        assert!((score.score()).abs() < f64::EPSILON);
    }

    #[test]
    fn test_task_score_partial() {
        let score = TaskScore {
            task_id: "test".into(),
            completed: true,
            correct_tools: 0.5,
            keyword_hits: 0.5,
            iterations_used: 1,
            iteration_budget: 3,
        };
        let expected = 0.4 + 0.15 + 0.1 + 0.1 * (2.0 / 3.0);
        assert!((score.score() - expected).abs() < 0.001);
    }

    #[test]
    fn test_task_score_over_budget() {
        let score = TaskScore {
            task_id: "test".into(),
            completed: true,
            correct_tools: 1.0,
            keyword_hits: 1.0,
            iterations_used: 5,
            iteration_budget: 3,
        };
        assert!((score.score() - 0.9).abs() < 0.001);
    }

    #[test]
    fn test_task_score_zero_budget() {
        let score = TaskScore {
            task_id: "test".into(),
            completed: true,
            correct_tools: 1.0,
            keyword_hits: 1.0,
            iterations_used: 0,
            iteration_budget: 0,
        };
        assert!((score.score() - 0.9).abs() < 0.001);
    }

    #[test]
    fn test_benchmark_result_composite() {
        let result = BenchmarkResult {
            task_scores: vec![
                TaskScore {
                    task_id: "a".into(),
                    completed: true,
                    correct_tools: 1.0,
                    keyword_hits: 1.0,
                    iterations_used: 0,
                    iteration_budget: 3,
                },
                TaskScore {
                    task_id: "b".into(),
                    completed: false,
                    correct_tools: 0.0,
                    keyword_hits: 0.0,
                    iterations_used: 3,
                    iteration_budget: 3,
                },
            ],
            duration_secs: 10.0,
        };
        assert!((result.composite_score() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_benchmark_result_empty() {
        let result = BenchmarkResult {
            task_scores: vec![],
            duration_secs: 0.0,
        };
        assert!((result.composite_score()).abs() < f64::EPSILON);
    }

    #[test]
    fn test_default_tasks_count() {
        let suite = BenchmarkSuite::default_tasks();
        assert_eq!(suite.tasks.len(), 8);
    }

    #[test]
    fn test_default_tasks_unique_ids() {
        let suite = BenchmarkSuite::default_tasks();
        let mut ids: Vec<&str> = suite.tasks.iter().map(|t| t.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), suite.tasks.len());
    }

    fn mock_result(answer: &str, tools: Vec<&str>, iterations: usize) -> AgentLoopResult {
        AgentLoopResult {
            answer: answer.to_string(),
            iterations_used: iterations,
            tools_called: tools.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn test_evaluate_task_response_keywords() {
        let task = BenchmarkTask {
            id: "test".into(),
            name: "test".into(),
            input: "test".into(),
            expected_tools: vec![],
            expected_keywords: vec!["hello".into(), "world".into()],
            max_iterations: 3,
            category: TaskCategory::Reasoning,
        };
        let result = mock_result("hello there", vec![], 1);
        let score = evaluate_task_response(&task, &result);
        assert!((score.keyword_hits - 0.5).abs() < f64::EPSILON);
        assert!((score.correct_tools - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_evaluate_task_response_all_keywords() {
        let task = BenchmarkTask {
            id: "test".into(),
            name: "test".into(),
            input: "test".into(),
            expected_tools: vec![],
            expected_keywords: vec!["1024".into()],
            max_iterations: 3,
            category: TaskCategory::Reasoning,
        };
        let result = mock_result("2の10乗は1024です", vec![], 1);
        let score = evaluate_task_response(&task, &result);
        assert!((score.keyword_hits - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_evaluate_task_response_with_tools() {
        let task = BenchmarkTask {
            id: "test".into(),
            name: "test".into(),
            input: "test".into(),
            expected_tools: vec!["file_read".into()],
            expected_keywords: vec!["README".into()],
            max_iterations: 3,
            category: TaskCategory::ToolUse,
        };
        let result = mock_result("READMEの内容は...", vec!["file_read"], 2);
        let score = evaluate_task_response(&task, &result);
        assert!((score.correct_tools - 1.0).abs() < f64::EPSILON);
        assert_eq!(score.iterations_used, 2);
    }

    #[test]
    fn test_evaluate_task_response_missing_keywords() {
        let task = BenchmarkTask {
            id: "test".into(),
            name: "test".into(),
            input: "test".into(),
            expected_tools: vec!["git".into()],
            expected_keywords: vec!["commit".into()],
            max_iterations: 3,
            category: TaskCategory::ToolUse,
        };
        let result = mock_result("エラーが発生しました", vec![], 1);
        let score = evaluate_task_response(&task, &result);
        assert!((score.correct_tools).abs() < f64::EPSILON);
    }

    #[test]
    fn test_evaluate_different_iterations_different_scores() {
        let task = BenchmarkTask {
            id: "test".into(),
            name: "test".into(),
            input: "test".into(),
            expected_tools: vec![],
            expected_keywords: vec!["ok".into()],
            max_iterations: 5,
            category: TaskCategory::Reasoning,
        };
        let fast = mock_result("ok", vec![], 1);
        let slow = mock_result("ok", vec![], 4);
        let score_fast = evaluate_task_response(&task, &fast);
        let score_slow = evaluate_task_response(&task, &slow);
        assert!(
            score_fast.score() > score_slow.score(),
            "少ないイテレーションの方が高スコア"
        );
    }

    #[test]
    fn test_evaluate_actual_tools_matched() {
        let task = BenchmarkTask {
            id: "test".into(),
            name: "test".into(),
            input: "test".into(),
            expected_tools: vec!["shell".into(), "file_read".into()],
            expected_keywords: vec!["ok".into()],
            max_iterations: 3,
            category: TaskCategory::MultiStep,
        };
        let partial = mock_result("ok", vec!["shell"], 1);
        let full = mock_result("ok", vec!["shell", "file_read"], 1);
        let score_partial = evaluate_task_response(&task, &partial);
        let score_full = evaluate_task_response(&task, &full);
        assert!((score_partial.correct_tools - 0.5).abs() < f64::EPSILON);
        assert!((score_full.correct_tools - 1.0).abs() < f64::EPSILON);
    }

    // --- pass^k テスト ---

    #[test]
    fn test_multi_run_task_score_all_pass() {
        let scores = vec![0.9, 0.85, 0.95];
        let mrt = MultiRunTaskScore::from_scores("t1".into(), scores, 0.5);
        assert!((mrt.pass_at_k - 1.0).abs() < f64::EPSILON);
        assert!((mrt.pass_consecutive_k - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_multi_run_task_score_some_pass() {
        let scores = vec![0.9, 0.3, 0.8];
        let mrt = MultiRunTaskScore::from_scores("t2".into(), scores, 0.5);
        assert!((mrt.pass_at_k - 2.0 / 3.0).abs() < 0.001);
        assert!((mrt.pass_consecutive_k - 1.0 / 3.0).abs() < 0.001);
    }

    #[test]
    fn test_pass_consecutive_k_streak() {
        let scores = vec![0.9, 0.9, 0.9, 0.1, 0.9];
        let mrt = MultiRunTaskScore::from_scores("t3".into(), scores, 0.5);
        assert!((mrt.pass_consecutive_k - 3.0 / 5.0).abs() < 0.001);
    }

    #[test]
    fn test_pass_consecutive_k_interleaved() {
        let scores = vec![0.9, 0.1, 0.9, 0.1];
        let mrt = MultiRunTaskScore::from_scores("t4".into(), scores, 0.5);
        assert!((mrt.pass_consecutive_k - 1.0 / 4.0).abs() < 0.001);
    }

    #[test]
    fn test_multi_run_variance_calculation() {
        let scores = vec![0.8, 0.8, 0.8];
        let mrt = MultiRunTaskScore::from_scores("t5".into(), scores, 0.5);
        assert!(mrt.variance.abs() < f64::EPSILON);
    }

    #[test]
    fn test_multi_run_variance_nonzero() {
        let scores = vec![0.0, 1.0];
        let mrt = MultiRunTaskScore::from_scores("t6".into(), scores, 0.5);
        assert!((mrt.variance - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_multi_run_benchmark_result_composite() {
        let result = MultiRunBenchmarkResult {
            task_scores: vec![
                MultiRunTaskScore::from_scores("a".into(), vec![1.0, 1.0, 1.0], 0.5),
                MultiRunTaskScore::from_scores("b".into(), vec![0.0, 0.0, 0.0], 0.5),
            ],
            duration_secs: 10.0,
        };
        assert!((result.composite_pass_at_k() - 0.5).abs() < 0.001);
        assert!((result.composite_score() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_multi_run_empty_scores() {
        let mrt = MultiRunTaskScore::from_scores("empty".into(), vec![], 0.5);
        assert!((mrt.pass_at_k).abs() < f64::EPSILON);
        assert!((mrt.mean_score).abs() < f64::EPSILON);
    }
}
