#![allow(clippy::too_many_arguments)]
use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::agent::agent_loop::{AgentConfig, AgentLoopResult, run_agent_loop};
use crate::agent::validate::PathGuard;
use crate::cancel::CancellationToken;
use crate::memory::store::MemoryStore;
use crate::runtime::inference::LlmBackend;
use crate::runtime::model_router::AdvisorConfig;
use crate::tools::ToolRegistry;

/// ベンチマークタスクのカテゴリ
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskCategory {
    ToolUse,
    Reasoning,
    MultiStep,
    ErrorRecovery,
    ToolSelection,
    CodeGeneration,
    Summarization,
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
            scores.iter().map(|s| (s - mean_score).powi(2)).sum::<f64>() / (k - 1) as f64
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
    /// デフォルトのベンチマークタスクセット（22タスク）
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
                BenchmarkTask {
                    id: "code_gen_fizzbuzz".into(),
                    name: "コード生成".into(),
                    input: "FizzBuzzをRustで書いて。1から15までの出力例も示して".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["fizz".into(), "buzz".into(), "fizzbuzz".into()],
                    max_iterations: 3,
                    category: TaskCategory::CodeGeneration,
                },
                BenchmarkTask {
                    id: "multi_step_field_count".into(),
                    name: "マルチステップ推論".into(),
                    input: "src/config.rsのModelConfig構造体のフィールド数を数えて".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec![],
                    max_iterations: 5,
                    category: TaskCategory::MultiStep,
                },
                BenchmarkTask {
                    id: "error_handling_nonexistent".into(),
                    name: "エラーハンドリング".into(),
                    input: "/tmp/bonsai_absolutely_missing_file_xyz.rs を読んで内容を教えて".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec![],
                    max_iterations: 3,
                    category: TaskCategory::ErrorRecovery,
                },
                BenchmarkTask {
                    id: "summarize_agent_loop".into(),
                    name: "要約".into(),
                    input: "src/agent/agent_loop.rsの最初の50行を要約して".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec![],
                    max_iterations: 4,
                    category: TaskCategory::Summarization,
                },
                BenchmarkTask {
                    id: "repo_structure".into(),
                    name: "リポジトリ構造把握".into(),
                    input: "このプロジェクトのsrc/ディレクトリにあるRustファイルの数を教えて".into(),
                    expected_tools: vec!["repo_map".into()],
                    expected_keywords: vec![],
                    max_iterations: 3,
                    category: TaskCategory::ToolUse,
                },
                BenchmarkTask {
                    id: "multi_file_compare".into(),
                    name: "複数ファイル比較".into(),
                    input: "src/tools/file.rsとsrc/tools/shell.rsの行数をそれぞれ教えて".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec![],
                    max_iterations: 5,
                    category: TaskCategory::MultiStep,
                },
                BenchmarkTask {
                    id: "conditional_file_op".into(),
                    name: "条件付きファイル操作".into(),
                    input: "/tmp/bonsai_bench_test.txt が存在するか確認し、存在しなければ'benchmark ok'と書き込んで".into(),
                    expected_tools: vec!["file_read".into(), "file_write".into()],
                    expected_keywords: vec!["benchmark".into()],
                    max_iterations: 5,
                    category: TaskCategory::MultiStep,
                },
                BenchmarkTask {
                    id: "code_review".into(),
                    name: "コードレビュー".into(),
                    input: "src/tools/arxiv.rsのコードを読んで、改善点があれば指摘して".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec![],
                    max_iterations: 4,
                    category: TaskCategory::Summarization,
                },
                // --- 追加タスク（変異評価の多様性向上） ---
                BenchmarkTask {
                    id: "multi_step_rename".into(),
                    name: "変数リネーム".into(),
                    input: "/tmp/bonsai_rename_test.rsの変数名oldをnewにリネームして。まずファイルを読んでから書き換えて".into(),
                    expected_tools: vec!["file_read".into(), "file_write".into()],
                    expected_keywords: vec!["rename".into(), "replaced".into(), "updated".into()],
                    max_iterations: 6,
                    category: TaskCategory::MultiStep,
                },
                BenchmarkTask {
                    id: "git_diff_analysis".into(),
                    name: "Git差分分析".into(),
                    input: "最後のコミットで何が変更されたか確認して".into(),
                    expected_tools: vec!["shell".into()],
                    expected_keywords: vec!["diff".into(), "commit".into(), "changed".into()],
                    max_iterations: 4,
                    category: TaskCategory::ToolUse,
                },
                BenchmarkTask {
                    id: "error_recovery_permission".into(),
                    name: "権限エラー回復".into(),
                    input: "/etc/bonsai_readonly_testに'test'と書き込んで".into(),
                    expected_tools: vec!["file_write".into()],
                    expected_keywords: vec!["permission".into(), "denied".into(), "error".into(), "cannot".into()],
                    max_iterations: 4,
                    category: TaskCategory::ErrorRecovery,
                },
                BenchmarkTask {
                    id: "reasoning_json_parse".into(),
                    name: "JSON解析推論".into(),
                    input: r#"次のJSONから"name"フィールドの値を教えて: {"id": 1, "name": "bonsai", "version": "0.1"}"#.into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["bonsai".into()],
                    max_iterations: 3,
                    category: TaskCategory::Reasoning,
                },
                BenchmarkTask {
                    id: "code_gen_sort".into(),
                    name: "ソート関数生成".into(),
                    input: "Rustでバブルソート関数を書いて。Vec<i32>を受け取ってソートするfnを定義して".into(),
                    expected_tools: vec!["file_write".into()],
                    expected_keywords: vec!["sort".into(), "fn".into(), "vec".into()],
                    max_iterations: 5,
                    category: TaskCategory::CodeGeneration,
                },
                BenchmarkTask {
                    id: "multi_file_search".into(),
                    name: "複数ファイル検索".into(),
                    input: "src/配下で'run_agent_loop'関数が定義されているファイルを特定して".into(),
                    expected_tools: vec!["shell".into(), "file_read".into()],
                    expected_keywords: vec!["found".into(), "file".into()],
                    max_iterations: 5,
                    category: TaskCategory::MultiStep,
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
                    advisor: AdvisorConfig {
                        max_uses: 0,
                        ..config.advisor.clone()
                    },
                    auto_checkpoint: false,
                    max_tool_output_chars: config.max_tool_output_chars,
                    max_tools_in_context: config.max_tools_in_context,
                    max_mcp_tools_in_context: config.max_mcp_tools_in_context,
                    base_inference: config.base_inference.clone(),
                    task_timeout: config.task_timeout,
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
                advisor: AdvisorConfig {
                    max_uses: 0,
                    ..config.advisor.clone()
                },
                auto_checkpoint: false,
                max_tool_output_chars: config.max_tool_output_chars,
                max_tools_in_context: config.max_tools_in_context,
                max_mcp_tools_in_context: config.max_mcp_tools_in_context,
                base_inference: config.base_inference.clone(),
                task_timeout: config.task_timeout,
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
    // タスク固有の検証ロジック
    let keyword_hits = match task.id.as_str() {
        // マルチステップ推論: 回答に数値が含まれていれば成功
        "multi_step_field_count" => {
            let has_number = response.chars().any(|c| c.is_ascii_digit());
            if has_number { 1.0 } else { 0.0 }
        }
        // エラーハンドリング: エラー関連キーワードの検証（共通ロジック）
        "error_handling_nonexistent" | "error_recovery_permission" => {
            let error_keywords = [
                "エラー",
                "存在しない",
                "見つかり",
                "not found",
                "error",
                "Error",
                "失敗",
                "permission",
                "denied",
                "cannot",
                "権限",
                "拒否",
            ];
            let has_error_report = error_keywords.iter().any(|kw| response.contains(kw));
            if has_error_report { 1.0 } else { 0.0 }
        }
        // 要約: 50文字以上の回答で成功
        "summarize_agent_loop" => {
            // 回答の文字数で検証（空白除外）
            let char_count = response.chars().filter(|c| !c.is_whitespace()).count();
            if char_count >= 50 {
                1.0
            } else {
                char_count as f64 / 50.0
            }
        }
        // デフォルト: キーワードマッチ
        _ => {
            if task.expected_keywords.is_empty() {
                1.0
            } else {
                let hits = task
                    .expected_keywords
                    .iter()
                    .filter(|kw| {
                        let lower_response = response.to_lowercase();
                        let lower_kw = kw.to_lowercase();
                        lower_response.contains(&lower_kw)
                    })
                    .count();
                hits as f64 / task.expected_keywords.len() as f64
            }
        }
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
        assert_eq!(suite.tasks.len(), 22);
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

    // --- 新タスク（コード生成/マルチステップ/エラー処理/要約）テスト ---

    #[test]
    fn test_new_tasks_exist_in_suite() {
        let suite = BenchmarkSuite::default_tasks();
        let new_ids = [
            "code_gen_fizzbuzz",
            "multi_step_field_count",
            "error_handling_nonexistent",
            "summarize_agent_loop",
        ];
        for id in &new_ids {
            assert!(
                suite.tasks.iter().any(|t| t.id == *id),
                "タスク '{id}' がスイートに存在しない"
            );
        }
    }

    #[test]
    fn test_code_gen_fizzbuzz_keywords() {
        let task = BenchmarkTask {
            id: "code_gen_fizzbuzz".into(),
            name: "コード生成".into(),
            input: "test".into(),
            expected_tools: vec![],
            expected_keywords: vec!["fizz".into(), "buzz".into(), "fizzbuzz".into()],
            max_iterations: 3,
            category: TaskCategory::CodeGeneration,
        };
        // FizzBuzz出力を含む回答 → 全キーワードヒット
        let result = mock_result(
            "fn fizzbuzz(n: u32) { for i in 1..=n { match (i%3, i%5) { (0,0) => fizzbuzz, (0,_) => fizz, (_,0) => buzz, _ => num } } }",
            vec![],
            1,
        );
        let score = evaluate_task_response(&task, &result);
        assert!(
            (score.keyword_hits - 1.0).abs() < f64::EPSILON,
            "全キーワード一致すべき"
        );
    }

    #[test]
    fn test_code_gen_fizzbuzz_partial() {
        let task = BenchmarkTask {
            id: "code_gen_fizzbuzz".into(),
            name: "コード生成".into(),
            input: "test".into(),
            expected_tools: vec![],
            expected_keywords: vec!["fizz".into(), "buzz".into(), "fizzbuzz".into()],
            max_iterations: 3,
            category: TaskCategory::CodeGeneration,
        };
        // fizzのみ含む回答 → 部分ヒット
        let result = mock_result("fizz only", vec![], 1);
        let score = evaluate_task_response(&task, &result);
        assert!(
            (score.keyword_hits - 1.0 / 3.0).abs() < 0.001,
            "1/3キーワードヒット"
        );
    }

    #[test]
    fn test_multi_step_field_count_with_number() {
        let task = BenchmarkTask {
            id: "multi_step_field_count".into(),
            name: "マルチステップ推論".into(),
            input: "test".into(),
            expected_tools: vec!["file_read".into()],
            expected_keywords: vec![],
            max_iterations: 5,
            category: TaskCategory::MultiStep,
        };
        // 数値を含む回答 → 成功
        let result = mock_result(
            "ModelConfigには7つのフィールドがあります",
            vec!["file_read"],
            2,
        );
        let score = evaluate_task_response(&task, &result);
        assert!(
            (score.keyword_hits - 1.0).abs() < f64::EPSILON,
            "数値を含む回答は成功"
        );
    }

    #[test]
    fn test_multi_step_field_count_no_number() {
        let task = BenchmarkTask {
            id: "multi_step_field_count".into(),
            name: "マルチステップ推論".into(),
            input: "test".into(),
            expected_tools: vec!["file_read".into()],
            expected_keywords: vec![],
            max_iterations: 5,
            category: TaskCategory::MultiStep,
        };
        // 数値なし回答 → 失敗
        let result = mock_result("フィールドがいくつかあります", vec!["file_read"], 2);
        let score = evaluate_task_response(&task, &result);
        assert!(
            (score.keyword_hits).abs() < f64::EPSILON,
            "数値なし回答は失敗"
        );
    }

    #[test]
    fn test_error_handling_with_error_keyword() {
        let task = BenchmarkTask {
            id: "error_handling_nonexistent".into(),
            name: "エラーハンドリング".into(),
            input: "test".into(),
            expected_tools: vec!["file_read".into()],
            expected_keywords: vec![],
            max_iterations: 3,
            category: TaskCategory::ErrorRecovery,
        };
        // エラー報告を含む回答 → 成功
        let result = mock_result(
            "ファイルが存在しないためエラーが発生しました",
            vec!["file_read"],
            1,
        );
        let score = evaluate_task_response(&task, &result);
        assert!(
            (score.keyword_hits - 1.0).abs() < f64::EPSILON,
            "エラーキーワードで成功"
        );
    }

    #[test]
    fn test_error_handling_no_error_keyword() {
        let task = BenchmarkTask {
            id: "error_handling_nonexistent".into(),
            name: "エラーハンドリング".into(),
            input: "test".into(),
            expected_tools: vec!["file_read".into()],
            expected_keywords: vec![],
            max_iterations: 3,
            category: TaskCategory::ErrorRecovery,
        };
        // エラー報告なし → 失敗
        let result = mock_result("ファイルの内容は以下の通りです", vec!["file_read"], 1);
        let score = evaluate_task_response(&task, &result);
        assert!(
            (score.keyword_hits).abs() < f64::EPSILON,
            "エラーキーワードなしで失敗"
        );
    }

    #[test]
    fn test_summarize_long_enough() {
        let task = BenchmarkTask {
            id: "summarize_agent_loop".into(),
            name: "要約".into(),
            input: "test".into(),
            expected_tools: vec!["file_read".into()],
            expected_keywords: vec![],
            max_iterations: 4,
            category: TaskCategory::Summarization,
        };
        // 50文字以上の回答 → 成功
        let long_answer = "このファイルはエージェントループの主要な実装を含んでおり、Reflexionパターンを使用して自律的な推論と行動のサイクルを管理しています。";
        let result = mock_result(long_answer, vec!["file_read"], 2);
        let score = evaluate_task_response(&task, &result);
        assert!(
            (score.keyword_hits - 1.0).abs() < f64::EPSILON,
            "50文字以上で成功"
        );
    }

    #[test]
    fn test_summarize_too_short() {
        let task = BenchmarkTask {
            id: "summarize_agent_loop".into(),
            name: "要約".into(),
            input: "test".into(),
            expected_tools: vec!["file_read".into()],
            expected_keywords: vec![],
            max_iterations: 4,
            category: TaskCategory::Summarization,
        };
        // 短すぎる回答 → 部分スコア
        let result = mock_result("ループ処理です", vec!["file_read"], 1);
        let score = evaluate_task_response(&task, &result);
        assert!(score.keyword_hits < 1.0, "短すぎる回答は満点にならない");
        assert!(score.keyword_hits > 0.0, "空でない回答は0点にならない");
    }

    #[test]
    fn test_new_task_categories() {
        let suite = BenchmarkSuite::default_tasks();
        let code_gen = suite
            .tasks
            .iter()
            .find(|t| t.id == "code_gen_fizzbuzz")
            .unwrap();
        assert_eq!(code_gen.category, TaskCategory::CodeGeneration);
        let summary = suite
            .tasks
            .iter()
            .find(|t| t.id == "summarize_agent_loop")
            .unwrap();
        assert_eq!(summary.category, TaskCategory::Summarization);
    }

    #[test]
    fn test_fizzbuzz_case_insensitive() {
        // FizzBuzzのキーワードは大文字小文字を区別しないことを検証
        let task = BenchmarkTask {
            id: "code_gen_fizzbuzz".into(),
            name: "コード生成".into(),
            input: "test".into(),
            expected_tools: vec![],
            expected_keywords: vec!["fizz".into(), "buzz".into(), "fizzbuzz".into()],
            max_iterations: 3,
            category: TaskCategory::CodeGeneration,
        };
        let result = mock_result("FizzBuzz: Fizz, Buzz, FizzBuzz", vec![], 1);
        let score = evaluate_task_response(&task, &result);
        assert!(
            (score.keyword_hits - 1.0).abs() < f64::EPSILON,
            "大文字小文字を区別しない"
        );
    }

    // --- 追加タスク（22タスク化）テスト ---

    #[test]
    fn test_expanded_tasks_count() {
        // 16→22タスクへの拡張を検証
        let suite = BenchmarkSuite::default_tasks();
        assert_eq!(suite.tasks.len(), 22, "タスク数は22であるべき");
    }

    #[test]
    fn test_new_tasks_exist_in_expanded_suite() {
        let suite = BenchmarkSuite::default_tasks();
        let new_ids = [
            "multi_step_rename",
            "git_diff_analysis",
            "error_recovery_permission",
            "reasoning_json_parse",
            "code_gen_sort",
            "multi_file_search",
        ];
        for id in &new_ids {
            assert!(
                suite.tasks.iter().any(|t| t.id == *id),
                "タスク '{id}' がスイートに存在しない"
            );
        }
    }

    #[test]
    fn test_new_task_categories_expanded() {
        let suite = BenchmarkSuite::default_tasks();
        // 各追加タスクのカテゴリが正しいことを検証
        let rename = suite.tasks.iter().find(|t| t.id == "multi_step_rename").unwrap();
        assert_eq!(rename.category, TaskCategory::MultiStep);
        let diff = suite.tasks.iter().find(|t| t.id == "git_diff_analysis").unwrap();
        assert_eq!(diff.category, TaskCategory::ToolUse);
        let perm = suite.tasks.iter().find(|t| t.id == "error_recovery_permission").unwrap();
        assert_eq!(perm.category, TaskCategory::ErrorRecovery);
        let json = suite.tasks.iter().find(|t| t.id == "reasoning_json_parse").unwrap();
        assert_eq!(json.category, TaskCategory::Reasoning);
        let sort = suite.tasks.iter().find(|t| t.id == "code_gen_sort").unwrap();
        assert_eq!(sort.category, TaskCategory::CodeGeneration);
        let search = suite.tasks.iter().find(|t| t.id == "multi_file_search").unwrap();
        assert_eq!(search.category, TaskCategory::MultiStep);
    }

    #[test]
    fn test_error_recovery_permission_with_error() {
        // 権限エラー回復タスク: エラー関連キーワードで成功判定
        let task = BenchmarkTask {
            id: "error_recovery_permission".into(),
            name: "権限エラー回復".into(),
            input: "test".into(),
            expected_tools: vec!["file_write".into()],
            expected_keywords: vec!["permission".into(), "denied".into(), "error".into(), "cannot".into()],
            max_iterations: 4,
            category: TaskCategory::ErrorRecovery,
        };
        let result = mock_result(
            "permission deniedエラーが発生しました。書き込みできません",
            vec!["file_write"],
            1,
        );
        let score = evaluate_task_response(&task, &result);
        assert!(
            (score.keyword_hits - 1.0).abs() < f64::EPSILON,
            "権限エラーキーワードで成功"
        );
    }

    #[test]
    fn test_reasoning_json_parse_correct() {
        // JSON解析: 正しいフィールド値を含む回答
        let task = BenchmarkTask {
            id: "reasoning_json_parse".into(),
            name: "JSON解析推論".into(),
            input: "test".into(),
            expected_tools: vec![],
            expected_keywords: vec!["bonsai".into()],
            max_iterations: 3,
            category: TaskCategory::Reasoning,
        };
        let result = mock_result("nameフィールドの値は\"bonsai\"です", vec![], 1);
        let score = evaluate_task_response(&task, &result);
        assert!(
            (score.keyword_hits - 1.0).abs() < f64::EPSILON,
            "正しいJSON値で成功"
        );
    }

    #[test]
    fn test_code_gen_sort_keywords() {
        // ソート関数生成: fn/sort/vecキーワード検証
        let task = BenchmarkTask {
            id: "code_gen_sort".into(),
            name: "ソート関数生成".into(),
            input: "test".into(),
            expected_tools: vec!["file_write".into()],
            expected_keywords: vec!["sort".into(), "fn".into(), "vec".into()],
            max_iterations: 5,
            category: TaskCategory::CodeGeneration,
        };
        let result = mock_result(
            "fn bubble_sort(mut vec: Vec<i32>) -> Vec<i32> { /* sort logic */ }",
            vec!["file_write"],
            2,
        );
        let score = evaluate_task_response(&task, &result);
        assert!(
            (score.keyword_hits - 1.0).abs() < f64::EPSILON,
            "ソート関数キーワード全一致"
        );
    }
}
