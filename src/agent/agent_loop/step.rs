//! agent_loop の 1 ステップ実行モジュール（refactor 7/8）
//!
//! `execute_step` ── ツール選択 → LLM 呼出 → パース → ツール実行までの一往復を担う。
//! テスト容易性のためループの内側として分離。

use anyhow::Result;

use crate::agent::conversation::{Message, Role, Session};
use crate::agent::error_recovery::{
    CircuitBreaker, FailureMode, LoopDetector, ParseErrorDetail, RecoveryAction, decide_recovery,
};
use crate::agent::parse::{coerce_tool_arguments, parse_assistant_output};
use crate::agent::tool_exec::{ValidatedCall, execute_validated_calls};
use crate::agent::validate::{Severity, validate_tool_call};
use crate::tools::ToolResultCache;
use crate::tools::detect_task_type;

use super::config::inference_for_task;
use super::state::{StepContext, StepOutcome};
use super::support::build_answer;

/// エージェントの1ステップを実行する（テスト容易性のためループの内側を分離）
pub fn execute_step(
    session: &mut Session,
    ctx: &StepContext<'_>,
    circuit_breaker: &mut CircuitBreaker,
    loop_detector: &mut LoopDetector,
    attempt: usize,
    tool_cache: &mut ToolResultCache,
) -> Result<StepOutcome> {
    if ctx.cancel.is_cancelled() {
        return Ok(StepOutcome::Aborted("キャンセルされました".to_string()));
    }

    // 1. 動的ツール選択
    let last_user_msg = session
        .messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, Role::User))
        .map(|m| m.content.as_str())
        .unwrap_or("");

    // セマンティックツール選択（ローカルONNX埋め込み、失敗時は自動でキーワード版にフォールバック）
    let selected_tools = ctx.tools.select_relevant_split_semantic(
        last_user_msg,
        ctx.config.max_tools_in_context,
        ctx.config.max_mcp_tools_in_context,
    );
    let tool_schemas: Vec<_> = selected_tools.iter().map(|t| t.schema()).collect();

    // 2. タスク種別に応じた推論パラメータ導出
    let task_type = detect_task_type(last_user_msg);
    let task_params = inference_for_task(task_type, &ctx.config.base_inference);

    // 3. LLM呼び出し（ストリーミング対応、タスク別パラメータ）
    let in_think = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let in_think_clone = in_think.clone();

    let result = ctx.backend.generate_with_params(
        &session.messages,
        &tool_schemas,
        &mut |token| {
            // ストリーミングトークンの表示
            if token.contains("<think>") {
                in_think_clone.store(true, std::sync::atomic::Ordering::Relaxed);
                eprint!("\x1b[2m"); // 薄色開始
            } else if token.contains("</think>") {
                in_think_clone.store(false, std::sync::atomic::Ordering::Relaxed);
                eprint!("\x1b[0m"); // 色リセット
            } else if token.contains("<tool_call>") || token.contains("</tool_call>") {
                // ツール呼び出しタグは非表示
            } else {
                eprint!("{token}");
            }
        },
        ctx.cancel,
        &task_params,
    )?;
    // 色リセットを保証
    if in_think.load(std::sync::atomic::Ordering::Relaxed) {
        eprint!("\x1b[0m");
    }
    eprintln!(); // 改行

    // 3. パース
    let parsed = match parse_assistant_output(&result.text) {
        Ok(p) => p,
        Err(e) => {
            let mode = FailureMode::ParseError(ParseErrorDetail::InvalidJson);
            let action = decide_recovery(&mode, attempt, ctx.config.max_retries);
            return match action {
                RecoveryAction::ExplainAndStop(msg) => Ok(StepOutcome::Aborted(msg)),
                _ => {
                    // エラー情報をコンテキストに追加してリトライを促す
                    session.add_message(Message::assistant(format!(
                        "パースエラー: {e}。修正します。"
                    )));
                    Ok(StepOutcome::Continue(Vec::new()))
                }
            };
        }
    };

    // 4. ツール呼び出しがなければ最終回答
    if parsed.tool_calls.is_empty() {
        let answer = build_answer(&parsed);
        session.add_message(Message::assistant(&answer));
        return Ok(StepOutcome::FinalAnswer(answer));
    }

    // 5. ツール呼び出し実行（並列対応）
    let assistant_text = result.text.clone();
    session.add_message(Message::assistant(&assistant_text));

    let known = ctx.tools.known_names();
    let mut validated: Vec<ValidatedCall<'_>> = Vec::new();

    for tool_call in &parsed.tool_calls {
        let action_key = format!("{}:{}", tool_call.name, tool_call.arguments);
        if loop_detector.record_and_check(&action_key) {
            let mode = FailureMode::LoopDetected;
            let action = decide_recovery(&mode, attempt, ctx.config.max_retries);
            if let RecoveryAction::Abort(msg) = action {
                return Ok(StepOutcome::Aborted(msg));
            }
        }
        if !circuit_breaker.is_available(&tool_call.name) {
            session.add_message(Message::tool(
                format!(
                    "ツール '{}' は連続で失敗したため使えません。別の方法を試してください。",
                    tool_call.name
                ),
                &tool_call.name,
            ));
            continue;
        }
        let validation = validate_tool_call(tool_call, &known, ctx.path_guard, None);
        if !validation.is_valid {
            let block_issues: Vec<_> = validation
                .issues
                .iter()
                .filter(|i| i.severity == Severity::Block)
                .map(|i| i.message.as_str())
                .collect();
            let alt = match tool_call.name.as_str() {
                "shell" => "代わりにfile_readやgitツールを使ってください。",
                "file_write" => "許可されたディレクトリのパスを指定してください。",
                _ => "別のツールか、別のパラメータで試してください。",
            };
            session.add_message(Message::tool(
                format!("拒否: {}。{}", block_issues.join(", "), alt),
                &tool_call.name,
            ));
            continue;
        }
        let tool = match ctx.tools.get(&tool_call.name) {
            Some(t) => t,
            None => continue,
        };
        let mut coerced_args = tool_call.arguments.clone();
        coerce_tool_arguments(&mut coerced_args);
        validated.push(ValidatedCall {
            name: tool_call.name.clone(),
            args_json: serde_json::to_string(&tool_call.arguments).unwrap_or_default(),
            coerced_args,
            tool,
            is_read_only: tool.is_read_only(),
        });
    }

    let step_tools = execute_validated_calls(
        &validated,
        session,
        circuit_breaker,
        ctx.secrets_filter,
        ctx.store,
        tool_cache,
    );
    Ok(StepOutcome::Continue(step_tools))
}
