//! ツール実行関連の構造体・関数群
//!
//! agent_loop.rs からの抽出モジュール。
//! ツール呼び出しの実行・並列化・結果反映を担う。

use crate::agent::conversation::{Message, Session};
use crate::agent::error_recovery::CircuitBreaker;
use crate::memory::graph::KnowledgeGraph;
use crate::memory::store::MemoryStore;
use crate::observability::audit::{AuditAction, AuditLog};
use crate::observability::logger::{log_event, LogLevel};
use crate::safety::secrets::SecretsFilter;
use crate::tools::{ToolResult, ToolResultCache};

/// バリデーション済みツール呼び出し（並列実行の単位）
pub(crate) struct ValidatedCall<'a> {
    pub name: String,
    pub args_json: String,
    pub coerced_args: serde_json::Value,
    pub tool: &'a dyn crate::tools::Tool,
    pub is_read_only: bool,
}

/// ツール実行結果（並列実行からの収集用）
pub(crate) struct ToolExecResult {
    pub name: String,
    pub args_json: String,
    pub output: String,
    pub success: bool,
    pub is_error: bool,
}

/// 単一ツール呼び出しを実行
pub(crate) fn execute_single_call(call: &ValidatedCall<'_>) -> ToolExecResult {
    match call.tool.call(call.coerced_args.clone()) {
        Ok(tool_result) => ToolExecResult {
            name: call.name.clone(), args_json: call.args_json.clone(),
            output: tool_result.output, success: tool_result.success, is_error: false,
        },
        Err(e) => ToolExecResult {
            name: call.name.clone(), args_json: call.args_json.clone(),
            output: format!("ツール実行エラー: {e}"), success: false, is_error: true,
        },
    }
}

/// 読取専用ツールをstd::thread::scopeで並列実行
pub(crate) fn execute_read_batch_parallel(batch: &[ValidatedCall<'_>]) -> Vec<ToolExecResult> {
    log_event(LogLevel::Debug, "parallel", &format!("読取ツール{}件を並列実行", batch.len()));
    std::thread::scope(|s| {
        let handles: Vec<_> = batch.iter().map(|call| s.spawn(move || execute_single_call(call))).collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    })
}

/// ツール実行結果をセッション・サーキットブレーカー・監査ログに反映
pub(crate) fn apply_tool_result(
    r: &ToolExecResult, session: &mut Session, circuit_breaker: &mut CircuitBreaker,
    secrets_filter: &SecretsFilter, store: Option<&MemoryStore>,
) {
    let file_path = serde_json::from_str::<serde_json::Value>(&r.args_json)
        .ok()
        .and_then(|v| v.get("path").and_then(|p| p.as_str().map(String::from)));

    if r.is_error {
        circuit_breaker.record_failure(&r.name);
        if let Some(s) = store {
            let audit = AuditLog::new(s.conn());
            let _ = audit.log(Some(&session.id), &AuditAction::ToolCall {
                tool_name: r.name.clone(), args: r.args_json.clone(), success: false, output_preview: r.output.clone(),
            });
            let graph = KnowledgeGraph::new(s.conn());
            let path = file_path.as_deref().unwrap_or("unknown");
            let _ = graph.record_error_pattern("tool_error", path, &r.name);
        }
        session.add_message(Message::tool(&r.output, &r.name));
    } else {
        circuit_breaker.record_success(&r.name);
        let redacted = secrets_filter.redact(&r.output);
        if let Some(s) = store {
            let audit = AuditLog::new(s.conn());
            let _ = audit.log(Some(&session.id), &AuditAction::ToolCall {
                tool_name: r.name.clone(), args: r.args_json.clone(), success: r.success,
                output_preview: redacted.chars().take(200).collect(),
            });
            if let Some(ref fp) = file_path {
                let graph = KnowledgeGraph::new(s.conn());
                let _ = graph.record_tool_usage(&r.name, fp);
            }
        }
        session.add_message(Message::tool(&redacted, &r.name));
    }
}

/// バリデーション済みツール呼び出しを実行（読取専用は並列、書き込みは逐次）
pub(crate) fn execute_validated_calls(
    calls: &[ValidatedCall<'_>],
    session: &mut Session,
    circuit_breaker: &mut CircuitBreaker,
    secrets_filter: &SecretsFilter,
    store: Option<&MemoryStore>,
    cache: &mut ToolResultCache,
) -> Vec<String> {
    let mut step_tools: Vec<String> = Vec::new();
    let mut i = 0;
    while i < calls.len() {
        let batch_start = i;
        while i < calls.len() && calls[i].is_read_only {
            i += 1;
        }
        let read_batch = &calls[batch_start..i];
        if read_batch.len() >= 2 {
            let results = execute_read_batch_parallel(read_batch);
            for r in results {
                if !r.is_error
                    && let Ok(args) = serde_json::from_str::<serde_json::Value>(&r.args_json)
                {
                    cache.put(&r.name, &args, ToolResult { output: r.output.clone(), success: r.success });
                }
                apply_tool_result(&r, session, circuit_breaker, secrets_filter, store);
                if !r.is_error {
                    step_tools.push(r.name);
                }
            }
        } else {
            for call in read_batch {
                if let Some(cached) = cache.get(&call.name, &call.coerced_args) {
                    let cached_output = cached.output.clone();
                    session.add_message(Message::tool(&cached_output, &call.name));
                    step_tools.push(call.name.clone());
                    log_event(LogLevel::Debug, "cache", &format!("キャッシュヒット: {}", call.name));
                    continue;
                }
                let r = execute_single_call(call);
                if !r.is_error
                    && let Ok(args) = serde_json::from_str::<serde_json::Value>(&r.args_json)
                {
                    cache.put(&r.name, &args, ToolResult { output: r.output.clone(), success: r.success });
                }
                apply_tool_result(&r, session, circuit_breaker, secrets_filter, store);
                if !r.is_error {
                    step_tools.push(r.name);
                }
            }
        }
        if i < calls.len() && !calls[i].is_read_only {
            let r = execute_single_call(&calls[i]);
            apply_tool_result(&r, session, circuit_breaker, secrets_filter, store);
            if !r.is_error {
                step_tools.push(r.name);
            }
            cache.invalidate("file_read");
            cache.invalidate("repo_map");
            i += 1;
        }
    }
    step_tools
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::error_recovery::CircuitBreaker;
    use crate::safety::secrets::SecretsFilter;
    use crate::tools::permission::Permission;

    struct DummyTool;
    impl crate::tools::Tool for DummyTool {
        fn name(&self) -> &str { "dummy" }
        fn description(&self) -> &str { "test" }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn permission(&self) -> Permission { Permission::Auto }
        fn call(&self, _args: serde_json::Value) -> anyhow::Result<crate::tools::ToolResult> {
            Ok(crate::tools::ToolResult { output: "ok".into(), success: true })
        }
        fn is_read_only(&self) -> bool { true }
    }

    struct ErrorTool;
    impl crate::tools::Tool for ErrorTool {
        fn name(&self) -> &str { "error_tool" }
        fn description(&self) -> &str { "fails" }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn permission(&self) -> Permission { Permission::Auto }
        fn call(&self, _args: serde_json::Value) -> anyhow::Result<crate::tools::ToolResult> {
            anyhow::bail!("test error")
        }
        fn is_read_only(&self) -> bool { true }
    }

    #[test]
    fn t_execute_single_call_success() {
        let tool = DummyTool;
        let call = ValidatedCall {
            name: "dummy".into(),
            args_json: "{}".into(),
            coerced_args: serde_json::json!({}),
            tool: &tool,
            is_read_only: true,
        };
        let result = execute_single_call(&call);
        assert!(!result.is_error);
        assert!(result.success);
        assert_eq!(result.output, "ok");
    }

    #[test]
    fn t_execute_single_call_error() {
        let tool = ErrorTool;
        let call = ValidatedCall {
            name: "error_tool".into(),
            args_json: "{}".into(),
            coerced_args: serde_json::json!({}),
            tool: &tool,
            is_read_only: true,
        };
        let result = execute_single_call(&call);
        assert!(result.is_error);
        assert!(!result.success);
        assert!(result.output.contains("エラー"));
    }

    #[test]
    fn t_apply_tool_result_success() {
        let mut session = Session::new();
        let mut cb = CircuitBreaker::default();
        let sf = SecretsFilter::default();
        let r = ToolExecResult {
            name: "dummy".into(),
            args_json: "{}".into(),
            output: "success output".into(),
            success: true,
            is_error: false,
        };
        apply_tool_result(&r, &mut session, &mut cb, &sf, None);
        assert!(session.messages.iter().any(|m| m.content.contains("success output")));
    }

    #[test]
    fn t_apply_tool_result_error() {
        let mut session = Session::new();
        let mut cb = CircuitBreaker::default();
        let sf = SecretsFilter::default();
        let r = ToolExecResult {
            name: "dummy".into(),
            args_json: "{}".into(),
            output: "error occurred".into(),
            success: false,
            is_error: true,
        };
        apply_tool_result(&r, &mut session, &mut cb, &sf, None);
        assert!(session.messages.iter().any(|m| m.content.contains("error occurred")));
    }

    #[test]
    fn t_validated_call_fields() {
        let tool = DummyTool;
        let call = ValidatedCall {
            name: "test".into(),
            args_json: r#"{"key":"val"}"#.into(),
            coerced_args: serde_json::json!({"key": "val"}),
            tool: &tool,
            is_read_only: true,
        };
        assert_eq!(call.name, "test");
        assert!(call.is_read_only);
    }
}
