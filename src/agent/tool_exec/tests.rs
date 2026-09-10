use super::*;
use crate::agent::agent_loop::AgentConfig;
use crate::agent::error_recovery::{CircuitBreaker, FileStuckGuard, TrialSummary};
use crate::agent::event_store::EventStore;
use crate::safety::autonomy::AutonomyLevel;
use crate::safety::secrets::SecretsFilter;
use crate::tools::permission::{DaemonPolicy, Permission};

struct MockPolicyTool {
    name: &'static str,
    perm: Permission,
    read_only: bool,
    fail: bool,
}
impl MockPolicyTool {
    fn ok(name: &'static str, perm: Permission, read_only: bool) -> Self {
        Self {
            name,
            perm,
            read_only,
            fail: false,
        }
    }
    fn fail(name: &'static str) -> Self {
        Self {
            name,
            perm: Permission::Auto,
            read_only: true,
            fail: true,
        }
    }
}
impl crate::tools::Tool for MockPolicyTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "test"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({})
    }
    fn permission(&self) -> Permission {
        self.perm
    }
    fn is_read_only(&self) -> bool {
        self.read_only
    }
    fn call(&self, _args: serde_json::Value) -> anyhow::Result<crate::tools::ToolResult> {
        if self.fail {
            anyhow::bail!("test error");
        }
        Ok(crate::tools::ToolResult {
            output: "ok".into(),
            success: true,
            ..Default::default()
        })
    }
}

fn mock_call<'a>(name: &'a str, tool: &'a MockPolicyTool) -> ValidatedCall<'a> {
    ValidatedCall {
        name: name.into(),
        args_json: "{}".into(),
        coerced_args: serde_json::json!({}),
        tool,
        is_read_only: tool.read_only,
    }
}

#[test]
fn t_execute_single_call_success() {
    let tool = MockPolicyTool::ok("dummy", Permission::Auto, true);
    let call = mock_call("dummy", &tool);
    let result = execute_single_call(&call);
    assert!(!result.is_error && result.success && result.output == "ok");
}

#[test]
fn t_execute_single_call_error() {
    let tool = MockPolicyTool::fail("error_tool");
    let call = mock_call("error_tool", &tool);
    let result = execute_single_call(&call);
    assert!(result.is_error && !result.success && result.output.contains("エラー"));
}

#[test]
fn t_execute_single_call_permission_denied() {
    let tool = MockPolicyTool::ok("deny_tool", Permission::Deny, true);
    let call = mock_call("deny_tool", &tool);
    let result = execute_single_call(&call);
    assert!(result.is_error && !result.success && result.output.contains("権限エラー"));
}

#[test]
fn t_execute_single_call_autonomy_readonly_blocks_write() {
    let tool = MockPolicyTool::ok("write_tool", Permission::Auto, false);
    let call = mock_call("write_tool", &tool);
    let result = execute_single_call_with_policy(
        &call,
        false,
        DaemonPolicy::AutoOnly,
        AutonomyLevel::ReadOnly,
        None,
    );
    assert!(result.is_error && !result.success && result.output.contains("自律レベル"));
}

#[test]
fn t_execute_single_call_confirm_supervised_blocks() {
    let tool = MockPolicyTool::ok("confirm_tool", Permission::Confirm, false);
    let call = mock_call("confirm_tool", &tool);
    let result = execute_single_call_with_policy(
        &call,
        false,
        DaemonPolicy::AutoOnly,
        AutonomyLevel::Supervised,
        None,
    );
    assert!(result.is_error && !result.success && result.output.contains("確認エラー"));
}

#[test]
fn t_execute_single_call_confirm_supervised_with_callback_allows() {
    let tool = MockPolicyTool::ok("confirm_tool", Permission::Confirm, false);
    let call = mock_call("confirm_tool", &tool);
    let cb = |_name: &str, _args: &str| true;
    let result = execute_single_call_with_policy(
        &call,
        false,
        DaemonPolicy::AutoOnly,
        AutonomyLevel::Supervised,
        Some(&cb),
    );
    assert!(!result.is_error && result.success && result.output == "ok");
}

#[test]
fn t_execute_single_call_confirm_supervised_with_callback_denies() {
    let tool = MockPolicyTool::ok("confirm_tool", Permission::Confirm, false);
    let call = mock_call("confirm_tool", &tool);
    let cb = |_name: &str, _args: &str| false;
    let result = execute_single_call_with_policy(
        &call,
        false,
        DaemonPolicy::AutoOnly,
        AutonomyLevel::Supervised,
        Some(&cb),
    );
    assert!(result.is_error && !result.success && result.output.contains("確認拒否"));
}

#[test]
fn t_execute_single_call_confirm_full_allows() {
    let tool = MockPolicyTool::ok("confirm_tool", Permission::Confirm, false);
    let call = mock_call("confirm_tool", &tool);
    let result = execute_single_call_with_policy(
        &call,
        false,
        DaemonPolicy::AutoOnly,
        AutonomyLevel::Full,
        None,
    );
    assert!(!result.is_error && result.success && result.output == "ok");
}

#[test]
fn t_apply_tool_result_redacts_error_and_args() {
    let mut session = Session::new();
    let mut cb = CircuitBreaker::default();
    let sf = SecretsFilter::default();
    let mut ts = TrialSummary::default();
    let mut guard = FileStuckGuard::default();
    let r = ToolExecResult {
        name: "dummy".into(),
        args_json: r#"{"token": "ghp_123456789012345678901234567890123456"}"#.into(),
        output: "error with secret: ghp_123456789012345678901234567890123456".into(),
        success: false,
        is_error: true,
        cancelled: false,
    };
    apply_tool_result(
        &r,
        &mut session,
        &mut cb,
        &mut ts,
        &mut guard,
        0,
        &sf,
        None,
        4000,
    );
    let msg = &session.messages.last().unwrap().content;
    assert!(!msg.contains("ghp_123456789012345678901234567890123456"));
    assert!(msg.contains("***REDACTED***"));
    assert_eq!(ts.len(), 1);
}

#[test]
fn t_apply_tool_result_success_and_error() {
    let mut session = Session::new();
    let mut cb = CircuitBreaker::default();
    let sf = SecretsFilter::default();
    let mut ts = TrialSummary::default();
    let mut guard = FileStuckGuard::default();
    let r_ok = ToolExecResult {
        name: "dummy".into(),
        args_json: "{}".into(),
        output: "success output".into(),
        success: true,
        is_error: false,
        cancelled: false,
    };
    apply_tool_result(
        &r_ok,
        &mut session,
        &mut cb,
        &mut ts,
        &mut guard,
        0,
        &sf,
        None,
        4000,
    );
    assert!(
        session
            .messages
            .iter()
            .any(|m| m.content.contains("success output"))
    );

    let r_err = ToolExecResult {
        name: "dummy".into(),
        args_json: "{}".into(),
        output: "error occurred".into(),
        success: false,
        is_error: true,
        cancelled: false,
    };
    apply_tool_result(
        &r_err,
        &mut session,
        &mut cb,
        &mut ts,
        &mut guard,
        0,
        &sf,
        None,
        4000,
    );
    assert!(
        session
            .messages
            .iter()
            .any(|m| m.content.contains("error occurred"))
    );
    assert_eq!(ts.len(), 1);
}

/// Issue #22 qa-reviewer MEDIUM-2: ユーザー取消 (`cancelled: true`) は
/// `success == false` (タスク未完了) を維持しつつ、circuit_breaker / trial_summary /
/// KnowledgeGraph への学習信号記録をスキップすること。
/// (通常のツールエラーとの対比は `t_apply_tool_result_success_and_error` を参照)
#[test]
fn t_apply_tool_result_cancelled_skips_learning_signals_but_stays_failed() {
    let mut session = Session::new();
    let mut cb = CircuitBreaker::default();
    let sf = SecretsFilter::default();
    let mut ts = TrialSummary::default();
    let mut guard = FileStuckGuard::default();
    let store = MemoryStore::in_memory().unwrap();

    let r_cancelled = ToolExecResult {
        name: "shell".into(),
        args_json: "{}".into(),
        output: "ユーザーにより取消されました".into(),
        success: false,
        is_error: false,
        cancelled: true,
    };
    apply_tool_result(
        &r_cancelled,
        &mut session,
        &mut cb,
        &mut ts,
        &mut guard,
        0,
        &sf,
        Some(&store),
        4000,
    );

    // タスク未完了として failed メッセージは session に残る（エージェントループの
    // 継続判断に必要）。
    assert!(session.messages.iter().any(|m| m.content.contains("取消")));
    // しかし品質の学習信号 (circuit_breaker / trial_summary) には一切記録されない。
    assert_eq!(
        cb.failure_count("shell"),
        0,
        "cancelled時はcircuit_breakerに記録されないべき"
    );
    assert_eq!(ts.len(), 0, "cancelled時はtrial_summaryに記録されないべき");

    // KnowledgeGraph にも tool_error パターンが記録されないこと。
    // record_error_pattern は tool_name ノードから file ノードへ "fixes" edge を張るため、
    // neighbors("shell", 1) が空であれば未記録と確認できる。
    let graph = KnowledgeGraph::new(store.conn());
    let neighbors = graph.neighbors("shell", 1).unwrap_or_default();
    assert!(
        neighbors.is_empty(),
        "cancelled時はKnowledgeGraphにtool_errorが記録されないべき: {neighbors:?}"
    );

    // 通常の失敗 (cancelled: false) と対比: 同条件では記録される。
    let r_normal_fail = ToolExecResult {
        name: "shell".into(),
        args_json: "{}".into(),
        output: "通常のエラー".into(),
        success: false,
        is_error: true,
        cancelled: false,
    };
    apply_tool_result(
        &r_normal_fail,
        &mut session,
        &mut cb,
        &mut ts,
        &mut guard,
        0,
        &sf,
        Some(&store),
        4000,
    );
    assert_eq!(
        cb.failure_count("shell"),
        1,
        "通常失敗はcircuit_breakerに記録されるべき"
    );
    assert_eq!(ts.len(), 1, "通常失敗はtrial_summaryに記録されるべき");
}

/// Issue #34: cancelled時、ToolCallStart/End の両 payload に
/// `"cancelled": true` が刻まれる（`success` は false のまま維持）。
#[test]
fn t_apply_tool_result_cancelled_marks_event_payload() {
    let mut session = Session::new();
    let mut cb = CircuitBreaker::default();
    let sf = SecretsFilter::default();
    let mut ts = TrialSummary::default();
    let mut guard = FileStuckGuard::default();
    let store = MemoryStore::in_memory().unwrap();

    let r = ToolExecResult {
        name: "shell".into(),
        args_json: "{}".into(),
        output: "取消されました".into(),
        success: false,
        is_error: false,
        cancelled: true,
    };
    apply_tool_result(
        &r,
        &mut session,
        &mut cb,
        &mut ts,
        &mut guard,
        0,
        &sf,
        Some(&store),
        4000,
    );

    let events = EventStore::new(store.conn()).replay(&session.id).unwrap();
    let start = events
        .iter()
        .find(|e| e.event_type == "tool_call_start")
        .expect("tool_call_start event");
    let end = events
        .iter()
        .find(|e| e.event_type == "tool_call_end")
        .expect("tool_call_end event");
    let start_v: serde_json::Value = serde_json::from_str(&start.event_data).unwrap();
    let end_v: serde_json::Value = serde_json::from_str(&end.event_data).unwrap();
    assert_eq!(start_v["cancelled"], serde_json::json!(true));
    assert_eq!(end_v["cancelled"], serde_json::json!(true));
    assert_eq!(end_v["success"], serde_json::json!(false));
}

/// Issue #34 後方互換確証: 非取消時は `cancelled` フィールド自体が出力されない
/// (成功/失敗いずれの経路でも)。
#[test]
fn t_apply_tool_result_non_cancelled_payload_has_no_cancelled_field() {
    let mut session = Session::new();
    let mut cb = CircuitBreaker::default();
    let sf = SecretsFilter::default();
    let mut ts = TrialSummary::default();
    let mut guard = FileStuckGuard::default();
    let store = MemoryStore::in_memory().unwrap();

    for r in [
        ToolExecResult {
            name: "shell".into(),
            args_json: "{}".into(),
            output: "ok".into(),
            success: true,
            is_error: false,
            cancelled: false,
        },
        ToolExecResult {
            name: "shell".into(),
            args_json: "{}".into(),
            output: "通常のエラー".into(),
            success: false,
            is_error: true,
            cancelled: false,
        },
    ] {
        apply_tool_result(
            &r,
            &mut session,
            &mut cb,
            &mut ts,
            &mut guard,
            0,
            &sf,
            Some(&store),
            4000,
        );
    }

    let events = EventStore::new(store.conn()).replay(&session.id).unwrap();
    for e in events
        .iter()
        .filter(|e| e.event_type == "tool_call_start" || e.event_type == "tool_call_end")
    {
        let v: serde_json::Value = serde_json::from_str(&e.event_data).unwrap();
        assert!(
            v.get("cancelled").is_none(),
            "非取消時はcancelledフィールドが出力されないべき: {v:?}"
        );
    }
}

/// Issue #34: cancelled時は FileStuckGuard の失敗記録をスキップする。
#[test]
fn t_apply_tool_result_cancelled_skips_file_stuck_guard() {
    let mut session = Session::new();
    let mut cb = CircuitBreaker::default();
    let sf = SecretsFilter::default();
    let mut ts = TrialSummary::default();
    let mut guard = FileStuckGuard::default();

    let r = ToolExecResult {
        name: "edit".into(),
        args_json: r#"{"path": "/tmp/foo.rs"}"#.into(),
        output: "取消されました".into(),
        success: false,
        is_error: false,
        cancelled: true,
    };
    apply_tool_result(
        &r,
        &mut session,
        &mut cb,
        &mut ts,
        &mut guard,
        0,
        &sf,
        None,
        4000,
    );
    assert_eq!(
        guard.tracked_files(),
        0,
        "cancelled時はFileStuckGuardに記録されないべき"
    );
}

/// 過剰除外の回帰ガード: 通常失敗 (cancelled: false) は従来どおり
/// FileStuckGuard に記録される。
#[test]
fn t_apply_tool_result_non_cancelled_records_file_stuck_guard() {
    let mut session = Session::new();
    let mut cb = CircuitBreaker::default();
    let sf = SecretsFilter::default();
    let mut ts = TrialSummary::default();
    let mut guard = FileStuckGuard::default();

    let r = ToolExecResult {
        name: "edit".into(),
        args_json: r#"{"path": "/tmp/foo.rs"}"#.into(),
        output: "通常のエラー".into(),
        success: false,
        is_error: true,
        cancelled: false,
    };
    apply_tool_result(
        &r,
        &mut session,
        &mut cb,
        &mut ts,
        &mut guard,
        0,
        &sf,
        None,
        4000,
    );
    assert_eq!(
        guard.tracked_files(),
        1,
        "非取消の失敗はFileStuckGuardに記録されるべき"
    );
}

#[test]
fn t_validated_call_fields() {
    let tool = MockPolicyTool::ok("test", Permission::Auto, true);
    let call = ValidatedCall {
        name: "test".into(),
        args_json: r#"{"key":"val"}"#.into(),
        coerced_args: serde_json::json!({"key":"val"}),
        tool: &tool,
        is_read_only: true,
    };
    assert_eq!(call.name, "test");
    assert!(call.is_read_only);
}

#[test]
fn t_execute_validated_calls_redacts_cache() {
    let tool = MockPolicyTool::ok("read_tool", Permission::Auto, true);
    let call = mock_call("read_tool", &tool);
    let mut session = Session::new();
    let mut cb = CircuitBreaker::default();
    let sf = SecretsFilter::default();
    let mut cache = ToolResultCache::new();
    let mut cycle = MultiFileEditCycleDetector::default();
    let mut ts = TrialSummary::default();
    let mut guard = FileStuckGuard::default();
    let config = AgentConfig::default();

    let raw_secret = "Bearer eyJhbGciOiJIUzI1NiJ9.test";
    cache.put(
        "read_tool",
        &serde_json::json!({}),
        ToolResult {
            output: format!("leaked secret: {raw_secret}"),
            success: true,
            ..Default::default()
        },
    );

    let (result, all_succeeded) = execute_validated_calls(
        &[call],
        &mut session,
        &mut cb,
        &mut ts,
        &mut guard,
        0,
        &sf,
        None,
        &mut cache,
        &mut cycle,
        &config,
    );
    assert_eq!(result, vec!["read_tool"]);
    assert!(all_succeeded);
    let msg = &session.messages.last().unwrap().content;
    assert!(!msg.contains(raw_secret));
    assert!(msg.contains("***REDACTED***"));
}

#[test]
fn test_truncate_tool_output() {
    use crate::agent::tool_spill::SpillStore;

    let dir = std::env::temp_dir().join(format!(
        "bonsai_tool_exec_test_truncate_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let spill = SpillStore::new_at(dir.clone(), 64, 64 * 1024 * 1024);

    assert_eq!(
        truncate_tool_output_with("hello world", 4000, Some(&spill)),
        "hello world"
    );
    assert_eq!(truncate_tool_output_with("hello", 0, Some(&spill)), "hello");
    let large = "a".repeat(5000);
    let res = truncate_tool_output_with(&large, 4000, Some(&spill));
    assert!(res.contains("...") && res.contains("全文保存") && res.contains("5000文字"));
    let unicode = "日本語テスト".repeat(1000);
    assert!(truncate_tool_output_with(&unicode, 100, Some(&spill)).contains("..."));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn t_parse_nudge_files() {
    assert_eq!(
        parse_nudge_files(
            "[edit-cycle] ファイル a.rs, b.rs を交互に編集しています — 進捗が見られません。"
        ),
        vec!["a.rs", "b.rs"]
    );
    assert_eq!(
        parse_nudge_files("[edit-cycle] ファイル a.rs, b.rs, c.rs を交互に編集しています — ..."),
        vec!["a.rs", "b.rs", "c.rs"]
    );
    assert!(parse_nudge_files("不明な形式の文字列").is_empty());
}

#[test]
fn t_record_edit_and_nudge_audit() {
    let store = MemoryStore::in_memory().unwrap();
    let mut session = Session::new();
    session.id = "test-session".to_string();
    let mut det = MultiFileEditCycleDetector::new(6);

    for p in ["/tmp/a.rs", "/tmp/b.rs", "/tmp/a.rs", "/tmp/b.rs"] {
        record_edit_and_nudge(
            "file_write",
            &serde_json::json!({"file_path": p}),
            &mut det,
            &mut session,
            Some(&store),
        );
    }

    let audit = AuditLog::new(store.conn());
    let entries = audit.recent(10).unwrap();
    let nudge_entries: Vec<_> = entries
        .iter()
        .filter(|e| e.action_type == "multi_file_nudge")
        .collect();
    assert_eq!(nudge_entries.len(), 1);
    let entry = &nudge_entries[0];
    assert_eq!(entry.session_id.as_deref(), Some("test-session"));
    let v: serde_json::Value = serde_json::from_str(&entry.action_data).unwrap();
    assert_eq!(v["type"], "MultiFileNudge");
    assert_eq!(v["fire_count"], 1);
    assert_eq!(v["files"].as_array().unwrap().len(), 2);
}

#[test]
fn t_record_edit_and_nudge_no_audit_without_store_and_non_edit() {
    let mut session = Session::new();
    let mut det = MultiFileEditCycleDetector::new(6);
    for p in ["/tmp/a.rs", "/tmp/b.rs", "/tmp/a.rs", "/tmp/b.rs"] {
        record_edit_and_nudge(
            "file_write",
            &serde_json::json!({"file_path": p}),
            &mut det,
            &mut session,
            None,
        );
    }
    assert_eq!(det.nudge_fire_count(), 1);

    let store = MemoryStore::in_memory().unwrap();
    let mut det2 = MultiFileEditCycleDetector::new(6);
    for _ in 0..6 {
        record_edit_and_nudge(
            "file_read",
            &serde_json::json!({"file_path": "/tmp/a.rs"}),
            &mut det2,
            &mut session,
            Some(&store),
        );
    }
    assert_eq!(det2.nudge_fire_count(), 0);
}

#[test]
fn t_execute_read_batch_parallel_confirm_serialized() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let tool1 = MockPolicyTool::ok("web_fetch_1", Permission::Confirm, true);
    let tool2 = MockPolicyTool::ok("web_fetch_2", Permission::Confirm, true);
    let call1 = mock_call("web_fetch_1", &tool1);
    let call2 = mock_call("web_fetch_2", &tool2);

    let batch = vec![call1, call2];

    let concurrent_count = AtomicUsize::new(0);
    let max_concurrent = AtomicUsize::new(0);
    let total_calls = AtomicUsize::new(0);

    let callback = |_name: &str, _args: &str| -> bool {
        let current = concurrent_count.fetch_add(1, Ordering::SeqCst) + 1;
        max_concurrent.fetch_max(current, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(10));
        concurrent_count.fetch_sub(1, Ordering::SeqCst);
        total_calls.fetch_add(1, Ordering::SeqCst);
        true
    };

    let cb_ref: ConfirmCheckRef<'_> = &callback;
    let results = execute_read_batch_parallel(
        &batch,
        false,
        DaemonPolicy::AutoOnly,
        AutonomyLevel::Supervised,
        Some(cb_ref),
    );

    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| !r.is_error && r.success));
    assert_eq!(total_calls.load(Ordering::SeqCst), 2);
    assert_eq!(max_concurrent.load(Ordering::SeqCst), 1);
}

#[test]
fn t_extract_edit_path_schema_keys() {
    assert_eq!(
        extract_edit_path("file_write", &serde_json::json!({"path": "src/main.rs"})),
        Some("src/main.rs".to_string())
    );
    assert_eq!(
        extract_edit_path("multi_edit", &serde_json::json!({"path": "src/lib.rs"})),
        Some("src/lib.rs".to_string())
    );
    assert_eq!(
        extract_edit_path(
            "file_write",
            &serde_json::json!({"file_path": "src/main.rs"})
        ),
        Some("src/main.rs".to_string())
    );
    assert_eq!(
        extract_edit_path("file_read", &serde_json::json!({"path": "src/main.rs"})),
        None
    );
}
