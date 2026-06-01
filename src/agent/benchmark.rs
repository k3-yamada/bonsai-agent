#![allow(clippy::too_many_arguments)]
use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::agent::agent_loop::{AgentConfig, AgentLoopResult, run_agent_loop};
use crate::agent::t6_prompt_augment::augment_system_prompt;
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

/// AgentFloor 6-tier capability ladder (CLAUDE.md 項目 209、arxiv 2605.00334)。
///
/// 由来: "How Far Up the Tool Use Ladder Can Small Open-Weight Models Go?" (2026-05)。
/// 小型 open-weight モデル専用 benchmark の 6 段能力梯子を bonsai-agent Lab 評価軸に統合。
///
/// 直交軸: `TaskTier` (Core/Extended、項目 172) は「実装年代」、本 enum は「能力梯子」。
/// `BenchmarkTask::capability_tier` に tag 付与し `compute_capability_tier_avg()` で集計。
/// `agentfloor_tasks()` (30 task = 5/tier × 6 tier) で専用スイートを提供。
/// `is_ladder_mode_enabled()` / `BONSAI_BENCH_LADDER=1` env で切替 (既存 40 task と後方互換)。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, Hash)]
pub enum CapabilityTier {
    #[default]
    InstructionFollowing, // T1
    SingleToolUse,       // T2
    ToolSelection,       // T3
    MultiStepToolChain,  // T4
    ErrorRecovery,       // T5
    LongHorizonPlanning, // T6
}

impl CapabilityTier {
    pub fn label(self) -> &'static str {
        match self {
            Self::InstructionFollowing => "T1-Instruct",
            Self::SingleToolUse => "T2-SingleTool",
            Self::ToolSelection => "T3-ToolSelect",
            Self::MultiStepToolChain => "T4-MultiStep",
            Self::ErrorRecovery => "T5-ErrorRecov",
            Self::LongHorizonPlanning => "T6-LongHorizon",
        }
    }

    pub fn short_code(self) -> &'static str {
        match self {
            Self::InstructionFollowing => "t1i",
            Self::SingleToolUse => "t2s",
            Self::ToolSelection => "t3x",
            Self::MultiStepToolChain => "t4m",
            Self::ErrorRecovery => "t5e",
            Self::LongHorizonPlanning => "t6l",
        }
    }

    pub fn all() -> [CapabilityTier; 6] {
        [
            Self::InstructionFollowing,
            Self::SingleToolUse,
            Self::ToolSelection,
            Self::MultiStepToolChain,
            Self::ErrorRecovery,
            Self::LongHorizonPlanning,
        ]
    }

    pub fn paper_baseline(self) -> f64 {
        match self {
            Self::InstructionFollowing => 0.85,
            Self::SingleToolUse => 0.75,
            Self::ToolSelection => 0.65,
            Self::MultiStepToolChain => 0.50,
            Self::ErrorRecovery => 0.45,
            Self::LongHorizonPlanning => 0.30,
        }
    }
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
    /// AgentFloor capability tier (項目 213 候補、Phase 2 Green stub)。
    /// serde default で旧データ互換、未指定時は T1 (InstructionFollowing)。
    #[serde(default)]
    pub capability_tier: CapabilityTier,
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

/// 項目 200 (Beyond pass@1): RDC default = 1.0 (減衰なし扱い、旧データ後方互換)
fn default_reliability_decay() -> f64 {
    1.0
}

/// 項目 200: GDS default = 1.0 (全 pass 扱い、旧データ後方互換)
fn default_graceful_degradation() -> f64 {
    1.0
}

/// 項目 200: RDC (Reliability Decay) 計算。
///
/// iteration 比 (iter_used / budget) と (1 - score) の負相関を Pearson 相関で
/// 算出し、`1 - max(0, corr)` を返す (corr が高いほど減衰、RDC 低)。
///
/// - 完璧 (全 score=1.0、iter=0): RDC = 1.0
/// - 完全減衰 (iter↑ で score↓ 強い負相関): RDC < 0.5
/// - budget=0 or k<2 or 全 score 同値 or 全 iter 同値: RDC = 1.0 (相関未定義 → 減衰なし扱い)
fn compute_reliability_decay(
    scores: &[f64],
    iterations_used: &[usize],
    iteration_budget: usize,
) -> f64 {
    let k = scores.len();
    if k < 2 || iterations_used.len() != k || iteration_budget == 0 {
        return 1.0;
    }
    let budget = iteration_budget as f64;
    let xs: Vec<f64> = iterations_used.iter().map(|&u| u as f64 / budget).collect();
    let ys: Vec<f64> = scores.iter().map(|s| 1.0 - s).collect();
    let mean_x = xs.iter().sum::<f64>() / k as f64;
    let mean_y = ys.iter().sum::<f64>() / k as f64;
    let cov: f64 = xs
        .iter()
        .zip(ys.iter())
        .map(|(x, y)| (x - mean_x) * (y - mean_y))
        .sum();
    let var_x: f64 = xs.iter().map(|x| (x - mean_x).powi(2)).sum();
    let var_y: f64 = ys.iter().map(|y| (y - mean_y).powi(2)).sum();
    if var_x.abs() < 1e-10 || var_y.abs() < 1e-10 {
        return 1.0;
    }
    let corr = cov / (var_x * var_y).sqrt();
    // corr in [-1, 1]、正値 = decay (iter↑→1-score↑ → score↓)
    1.0 - corr.clamp(0.0, 1.0)
}

/// 項目 200: GDS (Graceful Degradation Score) 計算。
///
/// 失敗 run (score < pass_threshold) の score を pass_threshold で正規化した
/// 近接度の平均を返す。全 pass / threshold==0 の場合は 1.0。
fn compute_graceful_degradation(scores: &[f64], pass_threshold: f64) -> f64 {
    if pass_threshold <= 0.0 {
        return 1.0;
    }
    let failures: Vec<f64> = scores
        .iter()
        .copied()
        .filter(|&s| s < pass_threshold)
        .collect();
    if failures.is_empty() {
        return 1.0;
    }
    let avg: f64 = failures.iter().map(|s| s / pass_threshold).sum::<f64>() / failures.len() as f64;
    avg.clamp(0.0, 1.0)
}

/// 項目 225 (arxiv 2604.14877): PASS@(k, T_steps) の計算。
/// k 回中 (score >= pass_threshold && iter <= T) を満たした割合を閾値ごとに返す。
/// `scores` と `iterations_used` の長さが不一致 / k=0 の場合は空 Vec。
fn compute_pass_at_k_t_steps(
    scores: &[f64],
    iterations_used: &[usize],
    pass_threshold: f64,
    thresholds: &[usize],
) -> Vec<(usize, f64)> {
    let k = scores.len();
    if k == 0 || iterations_used.len() != k {
        return Vec::new();
    }

    thresholds
        .iter()
        .map(|&t| {
            let pass_count = scores
                .iter()
                .zip(iterations_used.iter())
                .filter(|&(&score, &iter)| score >= pass_threshold && iter <= t)
                .count();
            (t, pass_count as f64 / k as f64)
        })
        .collect()
}

/// 項目 225 (arxiv 2604.14877): PASS@(k, T_seconds) の計算。
/// k 回中 (score >= pass_threshold && duration <= T) を満たした割合を閾値ごとに返す。
/// `scores` と `durations_secs` の長さが不一致 / k=0 の場合は空 Vec。
///
/// 非有限値 (NaN / ±Inf) の閾値は除外する: `serde_json` は非有限 f64 を encode 不可
/// なため、persistence 経路 (save_to_db / append_tsv) で実験全体が失敗するのを防ぐ。
fn compute_pass_at_k_t_seconds(
    scores: &[f64],
    durations_secs: &[f64],
    pass_threshold: f64,
    thresholds: &[f64],
) -> Vec<(f64, f64)> {
    let k = scores.len();
    if k == 0 || durations_secs.len() != k {
        return Vec::new();
    }

    thresholds
        .iter()
        .copied()
        .filter(|t| t.is_finite())
        .map(|t| {
            let pass_count = scores
                .iter()
                .zip(durations_secs.iter())
                .filter(|&(&score, &duration)| score >= pass_threshold && duration <= t)
                .count();
            (t, pass_count as f64 / k as f64)
        })
        .collect()
}

/// 項目 225: env `BONSAI_PASS_K_T_STEPS` から T_steps 閾値列を解析。
/// 例: `"3,5,7"` → `vec![3, 5, 7]`。未指定 / 解析失敗 → 空 Vec。
fn parse_t_steps_env() -> Vec<usize> {
    std::env::var("BONSAI_PASS_K_T_STEPS")
        .ok()
        .map(|s| {
            s.split(',')
                .filter_map(|part| part.trim().parse::<usize>().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// 項目 225: env `BONSAI_PASS_K_T_SECONDS` から T_seconds 閾値列を解析。
/// 例: `"60,180,600"` → `vec![60.0, 180.0, 600.0]`。負値 / 0 はフィルタで除外。
/// 非有限値 (NaN / ±Inf) も `is_finite()` で除外する: `BONSAI_PASS_K_T_SECONDS=60,inf`
/// 等の不正入力で persistence 経路が失敗しないよう防御 (Codex audit MEDIUM finding)。
fn parse_t_seconds_env() -> Vec<f64> {
    std::env::var("BONSAI_PASS_K_T_SECONDS")
        .ok()
        .map(|s| {
            s.split(',')
                .filter_map(|part| part.trim().parse::<f64>().ok())
                .filter(|value| value.is_finite() && *value > 0.0)
                .collect()
        })
        .unwrap_or_default()
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
    /// RDC スカラー値 [0, 1]（項目 200、Beyond pass@1）。
    /// 1.0 = 減衰なし、0.0 = 完全減衰。`from_scores_with_metrics` で計算。
    #[serde(default = "default_reliability_decay")]
    pub reliability_decay: f64,
    /// GDS スカラー値 [0, 1]（項目 200、Beyond pass@1）。
    /// 1.0 = 全 pass or 失敗時も threshold 近く、0.0 = 完全失敗。
    #[serde(default = "default_graceful_degradation")]
    pub graceful_degradation: f64,
    /// 項目 225 (arxiv 2604.14877): step 軸 PASS@(k, T_steps) 列。
    /// 各要素は `(T_steps, pass_rate)`。env `BONSAI_PASS_K_T_STEPS` 未指定時は
    /// 空 Vec で既存挙動を維持する。
    #[serde(default)]
    pub pass_at_k_t_steps: Vec<(usize, f64)>,
    /// 項目 225 (arxiv 2604.14877): 時間軸 PASS@(k, T_seconds) 列。
    /// 各要素は `(T_seconds, pass_rate)`。env `BONSAI_PASS_K_T_SECONDS` 未指定時は
    /// 空 Vec で既存挙動を維持する。
    #[serde(default)]
    pub pass_at_k_t_seconds: Vec<(f64, f64)>,
    /// Frontier benchmark (`frontier-benchmark-impl.md`、antirez/ds4 ds4-bench inspired):
    /// k 回 run の平均 iteration を context-token proxy として換算した推定値
    /// (`iter_mean × TOKENS_PER_ITERATION_ESTIMATE`)。bucket 振り分けの軸として
    /// [`MultiRunBenchmarkResult::composite_frontier_bucket_scores`] が使用する。
    /// run_k で populate されない経路 (legacy fixture / Phase 4 G-4a 等) では `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_context_tokens: Option<usize>,
    /// Sub-Phase 2F: T6-LongHorizon に filler context を inject した variant の (size_kb, mean_score) 列。
    /// `BONSAI_FRONTIER_INJECT_ENABLED=1` AND `capability_tier == LongHorizonPlanning` のとき
    /// run_k が populate する。非 T6 / env 未設定 / 非 LongHorizon タスクでは空 Vec のまま。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frontier_inject_scores: Vec<(usize, f64)>,
}

/// `MultiRunTaskScore::final_context_tokens` 推定用、1 iteration あたりの平均 token 数。
///
/// 実測値は llama-server の `n_tokens` API から取得できるが Phase 2C 時点では HTTP backend
/// に signature 追加を行わない (Sub-Phase 2C scope を絞る)。本定数は roughly:
///
/// - system prompt + tool schema (Deferred): ~600 token
/// - 1 step あたり LLM 入力 (history) + 出力 (think + tool_call/answer): ~500 token
/// - 1 step あたり tool 実行結果: ~200-1000 token (file ops で変動大)
///
/// 平均化して 1024 を採用 (項目 167 llama-server backend、`-c 16384` 16K context window)。
const TOKENS_PER_ITERATION_ESTIMATE: usize = 1024;

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
                reliability_decay: 1.0,
                graceful_degradation: 1.0,
                pass_at_k_t_steps: Vec::new(),
                pass_at_k_t_seconds: Vec::new(),
                final_context_tokens: None,
                frontier_inject_scores: Vec::new(),
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
            // 項目 200: legacy from_scores は信頼性軸を計算しない (default = 1.0)。
            // RDC/GDS を計算する経路は from_scores_with_metrics を使う。
            reliability_decay: 1.0,
            graceful_degradation: 1.0,
            // 項目 225: PASS@(k,T) は v2 経路 (from_scores_with_metrics_v2) のみで設定。
            pass_at_k_t_steps: Vec::new(),
            pass_at_k_t_seconds: Vec::new(),
            // Sub-Phase 2C: final_context_tokens は from_scores 経路では None (iteration 情報なし)。
            // run_k 経路では with_final_context_tokens(...) で後付け populate される。
            final_context_tokens: None,
            // Sub-Phase 2F: frontier_inject_scores は from_scores 経路では空。
            // run_k 経路で T6 task + env enabled のとき populate される。
            frontier_inject_scores: Vec::new(),
        }
    }

    /// k 個 scores + iteration 情報から RDC/GDS 込みのフル MultiRunTaskScore を構築
    /// （項目 200、Beyond pass@1）
    ///
    /// `iterations_used` は scores と同じ長さで各 run の使用 iteration 数。
    /// `iteration_budget` は task の `max_iterations`。
    ///
    /// **RDC (Reliability Decay)**: iteration 比 (iter/budget) と (1 - score) の
    /// 負相関を proxy とする。論文式 (survival function) は取得待ちのため近似。
    /// budget=0 or k<2 の特殊ケースは 1.0 (減衰なし扱い)。
    ///
    /// **GDS (Graceful Degradation)**: 失敗 run の score を pass_threshold で正規化した
    /// 平均近接度。全 pass なら 1.0、threshold=0 の場合も 1.0。
    pub fn from_scores_with_metrics(
        task_id: String,
        scores: Vec<f64>,
        iterations_used: Vec<usize>,
        iteration_budget: usize,
        pass_threshold: f64,
    ) -> Self {
        let mut score = Self::from_scores(task_id, scores.clone(), pass_threshold);
        score.reliability_decay =
            compute_reliability_decay(&scores, &iterations_used, iteration_budget);
        score.graceful_degradation = compute_graceful_degradation(&scores, pass_threshold);
        score
    }

    /// 項目 225 (arxiv 2604.14877): PASS@(k,T) を含むフル指標版。
    ///
    /// 既存 `from_scores_with_metrics` (v1) に T 軸 2 種 (steps / seconds) を追加。
    /// v1 signature を維持するため新規メソッドとして公開し、既存呼出側 (4 test fixture)
    /// は無変更で動作する。env `BONSAI_PASS_K_T_STEPS` / `BONSAI_PASS_K_T_SECONDS`
    /// 未指定時は `t_*_thresholds` を空 slice で渡すことで既存挙動を維持。
    ///
    /// - `durations_secs`: 各 run の wallclock 秒数 (失敗 run も elapsed を含む)
    /// - `t_steps_thresholds`: step 軸閾値 (例: `&[3, 5, 7]`)
    /// - `t_seconds_thresholds`: 時間軸閾値 (例: `&[60.0, 180.0, 600.0]`)
    pub fn from_scores_with_metrics_v2(
        task_id: String,
        scores: Vec<f64>,
        iterations_used: Vec<usize>,
        durations_secs: Vec<f64>,
        iteration_budget: usize,
        pass_threshold: f64,
        t_steps_thresholds: &[usize],
        t_seconds_thresholds: &[f64],
    ) -> Self {
        let mut score = Self::from_scores_with_metrics(
            task_id,
            scores.clone(),
            iterations_used.clone(),
            iteration_budget,
            pass_threshold,
        );
        score.pass_at_k_t_steps = compute_pass_at_k_t_steps(
            &scores,
            &iterations_used,
            pass_threshold,
            t_steps_thresholds,
        );
        score.pass_at_k_t_seconds = compute_pass_at_k_t_seconds(
            &scores,
            &durations_secs,
            pass_threshold,
            t_seconds_thresholds,
        );
        score
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

/// G1 Critic 別 LLM 分離の informational metric。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CriticStats {
    pub critic_calls: usize,
    pub agree_count: usize,
    pub disagree_count: usize,
    pub uncertain_count: usize,
    pub skipped_count: usize,
    pub backend_error_count: usize,
}

impl CriticStats {
    pub fn agreement_rate(&self) -> Option<f64> {
        let total = self.agree_count + self.disagree_count + self.uncertain_count;
        if total == 0 {
            None
        } else {
            Some(self.agree_count as f64 / total as f64)
        }
    }

    pub fn disagreement_rate(&self) -> Option<f64> {
        let total = self.agree_count + self.disagree_count + self.uncertain_count;
        if total == 0 {
            None
        } else {
            Some(self.disagree_count as f64 / total as f64)
        }
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
    /// AgentFloor 6-tier 別平均 mean_score (Phase 3 Refactor、arxiv 2605.00334)。
    ///
    /// 添字は `CapabilityTier::all()` 順 (T1=0 .. T6=5)。
    /// 該当 tier のタスクがない場合は `None`。agentfloor_tasks() 非使用時も `None`。
    /// serde default で旧データとの互換を保つ (旧 JSON に列がなければ None)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_avg_scores: Option<[Option<f64>; 6]>,
    /// G1 Critic 呼出の副次集計。Phase 1 では hook 定義のみで run_k 配線は行わない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critic_stats: Option<CriticStats>,
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

/// CapabilityTier 別平均 mean_score を算出 (CLAUDE.md 項目 209、arxiv 2605.00334)。
///
/// AgentFloor 6-tier capability ladder の各 tier について bonsai-8B の平均スコアを計算。
/// `task_scores` の各 score について、`task_descs` から `capability_tier` を引いて
/// 指定 tier に一致する task の mean_score 平均を返す。該当 tier の task が 1 件も
/// ないなら None を返す。
///
/// 結果は `MultiRunBenchmarkResult::tier_avg_scores` の各スロットに格納し、
/// `weakest_tier()` / `paper_delta_map()` で「攻めるべき tier」を特定するために使用する。
#[allow(dead_code)]
pub(crate) fn compute_capability_tier_avg(
    task_scores: &[MultiRunTaskScore],
    task_descs: &std::collections::HashMap<String, BenchmarkTask>,
    tier: CapabilityTier,
) -> Option<f64> {
    let scores: Vec<f64> = task_scores
        .iter()
        .filter(|s| {
            task_descs
                .get(&s.task_id)
                .map(|t| t.capability_tier == tier)
                .unwrap_or(false)
        })
        .map(|s| s.mean_score)
        .collect();
    if scores.is_empty() {
        None
    } else {
        Some(scores.iter().sum::<f64>() / scores.len() as f64)
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

    /// 全タスクの平均 reliability_decay（項目 200、Beyond pass@1）。
    /// task_scores が空なら 1.0 (減衰なし扱い)。
    pub fn composite_reliability_decay(&self) -> f64 {
        if self.task_scores.is_empty() {
            return 1.0;
        }
        let sum: f64 = self.task_scores.iter().map(|s| s.reliability_decay).sum();
        sum / self.task_scores.len() as f64
    }

    /// baseline result との variance 比 (VAF: Variance Amplification Factor、項目 200)。
    /// `baseline.mean_variance() == 0` なら None。
    /// 1.0 = 不変 / >1.0 = 不安定化 / <1.0 = 安定化。
    pub fn variance_amplification_vs(&self, baseline: &Self) -> Option<f64> {
        let bv = baseline.mean_variance();
        if bv.abs() < 1e-10 {
            return None;
        }
        Some(self.mean_variance() / bv)
    }

    /// 全タスクの平均 graceful_degradation（項目 200、Beyond pass@1）。
    /// task_scores が空なら 1.0 (全 pass 扱い)。
    pub fn composite_graceful_degradation(&self) -> f64 {
        if self.task_scores.is_empty() {
            return 1.0;
        }
        let sum: f64 = self
            .task_scores
            .iter()
            .map(|s| s.graceful_degradation)
            .sum();
        sum / self.task_scores.len() as f64
    }

    /// 項目 225 (arxiv 2604.14877): 全タスク平均 PASS@(k, T_steps) を閾値ごとに集計。
    /// 同一 `T_steps` 値の `pass_rate` を task 間で算術平均。`BTreeMap` で順序を
    /// 保証 (log 出力 / TSV の deterministic 順序のため)。
    /// task_scores が空 / 各 task の `pass_at_k_t_steps` が空なら空 Vec。
    pub fn composite_pass_at_k_t_steps(&self) -> Vec<(usize, f64)> {
        if self.task_scores.is_empty() {
            return Vec::new();
        }

        let mut acc: std::collections::BTreeMap<usize, (f64, usize)> =
            std::collections::BTreeMap::new();
        for task_score in &self.task_scores {
            for &(threshold, rate) in &task_score.pass_at_k_t_steps {
                let entry = acc.entry(threshold).or_insert((0.0, 0));
                entry.0 += rate;
                entry.1 += 1;
            }
        }

        acc.into_iter()
            .map(|(threshold, (sum, count))| (threshold, sum / count as f64))
            .collect()
    }

    /// Frontier benchmark (Sub-Phase 2F): T6-LongHorizon inject variant の (size_kb, mean_score) 列を
    /// 全 T6 タスクで集約。`PASS@(k, T_steps)` と同 pattern で `BTreeMap` 経由 deterministic 順序。
    /// run_k で populate されていない (env unset / 非 T6) タスクは集計対象外。
    /// 戻り値: `Vec<(size_kb, mean_score)>` (size_kb 昇順)、全 task が空なら空 Vec。
    pub fn composite_frontier_inject_scores(&self) -> Vec<(usize, f64)> {
        if self.task_scores.is_empty() {
            return Vec::new();
        }
        let mut acc: std::collections::BTreeMap<usize, (f64, usize)> =
            std::collections::BTreeMap::new();
        for task_score in &self.task_scores {
            for &(size_kb, mean) in &task_score.frontier_inject_scores {
                let entry = acc.entry(size_kb).or_insert((0.0, 0));
                entry.0 += mean;
                entry.1 += 1;
            }
        }
        acc.into_iter()
            .map(|(size_kb, (sum, count))| (size_kb, sum / count as f64))
            .collect()
    }

    /// Frontier benchmark (Sub-Phase 2C、`frontier-benchmark-impl.md`、antirez/ds4 inspired):
    /// 各タスクの `final_context_tokens` を bucket に振り分け、bucket 別 mean score を集計する。
    /// `boundaries` は [`crate::agent::frontier::parse_frontier_buckets_env`] と同 contract
    /// (昇順 sort 済 / 重複なし)。`final_context_tokens` が `None` のタスクは集計対象外。
    /// 戻り値: `Vec<(bucket_index, mean_score)>` (bucket index 昇順)、空 boundaries / 全 task が
    /// `None` 経路で空 Vec。
    pub fn composite_frontier_bucket_scores(&self, boundaries: &[usize]) -> Vec<(usize, f64)> {
        if boundaries.is_empty() || self.task_scores.is_empty() {
            return Vec::new();
        }
        let pairs: Vec<(usize, f64)> = self
            .task_scores
            .iter()
            .filter_map(|s| s.final_context_tokens.map(|t| (t, s.mean_score)))
            .collect();
        if pairs.is_empty() {
            return Vec::new();
        }
        crate::agent::frontier::compute_frontier_bucket_scores(&pairs, boundaries)
            .into_iter()
            .collect()
    }

    /// 項目 225 (arxiv 2604.14877): 全タスク平均 PASS@(k, T_seconds) を閾値ごとに集計。
    /// f64 キーは `BTreeMap` 不可のため `1e-6` epsilon バケットで集約し、末尾で
    /// `T_seconds` 昇順に sort する (log 出力の deterministic 順序のため)。
    pub fn composite_pass_at_k_t_seconds(&self) -> Vec<(f64, f64)> {
        if self.task_scores.is_empty() {
            return Vec::new();
        }

        let mut buckets: Vec<(f64, f64, usize)> = Vec::new();
        for task_score in &self.task_scores {
            for &(threshold, rate) in &task_score.pass_at_k_t_seconds {
                if let Some(bucket) = buckets
                    .iter_mut()
                    .find(|bucket| (bucket.0 - threshold).abs() < 1e-6)
                {
                    bucket.1 += rate;
                    bucket.2 += 1;
                } else {
                    buckets.push((threshold, rate, 1));
                }
            }
        }

        buckets.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        buckets
            .into_iter()
            .map(|(threshold, sum, count)| (threshold, sum / count as f64))
            .collect()
    }

    /// 全 6 tier の中で平均スコアが最低の tier を返す (AgentFloor Phase 3、arxiv 2605.00334)。
    ///
    /// `tier_avg_scores` が None / 全 None の場合は `None` を返す。
    /// tie 時は T1→T6 順で最初の最低値を返す (plan §5 Phase 3 仕様)。
    pub fn weakest_tier(&self) -> Option<(CapabilityTier, f64)> {
        let scores = self.tier_avg_scores.as_ref()?;
        let tiers = CapabilityTier::all();
        tiers
            .iter()
            .enumerate()
            .filter_map(|(i, &tier)| scores[i].map(|s| (tier, s)))
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    }

    /// 各 tier について `bonsai_avg - paper_baseline` を返す (AgentFloor Phase 3、arxiv 2605.00334)。
    ///
    /// 添字は `CapabilityTier::all()` 順 (T1=0 .. T6=5)。
    /// `tier_avg_scores` が None、または該当 tier が None の場合は `None`。
    /// 負値 = bonsai が paper 未満 = 攻めるべき tier を示す。
    pub fn paper_delta_map(&self) -> [Option<f64>; 6] {
        let mut out = [None; 6];
        if let Some(scores) = &self.tier_avg_scores {
            for (i, &tier) in CapabilityTier::all().iter().enumerate() {
                out[i] = scores[i].map(|s| s - tier.paper_baseline());
            }
        }
        out
    }
}

/// `BONSAI_BENCH_LADDER=1` が設定されているか確認する (AgentFloor Phase 3、arxiv 2605.00334)。
///
/// 設定されている場合は `agentfloor_tasks()` (30 task) を使用し、
/// 未設定 (default) は既存 `default_tasks()` (40 task) を維持する (後方互換)。
/// Cerememory 三本柱 (`BONSAI_DECAY_ENABLED` 等) と同パターン。
pub fn is_ladder_mode_enabled() -> bool {
    std::env::var("BONSAI_BENCH_LADDER").is_ok_and(|v| v == "1")
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
    // 項目 261 T6 案 A Phase 4 wiring follow-up (2026-05-22): SMOKE_TASK_IDS 先頭に T6 5 task 追加。
    // BONSAI_LAB_SMOKE=1 + BONSAI_LAB_TASK_LIMIT=5 で T6 のみ smoke を可能化、
    // BONSAI_T6_PROMPT_AUGMENT=1 の G-T6-1/G-T6-2 paired 計測 hook 点。
    "lh_plan_refactor_5files", // T6 LongHorizonPlanning: RepoMap → file_read × 3+ → リファクタ計画
    "lh_test_red_green",       // T6: benchmark.rs 読取 → test 2 件提案
    "lh_dependency_chain",     // T6: mod.rs 読取 → 各 module file_read → 依存グラフ
    "lh_plan_then_revise",     // T6: Cargo.toml 読取 → 計画 → リスク改訂
    "lh_multi_modal_audit",    // T6: shell(git log) + ファイル数 + Cargo.toml
    "file_read_simple",        // ToolUse: 単純ファイル読み取り
    "multi_step_write_read",   // MultiStep: 書込→読込
    "error_recovery",          // ErrorRecovery: エラー後の代替試行
    "tool_selection_git",      // ToolSelection: 適切なツール選択
    "code_gen_fizzbuzz",       // CodeGeneration: コード生成
    "smoke_failure_chain_pair", // ErrorRecovery (handoff 05-07g Phase 5 Phase 4): 2 step + 全 fail で AgentHER failed パス検証
    "smoke_partial_success_chain", // ErrorRecovery (handoff 05-07h 後継): 1 success + 1 fail = HSL relabel 候補 (relabels>=1 実証用)
    "halluc_parent_of_false_fact", // Plan A G-4c (T1): KG seed と矛盾する parent_of 出力で Conflict 発火
    "halluc_is_a_false_type",      // Plan A G-4c (T1): is_a 型分類で Conflict 発火
    "halluc_t2_file_context_misalign", // Plan A G-4c (T2): file context vs LLM 出力の不整合検出
    // 項目 242 Lab v21 補完 (Phase 4 G-7b smoke で matched>=1 確証用):
    // success_fact 5 task は default_tasks に追加済だが、SMOKE_TASK_IDS にも含めることで
    // BONSAI_LAB_SMOKE=1 smoke run でも seed_kg_for_factcheck_lab の match 経路を発火可能化。
    "success_bonsai_is_a_rust_project", // Pattern 2 is_a (正解誘導)
    "success_llama_runtime_of_bonsai",  // Pattern 1 runtime_of (正解誘導)
    "success_sqlite_storage_of_bonsai", // Pattern 1 storage_of (正解誘導)
    "success_reflexion_loop_of_bonsai", // Pattern 1 loop_of (正解誘導)
    "success_pathguard_sandbox_of_bonsai", // Pattern 1 sandbox_of dash subject (正解誘導)
];

/// halluc_t2_file_context_misalign task 用の file fixture を準備 (Plan A G-4c)。
///
/// `/tmp/bonsai_halluc_ctx.txt` に `"bonsai-agent is the child of bonsai-8B"` を書く。
/// 既存なら skip (冪等)。失敗は呼出側で silent 握り潰し (non-fatal、halluc_t2 のみ影響)。
/// `factcheck::seed_kg_for_factcheck_lab` で seed する KG fact と integrity 一致
/// (G-4c v1 反証受けて大文字始まり化、Pattern 1 regex match 経路確保):
///   - file 内: Bonsai-Agent is the child of Bonsai-8B
///   - KG seed: (Bonsai-Agent, child_of, Bonsai-8B)
///
/// LLM が file context を正しく抽出すれば `Match`、捏造すれば `Conflict`。
/// 既存 file は冪等再 write しないため、`/tmp/bonsai_halluc_ctx.txt` に旧小文字版が
/// 残っている環境では事前に手動削除するか reboot で /tmp clear する必要がある。
fn setup_halluc_fixtures() -> std::io::Result<()> {
    let path = std::path::Path::new("/tmp/bonsai_halluc_ctx.txt");
    let expected = "Bonsai-Agent is the child of Bonsai-8B\n";
    // 既存 content が新版と異なる場合は overwrite (大文字始まり化 v2 への migrate)
    let needs_write = match std::fs::read_to_string(path) {
        Ok(current) => current != expected,
        Err(_) => true,
    };
    if needs_write {
        std::fs::write(path, expected)?;
    }
    Ok(())
}

impl BenchmarkSuite {
    /// 開発時用の smoke タスクセット (15 タスク、項目 261 T6 案 A で 10→15 拡張)。
    ///
    /// `default_tasks()` + `agentfloor_tasks()` の集合から `SMOKE_TASK_IDS` に一致するものを抽出、
    /// **SMOKE_TASK_IDS の順序を保持**して返す (TASK_LIMIT=N が SMOKE_TASK_IDS 先頭から
    /// N 件を選択する挙動を保証、項目 261 T6 5 task が先頭のため LIMIT=5 で T6-only smoke)。
    /// CI/Lab 本番では `default_tasks()` (40 タスク) を使い、開発時の高速確認に
    /// 限定して `smoke_tasks()` を使う。
    pub fn smoke_tasks() -> Self {
        let mut all = Self::default_tasks().tasks;
        all.extend(Self::agentfloor_tasks().tasks);
        Self {
            tasks: SMOKE_TASK_IDS
                .iter()
                .filter_map(|id| all.iter().find(|t| t.id.as_str() == *id).cloned())
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

    /// AgentFloor 専用 30 task suite (CLAUDE.md 項目 209、arxiv 2605.00334)。
    ///
    /// 由来: "How Far Up the Tool Use Ladder Can Small Open-Weight Models Go?" (2026-05)。
    /// 6 tier × 5 task = 30 task。既存 `default_tasks()` から各 tier 代表を厳選し、
    /// T6 (LongHorizonPlanning) 5 task は新規追加。
    ///
    /// `BONSAI_BENCH_LADDER=1` env (`is_ladder_mode_enabled()`) で本スイートを使用。
    /// 未設定 (default) は既存 `default_tasks()` (40 task) 維持 (後方互換)。
    /// tier 別集計は `compute_capability_tier_avg()` / `MultiRunBenchmarkResult::tier_avg_scores`
    /// に格納し、`weakest_tier()` / `paper_delta_map()` で分析する。
    pub fn agentfloor_tasks() -> Self {
        Self {
            tasks: vec![
                // ── T1 InstructionFollowing (5 task) ─────────────────────────
                BenchmarkTask {
                    id: "af_t1_direct_answer".into(),
                    name: "AF T1: 直接回答".into(),
                    input: "Rustのマスコットの名前は？".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["Ferris".into()],
                    max_iterations: 2,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::InstructionFollowing,
                },
                BenchmarkTask {
                    id: "af_t1_reasoning_calc".into(),
                    name: "AF T1: 計算推論".into(),
                    input: "2の10乗はいくつですか".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["1024".into()],
                    max_iterations: 3,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::InstructionFollowing,
                },
                BenchmarkTask {
                    id: "af_t1_code_gen_fizzbuzz".into(),
                    name: "AF T1: コード生成".into(),
                    input: "FizzBuzzをRustで書いて。1から15までの出力例も示して".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["fizz".into(), "buzz".into(), "fizzbuzz".into()],
                    max_iterations: 3,
                    category: TaskCategory::CodeGeneration,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::InstructionFollowing,
                },
                BenchmarkTask {
                    id: "af_t1_json_parse".into(),
                    name: "AF T1: JSON解析推論".into(),
                    input: r#"次のJSONから"name"フィールドの値を教えて: {"id": 1, "name": "bonsai", "version": "0.1"}"#.into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["bonsai".into()],
                    max_iterations: 3,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::InstructionFollowing,
                },
                BenchmarkTask {
                    id: "af_t1_nested_logic".into(),
                    name: "AF T1: ネスト論理式".into(),
                    input: "x=3, y=5 のとき式 `x > y && (x + y) % 2 == 0` の真偽は何か。理由とともに答えて".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["false".into(), "偽".into()],
                    max_iterations: 3,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Extended,
                    capability_tier: CapabilityTier::InstructionFollowing,
                },
                // ── T2 SingleToolUse (5 task) ─────────────────────────────────
                BenchmarkTask {
                    id: "af_t2_file_read".into(),
                    name: "AF T2: ファイル読み取り".into(),
                    input: "README.mdの内容を教えて".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec!["README".into()],
                    max_iterations: 3,
                    category: TaskCategory::ToolUse,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::SingleToolUse,
                },
                BenchmarkTask {
                    id: "af_t2_shell_ls".into(),
                    name: "AF T2: ファイル一覧".into(),
                    input: "このディレクトリのファイル一覧を表示して".into(),
                    expected_tools: vec!["shell".into()],
                    expected_keywords: vec!["src".into()],
                    max_iterations: 3,
                    category: TaskCategory::ToolUse,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::SingleToolUse,
                },
                BenchmarkTask {
                    id: "af_t2_git_status".into(),
                    name: "AF T2: Git状態確認".into(),
                    input: "Gitの状態を確認して".into(),
                    expected_tools: vec!["git".into()],
                    expected_keywords: vec!["branch".into()],
                    max_iterations: 3,
                    category: TaskCategory::ToolUse,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::SingleToolUse,
                },
                BenchmarkTask {
                    id: "af_t2_summarize".into(),
                    name: "AF T2: ファイル要約".into(),
                    input: "src/agent/agent_loop.rsの最初の50行を要約して".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec![],
                    max_iterations: 4,
                    category: TaskCategory::Summarization,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::SingleToolUse,
                },
                BenchmarkTask {
                    id: "af_t2_git_diff".into(),
                    name: "AF T2: Git差分分析".into(),
                    input: "最後のコミットで何が変更されたか確認して".into(),
                    expected_tools: vec!["shell".into()],
                    expected_keywords: vec!["diff".into(), "commit".into(), "changed".into()],
                    max_iterations: 4,
                    category: TaskCategory::ToolUse,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::SingleToolUse,
                },
                // ── T3 ToolSelection (5 task) ─────────────────────────────────
                BenchmarkTask {
                    id: "af_t3_tool_selection_git".into(),
                    name: "AF T3: ツール選択".into(),
                    input: "このプロジェクトのGitログを見せて".into(),
                    expected_tools: vec!["git".into()],
                    expected_keywords: vec!["commit".into()],
                    max_iterations: 3,
                    category: TaskCategory::ToolSelection,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::ToolSelection,
                },
                BenchmarkTask {
                    id: "af_t3_repo_structure".into(),
                    name: "AF T3: リポジトリ構造把握".into(),
                    input: "このプロジェクトのsrc/ディレクトリにあるRustファイルの数を教えて".into(),
                    expected_tools: vec!["repo_map".into()],
                    expected_keywords: vec![],
                    max_iterations: 3,
                    category: TaskCategory::ToolUse,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::ToolSelection,
                },
                BenchmarkTask {
                    id: "af_t3_tool_fact_check".into(),
                    name: "AF T3: 事実確認".into(),
                    input: "現在のディレクトリに `Cargo.toml` が存在するかツールで確認して".into(),
                    expected_tools: vec!["file_read".into(), "shell".into()],
                    expected_keywords: vec!["Cargo.toml".into(), "存在".into()],
                    max_iterations: 4,
                    category: TaskCategory::ToolUse,
                    tier: TaskTier::Extended,
                    capability_tier: CapabilityTier::ToolSelection,
                },
                BenchmarkTask {
                    id: "af_t3_code_review".into(),
                    name: "AF T3: コードレビュー".into(),
                    input: "src/tools/file.rsのコードを読んで、改善点があれば指摘して".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec![],
                    max_iterations: 4,
                    category: TaskCategory::Summarization,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::ToolSelection,
                },
                BenchmarkTask {
                    id: "af_t3_git_log_summary".into(),
                    name: "AF T3: git ログ要約".into(),
                    input: "直近 5 コミットの「変更概要 + 影響範囲」を表形式で要約して".into(),
                    expected_tools: vec!["shell".into()],
                    expected_keywords: vec!["commit".into(), "要約".into()],
                    max_iterations: 5,
                    category: TaskCategory::Summarization,
                    tier: TaskTier::Extended,
                    capability_tier: CapabilityTier::ToolSelection,
                },
                // ── T4 MultiStepToolChain (5 task) ───────────────────────────
                BenchmarkTask {
                    id: "af_t4_write_read".into(),
                    name: "AF T4: 書き込み→読み返し".into(),
                    input: "hello.txtに'Hello World'と書いて、それを読み返して".into(),
                    expected_tools: vec!["file_write".into(), "file_read".into()],
                    expected_keywords: vec!["Hello World".into()],
                    max_iterations: 5,
                    category: TaskCategory::MultiStep,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::MultiStepToolChain,
                },
                BenchmarkTask {
                    id: "af_t4_multi_file_compare".into(),
                    name: "AF T4: 複数ファイル比較".into(),
                    input: "src/tools/file.rsとsrc/tools/shell.rsの行数をそれぞれ教えて".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec![],
                    max_iterations: 5,
                    category: TaskCategory::MultiStep,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::MultiStepToolChain,
                },
                BenchmarkTask {
                    id: "af_t4_conditional_file_op".into(),
                    name: "AF T4: 条件付きファイル操作".into(),
                    input: "/tmp/bonsai_bench_test.txt が存在するか確認し、存在しなければ'benchmark ok'と書き込んで".into(),
                    expected_tools: vec!["file_read".into(), "file_write".into()],
                    expected_keywords: vec!["benchmark".into()],
                    max_iterations: 5,
                    category: TaskCategory::MultiStep,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::MultiStepToolChain,
                },
                BenchmarkTask {
                    id: "af_t4_multi_file_summary".into(),
                    name: "AF T4: 複数ファイル役割要約".into(),
                    input: "src/agent/agent_loop.rs と src/agent/benchmark.rs の役割の違いを 200 字以内で要約して".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec!["agent_loop".into(), "benchmark".into()],
                    max_iterations: 5,
                    category: TaskCategory::Summarization,
                    tier: TaskTier::Extended,
                    capability_tier: CapabilityTier::MultiStepToolChain,
                },
                BenchmarkTask {
                    id: "af_t4_multi_file_search".into(),
                    name: "AF T4: 複数ファイル検索".into(),
                    input: "src/配下で'run_agent_loop'関数が定義されているファイルを特定して".into(),
                    expected_tools: vec!["shell".into(), "file_read".into()],
                    expected_keywords: vec!["found".into(), "file".into()],
                    max_iterations: 5,
                    category: TaskCategory::MultiStep,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::MultiStepToolChain,
                },
                // ── T5 ErrorRecovery (5 task) ─────────────────────────────────
                BenchmarkTask {
                    id: "af_t5_error_recovery".into(),
                    name: "AF T5: エラー回復".into(),
                    input: "存在しないファイル /tmp/bonsai_nonexistent_test.txt を読んで".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec!["存在".into()],
                    max_iterations: 3,
                    category: TaskCategory::ErrorRecovery,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::ErrorRecovery,
                },
                BenchmarkTask {
                    id: "af_t5_error_handling".into(),
                    name: "AF T5: エラーハンドリング".into(),
                    input: "/tmp/bonsai_absolutely_missing_file_xyz.rs を読んで内容を教えて".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec![],
                    max_iterations: 3,
                    category: TaskCategory::ErrorRecovery,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::ErrorRecovery,
                },
                BenchmarkTask {
                    id: "af_t5_tool_fail_pivot".into(),
                    name: "AF T5: ツール失敗→代替手段".into(),
                    input: "存在しないコマンド `fakecmd_xyz` を試した後、別の方法で現在のディレクトリを確認して".into(),
                    expected_tools: vec!["shell".into()],
                    expected_keywords: vec!["代替".into(), "ls".into()],
                    max_iterations: 5,
                    category: TaskCategory::ErrorRecovery,
                    tier: TaskTier::Extended,
                    capability_tier: CapabilityTier::ErrorRecovery,
                },
                BenchmarkTask {
                    id: "af_t5_error_recovery_permission".into(),
                    name: "AF T5: 権限エラー回復".into(),
                    input: "/etc/bonsai_readonly_testに'test'と書き込んで".into(),
                    expected_tools: vec!["file_write".into()],
                    expected_keywords: vec!["permission".into(), "denied".into(), "error".into(), "cannot".into()],
                    max_iterations: 4,
                    category: TaskCategory::ErrorRecovery,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::ErrorRecovery,
                },
                BenchmarkTask {
                    id: "af_t5_corrupt_repair".into(),
                    name: "AF T5: 破損JSON修復".into(),
                    input: "JSON 文字列 `{\"name\":\"test\"` （閉じ括弧欠落）を修復した正しい形を示せ".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["}".into(), "name".into()],
                    max_iterations: 4,
                    category: TaskCategory::ErrorRecovery,
                    tier: TaskTier::Extended,
                    capability_tier: CapabilityTier::ErrorRecovery,
                },
                // ── T6 LongHorizonPlanning (5 task、新規) ────────────────────
                // plan §4.3 T6 定義: max_iterations >= 8、5+ ステップ計画→実行→検証
                BenchmarkTask {
                    id: "lh_plan_refactor_5files".into(),
                    name: "AF T6: 5ファイル横断リファクタ計画".into(),
                    input: "RepoMap でファイル構造を把握し、src/agent/ 下の主要 3 ファイルを読み取り、\
                           共通パターンを抽出して、リファクタリング計画を具体的に示して".into(),
                    expected_tools: vec!["repo_map".into(), "file_read".into()],
                    expected_keywords: vec!["リファクタ".into(), "計画".into()],
                    max_iterations: 10,
                    category: TaskCategory::MultiStep,
                    tier: TaskTier::Extended,
                    capability_tier: CapabilityTier::LongHorizonPlanning,
                },
                BenchmarkTask {
                    id: "lh_test_red_green".into(),
                    name: "AF T6: テスト追加→実装確認".into(),
                    input: "src/agent/benchmark.rs を読んで BenchmarkTask 構造体の概要を把握し、\
                           新しいテストケースを 2 件提案し、それぞれが何を検証するか説明して".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec!["test".into(), "assert".into()],
                    max_iterations: 8,
                    category: TaskCategory::MultiStep,
                    tier: TaskTier::Extended,
                    capability_tier: CapabilityTier::LongHorizonPlanning,
                },
                BenchmarkTask {
                    id: "lh_dependency_chain".into(),
                    name: "AF T6: 依存関係連鎖分析".into(),
                    input: "src/agent/mod.rs を読んで公開モジュールを特定し、\
                           各モジュールのファイルを読んで相互依存を調べ、\
                           依存グラフと影響範囲レポートを作成して".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec!["依存".into(), "モジュール".into()],
                    max_iterations: 10,
                    category: TaskCategory::MultiStep,
                    tier: TaskTier::Extended,
                    capability_tier: CapabilityTier::LongHorizonPlanning,
                },
                BenchmarkTask {
                    id: "lh_plan_then_revise".into(),
                    name: "AF T6: 計画立案→改訂".into(),
                    input: "Cargo.toml を読んで依存クレートを把握し、\
                           セキュリティ観点での改善計画を立案し、\
                           リスクと代替案を含めた改訂版計画を示して".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec!["計画".into(), "改訂".into(), "リスク".into()],
                    max_iterations: 8,
                    category: TaskCategory::MultiStep,
                    tier: TaskTier::Extended,
                    capability_tier: CapabilityTier::LongHorizonPlanning,
                },
                BenchmarkTask {
                    id: "lh_multi_modal_audit".into(),
                    name: "AF T6: マルチモーダル repo audit".into(),
                    input: "shell で git log を確認し、src/ 配下のファイル数を調べ、\
                           Cargo.toml を読んで依存数を確認し、\
                           リポジトリの健全性レポートを作成して".into(),
                    expected_tools: vec!["shell".into(), "file_read".into(), "git".into()],
                    expected_keywords: vec!["コミット".into(), "依存".into(), "レポート".into()],
                    max_iterations: 10,
                    category: TaskCategory::MultiStep,
                    tier: TaskTier::Extended,
                    capability_tier: CapabilityTier::LongHorizonPlanning,
                },
            ],
        }
    }

    /// デフォルトのベンチマークタスクセット (Plan A G-4c で 42→45 task に拡張)。
    ///
    /// `halluc_t2_file_context_misalign` task は `/tmp/bonsai_halluc_ctx.txt` に依存。
    /// `setup_halluc_fixtures()` が冪等に file fixture を準備する。
    pub fn default_tasks() -> Self {
        // halluc_t2_file_context_misalign 用 fixture (idempotent、failure は silent
        // = halluc_t2 task が file_read で失敗するだけで他 task に影響なし)。
        let _ = setup_halluc_fixtures();
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
                    capability_tier: CapabilityTier::SingleToolUse, // T2: 1ツール
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
                    capability_tier: CapabilityTier::SingleToolUse, // T2: 1ツール
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
                    capability_tier: CapabilityTier::SingleToolUse, // T2: 1ツール
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
                    capability_tier: CapabilityTier::MultiStepToolChain, // T4: 2ツール連鎖
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
                    capability_tier: CapabilityTier::InstructionFollowing, // T1: ツール不要
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
                    capability_tier: CapabilityTier::ErrorRecovery, // T5: エラー回復
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
                    capability_tier: CapabilityTier::ToolSelection, // T3: ツール選択
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
                    capability_tier: CapabilityTier::InstructionFollowing, // T1: ツール不要
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
                    capability_tier: CapabilityTier::InstructionFollowing, // T1: ツール不要
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
                    capability_tier: CapabilityTier::MultiStepToolChain, // T4: 読込+推論
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
                    capability_tier: CapabilityTier::ErrorRecovery, // T5: エラー回復
                },
                // handoff 05-07g Phase 5 Phase 4: AgentHER failed パス検証用 smoke task
                // 2 つの非存在ファイルを **両方とも** 読ませ、tool_success_rate=0 < 0.8 と
                // total_steps>=2 を必ず踏ませることで `extract_failed_trajectories(0.8, 2)`
                // が確実に 1 セッションを extract できるようにする (1bit Bonsai-8B で頻出する
                // 1-step 完結を回避)。max_iterations=4 で model にリトライ余裕を与え、
                // 確実に 2 つの file_read を試行させる。
                BenchmarkTask {
                    id: "smoke_failure_chain_pair".into(),
                    name: "失敗連鎖 (2 ファイル)".into(),
                    input: "/tmp/bonsai_phase4_a.txt と /tmp/bonsai_phase4_b.txt の内容を読んで diff を要約して"
                        .into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec![
                        "存在".into(),
                        "見つかり".into(),
                        "error".into(),
                        "Error".into(),
                        "not found".into(),
                    ],
                    max_iterations: 4,
                    category: TaskCategory::ErrorRecovery,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::ErrorRecovery, // T5: エラー回復
                },
                // handoff 05-07h 後継: AgentHER HSL relabel 実機実証用 mixed-success task。
                // existing file (Cargo.toml) と nonexistent file を **両方** 読ませることで:
                //   - tool_success_rate = 1/2 = 0.5 < 0.8 → extract_failed_trajectories 対象
                //   - total_steps >= 2 → min_steps 閾値クリア
                //   - 失敗 trajectory 内に **成功 subgoal が 1 件存在** → HSL relabel 候補
                // 期待: AgentHER post-Lab で relabels>=1 / skills>=1 / insights>=1。
                BenchmarkTask {
                    id: "smoke_partial_success_chain".into(),
                    name: "部分成功連鎖 (1 success + 1 fail)".into(),
                    input: "Cargo.toml と /tmp/bonsai_phase5_nonexistent.md の内容を読んで diff を要約して"
                        .into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec![
                        "Cargo".into(),
                        "存在".into(),
                        "見つかり".into(),
                        "error".into(),
                        "Error".into(),
                        "not found".into(),
                    ],
                    max_iterations: 4,
                    category: TaskCategory::ErrorRecovery,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::ErrorRecovery, // T5: エラー回復
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
                    capability_tier: CapabilityTier::SingleToolUse, // T2: 1ツール読込→要約
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
                    capability_tier: CapabilityTier::ToolSelection, // T3: 複数候補からrepo_map選択
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
                    capability_tier: CapabilityTier::MultiStepToolChain, // T4: 2ファイル読込→比較
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
                    capability_tier: CapabilityTier::MultiStepToolChain, // T4: 確認→書込チェーン
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
                    capability_tier: CapabilityTier::SingleToolUse, // T2: 1ツール読込→要約
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
                    capability_tier: CapabilityTier::MultiStepToolChain, // T4: 読込→編集チェーン
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
                    capability_tier: CapabilityTier::SingleToolUse, // T2: 1ツール
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
                    capability_tier: CapabilityTier::ErrorRecovery, // T5: エラー回復
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
                    capability_tier: CapabilityTier::InstructionFollowing, // T1: ツール不要
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
                    capability_tier: CapabilityTier::SingleToolUse, // T2: file_write使用 (plan §4.2 code_gen_sort例外)
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
                    capability_tier: CapabilityTier::MultiStepToolChain, // T4: grep→読込チェーン
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
                    capability_tier: CapabilityTier::MultiStepToolChain, // T4: 複数ツール連鎖
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
                    capability_tier: CapabilityTier::MultiStepToolChain, // T4: grep→編集チェーン
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
                    capability_tier: CapabilityTier::LongHorizonPlanning, // T6: 5+ステップ計画→実行 (plan §4.2 例外)
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
                    capability_tier: CapabilityTier::LongHorizonPlanning, // T6: 長時間計画 (plan §4.2 例外)
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
                    capability_tier: CapabilityTier::MultiStepToolChain, // T4: 3ツール連鎖
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
                    capability_tier: CapabilityTier::MultiStepToolChain, // T4: grep→編集チェーン
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
                    capability_tier: CapabilityTier::ErrorRecovery, // T5: 失敗→代替
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
                    capability_tier: CapabilityTier::ErrorRecovery, // T5: エラー検出・修復
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
                    capability_tier: CapabilityTier::SingleToolUse, // T2: 1ツール
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
                    capability_tier: CapabilityTier::MultiStepToolChain, // T4: 読込→書込チェーン
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
                    capability_tier: CapabilityTier::InstructionFollowing, // T1: ツール不要
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
                    capability_tier: CapabilityTier::InstructionFollowing, // T1: ツール不要
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
                    capability_tier: CapabilityTier::InstructionFollowing, // T1: ツール不要
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
                    capability_tier: CapabilityTier::InstructionFollowing, // T1: ツール不要
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
                    capability_tier: CapabilityTier::MultiStepToolChain, // T4: 2ファイル読込→比較要約 (plan §4.2 例外)
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
                    capability_tier: CapabilityTier::SingleToolUse, // T2: 1ツール読込→要約
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
                    capability_tier: CapabilityTier::InstructionFollowing, // T1: ツール不要
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
                    capability_tier: CapabilityTier::ToolSelection, // T3: 複数ツール候補から選択
                },
                // ── Plan A G-4c: hallucination-inducing task 3 件 (項目 230 後続) ──
                // KG seed (`factcheck::seed_kg_for_factcheck_lab`) と組で `Conflict`
                // 経路を実機で発火させる。Lab v20 paired t-test の前提タスク。
                BenchmarkTask {
                    id: "halluc_parent_of_false_fact".into(),
                    name: "幻覚誘発: 親モデル捏造 (T1)".into(),
                    input: "Bonsai-8B is the parent model of which AI system? Answer in one sentence using the format 'X is the parent of Y'.".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["parent".into(), "of".into()],
                    max_iterations: 2,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::InstructionFollowing,
                },
                BenchmarkTask {
                    id: "halluc_is_a_false_type".into(),
                    name: "幻覚誘発: 型分類捏造 (T1)".into(),
                    input: "Describe what Prism-ml is. Use the format 'Prism-ml is a X' with capitalized 'Prism-ml' as subject.".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["is a".into(), "is an".into()],
                    max_iterations: 2,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::InstructionFollowing,
                },
                BenchmarkTask {
                    id: "halluc_t2_file_context_misalign".into(),
                    name: "幻覚誘発: context 不整合 (T2)".into(),
                    input: "Read /tmp/bonsai_halluc_ctx.txt and answer using the EXACT format from the file: 'Bonsai-Agent is the X of Y' (capitalize 'Bonsai-Agent' and the second entity).".into(),
                    expected_tools: vec!["file_read".into()],
                    expected_keywords: vec!["is the".into(), "of".into()],
                    max_iterations: 4,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::SingleToolUse,
                },
                // ── 項目 242 Lab v21: success_fact task 5 件 (KG seed 拡張で matched>0 シナリオ生成) ──
                // 起点: `.claude/plan/lab-v21-kg-seed-expansion.md` §2.1 (案 A)
                // halluc 3 task と対をなす「LLM が正解を述べる」shape の task。
                // 期待: LLM 出力が KG seed (`seed_kg_for_factcheck_lab` の 5 success fact) と
                // match → matched>0 cycle 出現 → Pearson r 計算可能化 (Lab v20 structural finding 解消)。
                // 全 task は context (Hint) を入力に含めるため、Bonsai-8B 1bit でも正解確率が
                // halluc task より高い (R1 mitigation: G-7b smoke で matched>=1 確証)。
                BenchmarkTask {
                    id: "success_bonsai_is_a_rust_project".into(),
                    name: "正解誘導: 言語ラベル (Pattern 2 is_a)".into(),
                    input: "Bonsai-Agent is implemented in Rust. Output one sentence in your reply using the EXACT format 'Bonsai-Agent is a rust_project' (use rust_project as a single token).".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["is a".into(), "rust_project".into()],
                    max_iterations: 2,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::InstructionFollowing,
                },
                BenchmarkTask {
                    id: "success_llama_runtime_of_bonsai".into(),
                    name: "正解誘導: runtime 関係 (Pattern 1 runtime_of)".into(),
                    input: "Llama-server is the inference runtime that Bonsai-Agent uses. Output one sentence in your reply using the EXACT format 'Llama-server is the runtime of Bonsai-Agent' (capitalize both 'Llama-server' and 'Bonsai-Agent').".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["is the runtime of".into(), "Bonsai-Agent".into()],
                    max_iterations: 2,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::InstructionFollowing,
                },
                BenchmarkTask {
                    id: "success_sqlite_storage_of_bonsai".into(),
                    name: "正解誘導: storage 関係 (Pattern 1 storage_of)".into(),
                    input: "Sqlite is the storage backend used by Bonsai-Agent. Output one sentence in your reply using the EXACT format 'Sqlite is the storage of Bonsai-Agent' (capitalize 'Sqlite' and 'Bonsai-Agent').".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["is the storage of".into(), "Bonsai-Agent".into()],
                    max_iterations: 2,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::InstructionFollowing,
                },
                BenchmarkTask {
                    id: "success_reflexion_loop_of_bonsai".into(),
                    name: "正解誘導: loop 関係 (Pattern 1 loop_of)".into(),
                    input: "Reflexion is the main loop pattern of Bonsai-Agent. Output one sentence in your reply using the EXACT format 'Reflexion is the loop of Bonsai-Agent' (capitalize 'Reflexion' and 'Bonsai-Agent').".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["is the loop of".into(), "Bonsai-Agent".into()],
                    max_iterations: 2,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::InstructionFollowing,
                },
                BenchmarkTask {
                    id: "success_pathguard_sandbox_of_bonsai".into(),
                    name: "正解誘導: sandbox 関係 (Pattern 1 sandbox_of、dash subject)".into(),
                    input: "Path-Guard is the sandbox mechanism of Bonsai-Agent. Output one sentence in your reply using the EXACT format 'Path-Guard is the sandbox of Bonsai-Agent' (capitalize 'Path-Guard' and 'Bonsai-Agent', keep the dash).".into(),
                    expected_tools: vec![],
                    expected_keywords: vec!["is the sandbox of".into(), "Bonsai-Agent".into()],
                    max_iterations: 2,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::InstructionFollowing,
                },
            ],
        }
    }

    /// 各タスクをk回実行してpass^k指標を計算
    ///
    /// `store` は AgentHER post-Lab pass で参照される persistent `MemoryStore`。各 run の
    /// events は `run_agent_loop` 内の `emit_event` 経由で直接ここに書き込まれる
    /// (Option A 移行、agenther-option-a-migration.md)。run 毎の messages/sessions/memories
    /// は `reset_session_data_for_lab` で each run 開始前にリセットして isolation を維持
    /// (events table は保護される)。pre-screen は呼出側で `MemoryStore::in_memory()?` を
    /// scratch として作成し本 method に渡すことで persistent.events 汚染を回避する。
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
        store: &MemoryStore,
    ) -> Result<MultiRunBenchmarkResult> {
        let start = std::time::Instant::now();
        let mut task_scores = Vec::new();
        // 項目 225 (arxiv 2604.14877): PASS@(k,T) 閾値は env 経由で 1 度だけ解析。
        // 未指定時は空 Vec が返り、`from_scores_with_metrics_v2` の T 軸計算が空 Vec
        // になることで既存挙動 (env 未設定環境) と 100% 互換になる。
        let t_steps_thresholds = parse_t_steps_env();
        let t_seconds_thresholds = parse_t_seconds_env();

        for task in &self.tasks {
            if cancel.is_cancelled() {
                break;
            }

            let mut scores = Vec::new();
            // 項目 200 (Beyond pass@1): 各 run の使用 iteration を収集して RDC 計算に渡す
            let mut iterations_per_run: Vec<usize> = Vec::with_capacity(multi.k);
            // 項目 225 (PASS@(k,T)): 各 run の wallclock 秒数を収集して T_seconds 軸計算に渡す。
            // 成功 run / 失敗 run 共に `Instant::now()` で計測 (RDC と一貫した記録方針)。
            let mut durations_per_run: Vec<f64> = Vec::with_capacity(multi.k);
            // 最終 run の代表 trajectory（B2: judge gate 用、最後に成功した run を優先）
            let mut last_run_capture: Option<(String, Vec<String>)> = None;
            for run_idx in 0..multi.k {
                if cancel.is_cancelled() {
                    break;
                }

                // Option A 移行: persistent store を直接使うため、run 毎に
                // messages/sessions/memories をクリアして per-run / per-task isolation を維持
                // (events / experiences / skills は保護)。run_idx==0 を含む全 run で reset。
                store.reset_session_data_for_lab()?;

                // jitter_seed時はプロンプトに実行番号を付加してキャッシュ回避
                // T6 prompt augment: env=1 + LongHorizonPlanning tier で directive を append
                let base_prompt = if multi.jitter_seed {
                    format!("{}\n<!-- run:{run_idx} -->", config.system_prompt)
                } else {
                    config.system_prompt.clone()
                };
                let system_prompt = augment_system_prompt(&base_prompt, task.capability_tier);

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
                    n_ctx_budget: config.n_ctx_budget,
                    memory_blocks: config.memory_blocks.clone(),
                };
                // 項目 225: per-run wallclock 計測。`run_agent_loop` 内 retry 含む total
                // が論文 (arxiv 2604.14877) の interaction depth 定義と整合する。
                let run_start = std::time::Instant::now();
                let result = run_agent_loop(
                    &task.input,
                    backend,
                    tools,
                    path_guard,
                    &task_config,
                    cancel,
                    Some(store),
                );
                let run_duration_secs = run_start.elapsed().as_secs_f64();

                let score = match result {
                    Ok(ref loop_result) => {
                        // judge gate 用: 最終 run の応答 + トラジェクトリを保持
                        last_run_capture =
                            Some((loop_result.answer.clone(), loop_result.tools_called.clone()));
                        // 項目 200: 各 run の iteration 使用数を収集
                        iterations_per_run.push(loop_result.iterations_used);
                        // 項目 225: 成功 run の wallclock を T_seconds 軸計算用に収集
                        durations_per_run.push(run_duration_secs);
                        evaluate_task_response(task, loop_result).score()
                    }
                    Err(_) => {
                        // 失敗 run は budget 完全消費とみなす (RDC で late-iteration fail 扱い)
                        iterations_per_run.push(task.max_iterations);
                        // 項目 225: 失敗 run も elapsed を push (RDC の "late-iteration fail" 方針と一貫)
                        durations_per_run.push(run_duration_secs);
                        0.0
                    }
                };
                scores.push(score);
            }

            // Sub-Phase 2C (frontier benchmark): final_context_tokens 推定値を
            // iterations の平均 × TOKENS_PER_ITERATION_ESTIMATE で計算、from_scores へ move 前に
            // borrow して mean を取得する (clone 不要)。失敗 run の iter = max_iterations が
            // push されている (line 1920 の `iterations_per_run.push(task.max_iterations)` 経路)
            // ため、mean は実際の context 規模を上振れで反映する (失敗ほど context が長い)。
            let mean_iter = if iterations_per_run.is_empty() {
                0.0
            } else {
                iterations_per_run.iter().sum::<usize>() as f64 / iterations_per_run.len() as f64
            };
            let est_context_tokens = (mean_iter * TOKENS_PER_ITERATION_ESTIMATE as f64) as usize;

            // 項目 200 + 225: from_scores_with_metrics_v2 で RDC/GDS/PASS@(k,T) を一括計算
            let mut task_score = MultiRunTaskScore::from_scores_with_metrics_v2(
                task.id.clone(),
                scores,
                iterations_per_run,
                durations_per_run,
                task.max_iterations,
                pass_threshold,
                &t_steps_thresholds,
                &t_seconds_thresholds,
            );
            // Sub-Phase 2C: frontier bucket 振り分けの軸となる context token 推定値を後付け populate。
            // env opt-in (`BONSAI_FRONTIER_ENABLED`) のチェックは Experiment::from_results 側で行い、
            // run_k は常に populate する (cost は加算 1 ops のみで観察コスト無視可能)。
            task_score.final_context_tokens = Some(est_context_tokens);

            // Sub-Phase 2F: T6-LongHorizon inject variant runs (案 C 2nd pillar)。
            // env `BONSAI_FRONTIER_INJECT_ENABLED=1` AND task が LongHorizonPlanning のときのみ
            // 4 size (default 0/4/8/16 KB) × k runs = 4k 追加 run を実行、(size_kb, mean) を
            // `frontier_inject_scores` に push する。production default OFF で観察コストゼロ。
            if crate::agent::frontier::is_frontier_inject_enabled()
                && task.capability_tier == CapabilityTier::LongHorizonPlanning
            {
                let inject_sizes = crate::agent::frontier::parse_frontier_inject_sizes_env();
                let mut inject_scores: Vec<(usize, f64)> = Vec::new();
                for &size_kb in &inject_sizes {
                    if cancel.is_cancelled() {
                        break;
                    }
                    let injected_input =
                        crate::agent::frontier::inject_filler_context(&task.input, size_kb);
                    let mut size_scores = Vec::new();
                    for inject_run_idx in 0..multi.k {
                        if cancel.is_cancelled() {
                            break;
                        }
                        store.reset_session_data_for_lab()?;
                        // T6 prompt augment: env=1 + LongHorizonPlanning tier で directive を append
                        let base_prompt = if multi.jitter_seed {
                            format!(
                                "{}\n<!-- run:inject:{size_kb}kb:{inject_run_idx} -->",
                                config.system_prompt
                            )
                        } else {
                            config.system_prompt.clone()
                        };
                        let system_prompt =
                            augment_system_prompt(&base_prompt, task.capability_tier);
                        let inject_config = AgentConfig {
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
                            n_ctx_budget: config.n_ctx_budget,
                            memory_blocks: config.memory_blocks.clone(),
                        };
                        let inject_result = run_agent_loop(
                            &injected_input,
                            backend,
                            tools,
                            path_guard,
                            &inject_config,
                            cancel,
                            Some(store),
                        );
                        let s = match inject_result {
                            Ok(ref lr) => evaluate_task_response(task, lr).score(),
                            Err(_) => 0.0,
                        };
                        size_scores.push(s);
                    }
                    if !size_scores.is_empty() {
                        let mean = size_scores.iter().sum::<f64>() / size_scores.len() as f64;
                        inject_scores.push((size_kb, mean));
                    }
                }
                task_score.frontier_inject_scores = inject_scores;
            }

            if let Some((response, trajectory)) = last_run_capture {
                task_score = task_score.with_last_run(response, trajectory);
            }
            task_scores.push(task_score);
            // Option A: events は run_agent_loop 内で persistent `store` に直接 emit されるため、
            // export_to による bulk copy 不要 (Option B の冗長性解消)。
        }

        // 項目 172 P1: tier 別平均 mean_score 集計（仮説 X / Y 分離用）
        let core_avg_score = compute_tier_avg(&self.tasks, &task_scores, TaskTier::Core);
        let extended_avg_score = compute_tier_avg(&self.tasks, &task_scores, TaskTier::Extended);

        // 項目 209 (AgentFloor): CapabilityTier (T1-T6) 別平均 mean_score 集計
        // compute_capability_tier_avg は HashMap<String, BenchmarkTask> を必要とするため inline 構築
        let task_descs: std::collections::HashMap<String, BenchmarkTask> = self
            .tasks
            .iter()
            .map(|t| (t.id.clone(), t.clone()))
            .collect();
        let tier_avg_scores = [
            compute_capability_tier_avg(
                &task_scores,
                &task_descs,
                CapabilityTier::InstructionFollowing,
            ),
            compute_capability_tier_avg(&task_scores, &task_descs, CapabilityTier::SingleToolUse),
            compute_capability_tier_avg(&task_scores, &task_descs, CapabilityTier::ToolSelection),
            compute_capability_tier_avg(
                &task_scores,
                &task_descs,
                CapabilityTier::MultiStepToolChain,
            ),
            compute_capability_tier_avg(&task_scores, &task_descs, CapabilityTier::ErrorRecovery),
            compute_capability_tier_avg(
                &task_scores,
                &task_descs,
                CapabilityTier::LongHorizonPlanning,
            ),
        ];

        Ok(MultiRunBenchmarkResult {
            task_scores,
            duration_secs: start.elapsed().as_secs_f64(),
            core_avg_score,
            extended_avg_score,
            // 項目 209: AgentFloor tier 別集計を populate (Phase 2/3/4 後の最終配線)
            tier_avg_scores: Some(tier_avg_scores),
            critic_stats: None,
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

            // T6 prompt augment: env=1 + LongHorizonPlanning tier で directive を append
            // env unset の場合は clone のみで副作用ゼロ (backward compat)
            let system_prompt = augment_system_prompt(&config.system_prompt, task.capability_tier);
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
                n_ctx_budget: config.n_ctx_budget,
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
        assert_eq!(
            suite.tasks.len(),
            50,
            "default = 50 (Plan A G-4c 42→45 + 項目 242 Lab v21 success_fact 5 task で 45→50)"
        );
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
        assert_eq!(
            suite.tasks.len(),
            32,
            "core tier は 32 タスク (項目 242 Lab v21 で 27→32、success_fact 5 task 全 Core)"
        );
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
        assert_eq!(
            all.tasks.len(),
            50,
            "default は core(32) + extended(18) = 50 (項目 242 Lab v21 で success_fact 5 追加)"
        );
        let ids: std::collections::HashSet<_> = all.tasks.iter().map(|t| t.id.clone()).collect();
        assert_eq!(ids.len(), 50, "重複なし");
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
            capability_tier: CapabilityTier::InstructionFollowing,
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
        let agentfloor = BenchmarkSuite::agentfloor_tasks();
        assert_eq!(
            smoke.tasks.len(),
            20,
            "smoke は 20 タスク (項目 261 T6 案 A で 15→20、T6 lh_* 5 task 先頭追加)"
        );
        // すべての smoke ID は default ∪ agentfloor に含まれる
        let known_ids: std::collections::HashSet<&str> = default
            .tasks
            .iter()
            .chain(agentfloor.tasks.iter())
            .map(|t| t.id.as_str())
            .collect();
        for t in &smoke.tasks {
            assert!(
                known_ids.contains(t.id.as_str()),
                "smoke ID {} は default ∪ agentfloor に存在すべき",
                t.id
            );
        }
    }

    // --- Plan A G-4c Phase 1 Red: hallucination-inducing task 3 件 ---
    // 起点: `.claude/plan/hallucination-inducing-benchmark-task.md` §4.1
    // halluc_parent_of_false_fact (T1) / halluc_is_a_false_type (T1) /
    // halluc_t2_file_context_misalign (T2) を default_tasks() に追加することで
    // Plan A factcheck の `Conflict` 経路を実機で発火させる。
    // Phase 2 Green で 3 task 実装、Phase 3 で既存 count assertion を更新。

    /// Phase 1 Red — 3 halluc task が `default_tasks()` に存在する。
    /// Phase 2 Green まで Red、3 task 未実装で `iter().any()` が false で FAIL。
    #[test]
    fn t_halluc_tasks_exist_in_default() {
        let suite = BenchmarkSuite::default_tasks();
        let halluc_ids = [
            "halluc_parent_of_false_fact",
            "halluc_is_a_false_type",
            "halluc_t2_file_context_misalign",
        ];
        for id in &halluc_ids {
            assert!(
                suite.tasks.iter().any(|t| t.id == *id),
                "halluc task '{id}' が default_tasks() に存在すべき (Plan A G-4c)"
            );
        }
    }

    /// Phase 1 Red — 3 halluc task は全て `TaskCategory::Reasoning`。
    #[test]
    fn t_halluc_tasks_use_reasoning_category() {
        let suite = BenchmarkSuite::default_tasks();
        let halluc_ids = [
            "halluc_parent_of_false_fact",
            "halluc_is_a_false_type",
            "halluc_t2_file_context_misalign",
        ];
        for id in &halluc_ids {
            let t = suite
                .tasks
                .iter()
                .find(|t| t.id == *id)
                .unwrap_or_else(|| panic!("halluc task '{id}' 未登録 (Phase 2 Green 待ち)"));
            assert_eq!(
                t.category,
                TaskCategory::Reasoning,
                "halluc task '{id}' は Reasoning category であるべき"
            );
        }
    }

    /// Phase 1 Red — 3 halluc task は全て `TaskTier::Core` (Lab v20 paired 対象)。
    #[test]
    fn t_halluc_tasks_tier_core() {
        let suite = BenchmarkSuite::default_tasks();
        let halluc_ids = [
            "halluc_parent_of_false_fact",
            "halluc_is_a_false_type",
            "halluc_t2_file_context_misalign",
        ];
        for id in &halluc_ids {
            let t = suite
                .tasks
                .iter()
                .find(|t| t.id == *id)
                .unwrap_or_else(|| panic!("halluc task '{id}' 未登録 (Phase 2 Green 待ち)"));
            assert_eq!(
                t.tier,
                TaskTier::Core,
                "halluc task '{id}' は Core tier であるべき (Lab v20 paired `BONSAI_BENCH_TIER=core` で hit)"
            );
        }
    }

    /// halluc 3 task 追加で default count 42→45 (Plan A G-4c)、
    /// 項目 242 success_fact 5 task 追加で 45→50 (Lab v21、Phase 3 で count 値更新済)。
    #[test]
    fn t_halluc_task_count_default_is_45() {
        let suite = BenchmarkSuite::default_tasks();
        assert_eq!(
            suite.tasks.len(),
            50,
            "default は 42 + 3 halluc + 5 success_fact = 50 task (Plan A G-4c + 項目 242 Lab v21)"
        );
    }

    /// halluc 3 task が全て Core tier なので core count 24→27 (Plan A G-4c)、
    /// 項目 242 success_fact 5 task で 27→32 (全 Core tier、Phase 3 で count 値更新済)。
    #[test]
    fn t_halluc_task_count_core_is_27() {
        let suite = BenchmarkSuite::core_tasks();
        assert_eq!(
            suite.tasks.len(),
            32,
            "core は 24 + 3 halluc + 5 success_fact = 32 task (全 Core tier、Lab v20/v21 paired 対象)"
        );
    }

    // --- 項目 242 Phase 1 Red: success_fact task 5 件 (Lab v21 KG seed 拡張) ---
    // 起点: `.claude/plan/lab-v21-kg-seed-expansion.md` §2.1 (案 A)
    // Lab v20 structural finding (`(conf+unk)/total = 1.0` deterministic、matched=0
    // で variance ゼロ → Pearson r=0.0 計算不可能) 解消のため、LLM が「正解」を
    // 述べる shape の task 5 件 + 対応 KG seed 5 fact を追加。
    // 期待: matched>0 cycle 出現で Pearson r 計算可能化 (Lab v21 paired 起動前提)。

    /// 項目 242 Phase 1 Red — 5 success_fact task が `default_tasks()` に存在する。
    /// Phase 2 Green まで Red、5 task 未実装で `iter().any()` が false で FAIL。
    #[test]
    fn t_success_fact_tasks_exist_in_default() {
        let suite = BenchmarkSuite::default_tasks();
        let success_ids = [
            "success_bonsai_is_a_rust_project",
            "success_llama_runtime_of_bonsai",
            "success_sqlite_storage_of_bonsai",
            "success_reflexion_loop_of_bonsai",
            "success_pathguard_sandbox_of_bonsai",
        ];
        for id in &success_ids {
            assert!(
                suite.tasks.iter().any(|t| t.id == *id),
                "success_fact task '{id}' が default_tasks() に存在すべき (項目 242 Phase 2 Green 待ち)"
            );
        }
    }

    /// 項目 242 Phase 1 Red — 5 success_fact task は全て `TaskCategory::Reasoning`。
    /// halluc 3 task と同 category で hindsight relabel / factcheck の対象軌跡として揃える。
    #[test]
    fn t_success_fact_tasks_use_reasoning_category() {
        let suite = BenchmarkSuite::default_tasks();
        let success_ids = [
            "success_bonsai_is_a_rust_project",
            "success_llama_runtime_of_bonsai",
            "success_sqlite_storage_of_bonsai",
            "success_reflexion_loop_of_bonsai",
            "success_pathguard_sandbox_of_bonsai",
        ];
        for id in &success_ids {
            let t =
                suite.tasks.iter().find(|t| t.id == *id).unwrap_or_else(|| {
                    panic!("success_fact task '{id}' 未登録 (Phase 2 Green 待ち)")
                });
            assert_eq!(
                t.category,
                TaskCategory::Reasoning,
                "success_fact task '{id}' は Reasoning category であるべき"
            );
        }
    }

    /// 項目 242 Phase 1 Red — 5 success_fact task は全て `TaskTier::Core` (Lab v21 paired 対象)。
    /// Lab v21 paired smoke で `BONSAI_BENCH_TIER=core` 起動時に hit する設計。
    #[test]
    fn t_success_fact_tasks_tier_core() {
        let suite = BenchmarkSuite::default_tasks();
        let success_ids = [
            "success_bonsai_is_a_rust_project",
            "success_llama_runtime_of_bonsai",
            "success_sqlite_storage_of_bonsai",
            "success_reflexion_loop_of_bonsai",
            "success_pathguard_sandbox_of_bonsai",
        ];
        for id in &success_ids {
            let t =
                suite.tasks.iter().find(|t| t.id == *id).unwrap_or_else(|| {
                    panic!("success_fact task '{id}' 未登録 (Phase 2 Green 待ち)")
                });
            assert_eq!(
                t.tier,
                TaskTier::Core,
                "success_fact task '{id}' は Core tier であるべき (Lab v21 paired `BONSAI_BENCH_TIER=core` で hit)"
            );
        }
    }

    /// 項目 242 Phase 1 Red — success_fact 5 task 追加で default count 45→50。
    /// 既存 `test_default_equals_core_plus_extended` / `t_halluc_task_count_default_is_45`
    /// は Phase 3 Refactor で 50 に更新する (本 test は Phase 2 Green 後 PASS)。
    #[test]
    fn t_success_fact_task_count_default_is_50() {
        let suite = BenchmarkSuite::default_tasks();
        assert_eq!(
            suite.tasks.len(),
            50,
            "default は 45 + 5 success_fact = 50 task (項目 242 Lab v21 前提)"
        );
    }

    /// 項目 242 Phase 1 Red — success_fact 5 task が全て Core tier なので core count 27→32。
    #[test]
    fn t_success_fact_task_count_core_is_32() {
        let suite = BenchmarkSuite::core_tasks();
        assert_eq!(
            suite.tasks.len(),
            32,
            "core は 27 + 5 success_fact = 32 task (全 success_fact Core tier、Lab v21 paired 対象)"
        );
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
            capability_tier: CapabilityTier::InstructionFollowing,
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
            capability_tier: CapabilityTier::InstructionFollowing,
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
            capability_tier: CapabilityTier::InstructionFollowing,
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
            capability_tier: CapabilityTier::InstructionFollowing,
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
            capability_tier: CapabilityTier::InstructionFollowing,
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
            capability_tier: CapabilityTier::InstructionFollowing,
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
            tier_avg_scores: None,
            critic_stats: None,
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

    // ─── 項目 200 (Beyond pass@1) Red phase tests ──────────────────────────

    #[test]
    fn t_rdc_perfect_no_decay() {
        // 全 run iter=0 完了 + 全 success (score=1.0) → RDC = 1.0 (減衰なし)
        let task = MultiRunTaskScore::from_scores_with_metrics(
            "t".into(),
            vec![1.0, 1.0, 1.0],
            vec![0, 0, 0],
            10,
            0.5,
        );
        assert!(
            (task.reliability_decay - 1.0).abs() < 1e-6,
            "perfect run RDC should be 1.0, got {}",
            task.reliability_decay
        );
    }

    #[test]
    fn t_rdc_full_decay_late_iterations_fail() {
        // 早期 (iter=0) は成功、後期 (iter=9) は失敗 → 強い負相関 → RDC < 0.5
        let task = MultiRunTaskScore::from_scores_with_metrics(
            "t".into(),
            vec![1.0, 0.5, 0.0],
            vec![0, 5, 9],
            10,
            0.5,
        );
        assert!(
            task.reliability_decay < 0.5,
            "expected RDC < 0.5 (decay observed), got {}",
            task.reliability_decay
        );
    }

    #[test]
    fn t_vaf_baseline_zero_variance_returns_none() {
        // baseline 完全一致 (var=0) なら VAF は分母 0 → None
        let baseline = MultiRunBenchmarkResult {
            task_scores: vec![MultiRunTaskScore::from_scores(
                "t".into(),
                vec![1.0, 1.0, 1.0],
                0.5,
            )],
            duration_secs: 0.0,
            core_avg_score: None,
            extended_avg_score: None,
            tier_avg_scores: None,
            critic_stats: None,
        };
        let experiment = MultiRunBenchmarkResult {
            task_scores: vec![MultiRunTaskScore::from_scores(
                "t".into(),
                vec![1.0, 0.5, 0.0],
                0.5,
            )],
            duration_secs: 0.0,
            core_avg_score: None,
            extended_avg_score: None,
            tier_avg_scores: None,
            critic_stats: None,
        };
        assert!(experiment.variance_amplification_vs(&baseline).is_none());
    }

    #[test]
    fn t_vaf_amplification_doubled() {
        // baseline 軽 variance、experiment 大 variance → VAF > 1.0
        let baseline = MultiRunBenchmarkResult {
            task_scores: vec![MultiRunTaskScore::from_scores(
                "t".into(),
                vec![1.0, 0.5, 0.0],
                0.5,
            )],
            duration_secs: 0.0,
            core_avg_score: None,
            extended_avg_score: None,
            tier_avg_scores: None,
            critic_stats: None,
        };
        let experiment = MultiRunBenchmarkResult {
            task_scores: vec![MultiRunTaskScore::from_scores(
                "t".into(),
                vec![1.0, 0.0, 0.0],
                0.5,
            )],
            duration_secs: 0.0,
            core_avg_score: None,
            extended_avg_score: None,
            tier_avg_scores: None,
            critic_stats: None,
        };
        let vaf = experiment.variance_amplification_vs(&baseline).unwrap();
        assert!(vaf > 1.0, "expected amplification VAF > 1.0, got {vaf}");
    }

    #[test]
    fn t_gds_all_pass_returns_one() {
        // 全 score >= pass_threshold → GDS = 1.0 (全 pass)
        let task = MultiRunTaskScore::from_scores_with_metrics(
            "t".into(),
            vec![1.0, 0.8, 0.6],
            vec![0, 1, 2],
            10,
            0.5,
        );
        assert!(
            (task.graceful_degradation - 1.0).abs() < 1e-6,
            "all-pass GDS should be 1.0, got {}",
            task.graceful_degradation
        );
    }

    // ─── 項目 225 (arxiv 2604.14877 PASS@(k,T)) Red phase tests ─────────────

    #[test]
    fn t_pass_at_k_t_steps_basic() {
        // 3 run: scores=[1.0, 1.0, 0.0], iters=[2, 5, 10], threshold=0.5
        // T=3 -> run0 (s=1.0, i=2) のみ -> 1/3
        // T=5 -> run0+run1 (s=1.0, i<=5) -> 2/3
        // T=10 -> run0+run1 (run2 は score<0.5 で fail) -> 2/3
        let task = MultiRunTaskScore::from_scores_with_metrics_v2(
            "t".into(),
            vec![1.0, 1.0, 0.0],
            vec![2, 5, 10],
            vec![10.0, 30.0, 100.0],
            10,
            0.5,
            &[3, 5, 10],
            &[],
        );
        assert_eq!(task.pass_at_k_t_steps.len(), 3);
        let map: std::collections::HashMap<usize, f64> =
            task.pass_at_k_t_steps.iter().copied().collect();
        assert!((map[&3] - 1.0 / 3.0).abs() < 1e-6);
        assert!((map[&5] - 2.0 / 3.0).abs() < 1e-6);
        assert!((map[&10] - 2.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn t_pass_at_k_t_seconds_basic() {
        // 同 scores、durations=[10, 60, 300], thresholds=[30, 120, 600]
        // T=30 -> run0 のみ pass、1/3
        // T=120 -> run0+run1 pass、2/3 (run1 dur=60 <= 120 + score=1.0)
        // T=600 -> run0+run1 (run2 score<0.5 で fail)、2/3
        let task = MultiRunTaskScore::from_scores_with_metrics_v2(
            "t".into(),
            vec![1.0, 1.0, 0.0],
            vec![2, 5, 10],
            vec![10.0, 60.0, 300.0],
            10,
            0.5,
            &[],
            &[30.0, 120.0, 600.0],
        );
        let map = task.pass_at_k_t_seconds;
        assert!((map[0].1 - 1.0 / 3.0).abs() < 1e-6); // T=30
        assert!((map[1].1 - 2.0 / 3.0).abs() < 1e-6); // T=120
        assert!((map[2].1 - 2.0 / 3.0).abs() < 1e-6); // T=600
    }

    #[test]
    fn t_pass_at_k_t_empty_thresholds_returns_empty_vec() {
        // env 未指定相当 -> 空 Vec で既存挙動互換
        let task = MultiRunTaskScore::from_scores_with_metrics_v2(
            "t".into(),
            vec![1.0, 0.5],
            vec![1, 2],
            vec![10.0, 20.0],
            5,
            0.5,
            &[],
            &[],
        );
        assert!(task.pass_at_k_t_steps.is_empty());
        assert!(task.pass_at_k_t_seconds.is_empty());
    }

    #[test]
    fn t_pass_at_k_t_seconds_non_finite_thresholds_are_ignored() {
        // Codex audit MEDIUM finding: `BONSAI_PASS_K_T_SECONDS=60,inf` 等で
        // 非有限 f64 が閾値に混入すると serde_json::to_string が失敗し persistence
        // 経路が壊れる。compute_pass_at_k_t_seconds + parse_t_seconds_env で
        // `is_finite()` フィルタを追加、混入時は静かにスキップして有限値のみ出力。
        let task = MultiRunTaskScore::from_scores_with_metrics_v2(
            "t".into(),
            vec![1.0],
            vec![1],
            vec![10.0],
            10,
            0.5,
            &[],
            &[30.0, f64::INFINITY, f64::NAN, f64::NEG_INFINITY],
        );
        assert_eq!(
            task.pass_at_k_t_seconds.len(),
            1,
            "有限な T=30.0 のみが残るべき (Inf/NaN/-Inf は除外)"
        );
        assert!((task.pass_at_k_t_seconds[0].0 - 30.0).abs() < 1e-6);
        assert!((task.pass_at_k_t_seconds[0].1 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn t_pass_at_k_t_all_pass_within_t() {
        // 全 run が T 制約内で pass -> pass_rate=1.0
        let task = MultiRunTaskScore::from_scores_with_metrics_v2(
            "t".into(),
            vec![1.0, 1.0, 1.0],
            vec![1, 1, 1],
            vec![5.0, 5.0, 5.0],
            10,
            0.5,
            &[3],
            &[10.0],
        );
        assert!((task.pass_at_k_t_steps[0].1 - 1.0).abs() < 1e-6);
        assert!((task.pass_at_k_t_seconds[0].1 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn t_pass_at_k_t_all_exceed_t_returns_zero() {
        // 全 run の iter > T_steps -> pass_rate=0.0 (score>=threshold でも T 違反)
        let task = MultiRunTaskScore::from_scores_with_metrics_v2(
            "t".into(),
            vec![1.0, 1.0, 1.0],
            vec![5, 6, 7],
            vec![100.0, 100.0, 100.0],
            10,
            0.5,
            &[3],
            &[50.0],
        );
        assert!((task.pass_at_k_t_steps[0].1 - 0.0).abs() < 1e-6);
        assert!((task.pass_at_k_t_seconds[0].1 - 0.0).abs() < 1e-6);
    }

    #[test]
    fn t_composite_pass_at_k_t_steps_average_across_tasks() {
        // 2 task: T=5 で task_a=0.5, task_b=1.0 -> average=0.75
        let task_a = MultiRunTaskScore::from_scores_with_metrics_v2(
            "a".into(),
            vec![1.0, 0.0],
            vec![3, 4],
            vec![10.0, 20.0],
            10,
            0.5,
            &[5],
            &[],
        );
        let task_b = MultiRunTaskScore::from_scores_with_metrics_v2(
            "b".into(),
            vec![1.0, 1.0],
            vec![3, 4],
            vec![10.0, 20.0],
            10,
            0.5,
            &[5],
            &[],
        );
        let result = MultiRunBenchmarkResult {
            task_scores: vec![task_a, task_b],
            duration_secs: 0.0,
            core_avg_score: None,
            extended_avg_score: None,
            tier_avg_scores: None,
            critic_stats: None,
        };
        let composite = result.composite_pass_at_k_t_steps();
        assert_eq!(composite.len(), 1);
        assert_eq!(composite[0].0, 5);
        assert!(
            (composite[0].1 - 0.75).abs() < 1e-6,
            "got {}",
            composite[0].1
        );
    }

    #[test]
    fn t_serde_backward_compat_old_json_loads_pass_k_t_empty() {
        // 旧 JSON (pass_at_k_t_* なし) -> load 成功 / Vec が空
        let old = r#"{"task_id":"x","pass_at_k":1.0,"pass_consecutive_k":1.0,
                      "mean_score":0.8,"variance":0.0,"individual_scores":[0.8,0.8]}"#;
        let task: MultiRunTaskScore = serde_json::from_str(old).unwrap();
        assert!(task.pass_at_k_t_steps.is_empty());
        assert!(task.pass_at_k_t_seconds.is_empty());
    }

    #[test]
    fn t_gds_partial_credit_proximity() {
        // pass_threshold = 0.5、失敗 run: 0.4, 0.45 → proximity = (0.4 + 0.45) / (2 * 0.5) = 0.85
        let task = MultiRunTaskScore::from_scores_with_metrics(
            "t".into(),
            vec![0.4, 0.45, 1.0],
            vec![0, 1, 2],
            10,
            0.5,
        );
        let expected = (0.4 / 0.5 + 0.45 / 0.5) / 2.0;
        assert!(
            (task.graceful_degradation - expected).abs() < 1e-6,
            "GDS partial credit: expected {}, got {}",
            expected,
            task.graceful_degradation
        );
    }

    #[test]
    fn t_composite_reliability_decay_average() {
        // 2 タスクの平均 RDC = (1.0 + 0.5) / 2 = 0.75
        let mut s_a = MultiRunTaskScore::from_scores("a".into(), vec![1.0, 1.0], 0.5);
        s_a.reliability_decay = 1.0;
        let mut s_b = MultiRunTaskScore::from_scores("b".into(), vec![1.0, 0.0], 0.5);
        s_b.reliability_decay = 0.5;
        let result = MultiRunBenchmarkResult {
            task_scores: vec![s_a, s_b],
            duration_secs: 0.0,
            core_avg_score: None,
            extended_avg_score: None,
            tier_avg_scores: None,
            critic_stats: None,
        };
        assert!(
            (result.composite_reliability_decay() - 0.75).abs() < 1e-6,
            "composite_reliability_decay should be 0.75, got {}",
            result.composite_reliability_decay()
        );
    }

    #[test]
    fn t_serde_backward_compat_old_json_loads() {
        // 旧 JSON (rdc/gds なし) → デフォルト値 1.0 が入って load 成功
        let old = r#"{"task_id":"x","pass_at_k":1.0,"pass_consecutive_k":1.0,
                      "mean_score":0.8,"variance":0.0,"individual_scores":[0.8,0.8]}"#;
        let task: MultiRunTaskScore = serde_json::from_str(old).unwrap();
        assert!((task.reliability_decay - 1.0).abs() < 1e-6);
        assert!((task.graceful_degradation - 1.0).abs() < 1e-6);
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
            capability_tier: CapabilityTier::InstructionFollowing,
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
            capability_tier: CapabilityTier::InstructionFollowing,
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
            capability_tier: CapabilityTier::InstructionFollowing,
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
            capability_tier: CapabilityTier::InstructionFollowing,
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
            capability_tier: CapabilityTier::InstructionFollowing,
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
            capability_tier: CapabilityTier::InstructionFollowing,
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
            capability_tier: CapabilityTier::InstructionFollowing,
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
            capability_tier: CapabilityTier::InstructionFollowing,
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
            capability_tier: CapabilityTier::InstructionFollowing,
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
        // handoff 05-07g Phase 5: smoke_failure_chain_pair 追加で 41
        // handoff 05-07h 後継: smoke_partial_success_chain 追加で 42
        // Plan A G-4c (項目 230 後続): halluc 3 task 追加で 45
        // 項目 242 Lab v21: success_fact 5 task 追加で 50
        let suite = BenchmarkSuite::default_tasks();
        assert_eq!(
            suite.tasks.len(),
            50,
            "タスク数は 50 であるべき (Plan A G-4c + 項目 242 Lab v21)"
        );
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
            capability_tier: CapabilityTier::InstructionFollowing,
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
            capability_tier: CapabilityTier::InstructionFollowing,
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
            capability_tier: CapabilityTier::InstructionFollowing,
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

    // ========================================================================
    // Phase 1 Red — Option A migration (.claude/plan/agenther-option-a-migration.md)
    //
    // Why: handoff 05-07h の Option B (run_k が `Option<&MemoryStore>` を取り、内部で
    // ephemeral store の events を `export_to` で persistent に bulk copy) は移行措置。
    // Option A 完成形では run_k が persistent `&MemoryStore` を必須引数で受け取り、
    // events は直接 persistent に書き込まれる (ephemeral / export_to 廃止)。
    //
    // 本 typecheck fn は実行されない (`#[allow(dead_code)]`)。コンパイル時に run_k の
    // 最終引数が `&MemoryStore` (Option ではない) であることを強制する。
    // 現状 (Option<&MemoryStore>): E0308 で build red → cargo test がそもそも通らない。
    // Phase 2 で signature を `&MemoryStore` に変更したら build green。
    // ========================================================================

    // ========================================================================
    // Phase 1 Red: AgentFloor 6-tier capability ladder tests (項目 213 候補)
    //
    // 由来: arxiv 2605.00334 AgentFloor — small open-weight 専用 6-tier benchmark
    // (T1 InstructionFollowing → T6 LongHorizonPlanning)。本テストは plan
    // `agentfloor-tier-eval-impl.md` の Phase 1 Red、CapabilityTier enum + tag +
    // agentfloor_tasks + compute_capability_tier_avg 未実装で全 fail (compile error)。
    // ========================================================================

    #[test]
    fn test_capability_tier_all_returns_six() {
        // T1〜T6 の 6 tier を返す
        let all = CapabilityTier::all();
        assert_eq!(all.len(), 6, "AgentFloor 6-tier 必須");
    }

    #[test]
    fn test_capability_tier_label_short_code_unique() {
        let all = CapabilityTier::all();
        let labels: std::collections::HashSet<&str> = all.iter().map(|t| t.label()).collect();
        let codes: std::collections::HashSet<&str> = all.iter().map(|t| t.short_code()).collect();
        assert_eq!(labels.len(), 6, "label 重複あり: {:?}", labels);
        assert_eq!(codes.len(), 6, "short_code 重複あり: {:?}", codes);
    }

    #[test]
    fn test_default_tasks_capability_tier_coverage() {
        let suite = BenchmarkSuite::default_tasks();
        // 全 task に capability_tier tag 付与
        for task in &suite.tasks {
            // capability_tier field の存在チェック (compile-time + 値チェック)
            let _t: CapabilityTier = task.capability_tier;
        }
        // T1-T5 各 ≥ 1 件、T6 ≥ 2 件 (plan `agentfloor-tier-eval-impl.md` 4.2 移行戦略)
        for tier in CapabilityTier::all() {
            let count = suite
                .tasks
                .iter()
                .filter(|t| t.capability_tier == tier)
                .count();
            let min_required = if tier == CapabilityTier::LongHorizonPlanning {
                2
            } else {
                1
            };
            assert!(
                count >= min_required,
                "{:?} には最低 {} 件必要、実際 {} 件",
                tier,
                min_required,
                count
            );
        }
    }

    #[test]
    fn test_agentfloor_tasks_30_count() {
        let suite = BenchmarkSuite::agentfloor_tasks();
        assert_eq!(
            suite.tasks.len(),
            30,
            "AgentFloor は 30 task (5/tier × 6 tier)"
        );
        // 各 tier 正確に 5 件
        for tier in CapabilityTier::all() {
            let count = suite
                .tasks
                .iter()
                .filter(|t| t.capability_tier == tier)
                .count();
            assert_eq!(
                count, 5,
                "AgentFloor 各 tier 5 task 規格、{:?} は {} 件",
                tier, count
            );
        }
    }

    #[test]
    fn test_compute_capability_tier_avg_basic() {
        // 既存 compute_tier_avg パターン参照
        let task_scores: Vec<MultiRunTaskScore> = vec![
            MultiRunTaskScore::from_scores("t1_a".into(), vec![0.8, 0.9, 0.7], 0.5),
            MultiRunTaskScore::from_scores("t1_b".into(), vec![0.6, 0.7, 0.5], 0.5),
        ];
        let descs = std::collections::HashMap::from([
            (
                "t1_a".to_string(),
                BenchmarkTask {
                    id: "t1_a".into(),
                    name: "test t1_a".into(),
                    input: "x".into(),
                    expected_tools: vec![],
                    expected_keywords: vec![],
                    max_iterations: 5,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::InstructionFollowing,
                },
            ),
            (
                "t1_b".to_string(),
                BenchmarkTask {
                    id: "t1_b".into(),
                    name: "test t1_b".into(),
                    input: "y".into(),
                    expected_tools: vec![],
                    expected_keywords: vec![],
                    max_iterations: 5,
                    category: TaskCategory::Reasoning,
                    tier: TaskTier::Core,
                    capability_tier: CapabilityTier::InstructionFollowing,
                },
            ),
        ]);
        let avg =
            compute_capability_tier_avg(&task_scores, &descs, CapabilityTier::InstructionFollowing);
        // T1 平均 = (0.8 + 0.6) / 2 = 0.7 (composite_score)
        assert!(avg.is_some(), "T1 平均は計算可能");
        let v = avg.unwrap();
        assert!((v - 0.7).abs() < 1e-6, "T1 平均 0.7 期待、実際 {:.4}", v);
        // 空 tier (T6) は None
        let t6_avg =
            compute_capability_tier_avg(&task_scores, &descs, CapabilityTier::LongHorizonPlanning);
        assert!(
            t6_avg.is_none(),
            "T6 task ゼロで None 期待、実際 {:?}",
            t6_avg
        );
    }

    // ========================================================================
    // Phase 3 Refactor: weakest_tier / paper_delta_map / is_ladder_mode_enabled
    //
    // 由来: arxiv 2605.00334 AgentFloor — plan §5 Phase 3 (Refactor)。
    // 実装前に先にテストを追加し Red を確認する (TDD strict)。
    // ========================================================================

    #[test]
    fn test_weakest_tier_basic() {
        // tier_avg_scores: T1=0.80, T2=0.70, T3=0.60, T4=0.50, T5=0.40, T6=0.20
        let scores: [Option<f64>; 6] = [
            Some(0.80),
            Some(0.70),
            Some(0.60),
            Some(0.50),
            Some(0.40),
            Some(0.20), // T6 が最低
        ];
        let result = MultiRunBenchmarkResult {
            task_scores: vec![],
            duration_secs: 0.0,
            core_avg_score: None,
            extended_avg_score: None,
            tier_avg_scores: Some(scores),
            critic_stats: None,
        };
        let weakest = result.weakest_tier();
        assert!(weakest.is_some(), "weakest_tier は Some を返すべき");
        let (tier, score) = weakest.unwrap();
        assert_eq!(tier, CapabilityTier::LongHorizonPlanning, "T6 が最低スコア");
        assert!(
            (score - 0.20).abs() < 1e-6,
            "score=0.20 期待、実際 {}",
            score
        );
    }

    #[test]
    fn test_paper_delta_map_basic() {
        // T1=0.80 (paper 0.85 → delta -0.05)、T4=0.55 (paper 0.50 → delta +0.05)、T6=None
        let scores: [Option<f64>; 6] = [
            Some(0.80), // T1
            Some(0.70), // T2
            Some(0.60), // T3
            Some(0.55), // T4
            Some(0.40), // T5
            None,       // T6 計測なし
        ];
        let result = MultiRunBenchmarkResult {
            task_scores: vec![],
            duration_secs: 0.0,
            core_avg_score: None,
            extended_avg_score: None,
            tier_avg_scores: Some(scores),
            critic_stats: None,
        };
        let deltas = result.paper_delta_map();
        // T1: 0.80 - 0.85 = -0.05
        let t1 = deltas[0].expect("T1 delta は Some");
        assert!(
            (t1 - (-0.05)).abs() < 1e-6,
            "T1 delta=-0.05 期待、実際 {}",
            t1
        );
        // T4: 0.55 - 0.50 = +0.05
        let t4 = deltas[3].expect("T4 delta は Some");
        assert!((t4 - 0.05).abs() < 1e-6, "T4 delta=+0.05 期待、実際 {}", t4);
        // T6: None (計測なし)
        assert!(deltas[5].is_none(), "T6 None (計測なし)");
    }

    #[test]
    fn test_is_ladder_mode_enabled() {
        // Rust 2024 edition では set_var/remove_var が unsafe (data race の可能性)
        // シングルスレッドテストのため unsafe ブロックで包む
        unsafe {
            // env 未設定ならば false
            std::env::remove_var("BONSAI_BENCH_LADDER");
            assert!(
                !is_ladder_mode_enabled(),
                "BONSAI_BENCH_LADDER 未設定で false"
            );
            // "1" なら true
            std::env::set_var("BONSAI_BENCH_LADDER", "1");
            assert!(is_ladder_mode_enabled(), "BONSAI_BENCH_LADDER=1 で true");
            // "0" なら false
            std::env::set_var("BONSAI_BENCH_LADDER", "0");
            assert!(!is_ladder_mode_enabled(), "BONSAI_BENCH_LADDER=0 で false");
            // 片付け
            std::env::remove_var("BONSAI_BENCH_LADDER");
        }
    }

    #[allow(dead_code)]
    fn _phase1_red_run_k_signature_typecheck(
        suite: &BenchmarkSuite,
        cfg: &AgentConfig,
        backend: &dyn LlmBackend,
        tools: &ToolRegistry,
        path_guard: &PathGuard,
        cancel: &CancellationToken,
        multi: &MultiRunConfig,
        store: &MemoryStore,
    ) -> Result<MultiRunBenchmarkResult> {
        // Option A 後の呼び出し形 (`&store` を直接渡す、`Some(..)` 不要)
        suite.run_k(cfg, backend, tools, path_guard, cancel, multi, 0.5, store)
    }

    // ── Sub-Phase 2C: frontier benchmark composite + run_k populate tests ──

    #[test]
    fn t_multirun_task_score_final_context_tokens_default_none() {
        // from_scores 経路 (legacy / fixture) では final_context_tokens は None で初期化される。
        // run_k 経路でのみ populate されることを契約として固定。
        let mrt = MultiRunTaskScore::from_scores("t1".into(), vec![0.5, 0.6, 0.7], 0.5);
        assert!(mrt.final_context_tokens.is_none());
        // 空入力経路 (k=0) も None で初期化
        let mrt_empty = MultiRunTaskScore::from_scores("t1".into(), vec![], 0.5);
        assert!(mrt_empty.final_context_tokens.is_none());
    }

    #[test]
    fn t_composite_frontier_bucket_scores_aggregates_tasks() {
        // 4 task が異なる final_context_tokens を持つとき、bucket 別 mean score を返す。
        // boundaries=[2048, 4096, 8192] (4 bucket = [0,2K) / [2K,4K) / [4K,8K) / [8K,∞))
        let mut t1 = MultiRunTaskScore::from_scores("t1".into(), vec![0.8], 0.5);
        t1.final_context_tokens = Some(1500); // bucket 0
        let mut t2 = MultiRunTaskScore::from_scores("t2".into(), vec![0.5], 0.5);
        t2.final_context_tokens = Some(3000); // bucket 1
        let mut t3 = MultiRunTaskScore::from_scores("t3".into(), vec![0.4], 0.5);
        t3.final_context_tokens = Some(5000); // bucket 2
        let mut t4 = MultiRunTaskScore::from_scores("t4".into(), vec![0.2], 0.5);
        t4.final_context_tokens = Some(12000); // bucket 3 (unbounded)

        let result = MultiRunBenchmarkResult {
            task_scores: vec![t1, t2, t3, t4],
            duration_secs: 1.0,
            core_avg_score: None,
            extended_avg_score: None,
            tier_avg_scores: None,
            critic_stats: None,
        };
        let composite = result.composite_frontier_bucket_scores(&[2048, 4096, 8192]);
        assert_eq!(composite.len(), 4);
        // 各 bucket は 1 task のみで mean = single task の mean_score (== single score)
        assert!((composite[0].1 - 0.8).abs() < 1e-9, "bucket 0 mean = 0.8");
        assert!((composite[1].1 - 0.5).abs() < 1e-9, "bucket 1 mean = 0.5");
        assert!((composite[2].1 - 0.4).abs() < 1e-9, "bucket 2 mean = 0.4");
        assert!((composite[3].1 - 0.2).abs() < 1e-9, "bucket 3 mean = 0.2");
    }

    #[test]
    fn t_composite_frontier_bucket_scores_empty_when_no_tokens() {
        // 全 task の final_context_tokens が None なら空 Vec を返す (env unset セッション互換)。
        let t1 = MultiRunTaskScore::from_scores("t1".into(), vec![0.8], 0.5);
        let t2 = MultiRunTaskScore::from_scores("t2".into(), vec![0.5], 0.5);
        let result = MultiRunBenchmarkResult {
            task_scores: vec![t1, t2],
            duration_secs: 1.0,
            core_avg_score: None,
            extended_avg_score: None,
            tier_avg_scores: None,
            critic_stats: None,
        };
        assert!(
            result
                .composite_frontier_bucket_scores(&[2048, 4096, 8192])
                .is_empty()
        );
    }

    #[test]
    fn t_composite_frontier_bucket_scores_empty_boundaries() {
        // boundaries 空 → 空 Vec (frontier 機構未利用の経路)
        let mut t1 = MultiRunTaskScore::from_scores("t1".into(), vec![0.8], 0.5);
        t1.final_context_tokens = Some(1500);
        let result = MultiRunBenchmarkResult {
            task_scores: vec![t1],
            duration_secs: 1.0,
            core_avg_score: None,
            extended_avg_score: None,
            tier_avg_scores: None,
            critic_stats: None,
        };
        assert!(result.composite_frontier_bucket_scores(&[]).is_empty());
    }

    // ── Sub-Phase 2F: composite_frontier_inject_scores tests ──

    #[test]
    fn t_composite_frontier_inject_scores_aggregates_t6_tasks() {
        // 2 T6 task が同じ 4 size の inject_scores を持つとき、size 別 mean を返す。
        let mut t1 = MultiRunTaskScore::from_scores("t6_a".into(), vec![0.6], 0.5);
        t1.frontier_inject_scores = vec![(0, 0.80), (4, 0.70), (8, 0.55), (16, 0.30)];
        let mut t2 = MultiRunTaskScore::from_scores("t6_b".into(), vec![0.5], 0.5);
        t2.frontier_inject_scores = vec![(0, 0.70), (4, 0.60), (8, 0.45), (16, 0.20)];

        let result = MultiRunBenchmarkResult {
            task_scores: vec![t1, t2],
            duration_secs: 1.0,
            core_avg_score: None,
            extended_avg_score: None,
            tier_avg_scores: None,
            critic_stats: None,
        };
        let composite = result.composite_frontier_inject_scores();
        // 4 size × mean (t1 + t2)/2 = [(0, 0.75), (4, 0.65), (8, 0.50), (16, 0.25)]
        assert_eq!(composite.len(), 4);
        assert_eq!(composite[0].0, 0);
        assert!((composite[0].1 - 0.75).abs() < 1e-9);
        assert_eq!(composite[1].0, 4);
        assert!((composite[1].1 - 0.65).abs() < 1e-9);
        assert_eq!(composite[2].0, 8);
        assert!((composite[2].1 - 0.50).abs() < 1e-9);
        assert_eq!(composite[3].0, 16);
        assert!((composite[3].1 - 0.25).abs() < 1e-9);
    }

    #[test]
    fn t_composite_frontier_inject_scores_empty_when_no_data() {
        // 全 task の frontier_inject_scores が空 → 空 Vec を返す (env unset セッション互換)。
        let t1 = MultiRunTaskScore::from_scores("t1".into(), vec![0.8], 0.5);
        let t2 = MultiRunTaskScore::from_scores("t2".into(), vec![0.5], 0.5);
        let result = MultiRunBenchmarkResult {
            task_scores: vec![t1, t2],
            duration_secs: 1.0,
            core_avg_score: None,
            extended_avg_score: None,
            tier_avg_scores: None,
            critic_stats: None,
        };
        assert!(result.composite_frontier_inject_scores().is_empty());
    }

    #[test]
    fn t_composite_frontier_inject_scores_size_ascending_order() {
        // 単一 task で size が逆順に格納されていても composite は size 昇順で出力する
        // (BTreeMap 経由の deterministic 順序保証)。
        let mut t1 = MultiRunTaskScore::from_scores("t6".into(), vec![0.5], 0.5);
        t1.frontier_inject_scores = vec![(16, 0.2), (4, 0.6), (8, 0.4), (0, 0.8)];
        let result = MultiRunBenchmarkResult {
            task_scores: vec![t1],
            duration_secs: 1.0,
            core_avg_score: None,
            extended_avg_score: None,
            tier_avg_scores: None,
            critic_stats: None,
        };
        let composite = result.composite_frontier_inject_scores();
        let sizes: Vec<usize> = composite.iter().map(|(s, _)| *s).collect();
        assert_eq!(sizes, vec![0, 4, 8, 16], "size 昇順");
    }
}
