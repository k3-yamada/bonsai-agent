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

/// ベンチマークタスクの tier (項目 172 P1: ベンチマーク階層分離)
///
/// Lab v14 baseline -35% 退行の原因仮説 X (Bench 拡張) / Y (MLX 環境) を分離するため、
/// 既存 22 タスクを `Core`、Phase C 追加 18 タスクを `Extended` として階層管理する。
/// `BONSAI_BENCH_TIER` env で実行時に tier を切替可能 (Phase 3 で実装)。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum TaskTier {
    #[default]
    Core,
    Extended,
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
    /// tier (項目 172 P1: ベンチマーク階層分離)。旧データとの serde 互換のため default を持つ。
    #[serde(default)]
    pub tier: TaskTier,
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

/// 軌跡評価スコア（NAT Trajectory Evaluation知見）
///
/// 期待ツール呼出順序と実際の順序を比較し、
/// 「正しい答えを間違った理由で出す」ケースを検出する。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajectoryScore {
    /// 順序一致率（LCS長 / 期待シーケンス長）
    pub sequence_accuracy: f64,
    /// 期待ツールのカバー率（一致ユニークツール数 / 期待ユニークツール数）
    pub tool_coverage: f64,
    /// 期待外のツール呼出数
    pub extra_calls: usize,
}

impl TrajectoryScore {
    /// 期待軌跡と実際の軌跡からスコアを計算
    pub fn compute(expected: &[String], actual: &[String]) -> Self {
        if expected.is_empty() {
            return Self {
                sequence_accuracy: 1.0,
                tool_coverage: 1.0,
                extra_calls: 0,
            };
        }

        // LCS（最長共通部分列）で順序一致率を計算
        let lcs_len = lcs_length(expected, actual);
        let sequence_accuracy = lcs_len as f64 / expected.len() as f64;

        // ユニークツールカバー率
        let expected_unique: std::collections::HashSet<&str> =
            expected.iter().map(|s| s.as_str()).collect();
        let actual_unique: std::collections::HashSet<&str> =
            actual.iter().map(|s| s.as_str()).collect();
        let covered = expected_unique.intersection(&actual_unique).count();
        let tool_coverage = covered as f64 / expected_unique.len() as f64;

        // 期待外呼出数
        let extra_calls = actual
            .iter()
            .filter(|a| !expected_unique.contains(a.as_str()))
            .count();

        Self {
            sequence_accuracy,
            tool_coverage,
            extra_calls,
        }
    }

    /// 複合スコア（0.0-1.0）: 60%順序 + 30%カバー + 10%効率ペナルティ
    pub fn composite(&self) -> f64 {
        let extra_penalty = if self.extra_calls == 0 {
            1.0
        } else {
            (1.0 / (1.0 + self.extra_calls as f64)).max(0.0)
        };
        0.6 * self.sequence_accuracy + 0.3 * self.tool_coverage + 0.1 * extra_penalty
    }
}

/// LCS（最長共通部分列）の長さを計算
fn lcs_length(a: &[String], b: &[String]) -> usize {
    let m = a.len();
    let n = b.len();
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in 1..=m {
        for j in 1..=n {
            dp[i][j] = if a[i - 1] == b[j - 1] {
                dp[i - 1][j - 1] + 1
            } else {
                dp[i - 1][j].max(dp[i][j - 1])
            };
        }
    }
    dp[m][n]
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
    /// 代表 run（通常は最終 run）の最終応答（Phase B2: judge gate 用）。
    /// 既存ベンチマーク経路では `None` のまま、judge 統合時に `with_last_run()` で注入。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_response: Option<String>,
    /// 代表 run のツール軌跡（呼出されたツール名の順序、Phase B2: judge gate 用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_trajectory: Option<Vec<String>>,
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
                last_response: None,
                last_trajectory: None,
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
            last_response: None,
            last_trajectory: None,
        }
    }

    /// 代表 run の応答と軌跡を後付けで注入するビルダー（Phase B2: judge gate 用）
    ///
    /// `from_scores` のシグネチャ互換性を維持するため、judge 統合経路はこのメソッドで
    /// trajectory を後付け enrich する。既存呼出側に影響しない。
    pub fn with_last_run(mut self, response: String, trajectory: Vec<String>) -> Self {
        self.last_response = Some(response);
        self.last_trajectory = Some(trajectory);
        self
    }
}

/// 複数回実行のベンチマーク全体結果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiRunBenchmarkResult {
    pub task_scores: Vec<MultiRunTaskScore>,
    pub duration_secs: f64,
    /// Core tier タスクの平均 mean_score (項目 172 P1)。該当タスクなしなら None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub core_avg_score: Option<f64>,
    /// Extended tier タスクの平均 mean_score (項目 172 P1)。該当タスクなしなら None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extended_avg_score: Option<f64>,
}

/// tier 別平均 mean_score を算出（項目 172 P1）。
///
/// `tasks` と `scores` は同順で対応している前提（`run_k` 内で生成順を維持）。
/// 該当 tier のタスクが 1 件もないなら None を返す。
fn compute_tier_avg(
    tasks: &[BenchmarkTask],
    scores: &[MultiRunTaskScore],
    tier: TaskTier,
) -> Option<f64> {
    let filtered: Vec<f64> = tasks
        .iter()
        .zip(scores.iter())
        .filter(|(t, _)| t.tier == tier)
        .map(|(_, s)| s.mean_score)
        .collect();
    if filtered.is_empty() {
        None
    } else {
        Some(filtered.iter().sum::<f64>() / filtered.len() as f64)
    }
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

/// 開発時に高速イテレーションするための smoke タスク ID 集合（5 件）。
///
/// `default_tasks()` 全 40 件のうち、各カテゴリから代表 1 件を選定。
/// `smoke_tasks()` でこの ID に一致するタスクのみを返し、Lab 中の dev iteration
/// を 1/8 に短縮する（k=3 で 15 ラン vs 120 ラン）。
const SMOKE_TASK_IDS: &[&str] = &[
    "file_read_simple",      // ToolUse: 単純ファイル読み取り
    "multi_step_write_read", // MultiStep: 書込→読込
    "error_recovery",        // ErrorRecovery: エラー後の代替試行
    "tool_selection_git",    // ToolSelection: 適切なツール選択
    "code_gen_fizzbuzz",     // CodeGeneration: コード生成
];

impl BenchmarkSuite {
    /// 開発時用の smoke タスクセット（5 タスク）。
    ///
    /// `default_tasks()` のうち `SMOKE_TASK_IDS` に一致するものだけを抽出。
    /// CI/Lab 本番では `default_tasks()`（40 タスク）を使い、開発時の高速確認に
    /// 限定して `smoke_tasks()` を使う。
    pub fn smoke_tasks() -> Self {
        Self {
            tasks: Self::default_tasks()
                .tasks
                .into_iter()
                .filter(|t| SMOKE_TASK_IDS.contains(&t.id.as_str()))
                .collect(),
        }
    }

    /// Core tier のみ (22 タスク、項目 172 P1: ベンチマーク階層分離)。
    ///
    /// Lab v9/v10 当時の 22 タスクと同等。MLX 環境劣化 (仮説 Y) の
    /// 切り分けに使用する。`BONSAI_BENCH_TIER=core` で Lab 起動可。
    pub fn core_tasks() -> Self {
        Self {
            tasks: Self::default_tasks()
                .tasks
                .into_iter()
                .filter(|t| t.tier == TaskTier::Core)
                .collect(),
        }
    }

    /// Extended tier (Phase C 追加分、18 タスク、項目 172 P1)。
    ///
    /// MultiFileEdit/LongRun/ToolChain/McpInteg/Semantic/Reasoning/
    /// Summarization/Verification の 9 領域 ×2 = 18 タスク。
    /// Bench 拡張による退行 (仮説 X) の切り分けに使用する。
    pub fn extended_tasks() -> Self {
        Self {
            tasks: Self::default_tasks()
                .tasks
                .into_iter()
                .filter(|t| t.tier == TaskTier::Extended)
                .collect(),
        }
    }

    /// デフォルトのベンチマークタスクセット（40タスク）
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
                    tier: TaskTier::Core,
                },
                BenchmarkTask {
                    id: "shell_ls".into(),
                    name: "ファイル一覧".into(),
                    input: "このディレクトリのファイル一覧を表示して".into(),
                    expected_tools: vec!["shell".into()],
                    expected_keywords: vec!["src".into()],
                    max_iterations: 3,
                    category: TaskCategory::ToolUse,
                    tier: TaskTier::Core,
                },
                BenchmarkTask {
                    id: "git_status".into(),
                    name: "Git状態確認".into(),
                    input: "Gitの状態を確認して".into(),
                    expected_tools: vec!["git".into()],
                    expected_keywords: vec!["branch".into()],
                    max_iterations: 3,
                    category: TaskCategory::ToolUse,
                    tier: TaskTier::Core,
                },
                BenchmarkTask {
                    id: "multi_step_write_read".into(),
                    name: "書き込み→読み返し".into(),
                    input: "hello.txtに'Hello World'と書いて、それを読み返して".into(),
                    expected_tools: vec!["file_write".into(), "file_read".into()],
                    expected_keywords: vec!["Hello World".into()],
                    max_iterations: 5,
                    category: TaskCategory::MultiStep,
                    tier: TaskTier::Core,
                },
                BenchmarkTask {
                    id: "reasoning_calc".into(),
                    name: "計算推論".into(),
                    input: "2の10乗はいくつですか".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["1024".into()],
                    max_iterations: 3,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Core,
                },
                BenchmarkTask {
                    id: "error_recovery".into(),
                    name: "エラー回復".into(),
                    input: "存在しないファイル /tmp/bonsai_nonexistent_test.txt を読んで".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec!["存在".into()],
                    max_iterations: 3,
                    category: TaskCategory::ErrorRecovery,
                    tier: TaskTier::Core,
                },
                BenchmarkTask {
                    id: "tool_selection_git".into(),
                    name: "ツール選択".into(),
                    input: "このプロジェクトのGitログを見せて".into(),
                    expected_tools: vec!["git".into()],
                    expected_keywords: vec!["commit".into()],
                    max_iterations: 3,
                    category: TaskCategory::ToolSelection,
                    tier: TaskTier::Core,
                },
                BenchmarkTask {
                    id: "direct_answer".into(),
                    name: "直接回答".into(),
                    input: "Rustのマスコットの名前は？".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["Ferris".into()],
                    max_iterations: 2,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Core,
                },
                BenchmarkTask {
                    id: "code_gen_fizzbuzz".into(),
                    name: "コード生成".into(),
                    input: "FizzBuzzをRustで書いて。1から15までの出力例も示して".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["fizz".into(), "buzz".into(), "fizzbuzz".into()],
                    max_iterations: 3,
                    category: TaskCategory::CodeGeneration,
                    tier: TaskTier::Core,
                },
                BenchmarkTask {
                    id: "multi_step_field_count".into(),
                    name: "マルチステップ推論".into(),
                    input: "src/config.rsのModelConfig構造体のフィールド数を数えて".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec![],
                    max_iterations: 5,
                    category: TaskCategory::MultiStep,
                    tier: TaskTier::Core,
                },
                BenchmarkTask {
                    id: "error_handling_nonexistent".into(),
                    name: "エラーハンドリング".into(),
                    input: "/tmp/bonsai_absolutely_missing_file_xyz.rs を読んで内容を教えて".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec![],
                    max_iterations: 3,
                    category: TaskCategory::ErrorRecovery,
                    tier: TaskTier::Core,
                },
                BenchmarkTask {
                    id: "summarize_agent_loop".into(),
                    name: "要約".into(),
                    input: "src/agent/agent_loop.rsの最初の50行を要約して".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec![],
                    max_iterations: 4,
                    category: TaskCategory::Summarization,
                    tier: TaskTier::Core,
                },
                BenchmarkTask {
                    id: "repo_structure".into(),
                    name: "リポジトリ構造把握".into(),
                    input: "このプロジェクトのsrc/ディレクトリにあるRustファイルの数を教えて".into(),
                    expected_tools: vec!["repo_map".into()],
                    expected_keywords: vec![],
                    max_iterations: 3,
                    category: TaskCategory::ToolUse,
                    tier: TaskTier::Core,
                },
                BenchmarkTask {
                    id: "multi_file_compare".into(),
                    name: "複数ファイル比較".into(),
                    input: "src/tools/file.rsとsrc/tools/shell.rsの行数をそれぞれ教えて".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec![],
                    max_iterations: 5,
                    category: TaskCategory::MultiStep,
                    tier: TaskTier::Core,
                },
                BenchmarkTask {
                    id: "conditional_file_op".into(),
                    name: "条件付きファイル操作".into(),
                    input: "/tmp/bonsai_bench_test.txt が存在するか確認し、存在しなければ'benchmark ok'と書き込んで".into(),
                    expected_tools: vec!["file_read".into(), "file_write".into()],
                    expected_keywords: vec!["benchmark".into()],
                    max_iterations: 5,
                    category: TaskCategory::MultiStep,
                    tier: TaskTier::Core,
                },
                BenchmarkTask {
                    id: "code_review".into(),
                    name: "コードレビュー".into(),
                    input: "src/tools/arxiv.rsのコードを読んで、改善点があれば指摘して".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec![],
                    max_iterations: 4,
                    category: TaskCategory::Summarization,
                    tier: TaskTier::Core,
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
                    tier: TaskTier::Core,
                },
                BenchmarkTask {
                    id: "git_diff_analysis".into(),
                    name: "Git差分分析".into(),
                    input: "最後のコミットで何が変更されたか確認して".into(),
                    expected_tools: vec!["shell".into()],
                    expected_keywords: vec!["diff".into(), "commit".into(), "changed".into()],
                    max_iterations: 4,
                    category: TaskCategory::ToolUse,
                    tier: TaskTier::Core,
                },
                BenchmarkTask {
                    id: "error_recovery_permission".into(),
                    name: "権限エラー回復".into(),
                    input: "/etc/bonsai_readonly_testに'test'と書き込んで".into(),
                    expected_tools: vec!["file_write".into()],
                    expected_keywords: vec!["permission".into(), "denied".into(), "error".into(), "cannot".into()],
                    max_iterations: 4,
                    category: TaskCategory::ErrorRecovery,
                    tier: TaskTier::Core,
                },
                BenchmarkTask {
                    id: "reasoning_json_parse".into(),
                    name: "JSON解析推論".into(),
                    input: r#"次のJSONから"name"フィールドの値を教えて: {"id": 1, "name": "bonsai", "version": "0.1"}"#.into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["bonsai".into()],
                    max_iterations: 3,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Core,
                },
                BenchmarkTask {
                    id: "code_gen_sort".into(),
                    name: "ソート関数生成".into(),
                    input: "Rustでバブルソート関数を書いて。Vec<i32>を受け取ってソートするfnを定義して".into(),
                    expected_tools: vec!["file_write".into()],
                    expected_keywords: vec!["sort".into(), "fn".into(), "vec".into()],
                    max_iterations: 5,
                    category: TaskCategory::CodeGeneration,
                    tier: TaskTier::Core,
                },
                BenchmarkTask {
                    id: "multi_file_search".into(),
                    name: "複数ファイル検索".into(),
                    input: "src/配下で'run_agent_loop'関数が定義されているファイルを特定して".into(),
                    expected_tools: vec!["shell".into(), "file_read".into()],
                    expected_keywords: vec!["found".into(), "file".into()],
                    max_iterations: 5,
                    category: TaskCategory::MultiStep,
                    tier: TaskTier::Core,
                },
                // ============================================================
                // Phase C 追加タスク（22→40, .claude/plan/phase-c-and-refactor-draft.md Part 1）
                // 既存 TaskCategory を再利用（MultiFileEdit/LongRun/ToolChain→MultiStep,
                // McpInteg/Verification→ToolUse, Semantic→Reasoning へマッピング）
                // ============================================================
                // --- MultiFileEdit (×2) -------------------------------------
                BenchmarkTask {
                    id: "rename_var_3files".into(),
                    name: "3ファイル変数リネーム".into(),
                    input: "src/foo.rs と src/bar.rs と src/baz.rs の変数 `old_name` を `new_name` にリネームする手順を示して".into(),
                    expected_tools: vec!["repo_map".into(), "multi_edit".into()],
                    expected_keywords: vec!["new_name".into(), "リネーム".into()],
                    max_iterations: 8,
                    category: TaskCategory::MultiStep,
                    tier: TaskTier::Extended,
                },
                BenchmarkTask {
                    id: "sig_change_4files".into(),
                    name: "4ファイル関数シグネチャ変更".into(),
                    input: "関数 `foo(a: i32)` を `foo(a: i32, b: bool)` に変更し、全呼出元を更新する手順を示して".into(),
                    expected_tools: vec!["shell".into(), "multi_edit".into()],
                    expected_keywords: vec!["b: bool".into(), "呼出".into()],
                    max_iterations: 8,
                    category: TaskCategory::MultiStep,
                    tier: TaskTier::Extended,
                },
                // --- LongRun (×2) -------------------------------------------
                BenchmarkTask {
                    id: "tool_chain_10steps".into(),
                    name: "10ステップツールチェーン".into(),
                    input: "RepoMap で全ファイルをリストし、各ファイルの先頭5行を読み取り、構造を要約して".into(),
                    expected_tools: vec!["repo_map".into(), "file_read".into()],
                    expected_keywords: vec!["要約".into(), "構造".into()],
                    max_iterations: 10,
                    category: TaskCategory::MultiStep,
                    tier: TaskTier::Extended,
                },
                BenchmarkTask {
                    id: "implement_50steps".into(),
                    name: "FizzBuzz拡張仕様".into(),
                    input: "FizzBuzz 拡張版（7→Bazz, 11→Lazz）の仕様を示し、エッジケースを 3 件挙げて".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["Bazz".into(), "Lazz".into(), "FizzBuzz".into()],
                    max_iterations: 6,
                    category: TaskCategory::CodeGeneration,
                    tier: TaskTier::Extended,
                },
                // --- ToolChain (×2) -----------------------------------------
                BenchmarkTask {
                    id: "repomap_read_edit_test".into(),
                    name: "RepoMap+読込+編集連鎖".into(),
                    input: "ファイル src/foo.rs の `parse` 関数を `parse_v2` に改名する手順を、依存ファイル特定→編集の順で示して".into(),
                    expected_tools: vec!["repo_map".into(), "file_read".into(), "multi_edit".into()],
                    expected_keywords: vec!["parse_v2".into(), "改名".into()],
                    max_iterations: 8,
                    category: TaskCategory::MultiStep,
                    tier: TaskTier::Extended,
                },
                BenchmarkTask {
                    id: "grep_multiedit".into(),
                    name: "grep+一括編集".into(),
                    input: "`anyhow::Result` を `Result<T, MyError>` に置換するために、grep で対象を特定する手順を示して".into(),
                    expected_tools: vec!["shell".into(), "multi_edit".into()],
                    expected_keywords: vec!["grep".into(), "置換".into()],
                    max_iterations: 6,
                    category: TaskCategory::MultiStep,
                    tier: TaskTier::Extended,
                },
                // --- ErrorRecovery (×2) -------------------------------------
                BenchmarkTask {
                    id: "tool_fail_pivot".into(),
                    name: "ツール失敗→代替手段".into(),
                    input: "存在しないコマンド `fakecmd_xyz` を試した後、別の方法で現在のディレクトリを確認して".into(),
                    expected_tools: vec!["shell".into()],
                    expected_keywords: vec!["代替".into(), "ls".into()],
                    max_iterations: 5,
                    category: TaskCategory::ErrorRecovery,
                    tier: TaskTier::Extended,
                },
                BenchmarkTask {
                    id: "corrupt_file_repair".into(),
                    name: "破損JSON修復".into(),
                    input: "JSON 文字列 `{\"name\":\"test\"` （閉じ括弧欠落）を修復した正しい形を示せ".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["}".into(), "name".into()],
                    max_iterations: 4,
                    category: TaskCategory::ErrorRecovery,
                    tier: TaskTier::Extended,
                },
                // --- McpInteg (×2) — MCP 未接続時はツール不在で keyword 評価のみ ---
                BenchmarkTask {
                    id: "mcp_filesystem_list".into(),
                    name: "MCP filesystem list".into(),
                    input: "filesystem MCP で `/tmp` ディレクトリの一覧を取得する例を示して".into(),
                    expected_tools: vec!["filesystem:list_directory".into()],
                    expected_keywords: vec!["filesystem".into(), "list".into()],
                    max_iterations: 4,
                    category: TaskCategory::ToolUse,
                    tier: TaskTier::Extended,
                },
                BenchmarkTask {
                    id: "mcp_search_replace".into(),
                    name: "MCP filesystem 置換".into(),
                    input: "filesystem MCP で `/tmp/test.txt` 内の `foo` を `bar` に置換する手順を示して".into(),
                    expected_tools: vec!["filesystem:read_file".into(), "filesystem:write_file".into()],
                    expected_keywords: vec!["foo".into(), "bar".into()],
                    max_iterations: 5,
                    category: TaskCategory::ToolUse,
                    tier: TaskTier::Extended,
                },
                // --- Semantic (×2) — 曖昧な指示への構造化応答 ---------------
                BenchmarkTask {
                    id: "vague_log_improve".into(),
                    name: "曖昧指示: ログ改善".into(),
                    input: "ログを改善したい。具体的な改善案を 3 点挙げて".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["ログ".into(), "改善".into()],
                    max_iterations: 4,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Extended,
                },
                BenchmarkTask {
                    id: "refactor_intent".into(),
                    name: "曖昧指示: リファクタ意図".into(),
                    input: "このコードをもっと綺麗にしたい。一般的なリファクタリングの指針を 3 点述べて".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["リファクタ".into(), "指針".into()],
                    max_iterations: 4,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Extended,
                },
                // --- Reasoning (×2) -----------------------------------------
                BenchmarkTask {
                    id: "nested_logic".into(),
                    name: "ネスト論理式".into(),
                    input: "x=3, y=5 のとき式 `x > y && (x + y) % 2 == 0` の真偽は何か。理由とともに答えて".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["false".into(), "偽".into()],
                    max_iterations: 3,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Extended,
                },
                BenchmarkTask {
                    id: "ambiguous_calc".into(),
                    name: "あいまい計算".into(),
                    input: "2 の 8 乗を 3 で割った余りはいくつか。途中式も示して".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["1".into(), "256".into()],
                    max_iterations: 3,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Extended,
                },
                // --- Summarization (×2) -------------------------------------
                BenchmarkTask {
                    id: "multi_file_summary".into(),
                    name: "複数ファイル役割要約".into(),
                    input: "src/agent/agent_loop.rs と src/agent/tool_exec.rs の役割の違いを 200 字以内で要約して".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec!["agent_loop".into(), "tool_exec".into()],
                    max_iterations: 5,
                    category: TaskCategory::Summarization,
                    tier: TaskTier::Extended,
                },
                BenchmarkTask {
                    id: "git_log_summary".into(),
                    name: "git ログ要約".into(),
                    input: "直近 5 コミットの「変更概要 + 影響範囲」を表形式で要約して".into(),
                    expected_tools: vec!["shell".into()],
                    expected_keywords: vec!["commit".into(), "要約".into()],
                    max_iterations: 5,
                    category: TaskCategory::Summarization,
                    tier: TaskTier::Extended,
                },
                // --- Verification (×2) — 自己検証 / 事実確認 ----------------
                BenchmarkTask {
                    id: "self_check_arithmetic".into(),
                    name: "自己検算".into(),
                    input: "(17 × 23) を計算し、別の方法で検算してから最終答を示して".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["391".into(), "検算".into()],
                    max_iterations: 3,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Extended,
                },
                BenchmarkTask {
                    id: "tool_fact_check".into(),
                    name: "事実確認: ファイル存在".into(),
                    input: "現在のディレクトリに `Cargo.toml` が存在するかツールで確認して".into(),
                    expected_tools: vec!["file_read".into(), "shell".into()],
                    expected_keywords: vec!["Cargo.toml".into(), "存在".into()],
                    max_iterations: 4,
                    category: TaskCategory::ToolUse,
                    tier: TaskTier::Extended,
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
            // 最終 run の代表 trajectory（B2: judge gate 用、最後に成功した run を優先）
            let mut last_run_capture: Option<(String, Vec<String>)> = None;
            // タスク毎に1 DB作成、k回ループ間でリセット（66→22 DB作成に削減）
            let store = MemoryStore::in_memory()?;
            for run_idx in 0..multi.k {
                if cancel.is_cancelled() {
                    break;
                }

                if run_idx > 0 {
                    store.reset_session_data()?;
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
                    soul_path: config.soul_path.clone(),
                    memory_blocks: config.memory_blocks.clone(),
                };
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
                    Ok(ref loop_result) => {
                        // judge gate 用: 最終 run の応答 + トラジェクトリを保持
                        last_run_capture =
                            Some((loop_result.answer.clone(), loop_result.tools_called.clone()));
                        evaluate_task_response(task, loop_result).score()
                    }
                    Err(_) => 0.0,
                };
                scores.push(score);
            }

            let mut task_score =
                MultiRunTaskScore::from_scores(task.id.clone(), scores, pass_threshold);
            if let Some((response, trajectory)) = last_run_capture {
                task_score = task_score.with_last_run(response, trajectory);
            }
            task_scores.push(task_score);
        }

        // 項目 172 P1: tier 別平均 mean_score 集計（仮説 X / Y 分離用）
        let core_avg_score = compute_tier_avg(&self.tasks, &task_scores, TaskTier::Core);
        let extended_avg_score = compute_tier_avg(&self.tasks, &task_scores, TaskTier::Extended);

        Ok(MultiRunBenchmarkResult {
            task_scores,
            duration_secs: start.elapsed().as_secs_f64(),
            core_avg_score,
            extended_avg_score,
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
                soul_path: config.soul_path.clone(),
                memory_blocks: config.memory_blocks.clone(),
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
        assert_eq!(suite.tasks.len(), 40);
    }

    #[test]
    fn test_default_tasks_unique_ids() {
        let suite = BenchmarkSuite::default_tasks();
        let mut ids: Vec<&str> = suite.tasks.iter().map(|t| t.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), suite.tasks.len());
    }

    // --- Tier 分割テスト (Red, 項目 172 P1: ベンチマーク階層分離) -----------------
    // 仮説 X (Bench 拡張主因) / 仮説 Y (MLX 環境主因) を 1 セッションで分離するため、
    // core (22) / extended (18) tier を導入し full baseline を計測可能にする。

    #[test]
    fn test_core_tasks_count_22() {
        let suite = BenchmarkSuite::core_tasks();
        assert_eq!(suite.tasks.len(), 22, "core tier は 22 タスク");
    }

    #[test]
    fn test_extended_tasks_count_18() {
        let suite = BenchmarkSuite::extended_tasks();
        assert_eq!(
            suite.tasks.len(),
            18,
            "extended tier (Phase C) は 18 タスク"
        );
    }

    #[test]
    fn test_default_equals_core_plus_extended() {
        let all = BenchmarkSuite::default_tasks();
        assert_eq!(all.tasks.len(), 40, "default は core + extended = 40");
        let ids: std::collections::HashSet<_> = all.tasks.iter().map(|t| t.id.clone()).collect();
        assert_eq!(ids.len(), 40, "重複なし");
    }

    #[test]
    fn test_task_tier_field_set() {
        let core = BenchmarkSuite::core_tasks();
        let ext = BenchmarkSuite::extended_tasks();
        assert!(
            core.tasks.iter().all(|t| t.tier == TaskTier::Core),
            "core tier の全タスクは TaskTier::Core"
        );
        assert!(
            ext.tasks.iter().all(|t| t.tier == TaskTier::Extended),
            "extended tier の全タスクは TaskTier::Extended"
        );
    }

    #[test]
    fn test_multi_run_result_tier_aggregation() {
        // モックタスク: Core 2 件 + Extended 1 件
        let make_task = |id: &str, tier: TaskTier| BenchmarkTask {
            id: id.into(),
            name: id.into(),
            input: String::new(),
            expected_tools: vec![],
            expected_keywords: vec![],
            max_iterations: 1,
            category: TaskCategory::ToolUse,
            tier,
        };
        let tasks = vec![
            make_task("c1", TaskTier::Core),
            make_task("c2", TaskTier::Core),
            make_task("e1", TaskTier::Extended),
        ];
        let scores = vec![
            MultiRunTaskScore::from_scores("c1".into(), vec![0.8, 0.8], 0.5),
            MultiRunTaskScore::from_scores("c2".into(), vec![0.6, 0.6], 0.5),
            MultiRunTaskScore::from_scores("e1".into(), vec![0.4, 0.4], 0.5),
        ];

        let core_avg = compute_tier_avg(&tasks, &scores, TaskTier::Core).unwrap();
        let ext_avg = compute_tier_avg(&tasks, &scores, TaskTier::Extended).unwrap();
        assert!((core_avg - 0.7).abs() < 0.001, "core_avg={core_avg}");
        assert!((ext_avg - 0.4).abs() < 0.001, "ext_avg={ext_avg}");

        // 該当 tier ゼロなら None
        let core_only_tasks = vec![make_task("c1", TaskTier::Core)];
        let core_only_scores = vec![MultiRunTaskScore::from_scores("c1".into(), vec![1.0], 0.5)];
        assert!(
            compute_tier_avg(&core_only_tasks, &core_only_scores, TaskTier::Extended).is_none(),
            "extended 該当なしなら None"
        );
    }

    #[test]
    fn test_smoke_tasks_subset_of_default() {
        let smoke = BenchmarkSuite::smoke_tasks();
        let default = BenchmarkSuite::default_tasks();
        assert_eq!(smoke.tasks.len(), 5, "smoke は 5 タスク");
        assert!(smoke.tasks.len() < default.tasks.len(), "smoke ⊂ default");
        // すべての smoke ID は default に含まれる
        let default_ids: std::collections::HashSet<&str> =
            default.tasks.iter().map(|t| t.id.as_str()).collect();
        for t in &smoke.tasks {
            assert!(
                default_ids.contains(t.id.as_str()),
                "smoke ID {} は default に存在すべき",
                t.id
            );
        }
    }

    #[test]
    fn test_smoke_tasks_cover_distinct_categories() {
        let smoke = BenchmarkSuite::smoke_tasks();
        let mut cats: Vec<TaskCategory> = smoke.tasks.iter().map(|t| t.category.clone()).collect();
        cats.sort_by_key(|c| format!("{c:?}"));
        cats.dedup();
        assert!(
            cats.len() >= 5,
            "smoke は 5+ カテゴリをカバー: got {cats:?}"
        );
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
            tier: TaskTier::Core,
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
            tier: TaskTier::Core,
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
            tier: TaskTier::Core,
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
            tier: TaskTier::Core,
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
            tier: TaskTier::Core,
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
            tier: TaskTier::Core,
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
            core_avg_score: None,
            extended_avg_score: None,
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

    #[test]
    fn test_multi_run_task_score_default_last_run_none() {
        // Phase B2 Step 1: 既存経路では last_response/last_trajectory は None
        let mrt = MultiRunTaskScore::from_scores("t".into(), vec![0.8], 0.5);
        assert!(mrt.last_response.is_none());
        assert!(mrt.last_trajectory.is_none());
    }

    #[test]
    fn test_multi_run_task_score_with_last_run_builder() {
        // Phase B2 Step 1: with_last_run() で trajectory を後付け enrich
        let mrt = MultiRunTaskScore::from_scores("t".into(), vec![0.8], 0.5).with_last_run(
            "答え: 42".to_string(),
            vec!["shell".to_string(), "file_read".to_string()],
        );
        assert_eq!(mrt.last_response.as_deref(), Some("答え: 42"));
        assert_eq!(
            mrt.last_trajectory.as_deref().unwrap(),
            &["shell", "file_read"]
        );
        // 既存スコアフィールドは無変化
        assert!((mrt.mean_score - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn test_multi_run_task_score_serde_roundtrip_with_last_run() {
        // Phase B2 Step 1: serde(skip_serializing_if = Option::is_none) の動作確認
        let mrt = MultiRunTaskScore::from_scores("t".into(), vec![0.8], 0.5)
            .with_last_run("ans".into(), vec!["s".into()]);
        let json = serde_json::to_string(&mrt).unwrap();
        assert!(json.contains("last_response"));
        assert!(json.contains("last_trajectory"));
        let restored: MultiRunTaskScore = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.last_response.as_deref(), Some("ans"));

        // None 値はシリアライズ時に省略される
        let mrt_none = MultiRunTaskScore::from_scores("t".into(), vec![0.8], 0.5);
        let json_none = serde_json::to_string(&mrt_none).unwrap();
        assert!(!json_none.contains("last_response"));
        assert!(!json_none.contains("last_trajectory"));
        // 省略された JSON も None として復元できる（後方互換）
        let restored_none: MultiRunTaskScore = serde_json::from_str(&json_none).unwrap();
        assert!(restored_none.last_response.is_none());
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
            tier: TaskTier::Core,
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
            tier: TaskTier::Core,
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
            tier: TaskTier::Core,
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
            tier: TaskTier::Core,
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
            tier: TaskTier::Core,
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
            tier: TaskTier::Core,
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
            tier: TaskTier::Core,
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
            tier: TaskTier::Core,
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
            tier: TaskTier::Core,
        };
        let result = mock_result("FizzBuzz: Fizz, Buzz, FizzBuzz", vec![], 1);
        let score = evaluate_task_response(&task, &result);
        assert!(
            (score.keyword_hits - 1.0).abs() < f64::EPSILON,
            "大文字小文字を区別しない"
        );
    }

    // --- 追加タスク（40タスク化）テスト ---

    #[test]
    fn test_expanded_tasks_count() {
        // 22→40タスクへの拡張を検証（Phase C: +18タスク）
        let suite = BenchmarkSuite::default_tasks();
        assert_eq!(suite.tasks.len(), 40, "タスク数は40であるべき");
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
        let rename = suite
            .tasks
            .iter()
            .find(|t| t.id == "multi_step_rename")
            .unwrap();
        assert_eq!(rename.category, TaskCategory::MultiStep);
        let diff = suite
            .tasks
            .iter()
            .find(|t| t.id == "git_diff_analysis")
            .unwrap();
        assert_eq!(diff.category, TaskCategory::ToolUse);
        let perm = suite
            .tasks
            .iter()
            .find(|t| t.id == "error_recovery_permission")
            .unwrap();
        assert_eq!(perm.category, TaskCategory::ErrorRecovery);
        let json = suite
            .tasks
            .iter()
            .find(|t| t.id == "reasoning_json_parse")
            .unwrap();
        assert_eq!(json.category, TaskCategory::Reasoning);
        let sort = suite
            .tasks
            .iter()
            .find(|t| t.id == "code_gen_sort")
            .unwrap();
        assert_eq!(sort.category, TaskCategory::CodeGeneration);
        let search = suite
            .tasks
            .iter()
            .find(|t| t.id == "multi_file_search")
            .unwrap();
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
            expected_keywords: vec![
                "permission".into(),
                "denied".into(),
                "error".into(),
                "cannot".into(),
            ],
            max_iterations: 4,
            category: TaskCategory::ErrorRecovery,
            tier: TaskTier::Core,
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
            tier: TaskTier::Core,
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
            tier: TaskTier::Core,
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

    // --- Phase 2: TrajectoryScore テスト（NAT軌跡評価知見） ---

    #[test]
    fn t_trajectory_score_perfect_match() {
        let expected = vec![
            "file_read".to_string(),
            "shell".to_string(),
            "file_write".to_string(),
        ];
        let actual = vec![
            "file_read".to_string(),
            "shell".to_string(),
            "file_write".to_string(),
        ];
        let ts = TrajectoryScore::compute(&expected, &actual);
        assert!((ts.sequence_accuracy - 1.0).abs() < f64::EPSILON);
        assert!((ts.tool_coverage - 1.0).abs() < f64::EPSILON);
        assert_eq!(ts.extra_calls, 0);
    }

    #[test]
    fn t_trajectory_score_empty_expected() {
        let expected: Vec<String> = vec![];
        let actual = vec!["shell".to_string()];
        let ts = TrajectoryScore::compute(&expected, &actual);
        assert!((ts.sequence_accuracy - 1.0).abs() < f64::EPSILON);
        assert!((ts.tool_coverage - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn t_trajectory_score_partial_match() {
        let expected = vec![
            "file_read".to_string(),
            "shell".to_string(),
            "file_write".to_string(),
        ];
        let actual = vec!["file_read".to_string(), "file_write".to_string()];
        let ts = TrajectoryScore::compute(&expected, &actual);
        assert!(ts.tool_coverage > 0.5);
        assert!(ts.tool_coverage < 1.0);
        assert!(ts.sequence_accuracy > 0.0);
    }

    #[test]
    fn t_trajectory_score_wrong_order() {
        let expected = vec!["file_read".to_string(), "shell".to_string()];
        let actual = vec!["shell".to_string(), "file_read".to_string()];
        let ts = TrajectoryScore::compute(&expected, &actual);
        assert!(ts.sequence_accuracy < 1.0, "逆順はsequence_accuracy < 1.0");
        assert!(
            (ts.tool_coverage - 1.0).abs() < f64::EPSILON,
            "ツールは全カバー"
        );
    }

    #[test]
    fn t_trajectory_score_extra_calls() {
        let expected = vec!["file_read".to_string()];
        let actual = vec![
            "shell".to_string(),
            "file_read".to_string(),
            "shell".to_string(),
        ];
        let ts = TrajectoryScore::compute(&expected, &actual);
        assert_eq!(ts.extra_calls, 2);
        assert!((ts.tool_coverage - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn t_trajectory_score_no_match() {
        let expected = vec!["file_read".to_string(), "git".to_string()];
        let actual = vec!["shell".to_string(), "web_fetch".to_string()];
        let ts = TrajectoryScore::compute(&expected, &actual);
        assert!((ts.tool_coverage - 0.0).abs() < f64::EPSILON);
        assert!((ts.sequence_accuracy - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn t_trajectory_composite_score() {
        let expected = vec!["file_read".to_string(), "shell".to_string()];
        let actual = vec!["file_read".to_string(), "shell".to_string()];
        let ts = TrajectoryScore::compute(&expected, &actual);
        let composite = ts.composite();
        assert!((composite - 1.0).abs() < f64::EPSILON, "完全一致=1.0");
    }

    #[test]
    fn t_trajectory_composite_with_extras() {
        let expected = vec!["file_read".to_string()];
        let actual = vec![
            "file_read".to_string(),
            "shell".to_string(),
            "shell".to_string(),
        ];
        let ts = TrajectoryScore::compute(&expected, &actual);
        let composite = ts.composite();
        assert!(composite < 1.0, "余分呼出でペナルティ");
        assert!(composite > 0.0);
    }

    // ========================================================================
    // Phase C スケルトン（TDD Red、Lab v12 完走後に Green へ）
    // 仕様: .claude/plan/phase-c-and-refactor-draft.md Part 1
    // 既存 22 タスク + 新規 18 タスク = 40 タスク（Smoke=5/Full=40 への TaskTag 化を想定）
    // 全テスト #[ignore]（CI 影響なし）、各タスク実装時に1件ずつ ignore 解除
    // ========================================================================

    fn assert_task_present(id: &str) {
        let suite = BenchmarkSuite::default_tasks();
        assert!(
            suite.tasks.iter().any(|t| t.id == id),
            "Phase C task '{}' は default_tasks() に未追加",
            id
        );
    }

    // --- MultiFileEdit (×2) -------------------------------------------------
    #[test]
    fn phase_c_rename_var_3files() {
        assert_task_present("rename_var_3files");
    }
    #[test]
    fn phase_c_sig_change_4files() {
        assert_task_present("sig_change_4files");
    }

    // --- LongRun (×2) -------------------------------------------------------
    #[test]
    fn phase_c_tool_chain_10steps() {
        assert_task_present("tool_chain_10steps");
    }
    #[test]
    fn phase_c_implement_50steps() {
        assert_task_present("implement_50steps");
    }

    // --- ToolChain (×2) -----------------------------------------------------
    #[test]
    fn phase_c_repomap_read_edit_test() {
        assert_task_present("repomap_read_edit_test");
    }
    #[test]
    fn phase_c_grep_multiedit() {
        assert_task_present("grep_multiedit");
    }

    // --- ErrorRecovery (×2) -------------------------------------------------
    #[test]
    fn phase_c_tool_fail_pivot() {
        assert_task_present("tool_fail_pivot");
    }
    #[test]
    fn phase_c_corrupt_file_repair() {
        assert_task_present("corrupt_file_repair");
    }

    // --- McpInteg (×2) ------------------------------------------------------
    #[test]
    fn phase_c_mcp_filesystem_list() {
        assert_task_present("mcp_filesystem_list");
    }
    #[test]
    fn phase_c_mcp_search_replace() {
        assert_task_present("mcp_search_replace");
    }

    // --- Semantic (×2) ------------------------------------------------------
    #[test]
    fn phase_c_vague_log_improve() {
        assert_task_present("vague_log_improve");
    }
    #[test]
    fn phase_c_refactor_intent() {
        assert_task_present("refactor_intent");
    }

    // --- Reasoning (×2) -----------------------------------------------------
    #[test]
    fn phase_c_nested_logic() {
        assert_task_present("nested_logic");
    }
    #[test]
    fn phase_c_ambiguous_calc() {
        assert_task_present("ambiguous_calc");
    }

    // --- Summarization (×2) -------------------------------------------------
    #[test]
    fn phase_c_multi_file_summary() {
        assert_task_present("multi_file_summary");
    }
    #[test]
    fn phase_c_git_log_summary() {
        assert_task_present("git_log_summary");
    }

    // --- Verification (×2) --------------------------------------------------
    #[test]
    fn phase_c_self_check_arithmetic() {
        assert_task_present("self_check_arithmetic");
    }
    #[test]
    fn phase_c_tool_fact_check() {
        assert_task_present("tool_fact_check");
    }
}
