//! agent_loop の型定義集約モジュール（refactor 3/8）
//!
//! `AgentLoopResult`/`StepOutcome`/`LoopState`/`TokenBudgetTracker`/
//! `OutcomeAction`/`StallDetector`/`StepContext` を集約。
//!
//! ライフタイム伝播注意: `LoopState<'a>`/`StepContext<'a>` は
//! `MiddlewareChain<'a>` 経由で借用元のミドルウェアにライフタイムを束縛する。

use crate::agent::error_recovery::{CircuitBreaker, LoopDetector, TrialSummary};
use crate::agent::middleware::MiddlewareChain;
use crate::agent::validate::PathGuard;
use crate::cancel::CancellationToken;
use crate::memory::store::MemoryStore;
use crate::runtime::inference::LlmBackend;
use crate::runtime::model_router::AdvisorConfig;
use crate::safety::secrets::SecretsFilter;
use crate::tools::{ToolRegistry, ToolResultCache};

use super::config::AgentConfig;

/// エージェントループの構造化戻り値
#[derive(Debug, Clone)]
pub struct AgentLoopResult {
    pub answer: String,
    pub iterations_used: usize,
    pub tools_called: Vec<String>,
}

/// エージェントのステップ結果
#[derive(Debug)]
pub enum StepOutcome {
    /// 最終回答（ループ終了）
    FinalAnswer(String),
    /// ツール実行後、ループ継続（使用ツール名を保持）
    Continue(Vec<String>),
    /// エラーで中断
    Aborted(String),
}

/// エージェントループのミュータブル状態を集約
///
/// run_agent_loop_with_session の局所変数が多すぎるため構造体に抽出。
/// 将来のミドルウェアチェーン化の基盤。
pub struct LoopState<'a> {
    pub circuit_breaker: CircuitBreaker,
    pub loop_detector: LoopDetector,
    pub stall_detector: StallDetector,
    pub advisor: AdvisorConfig,
    pub all_tools: Vec<String>,
    pub consecutive_failures: usize,
    pub iteration: usize,
    /// トークン予算追跡（diminishing returns検出用、macOS26/Agent知見）
    pub token_budget: TokenBudgetTracker,
    /// ミドルウェアチェーン（DeerFlow知見: 5段パイプライン）
    pub middleware_chain: MiddlewareChain<'a>,
    /// ツール結果キャッシュ（読取専用ツールの重複呼び出し回避）
    pub tool_cache: ToolResultCache,
    /// 試行サマリー記憶（GrandCode知見: 失敗履歴を保持し再計画時に注入）
    pub trial_summary: TrialSummary,
}

impl<'a> LoopState<'a> {
    pub fn new(advisor: AdvisorConfig) -> Self {
        Self {
            circuit_breaker: CircuitBreaker::default(),
            loop_detector: LoopDetector::default(),
            stall_detector: StallDetector::default(),
            advisor,
            all_tools: Vec::new(),
            consecutive_failures: 0,
            iteration: 0,
            token_budget: TokenBudgetTracker::default(),
            middleware_chain: MiddlewareChain::default(),
            tool_cache: ToolResultCache::new(),
            trial_summary: TrialSummary::default(),
        }
    }
}

/// トークン予算追跡器（macOS26/Agent TokenBudgetTracker パターン）
///
/// 累積トークンを追跡し、diminishing returns（連続低出力）を検出。
/// 90%でnudge、100%で停止、5ターン連続100トークン未満で早期停止推奨。
pub struct TokenBudgetTracker {
    total_tokens: usize,
    budget: usize,
    recent_outputs: Vec<usize>,
    low_output_threshold: usize,
    diminishing_window: usize,
}

impl TokenBudgetTracker {
    pub fn new(budget: usize) -> Self {
        Self {
            total_tokens: 0,
            budget,
            recent_outputs: Vec::new(),
            low_output_threshold: 100,
            diminishing_window: 5,
        }
    }

    /// ステップのトークン使用量を記録
    pub fn record(&mut self, tokens: usize) {
        self.total_tokens += tokens;
        self.recent_outputs.push(tokens);
        if self.recent_outputs.len() > self.diminishing_window * 2 {
            self.recent_outputs.remove(0);
        }
    }

    /// 予算使用率 (0.0〜1.0+)
    pub fn usage_ratio(&self) -> f64 {
        self.total_tokens as f64 / self.budget as f64
    }

    /// diminishing returns 検出（直近N回が低出力）
    pub fn is_diminishing(&self) -> bool {
        if self.recent_outputs.len() < self.diminishing_window {
            return false;
        }
        let recent = &self.recent_outputs[self.recent_outputs.len() - self.diminishing_window..];
        recent.iter().all(|&t| t < self.low_output_threshold)
    }

    /// 予算チェック: None=OK, Some(msg)=nudge/stop
    pub fn check(&self) -> Option<&'static str> {
        if self.usage_ratio() >= 1.0 {
            Some("トークン予算の上限に達しました。タスクを完了してください。")
        } else if self.is_diminishing() {
            Some("出力が少なくなっています。早めにタスクを完了してください。")
        } else if self.usage_ratio() >= 0.9 {
            Some("トークン予算の90%を使いました。すぐにタスクを完了してください。")
        } else {
            None
        }
    }
}

impl Default for TokenBudgetTracker {
    fn default() -> Self {
        Self::new(8000) // llama-server の max_tokens デフォルト
    }
}

/// Outcome ハンドラの結果
pub enum OutcomeAction {
    /// ループ終了（最終結果）
    Return(AgentLoopResult),
    /// 次のイテレーションへ継続
    Continue,
}

/// 停滞検出器: 進捗のないステップが続いた場合に再計画を促す
pub struct StallDetector {
    no_progress_count: usize,
    stall_threshold: usize,
    last_output_hash: u64,
}

impl StallDetector {
    pub fn new(threshold: usize) -> Self {
        Self {
            no_progress_count: 0,
            stall_threshold: threshold,
            last_output_hash: 0,
        }
    }

    /// ステップ結果を記録し、停滞を検出したらtrueを返す
    pub fn record_step(&mut self, tools_succeeded: bool, output_hash: u64) -> bool {
        if !tools_succeeded || output_hash == self.last_output_hash {
            self.no_progress_count += 1;
        } else {
            self.no_progress_count = 0;
        }
        self.last_output_hash = output_hash;
        self.no_progress_count >= self.stall_threshold
    }

    pub fn reset(&mut self) {
        self.no_progress_count = 0;
    }

    /// 停滞検出のしきい値（テスト用）
    pub fn stall_threshold(&self) -> usize {
        self.stall_threshold
    }
}

impl Default for StallDetector {
    fn default() -> Self {
        Self::new(3)
    }
}

/// ステップ実行に必要なコンテキスト
pub struct StepContext<'a> {
    pub backend: &'a dyn LlmBackend,
    pub tools: &'a ToolRegistry,
    pub path_guard: &'a PathGuard,
    pub config: &'a AgentConfig,
    pub cancel: &'a CancellationToken,
    pub secrets_filter: &'a SecretsFilter,
    pub store: Option<&'a MemoryStore>,
}
