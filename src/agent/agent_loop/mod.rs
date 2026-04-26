//! agent_loop モジュールファサード
//!
//! 構造改善 v2 Step 7（refactor 1/8〜8/8）で 2661 行のモノリスを 8 モジュールに
//! 分割した結果のエントリポイント。公開 API はこの mod.rs から `pub use` で
//! 全件再エクスポートし、外部呼び出しは 100% 後方互換。

#![allow(clippy::collapsible_if)]

mod advisor_inject;
mod config;
mod core;
mod outcome;
mod state;
mod step;
mod support;

pub use config::{AgentConfig, inference_for_task};
pub use core::{run_agent_loop, run_agent_loop_with_session};
pub use state::{
    AgentLoopResult, LoopState, OutcomeAction, StallDetector, StepContext, StepOutcome,
    TokenBudgetTracker,
};
pub use step::execute_step;

pub(crate) use core::emit_event;

#[cfg(test)]
mod tests;
