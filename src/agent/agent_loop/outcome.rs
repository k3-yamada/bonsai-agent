//! agent_loop の Outcome ディスパッチモジュール（refactor 6/8）
//!
//! `StepOutcome` を `OutcomeAction` に解釈する `handle_outcome` と、
//! 計画プレステップ要否を判定する `detect_task_complexity` を集約。

use crate::agent::conversation::Session;
use crate::agent::middleware::StepResult as MwStepResult;
use crate::memory::store::MemoryStore;
use crate::observability::logger::{LogLevel, log_event};

use super::advisor_inject::{inject_replan_on_stall, inject_verification_step};
use super::state::{AgentLoopResult, LoopState, OutcomeAction, StepOutcome};
use super::support::{check_invariants, compute_output_hash, record_abort, record_success};

/// Outcome ディスパッチ
///
/// FinalAnswer → 完了前自己検証 or record_success
/// Aborted → 中断記録
/// Continue → 停滞検出+再計画+コンパクション
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_outcome(
    outcome: StepOutcome,
    session: &mut Session,
    state: &mut LoopState,
    task_context: &str,
    store: Option<&MemoryStore>,
    max_iterations: usize,
    final_iteration: usize,
    iteration: usize,
    duration_ms: u64,
) -> OutcomeAction {
    match outcome {
        StepOutcome::FinalAnswer(answer) => {
            let mw_result = MwStepResult {
                outcome_type: "final_answer",
                iteration,
                duration_ms,
                tools_used: vec![],
                tools_succeeded: true,
                output_hash: 0,
                consecutive_failures: 0,
            };
            state.middleware_chain.run_after_step(session, &mw_result);
            if inject_verification_step(
                session,
                &mut state.advisor,
                task_context,
                &answer,
                iteration,
                max_iterations,
                store,
                &state.trial_summary,
            ) {
                return OutcomeAction::Continue;
            }
            // 不変条件チェック（非ブロッキング警告）
            let violations = check_invariants(session, task_context);
            for v in &violations {
                log_event(LogLevel::Warn, "invariant", v);
            }
            record_success(store, session, task_context, &answer);
            OutcomeAction::Return(AgentLoopResult {
                answer,
                iterations_used: final_iteration,
                tools_called: std::mem::take(&mut state.all_tools),
            })
        }
        StepOutcome::Aborted(reason) => {
            state.consecutive_failures += 1;
            let mw_result = MwStepResult {
                outcome_type: "aborted",
                iteration,
                duration_ms,
                tools_used: vec![],
                tools_succeeded: false,
                output_hash: 0,
                consecutive_failures: state.consecutive_failures,
            };
            state.middleware_chain.run_after_step(session, &mw_result);
            record_abort(store, session, task_context, &reason);
            OutcomeAction::Return(AgentLoopResult {
                answer: format!("[中断] {reason}"),
                iterations_used: final_iteration,
                tools_called: std::mem::take(&mut state.all_tools),
            })
        }
        StepOutcome::Continue(step_tools) => {
            let tools_succeeded = !step_tools.is_empty();
            if !tools_succeeded {
                state.consecutive_failures += 1;
            } else {
                state.consecutive_failures = 0;
            }
            // ミドルウェアチェーンでafter_step処理（Audit/ToolTrack/Stall/Compact/TokenBudget）
            let output_hash = compute_output_hash(session);
            let mw_result = MwStepResult {
                outcome_type: "continue",
                iteration,
                duration_ms,
                tools_used: step_tools.clone(),
                tools_succeeded,
                output_hash,
                consecutive_failures: state.consecutive_failures,
            };
            state.middleware_chain.run_after_step(session, &mw_result);
            // ツール追跡はミドルウェア外で保持（ReturnでのAgentLoopResult構築に必要）
            state.all_tools.extend(step_tools);
            // Advisor連携の停滞検出（ミドルウェアのStallとは別に、Advisor呼び出しが必要）
            inject_replan_on_stall(
                session,
                &mut state.stall_detector,
                &mut state.advisor,
                task_context,
                tools_succeeded,
                output_hash,
                store,
                &state.trial_summary,
            );
            OutcomeAction::Continue
        }
    }
}

/// タスクの複雑さを判定（複数ステップが必要か）
pub(super) fn detect_task_complexity(input: &str) -> bool {
    let complex_signals = [
        "作成して",
        "実装して",
        "修正して",
        "リファクタ",
        "調べて",
        "分析して",
        "比較して",
        "設計して",
        "テストを書",
        "ビルドして",
        "デプロイ",
        "ファイルを.*して.*して", // 複数動詞
    ];
    let signal_count = complex_signals
        .iter()
        .filter(|s| input.contains(*s))
        .count();
    // 2つ以上のシグナル or 長い入力（複雑なタスクの兆候）
    signal_count >= 2 || input.len() > 200
}
