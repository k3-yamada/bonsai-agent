use super::*;
use crate::memory::store::MemoryStore;

fn test_conn() -> MemoryStore {
    MemoryStore::in_memory().unwrap()
}

fn success_params<'a>(ctx: &'a str, action: &'a str) -> RecordParams<'a> {
    RecordParams {
        exp_type: ExperienceType::Success,
        task_context: ctx,
        action,
        outcome: "OK",
        lesson: None,
        tool_name: Some("shell"),
        error_type: None,
        error_detail: None,
    }
}

#[test]
fn test_record_success() {
    let store = test_conn();
    let exp = ExperienceStore::new(store.conn());
    let id = exp
        .record(&RecordParams {
            exp_type: ExperienceType::Success,
            task_context: "list files",
            action: "shell: ls -la",
            outcome: "files listed",
            lesson: None,
            tool_name: Some("shell"),
            error_type: None,
            error_detail: None,
        })
        .unwrap();
    assert!(id > 0);
}

#[test]
fn test_record_failure() {
    let store = test_conn();
    let exp = ExperienceStore::new(store.conn());
    exp.record(&RecordParams {
        exp_type: ExperienceType::Failure,
        task_context: "delete file",
        action: "shell: rm important.txt",
        outcome: "Permission denied",
        lesson: Some("insufficient permissions"),
        tool_name: Some("shell"),
        error_type: Some("ToolExecError"),
        error_detail: Some("PermissionDenied"),
    })
    .unwrap();
}

#[test]
fn test_find_similar() {
    let store = test_conn();
    let exp = ExperienceStore::new(store.conn());
    exp.record(&success_params("list files", "shell: ls"))
        .unwrap();
    exp.record(&success_params("create directory", "shell: mkdir test"))
        .unwrap();

    let results = exp.find_similar("list", 10).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].exp_type, ExperienceType::Success);
}

#[test]
fn test_failure_patterns() {
    let store = test_conn();
    let exp = ExperienceStore::new(store.conn());

    for _ in 0..3 {
        exp.record(&RecordParams {
            exp_type: ExperienceType::Failure,
            task_context: "API call",
            action: "shell: curl api.example.com",
            outcome: "timeout",
            lesson: None,
            tool_name: Some("shell"),
            error_type: Some("ToolExecError"),
            error_detail: Some("Timeout"),
        })
        .unwrap();
    }
    exp.record(&RecordParams {
        exp_type: ExperienceType::Failure,
        task_context: "run command",
        action: "shell: nonexistent",
        outcome: "not found",
        lesson: None,
        tool_name: Some("shell"),
        error_type: Some("ToolExecError"),
        error_detail: Some("CommandNotFound"),
    })
    .unwrap();

    let patterns = exp.failure_patterns("shell").unwrap();
    assert_eq!(patterns.len(), 2);
    assert_eq!(patterns[0].0, "Timeout");
    assert_eq!(patterns[0].1, 3);
}

#[test]
fn t_purge_expired_removes_old() {
    let store = test_conn();
    let exp = ExperienceStore::new(store.conn());
    let id = exp
        .record(&success_params("old task", "old action"))
        .unwrap();
    // 過去の日時を直接設定
    store
        .conn()
        .execute(
            "UPDATE experiences SET expires_at = '2020-01-01T00:00:00Z' WHERE id = ?1",
            params![id],
        )
        .unwrap();
    let deleted = exp.purge_expired().unwrap();
    assert_eq!(deleted, 1);
}

#[test]
fn t_set_ttl_updates_expires() {
    let store = test_conn();
    let exp = ExperienceStore::new(store.conn());
    let id = exp
        .record(&success_params("ttl task", "ttl action"))
        .unwrap();
    exp.set_ttl(id, 30).unwrap();
    let expires: Option<String> = store
        .conn()
        .query_row(
            "SELECT expires_at FROM experiences WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(expires.is_some());
}

#[test]
fn t_purge_keeps_valid() {
    let store = test_conn();
    let exp = ExperienceStore::new(store.conn());
    let id = exp.record(&success_params("valid", "valid")).unwrap();
    // 未来の日時を設定
    store
        .conn()
        .execute(
            "UPDATE experiences SET expires_at = '2099-01-01T00:00:00Z' WHERE id = ?1",
            params![id],
        )
        .unwrap();
    let deleted = exp.purge_expired().unwrap();
    assert_eq!(deleted, 0);
}

#[test]
fn test_success_count() {
    let store = test_conn();
    let exp = ExperienceStore::new(store.conn());
    exp.record(&success_params("t1", "a1")).unwrap();
    exp.record(&success_params("t2", "a2")).unwrap();
    exp.record(&RecordParams {
        exp_type: ExperienceType::Failure,
        task_context: "t3",
        action: "a3",
        outcome: "err",
        lesson: None,
        tool_name: Some("shell"),
        error_type: None,
        error_detail: None,
    })
    .unwrap();

    assert_eq!(exp.success_count("shell").unwrap(), 2);
}

// ============ AgentHER HSL/ECHO テスト (Phase 1 Red) ============

fn make_event(id: i64, session_id: &str, event_type: &str, data: &str) -> Event {
    Event {
        id,
        session_id: session_id.to_string(),
        event_type: event_type.to_string(),
        event_data: data.to_string(),
        step_index: None,
        created_at: "2026-05-07T00:00:00Z".to_string(),
    }
}

#[test]
fn t_extract_hsl_basic_success_subgoal() {
    // file_write 成功 + shell 失敗 = 1 trajectory に 1 subgoal
    let events = vec![
        make_event(1, "s1", "session_start", "{}"),
        make_event(2, "s1", "user_message", r#"{"content":"FizzBuzz実装"}"#),
        make_event(3, "s1", "tool_call_start", r#"{"tool":"file_write"}"#),
        make_event(
            4,
            "s1",
            "tool_call_end",
            r#"{"tool":"file_write","success":true}"#,
        ),
        make_event(5, "s1", "tool_call_start", r#"{"tool":"shell"}"#),
        make_event(
            6,
            "s1",
            "tool_call_end",
            r#"{"tool":"shell","success":false}"#,
        ),
        make_event(7, "s1", "session_end", "{}"),
    ];
    let result =
        extract_hindsight_relabels(&events, SubgoalJudgeMethod::ToolEndSuccessOrSideEffect);
    assert_eq!(result.len(), 1, "1 trajectory から 1 HindsightRelabel");
    let r = &result[0];
    assert_eq!(r.original_goal, "FizzBuzz実装");
    assert_eq!(
        r.achieved_subgoals.len(),
        1,
        "file_write 成功 1 件のみ subgoal"
    );
    assert!(r.achieved_subgoals[0].contains("file_write"));
    assert_eq!(
        r.subgoal_indices,
        vec![0],
        "trajectory index 0 = file_write"
    );
    assert_eq!(r.trajectory, vec!["file_write", "shell"]);
    assert!((r.tool_success_rate - 0.5).abs() < 1e-9);
    assert_eq!(r.session_id, "s1");
    assert_eq!(r.total_steps, 2);
}

#[test]
fn t_extract_hsl_filters_all_failures() {
    // 全 ToolCallEnd success=false → 0 件 (false-positive 防止)
    let events = vec![
        make_event(1, "s1", "session_start", "{}"),
        make_event(2, "s1", "user_message", r#"{"content":"全失敗タスク"}"#),
        make_event(3, "s1", "tool_call_start", r#"{"tool":"shell"}"#),
        make_event(
            4,
            "s1",
            "tool_call_end",
            r#"{"tool":"shell","success":false}"#,
        ),
        make_event(5, "s1", "tool_call_start", r#"{"tool":"file_write"}"#),
        make_event(
            6,
            "s1",
            "tool_call_end",
            r#"{"tool":"file_write","success":false}"#,
        ),
        make_event(7, "s1", "session_end", "{}"),
    ];
    let result =
        extract_hindsight_relabels(&events, SubgoalJudgeMethod::ToolEndSuccessOrSideEffect);
    assert!(result.is_empty(), "全 success=false なら relabel 0 件");
}

#[test]
fn t_extract_hsl_side_effect_method() {
    // SideEffectOnly: shell exit 0 を除外、file_write のみ subgoal
    let events = vec![
        make_event(1, "s1", "session_start", "{}"),
        make_event(2, "s1", "user_message", r#"{"content":"混合タスク"}"#),
        make_event(3, "s1", "tool_call_start", r#"{"tool":"shell"}"#),
        make_event(
            4,
            "s1",
            "tool_call_end",
            r#"{"tool":"shell","success":true}"#,
        ),
        make_event(5, "s1", "tool_call_start", r#"{"tool":"file_write"}"#),
        make_event(
            6,
            "s1",
            "tool_call_end",
            r#"{"tool":"file_write","success":true}"#,
        ),
        make_event(7, "s1", "session_end", "{}"),
    ];
    let result = extract_hindsight_relabels(&events, SubgoalJudgeMethod::SideEffectOnly);
    assert_eq!(result.len(), 1);
    let r = &result[0];
    assert_eq!(
        r.achieved_subgoals.len(),
        1,
        "SideEffectOnly では shell を除外、file_write のみ"
    );
    assert_eq!(
        r.subgoal_indices,
        vec![1],
        "trajectory index 1 = file_write"
    );
    assert!(r.achieved_subgoals[0].contains("file_write"));
}

#[test]
fn t_extract_hsl_session_end_required() {
    // SessionEnd 不在 trajectory は除外 (項目 162 整合)
    let events = vec![
        make_event(1, "s1", "session_start", "{}"),
        make_event(2, "s1", "user_message", r#"{"content":"未完了タスク"}"#),
        make_event(3, "s1", "tool_call_start", r#"{"tool":"file_write"}"#),
        make_event(
            4,
            "s1",
            "tool_call_end",
            r#"{"tool":"file_write","success":true}"#,
        ),
        make_event(5, "s1", "tool_call_start", r#"{"tool":"file_write"}"#),
        make_event(
            6,
            "s1",
            "tool_call_end",
            r#"{"tool":"file_write","success":true}"#,
        ),
        // SessionEnd なし
    ];
    let result =
        extract_hindsight_relabels(&events, SubgoalJudgeMethod::ToolEndSuccessOrSideEffect);
    assert!(result.is_empty(), "SessionEnd 不在は除外");
}

#[test]
fn t_extract_hsl_min_steps_filter() {
    // tool_call_start < 2 (min_steps) は除外
    let events = vec![
        make_event(1, "s1", "session_start", "{}"),
        make_event(2, "s1", "user_message", r#"{"content":"短すぎ"}"#),
        make_event(3, "s1", "tool_call_start", r#"{"tool":"file_write"}"#),
        make_event(
            4,
            "s1",
            "tool_call_end",
            r#"{"tool":"file_write","success":true}"#,
        ),
        make_event(5, "s1", "session_end", "{}"),
    ];
    let result =
        extract_hindsight_relabels(&events, SubgoalJudgeMethod::ToolEndSuccessOrSideEffect);
    assert!(result.is_empty(), "tool_call < 2 (min_steps) は除外");
}

/// Issue #34: cancelled な tool_call は AgentHER HSL の trajectory・subgoal・
/// tool_success_rate 集計から除外される。
#[test]
fn t_extract_hsl_excludes_cancelled_tool_calls() {
    let events = vec![
        make_event(1, "s1", "session_start", "{}"),
        make_event(2, "s1", "user_message", r#"{"content":"混合タスク"}"#),
        make_event(3, "s1", "tool_call_start", r#"{"tool":"a"}"#),
        make_event(4, "s1", "tool_call_end", r#"{"tool":"a","success":true}"#),
        make_event(5, "s1", "tool_call_start", r#"{"tool":"b"}"#),
        make_event(6, "s1", "tool_call_end", r#"{"tool":"b","success":true}"#),
        make_event(
            7,
            "s1",
            "tool_call_start",
            r#"{"tool":"c","cancelled":true}"#,
        ),
        make_event(
            8,
            "s1",
            "tool_call_end",
            r#"{"tool":"c","success":false,"cancelled":true}"#,
        ),
        make_event(9, "s1", "session_end", "{}"),
    ];
    let result =
        extract_hindsight_relabels(&events, SubgoalJudgeMethod::ToolEndSuccessOrSideEffect);
    assert_eq!(result.len(), 1);
    let r = &result[0];
    assert_eq!(
        r.trajectory,
        vec!["a", "b"],
        "cancelled toolはtrajectoryから除外"
    );
    assert!((r.tool_success_rate - 1.0).abs() < 1e-9);
    assert_eq!(r.total_steps, 2);
    assert_eq!(r.achieved_subgoals.len(), 2);
}

/// 全 tool_call が cancelled の場合、trajectory.len() < 2 (min_steps) により
/// relabel 0 件となる。
#[test]
fn t_extract_hsl_all_cancelled_yields_no_relabel() {
    let events = vec![
        make_event(1, "s1", "session_start", "{}"),
        make_event(2, "s1", "user_message", r#"{"content":"全取消タスク"}"#),
        make_event(
            3,
            "s1",
            "tool_call_start",
            r#"{"tool":"a","cancelled":true}"#,
        ),
        make_event(
            4,
            "s1",
            "tool_call_end",
            r#"{"tool":"a","success":false,"cancelled":true}"#,
        ),
        make_event(
            5,
            "s1",
            "tool_call_start",
            r#"{"tool":"b","cancelled":true}"#,
        ),
        make_event(
            6,
            "s1",
            "tool_call_end",
            r#"{"tool":"b","success":false,"cancelled":true}"#,
        ),
        make_event(7, "s1", "session_end", "{}"),
    ];
    let result =
        extract_hindsight_relabels(&events, SubgoalJudgeMethod::ToolEndSuccessOrSideEffect);
    assert!(result.is_empty(), "全cancelledはtrajectory<2で除外");
}

#[test]
fn t_record_hindsight_insight_creates_experience() {
    // ExperienceStore に type=insight で 1 レコード追加
    let store = test_conn();
    let exp = ExperienceStore::new(store.conn());
    let relabel = HindsightRelabel {
        original_goal: "FizzBuzz実装".into(),
        achieved_subgoals: vec!["file_write 成功".into()],
        subgoal_indices: vec![0],
        trajectory: vec!["file_write".into(), "shell".into()],
        tool_success_rate: 0.5,
        session_id: "s1".into(),
        total_steps: 2,
    };
    let id = exp.record_hindsight_insight(&relabel).unwrap();
    assert!(id > 0);

    let count: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM experiences WHERE type = 'insight' AND task_context LIKE '%FizzBuzz%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(count, 1, "type=insight で 1 レコード追加");
}
