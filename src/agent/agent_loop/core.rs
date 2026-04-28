//! agent_loop の中核ループモジュール（refactor 8/8）
//!
//! `run_agent_loop` / `run_agent_loop_with_session` のメインループ、
//! タスク開始時の自動チェックポイント、`emit_event` ヘルパー（EventStore 疎結合）を集約。

use anyhow::Result;

use crate::agent::checkpoint::CheckpointManager;
use crate::agent::context_inject::inject_contextual_memories;
use crate::agent::conversation::{Message, Role, Session};
use crate::agent::event_store::{EventStore, EventType};
use crate::agent::validate::PathGuard;
use crate::cancel::CancellationToken;
use crate::memory::store::MemoryStore;
use crate::observability::logger::{LogLevel, log_event};
use crate::runtime::inference::LlmBackend;
use crate::safety::secrets::SecretsFilter;
use crate::tools::ToolRegistry;

use super::advisor_inject::inject_planning_step;
use super::config::AgentConfig;
use super::outcome::handle_outcome;
use super::state::{AgentLoopResult, LoopState, OutcomeAction, StepContext};
use super::step::execute_step;

/// EventStore へイベントを emit する疎結合ヘルパー（項目162: P1 Step 5 ランタイム統合）
///
/// - `store=None` ならno-op（インメモリモードでイベント記録不要）
/// - `append` 失敗は `log_event(Warn, "event", ...)` で握る（コアループは止めない）
/// - AuditLog は粗粒度メトリクス、Event はシーケンス保存（役割分離）
pub(crate) fn emit_event(
    store: Option<&MemoryStore>,
    session_id: &str,
    event_type: &EventType,
    event_data: &str,
    step_index: Option<usize>,
) {
    if let Some(s) = store {
        let es = EventStore::new(s.conn());
        if let Err(e) = es.append(session_id, event_type, event_data, step_index) {
            log_event(
                LogLevel::Warn,
                "event",
                &format!("EventStore.append 失敗 (無視): {e}"),
            );
        }
    }
}

/// エージェントループ全体を実行
pub fn run_agent_loop(
    input: &str,
    backend: &dyn LlmBackend,
    tools: &ToolRegistry,
    path_guard: &PathGuard,
    config: &AgentConfig,
    cancel: &CancellationToken,
    store: Option<&MemoryStore>,
) -> Result<AgentLoopResult> {
    let mut session = Session::new();
    let now = chrono::Local::now();
    let date_str = now.format("%Y年%m月%d日(%A) %H:%M");
    let system_with_date = format!(
        "{}

## 現在の日時
現在は{}です。正確な現在時刻が必要な場合は shell ツールで date コマンドを実行してください。",
        config.system_prompt, date_str
    );
    session.add_message(Message::system(&system_with_date));
    session.add_message(Message::user(input));

    run_agent_loop_with_session(
        &mut session,
        backend,
        tools,
        path_guard,
        config,
        cancel,
        store,
    )
}

/// 既存セッションでエージェントループを実行（セッション再開用）
pub fn run_agent_loop_with_session(
    session: &mut Session,
    backend: &dyn LlmBackend,
    tools: &ToolRegistry,
    path_guard: &PathGuard,
    config: &AgentConfig,
    cancel: &CancellationToken,
    store: Option<&MemoryStore>,
) -> Result<AgentLoopResult> {
    // 経験記録用にユーザー入力を取得
    let task_context: String = session
        .messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, Role::User))
        .map(|m| m.content.clone())
        .unwrap_or_default();

    let secrets_filter = SecretsFilter::default();

    // セッション開始時に期限切れ情報を自動パージ
    if let Some(s) = store {
        match s.purge_all_expired() {
            Ok(n) if n > 0 => log_event(LogLevel::Info, "ttl", &format!("期限切れ{}件をパージ", n)),
            _ => {}
        }
    }

    // EventStore へ SessionStart + UserMessage emit (項目162: P1 Step 5 ランタイム統合)
    emit_event(
        store,
        &session.id,
        &EventType::SessionStart,
        &serde_json::json!({ "task_context": task_context.chars().take(500).collect::<String>() })
            .to_string(),
        None,
    );
    emit_event(
        store,
        &session.id,
        &EventType::UserMessage,
        &serde_json::json!({ "content": task_context.chars().take(500).collect::<String>() })
            .to_string(),
        None,
    );

    inject_contextual_memories(session, &task_context, store);
    inject_planning_step(session, &task_context);

    // Advisor設定ログ（初回のみ、セッション最初のメッセージが2件=system+userの場合）
    if session.messages.len() <= 2 {
        config.advisor.log_startup();
    }

    // タスク開始時の自動チェックポイント（auto_checkpoint=true 時、git+DB）
    if config.auto_checkpoint {
        let _ = create_task_start_checkpoint(session, &task_context, store);
    }

    let mut state = LoopState::new(config.advisor.clone());
    // ミドルウェアチェーン構築（DeerFlow知見: 5段パイプライン）
    state.middleware_chain = crate::agent::middleware::build_default_chain(&session.id, store);

    let ctx = StepContext {
        backend,
        tools,
        path_guard,
        config,
        cancel,
        secrets_filter: &secrets_filter,
        store,
    };

    let task_start = std::time::Instant::now();
    let mut final_iteration = 0;
    for iteration in 0..config.max_iterations {
        // ウォールクロックタイムアウトチェック
        if let Some(timeout) = config.task_timeout {
            if task_start.elapsed() > timeout {
                let timeout_msg = format!(
                    "[タイムアウト] {}秒以内に完了できませんでした",
                    timeout.as_secs()
                );
                log_event(LogLevel::Warn, "timeout", &timeout_msg);
                emit_event(
                    store,
                    &session.id,
                    &EventType::SessionEnd,
                    &serde_json::json!({
                        "reason": "timeout",
                        "iterations": iteration,
                        "tool_count": state.all_tools.len()
                    })
                    .to_string(),
                    Some(iteration),
                );
                return Ok(AgentLoopResult {
                    answer: timeout_msg,
                    iterations_used: iteration,
                    tools_called: state.all_tools,
                });
            }
        }
        state.iteration = iteration;
        final_iteration = iteration + 1;

        // before_stepフック: LLM呼出前にミドルウェア介入（NAT知見、項目142）
        if let Some(abort_reason) = state.middleware_chain.run_before_step(session, iteration) {
            emit_event(
                store,
                &session.id,
                &EventType::SessionEnd,
                &serde_json::json!({
                    "reason": "abort",
                    "abort_reason": abort_reason,
                    "iterations": iteration,
                    "tool_count": state.all_tools.len()
                })
                .to_string(),
                Some(iteration),
            );
            return Ok(AgentLoopResult {
                answer: format!("[中断] {abort_reason}"),
                iterations_used: iteration,
                tools_called: state.all_tools,
            });
        }

        let step_start = std::time::Instant::now();
        let outcome = execute_step(
            session,
            &ctx,
            &mut state.circuit_breaker,
            &mut state.loop_detector,
            iteration,
            &mut state.tool_cache,
            &mut state.cycle_detector,
        )?;

        let duration_ms = step_start.elapsed().as_millis() as u64;

        match handle_outcome(
            outcome,
            session,
            &mut state,
            &task_context,
            store,
            config.max_iterations,
            final_iteration,
            iteration,
            duration_ms,
        ) {
            OutcomeAction::Return(result) => {
                emit_event(
                    store,
                    &session.id,
                    &EventType::SessionEnd,
                    &serde_json::json!({
                        "reason": "completed",
                        "iterations": result.iterations_used,
                        "tool_count": result.tools_called.len()
                    })
                    .to_string(),
                    Some(iteration),
                );
                return Ok(result);
            }
            OutcomeAction::Continue => continue,
        }
    }

    let timeout_msg = format!(
        "最大ステップ数({})に達しました。タスクを完了できませんでした。",
        config.max_iterations
    );
    emit_event(
        store,
        &session.id,
        &EventType::SessionEnd,
        &serde_json::json!({
            "reason": "max_iterations",
            "iterations": final_iteration,
            "tool_count": state.all_tools.len()
        })
        .to_string(),
        Some(final_iteration),
    );
    Ok(AgentLoopResult {
        answer: format!("[中断] {timeout_msg}"),
        iterations_used: final_iteration,
        tools_called: state.all_tools,
    })
}

/// タスク開始時の自動チェックポイントを作成
///
/// store があれば SQLite 永続化、なければインメモリ。
/// git stash 失敗 / リポジトリ外でも黙殺（コア機能ではない）。
pub(super) fn create_task_start_checkpoint(
    session: &Session,
    task_context: &str,
    store: Option<&MemoryStore>,
) -> Option<i64> {
    let desc = format!(
        "auto-start: {}",
        task_context.chars().take(60).collect::<String>()
    );
    let session_id = session.id.clone();
    let mut mgr = if let Some(s) = store {
        CheckpointManager::with_persistence(s.conn(), Some(session_id))
    } else {
        CheckpointManager::new()
    };
    match mgr.create(&desc) {
        Ok(id) => {
            log_event(
                LogLevel::Info,
                "checkpoint",
                &format!("タスク開始時CP作成 id={id}"),
            );
            Some(id)
        }
        Err(e) => {
            log_event(
                LogLevel::Warn,
                "checkpoint",
                &format!("CP作成失敗（無視）: {e}"),
            );
            None
        }
    }
}
