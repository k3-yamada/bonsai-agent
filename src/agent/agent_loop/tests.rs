use anyhow::Result;

use super::*;
use super::advisor_inject::{
    AdvisorResolution, inject_planning_step, inject_replan_on_stall, inject_verification_step,
    log_advisor_call, resolve_advisor_prompt,
};
use super::core::create_task_start_checkpoint;
use super::outcome::{detect_task_complexity, handle_outcome};
use super::support::{check_invariants, compute_output_hash};

use crate::agent::context_inject::inject_experience_context;
use crate::agent::conversation::{Message, Role, Session};
use crate::agent::error_recovery::{CircuitBreaker, TrialSummary};
use crate::agent::middleware::MiddlewareChain;
use crate::agent::tool_exec::{ToolExecResult, apply_tool_result, execute_validated_calls};
use crate::agent::validate::PathGuard;
use crate::cancel::CancellationToken;
use crate::config::InferenceParams;
use crate::memory::experience::{ExperienceStore, ExperienceType, RecordParams};
use crate::memory::graph::KnowledgeGraph;
use crate::memory::store::MemoryStore;
use crate::observability::audit::AuditLog;
use crate::runtime::inference::MockLlmBackend;
use crate::runtime::model_router::{AdvisorConfig, AdvisorRole};
use crate::tools::permission::Permission;
use crate::tools::{TaskType, Tool, ToolRegistry, ToolResult, ToolResultCache};

/// テスト用のエコーツール
struct EchoTool;
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "入力をそのまま返す"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}})
    }
    fn permission(&self) -> Permission {
        Permission::Auto
    }
    fn call(&self, args: serde_json::Value) -> Result<ToolResult> {
        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("(empty)");
        Ok(ToolResult {
            output: text.to_string(),
            success: true,
        })
    }
}

/// テスト用の失敗ツール
struct FailTool;
impl Tool for FailTool {
    fn name(&self) -> &str {
        "fail"
    }
    fn description(&self) -> &str {
        "常に失敗する"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({})
    }
    fn permission(&self) -> Permission {
        Permission::Auto
    }
    fn call(&self, _args: serde_json::Value) -> Result<ToolResult> {
        anyhow::bail!("意図的なエラー")
    }
}

fn test_registry() -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(EchoTool));
    reg.register(Box::new(FailTool));
    reg
}

// テスト1: ツール不要 → 直接回答
#[test]
fn test_direct_answer() {
    let mock = MockLlmBackend::single("東京の天気は晴れです。");
    let tools = test_registry();
    let guard = PathGuard::default_deny_list();
    let config = AgentConfig::default();
    let cancel = CancellationToken::new();

    let result = run_agent_loop(
        "天気は？",
        &mock,
        &tools,
        &guard,
        &config,
        &cancel,
        None::<&MemoryStore>,
    )
    .unwrap();
    assert!(result.answer.contains("晴れ"));
    assert_eq!(result.iterations_used, 1);
    assert!(result.tools_called.is_empty());
}

// テスト2: ツール1回 → 回答
#[test]
fn test_single_tool_call() {
    let mock = MockLlmBackend::new(vec![
        r#"<tool_call>{"name":"echo","arguments":{"text":"hello"}}</tool_call>"#.to_string(),
        "ツール結果: hello".to_string(),
    ]);
    let tools = test_registry();
    let guard = PathGuard::default_deny_list();
    let config = AgentConfig::default();
    let cancel = CancellationToken::new();

    let result = run_agent_loop(
        "echo test",
        &mock,
        &tools,
        &guard,
        &config,
        &cancel,
        None::<&MemoryStore>,
    )
    .unwrap();
    assert!(result.answer.contains("hello"));
    assert_eq!(result.iterations_used, 2);
    assert!(result.tools_called.contains(&"echo".to_string()));
}

// テスト3: 最大イテレーション到達
#[test]
fn test_max_iterations() {
    // 常にツール呼び出しを返すモック（終了しない）
    let responses: Vec<String> = (0..15)
        .map(|i| {
            format!(
                r#"<tool_call>{{"name":"echo","arguments":{{"text":"iter{}"}}}}</tool_call>"#,
                i
            )
        })
        .collect();
    let mock = MockLlmBackend::new(responses);
    let tools = test_registry();
    let guard = PathGuard::default_deny_list();
    let config = AgentConfig {
        max_iterations: 3,
        ..Default::default()
    };
    let cancel = CancellationToken::new();

    let result = run_agent_loop(
        "loop",
        &mock,
        &tools,
        &guard,
        &config,
        &cancel,
        None::<&MemoryStore>,
    )
    .unwrap();
    assert!(result.answer.contains("中断"));
    assert_eq!(result.iterations_used, 3);
}

// テスト4: Ctrl+Cキャンセル
#[test]
fn test_cancellation() {
    let mock = MockLlmBackend::single("回答");
    let tools = test_registry();
    let guard = PathGuard::default_deny_list();
    let config = AgentConfig::default();
    let cancel = CancellationToken::new();
    cancel.cancel();

    let result = run_agent_loop(
        "test",
        &mock,
        &tools,
        &guard,
        &config,
        &cancel,
        None::<&MemoryStore>,
    );
    // MockLlmBackend::generateがキャンセルエラーを返す
    assert!(result.is_err() || result.unwrap().answer.contains("キャンセル"));
}

// テスト5: 不正ツール名 → バリデーション拒否
#[test]
fn test_unknown_tool_blocked() {
    let mock = MockLlmBackend::new(vec![
        r#"<tool_call>{"name":"hack","arguments":{}}</tool_call>"#.to_string(),
        "バリデーションエラーのため別の方法を試します。回答: 了解".to_string(),
    ]);
    let tools = test_registry();
    let guard = PathGuard::default_deny_list();
    let config = AgentConfig::default();
    let cancel = CancellationToken::new();

    let result = run_agent_loop(
        "hack",
        &mock,
        &tools,
        &guard,
        &config,
        &cancel,
        None::<&MemoryStore>,
    )
    .unwrap();
    assert!(result.answer.contains("了解"));
}

// テスト6: ツール失敗 → サーキットブレーカー記録
#[test]
fn test_tool_failure_recorded() {
    let mock = MockLlmBackend::new(vec![
        r#"<tool_call>{"name":"fail","arguments":{}}</tool_call>"#.to_string(),
        "ツールが失敗しました。回答: エラーが発生しました".to_string(),
    ]);
    let tools = test_registry();
    let guard = PathGuard::default_deny_list();
    let config = AgentConfig::default();
    let cancel = CancellationToken::new();

    let result = run_agent_loop(
        "fail",
        &mock,
        &tools,
        &guard,
        &config,
        &cancel,
        None::<&MemoryStore>,
    )
    .unwrap();
    assert!(result.answer.contains("エラー"));
}

// テスト7: 経験メモリへの記録
#[test]
fn test_experience_recording() {
    let store = MemoryStore::in_memory().unwrap();
    let mock = MockLlmBackend::single("回答です。");
    let tools = test_registry();
    let guard = PathGuard::default_deny_list();
    let config = AgentConfig::default();
    let cancel = CancellationToken::new();

    run_agent_loop(
        "test query",
        &mock,
        &tools,
        &guard,
        &config,
        &cancel,
        Some(&store),
    )
    .unwrap();

    let exp = ExperienceStore::new(store.conn());
    let experiences = exp.find_similar("test", 10).unwrap();
    assert!(!experiences.is_empty());
}

// テスト8: ループ検出
#[test]
fn test_loop_detection() {
    // 全く同じツール呼び出しを繰り返すモック
    let same_call = r#"<tool_call>{"name":"echo","arguments":{"text":"same"}}</tool_call>"#;
    let responses: Vec<String> = (0..10).map(|_| same_call.to_string()).collect();
    let mock = MockLlmBackend::new(responses);
    let tools = test_registry();
    let guard = PathGuard::default_deny_list();
    let config = AgentConfig {
        max_iterations: 10,
        ..Default::default()
    };
    let cancel = CancellationToken::new();

    let result = run_agent_loop(
        "loop",
        &mock,
        &tools,
        &guard,
        &config,
        &cancel,
        None::<&MemoryStore>,
    )
    .unwrap();
    assert!(result.answer.contains("中断"));
}

// --- StallDetector テスト ---

#[test]
fn test_stall_detector_no_progress() {
    let mut sd = StallDetector::new(3);
    assert!(!sd.record_step(false, 1));
    assert!(!sd.record_step(false, 2));
    assert!(sd.record_step(false, 3));
}

#[test]
fn test_stall_detector_resets_on_progress() {
    let mut sd = StallDetector::new(3);
    assert!(!sd.record_step(false, 1));
    assert!(!sd.record_step(false, 2));
    assert!(!sd.record_step(true, 99));
    assert!(!sd.record_step(false, 100));
    assert!(!sd.record_step(false, 101));
    assert!(sd.record_step(false, 102));
}

#[test]
fn test_stall_detector_same_output_hash() {
    let mut sd = StallDetector::new(3);
    // 初回はハッシュが0→42で変化するため進捗あり
    assert!(!sd.record_step(true, 42));
    // 2回目以降は同じハッシュ → 停滞カウント
    assert!(!sd.record_step(true, 42));
    assert!(!sd.record_step(true, 42));
    assert!(sd.record_step(true, 42)); // 3回停滞で検出
}

#[test]
fn test_stall_detector_default_threshold() {
    let sd = StallDetector::default();
    assert_eq!(sd.stall_threshold(), 3);
}

// --- SOUL.md テスト ---

#[test]
fn test_load_soul_missing_is_none() {
    let result = crate::agent::context_inject::load_soul(&Some(std::path::PathBuf::from(
        "/tmp/nonexistent_soul_bonsai.md",
    )));
    assert!(result.is_none());
}

#[test]
fn test_load_soul_from_explicit_path() {
    let path = format!("/tmp/bonsai-test-soul-{}.md", uuid::Uuid::new_v4());
    std::fs::write(&path, "私はテスト用ペルソナです").unwrap();
    let result =
        crate::agent::context_inject::load_soul(&Some(std::path::PathBuf::from(&path)));
    assert!(result.is_some());
    assert!(result.unwrap().contains("ペルソナ"));
    std::fs::remove_file(&path).ok();
}

#[test]
fn test_load_soul_empty_file_is_none() {
    let path = format!("/tmp/bonsai-test-soul-empty-{}.md", uuid::Uuid::new_v4());
    std::fs::write(&path, "   ").unwrap();
    let result =
        crate::agent::context_inject::load_soul(&Some(std::path::PathBuf::from(&path)));
    assert!(result.is_none());
    std::fs::remove_file(&path).ok();
}

#[test]
fn test_load_soul_none_path() {
    // Noneパスの場合、.bonsai/SOUL.mdなどを探すが通常存在しない
    let result = crate::agent::context_inject::load_soul(&None);
    // テスト環境では存在しないのでNone（存在する場合はSome）
    // assertはしない — 環境依存
    let _ = result;
}

// テスト: デフォルトシステムプロンプトに計画強制ルールが含まれる
#[test]
fn test_default_prompt_contains_plan_rule() {
    let config = AgentConfig::default();
    assert!(
        config.system_prompt.contains("計画"),
        "デフォルトプロンプトに計画強制ルールが含まれるべき"
    );
}

// テスト: RepoMapツールがレジストリに登録される
#[test]
fn test_repomap_registered() {
    let reg = test_registry_with_repomap();
    assert!(reg.get("repo_map").is_some(), "repo_mapが登録されるべき");
}

fn test_registry_with_repomap() -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(EchoTool));
    reg.register(Box::new(crate::tools::repomap::RepoMapTool));
    reg
}

// テスト: StepOutcomeが監査ログに記録される
#[test]
fn test_step_outcome_audit_logged() {
    let store = MemoryStore::in_memory().unwrap();
    let mock = MockLlmBackend::single("回答です。");
    let tools = test_registry();
    let guard = PathGuard::default_deny_list();
    let config = AgentConfig::default();
    let cancel = CancellationToken::new();

    run_agent_loop(
        "test",
        &mock,
        &tools,
        &guard,
        &config,
        &cancel,
        Some(&store),
    )
    .unwrap();

    let audit = AuditLog::new(store.conn());
    let entries = audit.recent(50).unwrap();
    let step_outcomes: Vec<_> = entries
        .iter()
        .filter(|e| e.action_type == "step_outcome")
        .collect();
    assert!(
        !step_outcomes.is_empty(),
        "StepOutcomeが監査ログに記録されるべき"
    );
}

// テスト: タスク複雑さ検出
#[test]
fn test_detect_task_complexity_simple() {
    assert!(!detect_task_complexity("天気は？"));
    assert!(!detect_task_complexity("ファイルを読んで"));
}

#[test]
fn test_detect_task_complexity_complex() {
    assert!(detect_task_complexity(
        "テストを書いて、実装して、リファクタリングして"
    ));
    assert!(detect_task_complexity(&"a".repeat(201)));
}

// テスト: 計画プレステップ注入
#[test]
fn test_inject_planning_step_complex() {
    let mut session = Session::new();
    session.add_message(Message::user("テストを書いて実装して"));
    inject_planning_step(
        &mut session,
        "テストを書いて、実装して、リファクタリングして",
    );
    let has_plan = session.messages.iter().any(|m| m.content.contains("計画"));
    assert!(has_plan, "複雑タスクに計画プレステップが注入されるべき");
}

#[test]
fn test_inject_planning_step_simple() {
    let mut session = Session::new();
    inject_planning_step(&mut session, "天気は？");
    let msg_count = session.messages.len();
    assert_eq!(msg_count, 0, "単純タスクには計画プレステップ不要");
}

// テスト: AdvisorConfig が AgentConfig に統合されている
#[test]
fn test_agent_config_includes_advisor() {
    let config = AgentConfig::default();
    assert_eq!(config.advisor.max_uses, 3);
    assert_eq!(config.advisor.calls_used, 0);
    assert!(config.advisor.can_advise());
}

// テスト: AdvisorConfig をカスタマイズ可能
#[test]
fn test_agent_config_custom_advisor() {
    let config = AgentConfig {
        advisor: AdvisorConfig {
            max_uses: 1,
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(config.advisor.max_uses, 1);
}

// テスト: task_timeoutが設定されるとエージェントループがタイムアウトする
#[test]
fn test_task_timeout_triggers() {
    // 各ステップ遅延が発生するためタイムアウトする想定（0秒タイムアウト）
    let responses: Vec<String> = (0..100).map(|_| "考え中です...".to_string()).collect();
    let mock = MockLlmBackend::new(responses);
    let tools = test_registry();
    let guard = PathGuard::default_deny_list();
    let config = AgentConfig {
        max_iterations: 100,
        task_timeout: Some(std::time::Duration::from_millis(1)),
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    let result = run_agent_loop("test", &mock, &tools, &guard, &config, &cancel, None);
    assert!(result.is_ok());
    let r = result.unwrap();
    // タイムアウトまたは少ないイテレーションで完了
    assert!(r.answer.contains("タイムアウト") || r.iterations_used < 100);
}

// テスト: task_timeout=Noneではタイムアウトしない
#[test]
fn test_no_timeout_by_default() {
    let config = AgentConfig::default();
    assert!(config.task_timeout.is_none());
}

// テスト: inject_verification_step — 複雑タスク＋初回以降で検証挿入
#[test]
fn test_inject_verification_step_injects() {
    let mut session = Session::new();
    let mut advisor = AdvisorConfig::default();
    let injected = inject_verification_step(
        &mut session,
        &mut advisor,
        "テストを書いて、実装して、リファクタしてください",
        "部分的な回答",
        1, // iteration > 0
        10,
        None,
        &TrialSummary::default(),
    );
    assert!(injected, "複雑タスクは検証ステップを挿入");
    assert_eq!(advisor.calls_used, 1);
    assert!(session.messages.iter().any(|m| m.content.contains("検証")));
}

// テスト: 初回イテレーションでは検証スキップ
#[test]
fn test_inject_verification_step_skips_first_iteration() {
    let mut session = Session::new();
    let mut advisor = AdvisorConfig::default();
    let injected = inject_verification_step(
        &mut session,
        &mut advisor,
        "テストを書いて、実装して、リファクタしてください",
        "回答",
        0, // 初回
        10,
        None,
        &TrialSummary::default(),
    );
    assert!(!injected);
    assert_eq!(advisor.calls_used, 0);
}

// テスト: [検証済] マーカーがある場合はスキップ
#[test]
fn test_inject_verification_step_skips_verified() {
    let mut session = Session::new();
    let mut advisor = AdvisorConfig::default();
    let injected = inject_verification_step(
        &mut session,
        &mut advisor,
        "テストを書いて、実装して、リファクタしてください",
        "[検証済] 完了しました",
        1,
        10,
        None,
        &TrialSummary::default(),
    );
    assert!(!injected);
}

// テスト: max_uses 超過時はスキップ
#[test]
fn test_inject_verification_step_respects_max_uses() {
    let mut session = Session::new();
    let mut advisor = AdvisorConfig {
        max_uses: 1,
        calls_used: 1, // 既に上限
        ..Default::default()
    };
    let injected = inject_verification_step(
        &mut session,
        &mut advisor,
        "テストを書いて、実装して、リファクタしてください",
        "回答",
        1,
        10,
        None,
        &TrialSummary::default(),
    );
    assert!(!injected);
}

// テスト: 単純タスクはスキップ
#[test]
fn test_inject_verification_step_skips_simple_task() {
    let mut session = Session::new();
    let mut advisor = AdvisorConfig::default();
    let injected = inject_verification_step(
        &mut session,
        &mut advisor,
        "天気は？",
        "晴れです",
        1,
        10,
        None,
        &TrialSummary::default(),
    );
    assert!(!injected);
}

// テスト: inject_replan_on_stall — 閾値到達で再計画注入
#[test]
fn test_inject_replan_on_stall_triggers_after_threshold() {
    let mut session = Session::new();
    let mut stall = StallDetector::new(3);
    let mut advisor = AdvisorConfig::default();
    // 1〜2回目: 検出されない
    assert!(!inject_replan_on_stall(
        &mut session,
        &mut stall,
        &mut advisor,
        "task",
        false,
        0,
        None,
        &TrialSummary::default()
    ));
    assert!(!inject_replan_on_stall(
        &mut session,
        &mut stall,
        &mut advisor,
        "task",
        false,
        0,
        None,
        &TrialSummary::default()
    ));
    // 3回目: 停滞検出→再計画注入
    assert!(inject_replan_on_stall(
        &mut session,
        &mut stall,
        &mut advisor,
        "task",
        false,
        0,
        None,
        &TrialSummary::default()
    ));
    assert_eq!(advisor.calls_used, 1);
    assert!(session.messages.iter().any(|m| m.content.contains("停滞")));
}

// テスト: inject_replan_on_stall — advisor max_uses超過時はreset+スキップ
#[test]
fn test_inject_replan_on_stall_respects_advisor_max_uses() {
    let mut session = Session::new();
    let mut stall = StallDetector::new(2);
    let mut advisor = AdvisorConfig {
        max_uses: 1,
        calls_used: 1,
        ..Default::default()
    };
    assert!(!inject_replan_on_stall(
        &mut session,
        &mut stall,
        &mut advisor,
        "task",
        false,
        0,
        None,
        &TrialSummary::default()
    ));
    let injected = inject_replan_on_stall(
        &mut session,
        &mut stall,
        &mut advisor,
        "task",
        false,
        0,
        None,
        &TrialSummary::default(),
    );
    assert!(!injected, "max_uses超過時は注入しない");
    assert_eq!(advisor.calls_used, 1, "calls_usedは増えない");
}

// テスト: inject_replan_on_stall — 進捗ありでスキップ
#[test]
fn test_inject_replan_on_stall_skips_on_progress() {
    let mut session = Session::new();
    let mut stall = StallDetector::new(2);
    let mut advisor = AdvisorConfig::default();
    assert!(!inject_replan_on_stall(
        &mut session,
        &mut stall,
        &mut advisor,
        "task",
        true,
        1,
        None,
        &TrialSummary::default()
    ));
    assert!(!inject_replan_on_stall(
        &mut session,
        &mut stall,
        &mut advisor,
        "task",
        true,
        2,
        None,
        &TrialSummary::default()
    ));
    assert!(!inject_replan_on_stall(
        &mut session,
        &mut stall,
        &mut advisor,
        "task",
        true,
        3,
        None,
        &TrialSummary::default()
    ));
    assert_eq!(advisor.calls_used, 0);
}

// テスト: compute_output_hash は変化を検出
#[test]
fn test_compute_output_hash_differs_for_different_content() {
    let mut s1 = Session::new();
    s1.add_message(Message::user("A"));
    let h1 = compute_output_hash(&s1);
    let mut s2 = Session::new();
    s2.add_message(Message::user("B"));
    let h2 = compute_output_hash(&s2);
    assert_ne!(h1, h2);
}

// テスト: resolve_advisor_prompt はリモート未設定時にローカルを返す
#[test]
fn test_resolve_advisor_prompt_local_when_no_endpoint() {
    let mut advisor = AdvisorConfig::default();
    let v = resolve_advisor_prompt(&mut advisor, AdvisorRole::Verification, "task");
    let r = resolve_advisor_prompt(&mut advisor, AdvisorRole::Replan, "task");
    assert_eq!(v.source, "local");
    assert_eq!(r.source, "local");
    assert_eq!(v.duration_ms, 0);
    assert!(v.prompt.contains("検証"));
    assert!(r.prompt.contains("停滞"));
}

// テスト: log_advisor_call は store=None でもパニックしない
#[test]
fn test_log_advisor_call_with_no_store() {
    let session = Session::new();
    let resolution = AdvisorResolution {
        prompt: "test".to_string(),
        source: "local",
        duration_ms: 0,
    };
    // store=None: 何もしない（パニックしない）
    log_advisor_call(None, &session, AdvisorRole::Verification, &resolution);
}

// テスト: log_advisor_call が store にエントリを追加
#[test]
fn test_log_advisor_call_writes_to_store() {
    let store = MemoryStore::in_memory().unwrap();
    let session = Session::new();
    let resolution = AdvisorResolution {
        prompt: "verification prompt content".to_string(),
        source: "remote",
        duration_ms: 123,
    };
    log_advisor_call(
        Some(&store),
        &session,
        AdvisorRole::Verification,
        &resolution,
    );

    let audit = AuditLog::new(store.conn());
    let entries = audit.for_session(&session.id).unwrap();
    let advisor_entries: Vec<_> = entries
        .iter()
        .filter(|e| e.action_type == "advisor_call")
        .collect();
    assert_eq!(advisor_entries.len(), 1);
    assert!(
        advisor_entries[0]
            .action_data
            .contains("\"role\":\"verification\"")
    );
    assert!(
        advisor_entries[0]
            .action_data
            .contains("\"source\":\"remote\"")
    );
}

// テスト: handle_outcome — FinalAnswer で Return
#[test]
fn test_handle_outcome_final_answer_returns() {
    let mut session = Session::new();
    let mut state = LoopState::new(AdvisorConfig::default());
    let outcome = StepOutcome::FinalAnswer("回答".to_string());
    let action = handle_outcome(
        outcome,
        &mut session,
        &mut state,
        "simple",
        None,
        10,
        1,
        0,
        100,
    );
    assert!(matches!(action, OutcomeAction::Return(_)));
}

// テスト: handle_outcome — Continue で Continue
#[test]
fn test_handle_outcome_continue_returns_continue() {
    let mut session = Session::new();
    let mut state = LoopState::new(AdvisorConfig::default());
    let outcome = StepOutcome::Continue(vec!["shell".to_string()]);
    let action = handle_outcome(
        outcome,
        &mut session,
        &mut state,
        "task",
        None,
        10,
        1,
        0,
        100,
    );
    assert!(matches!(action, OutcomeAction::Continue));
    assert_eq!(state.all_tools.len(), 1);
}

// テスト: handle_outcome — Aborted で Return
#[test]
fn test_handle_outcome_aborted_returns() {
    let mut session = Session::new();
    let mut state = LoopState::new(AdvisorConfig::default());
    let outcome = StepOutcome::Aborted("cancelled".to_string());
    let action = handle_outcome(
        outcome,
        &mut session,
        &mut state,
        "task",
        None,
        10,
        1,
        0,
        100,
    );
    assert!(matches!(action, OutcomeAction::Return(_)));
    assert_eq!(state.consecutive_failures, 1);
}

// テスト: LoopState 初期状態
#[test]
fn test_loop_state_new() {
    let state = LoopState::new(AdvisorConfig::default());
    assert!(state.all_tools.is_empty());
    assert_eq!(state.consecutive_failures, 0);
    assert_eq!(state.iteration, 0);
    assert!(state.advisor.can_advise());
}

// テスト: AgentConfig に auto_checkpoint デフォルト値 true
#[test]
fn test_agent_config_default_auto_checkpoint_enabled() {
    let config = AgentConfig::default();
    assert!(config.auto_checkpoint);
}

// テスト: create_task_start_checkpoint — store なしでも動作
#[test]
fn test_create_task_start_checkpoint_no_store() {
    let session = Session::new();
    // git stash の結果に依存するが、関数自体は panic しない
    let _id = create_task_start_checkpoint(&session, "テストタスク", None);
    // インメモリ or git失敗 のどちらでもOK
}

// テスト: create_task_start_checkpoint — store ありで永続化
#[test]
fn test_create_task_start_checkpoint_with_store() {
    use crate::agent::checkpoint::CheckpointManager;
    let store = MemoryStore::in_memory().unwrap();
    let session = Session::new();
    let id_opt = create_task_start_checkpoint(&session, "永続化テスト", Some(&store));
    // git stash が成功する場合（リポ内）は Some、失敗してもエラーなし
    if let Some(id) = id_opt {
        assert!(id > 0, "永続IDは正");
        let loaded =
            CheckpointManager::load_persisted(store.conn(), Some(&session.id)).unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].description.contains("auto-start"));
    }
}

// テスト: 読取専用ツール並列化 — ValidatedCall構造体
#[test]
fn test_validated_call_read_only_flag() {
    let tool = EchoTool;
    assert!(!tool.is_read_only(), "EchoToolはis_read_only=false");
}

// テスト: FileReadToolはis_read_only=true
#[test]
fn test_file_read_is_read_only() {
    let tool = crate::tools::file::FileReadTool;
    assert!(tool.is_read_only());
}

// テスト: RepoMapToolはis_read_only=true
#[test]
fn test_repo_map_is_read_only() {
    let tool = crate::tools::repomap::RepoMapTool;
    assert!(tool.is_read_only());
}

// テスト: execute_validated_calls — 空リストでパニックしない
#[test]
fn test_execute_validated_calls_empty() {
    use crate::safety::secrets::SecretsFilter;
    let mut session = Session::new();
    let mut cb = CircuitBreaker::default();
    let sf = SecretsFilter::default();
    let mut cache = ToolResultCache::new();
    let result = execute_validated_calls(&[], &mut session, &mut cb, &sf, None, &mut cache);
    assert!(result.is_empty());
}

#[test]
fn test_inference_for_task_file_operation() {
    let base = InferenceParams::default();
    let params = inference_for_task(TaskType::FileOperation, &base);
    assert!((params.temperature - 0.3).abs() < f64::EPSILON);
    assert_eq!(params.max_tokens, base.max_tokens); // 他のフィールドは保持
}

#[test]
fn test_inference_for_task_research() {
    let base = InferenceParams::default();
    let params = inference_for_task(TaskType::Research, &base);
    assert!((params.temperature - 0.6).abs() < f64::EPSILON);
}

#[test]
fn test_inference_for_task_general_unchanged() {
    let base = InferenceParams::default();
    let params = inference_for_task(TaskType::General, &base);
    assert!((params.temperature - base.temperature).abs() < f64::EPSILON);
}

// テスト: apply_tool_result でツール成功時にKnowledgeGraphにツール使用が記録される
#[test]
fn test_apply_tool_result_records_graph_tool_usage() {
    use crate::safety::secrets::SecretsFilter;
    let store = MemoryStore::in_memory().unwrap();
    let mut session = Session::new();
    let mut cb = CircuitBreaker::default();
    let sf = SecretsFilter::default();

    let r = ToolExecResult {
        name: "file_read".to_string(),
        args_json: r#"{"path": "src/main.rs"}"#.to_string(),
        output: "file contents here".to_string(),
        success: true,
        is_error: false,
    };
    apply_tool_result(&r, &mut session, &mut cb, &sf, Some(&store), 4000);

    // グラフにツール使用が記録されていることを確認
    let graph = KnowledgeGraph::new(store.conn());
    let neighbors = graph.neighbors("file_read", 1).unwrap();
    assert_eq!(
        neighbors.len(),
        1,
        "ツール→ファイルのエッジが記録されるべき"
    );
    assert_eq!(neighbors[0].0, "src/main.rs");
    assert_eq!(neighbors[0].1, "uses");
}

// テスト: apply_tool_result でツール失敗時にエラーパターンが記録される
#[test]
fn test_apply_tool_result_records_graph_error_pattern() {
    use crate::safety::secrets::SecretsFilter;
    let store = MemoryStore::in_memory().unwrap();
    let mut session = Session::new();
    let mut cb = CircuitBreaker::default();
    let sf = SecretsFilter::default();

    let r = ToolExecResult {
        name: "shell".to_string(),
        args_json: r#"{"path": "src/lib.rs"}"#.to_string(),
        output: "error: compilation failed".to_string(),
        success: false,
        is_error: true,
    };
    apply_tool_result(&r, &mut session, &mut cb, &sf, Some(&store), 4000);

    // グラフにエラーパターンが記録されていることを確認
    let graph = KnowledgeGraph::new(store.conn());
    let error_neighbors = graph.neighbors("tool_error", 1).unwrap();
    assert!(
        error_neighbors
            .iter()
            .any(|(name, rel, _)| name == "src/lib.rs" && rel == "caused_by"),
        "エラー→ファイルのcaused_byエッジが記録されるべき"
    );
}

// テスト: inject_experience_context — 成功/失敗を分離してフォーマット
#[test]
fn t_inject_experience_context_formats_correctly() {
    let store = MemoryStore::in_memory().unwrap();
    let exp = ExperienceStore::new(store.conn());

    // 成功経験を記録
    exp.record(&RecordParams {
        exp_type: ExperienceType::Success,
        task_context: "file editing",
        action: "file_write with fuzzy match",
        outcome: "edit succeeded",
        lesson: Some("fuzzyマッチで成功"),
        tool_name: Some("file_write"),
        error_type: None,
        error_detail: None,
    })
    .unwrap();

    // 失敗経験を記録
    exp.record(&RecordParams {
        exp_type: ExperienceType::Failure,
        task_context: "file reading",
        action: "file_read timeout",
        outcome: "timeout error",
        lesson: Some("タイムアウト、リトライで解決"),
        tool_name: Some("file_read"),
        error_type: Some("Timeout"),
        error_detail: Some("read timeout"),
    })
    .unwrap();

    let mut session = Session::new();
    inject_experience_context(&mut session, "file", &store);

    // メッセージが追加されていること
    assert_eq!(session.messages.len(), 1);
    let msg = &session.messages[0].content;
    assert!(
        msg.contains("<context type=\"experience\">"),
        "統一コンテキストタグで囲まれるべき"
    );
    assert!(
        msg.contains("[成功パターン]"),
        "成功パターンセクションがあるべき"
    );
    assert!(
        msg.contains("[失敗パターン]"),
        "失敗パターンセクションがあるべき"
    );
    assert!(
        msg.contains("fuzzyマッチで成功"),
        "成功のlessonが含まれるべき"
    );
    assert!(
        msg.contains("タイムアウト、リトライで解決"),
        "失敗のlessonが含まれるべき"
    );
}

// テスト: inject_experience_context — 経験が空の場合にメッセージ追加しない
#[test]
fn t_inject_experience_context_empty_no_message() {
    let store = MemoryStore::in_memory().unwrap();
    let mut session = Session::new();
    inject_experience_context(&mut session, "nonexistent_task_xyz", &store);
    assert!(session.messages.is_empty(), "経験が空ならメッセージ不追加");
}

// テスト: inject_experience_context — Insightタイプも含まれる
#[test]
fn t_inject_experience_context_includes_insights() {
    let store = MemoryStore::in_memory().unwrap();
    let exp = ExperienceStore::new(store.conn());

    exp.record(&RecordParams {
        exp_type: ExperienceType::Insight,
        task_context: "deploy task",
        action: "deploy analysis",
        outcome: "rollback needed",
        lesson: Some("デプロイ前にテスト必須"),
        tool_name: None,
        error_type: None,
        error_detail: None,
    })
    .unwrap();

    let mut session = Session::new();
    inject_experience_context(&mut session, "deploy", &store);

    assert_eq!(session.messages.len(), 1);
    let msg = &session.messages[0].content;
    assert!(msg.contains("[学び]"), "学びセクションがあるべき");
    assert!(
        msg.contains("デプロイ前にテスト必須"),
        "Insightのlessonが含まれるべき"
    );
}

// テスト: 全コンテキスト注入が統一タグフォーマット <context type="xxx"> を使用する
#[test]
fn t_context_tags_consistent() {
    let store = MemoryStore::in_memory().unwrap();
    let exp = ExperienceStore::new(store.conn());

    // 経験注入が統一タグを使用
    exp.record(&RecordParams {
        exp_type: ExperienceType::Success,
        task_context: "consistency check",
        action: "test action",
        outcome: "ok",
        lesson: Some("lesson"),
        tool_name: None,
        error_type: None,
        error_detail: None,
    })
    .unwrap();

    let mut session = Session::new();
    inject_experience_context(&mut session, "consistency", &store);

    if !session.messages.is_empty() {
        let msg = &session.messages[0].content;
        assert!(
            msg.starts_with("<context type="),
            "経験注入は<context type=で始まるべき"
        );
        assert!(
            msg.ends_with("</context>"),
            "経験注入は</context>で終わるべき"
        );
    }
}

#[test]
fn t_loop_state_has_trial_summary() {
    let state = LoopState::new(AdvisorConfig::default());
    assert!(state.trial_summary.is_empty());
}

#[test]
fn t_planning_step_contains_hypothesis() {
    let mut session = Session::new();
    inject_planning_step(
        &mut session,
        "テストを書いて、実装して、リファクタリングして",
    );
    let last = session.messages.last().unwrap();
    assert!(
        last.content.contains("仮説"),
        "仮説キーワード: {}",
        last.content
    );
}

#[test]
fn t_verification_checklist() {
    let mut session = Session::new();
    let mut advisor = AdvisorConfig::default();
    let injected = inject_verification_step(
        &mut session,
        &mut advisor,
        "テストを書いて、実装して、リファクタリングして",
        "完了しました",
        1,
        10,
        None,
        &TrialSummary::default(),
    );
    if injected {
        let has_checklist = session
            .messages
            .iter()
            .any(|m| m.content.contains("チェックリスト"));
        assert!(has_checklist, "検証チェックリストが注入される");
    }
}

#[test]
fn t_replan_with_trial_summary() {
    let mut session = Session::new();
    let mut stall = StallDetector::default();
    let mut advisor = AdvisorConfig::default();
    let mut ts = TrialSummary::default();
    ts.record_failure("shell", r#"{"command":"cargo build"}"#, "compile error", 1);
    // 閾値到達させる
    inject_replan_on_stall(
        &mut session,
        &mut stall,
        &mut advisor,
        "task",
        false,
        0,
        None,
        &ts,
    );
    inject_replan_on_stall(
        &mut session,
        &mut stall,
        &mut advisor,
        "task",
        false,
        0,
        None,
        &ts,
    );
    let triggered = inject_replan_on_stall(
        &mut session,
        &mut stall,
        &mut advisor,
        "task",
        false,
        0,
        None,
        &ts,
    );
    if triggered {
        let has_trial = session
            .messages
            .iter()
            .any(|m| m.content.contains("[EVALUATION]") || m.content.contains("試した方法"));
        assert!(
            has_trial,
            "構造化フィードバックまたは試行サマリーがreplanに含まれる"
        );
    }
}

// テスト: check_invariants — 正常セッションで違反なし
#[test]
fn t_check_invariants_no_violations() {
    let mut session = Session::new();
    session.add_message(Message::user(
        "テストを書いて、実装して、リファクタリングして",
    ));
    session.add_message(Message::assistant(
        "実装が完了しました。テスト結果: 全パス".to_string(),
    ));
    session.add_message(Message {
        role: Role::Tool,
        content: "ファイルを正常に読み込みました".to_string(),
        attachments: Vec::new(),
        tool_call_id: None,
    });
    let violations = check_invariants(&session, "テストを書いて実装して");
    assert!(
        violations.is_empty(),
        "正常セッションでは違反なし: {:?}",
        violations
    );
}

// テスト: check_invariants — ツール失敗多い場合に違反検出
#[test]
fn t_check_invariants_low_success_rate() {
    let mut session = Session::new();
    session.add_message(Message::user("テストを書いて"));
    // ツール失敗メッセージ3件
    for _ in 0..3 {
        session.add_message(Message {
            role: Role::Tool,
            content: "エラー: ファイルが見つかりません".to_string(),
            attachments: Vec::new(),
            tool_call_id: None,
        });
    }
    // ツール成功メッセージ1件（成功率25% < 50%）
    session.add_message(Message {
        role: Role::Tool,
        content: "OK".to_string(),
        attachments: Vec::new(),
        tool_call_id: None,
    });
    let violations = check_invariants(&session, "テストを書いて");
    assert!(!violations.is_empty(), "低成功率で違反検出されるべき");
    assert!(
        violations[0].contains("ツール成功率が低い"),
        "成功率警告: {}",
        violations[0]
    );
}

// テスト: before_stepフックがAbort時にループを中断する（NAT知見、項目142統合）
#[test]
fn test_before_step_abort_stops_loop() {
    use crate::agent::middleware::{Middleware, MiddlewareSignal, StepResult as MwStepResult};

    struct AbortMiddleware;
    impl Middleware for AbortMiddleware {
        fn name(&self) -> &str {
            "abort_test"
        }
        fn before_step(&mut self, _session: &Session, _iteration: usize) -> MiddlewareSignal {
            MiddlewareSignal::Abort("テスト中断".to_string())
        }
        fn after_step(
            &mut self,
            _session: &mut Session,
            _result: &MwStepResult,
        ) -> MiddlewareSignal {
            MiddlewareSignal::Ok
        }
    }

    let mut chain = MiddlewareChain::new();
    chain.add(Box::new(AbortMiddleware));
    let mut session = Session::new();
    let abort = chain.run_before_step(&mut session, 0);
    assert!(abort.is_some(), "Abortミドルウェアはループ中断を返すべき");
    assert!(abort.unwrap().contains("テスト中断"));
}

// テスト: before_stepフックがInject時にセッションにメッセージ追加
#[test]
fn test_before_step_inject_adds_message() {
    use crate::agent::middleware::{Middleware, MiddlewareSignal, StepResult as MwStepResult};

    struct InjectMiddleware;
    impl Middleware for InjectMiddleware {
        fn name(&self) -> &str {
            "inject_test"
        }
        fn before_step(&mut self, _session: &Session, _iteration: usize) -> MiddlewareSignal {
            MiddlewareSignal::Inject("注入テスト".to_string())
        }
        fn after_step(
            &mut self,
            _session: &mut Session,
            _result: &MwStepResult,
        ) -> MiddlewareSignal {
            MiddlewareSignal::Ok
        }
    }

    let mut chain = MiddlewareChain::new();
    chain.add(Box::new(InjectMiddleware));
    let mut session = Session::new();
    let msg_count_before = session.messages.len();
    let abort = chain.run_before_step(&mut session, 0);
    assert!(abort.is_none(), "Injectはループ中断しない");
    assert_eq!(session.messages.len(), msg_count_before + 1);
    assert!(
        session
            .messages
            .last()
            .unwrap()
            .content
            .contains("注入テスト")
    );
}

/// 項目162: P1 Step 5 ランタイム統合
/// run_agent_loop が EventStore に SessionStart/UserMessage/ToolCallStart/End/SessionEnd を emit
#[test]
fn test_run_agent_loop_emits_events() {
    use crate::agent::event_store::EventStore;

    let store = MemoryStore::in_memory().expect("in-memory store");

    // ツール1回 → 回答（test_single_tool_call と同じシナリオ）
    let mock = MockLlmBackend::new(vec![
        r#"<tool_call>{"name":"echo","arguments":{"text":"hello"}}</tool_call>"#.to_string(),
        "ツール結果: hello".to_string(),
    ]);
    let tools = test_registry();
    let guard = PathGuard::default_deny_list();
    let config = AgentConfig::default();
    let cancel = CancellationToken::new();

    let result = run_agent_loop(
        "echo test",
        &mock,
        &tools,
        &guard,
        &config,
        &cancel,
        Some(&store),
    )
    .unwrap();
    assert!(result.tools_called.contains(&"echo".to_string()));

    // EventStore に積まれたイベントを検証
    let es = EventStore::new(store.conn());
    let sessions = es.list_sessions().expect("list_sessions");
    assert_eq!(sessions.len(), 1, "1セッション分のイベントが記録される");
    let events = es.replay(&sessions[0]).expect("replay");
    let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();

    assert!(types.contains(&"session_start"), "SessionStart 必須");
    assert!(types.contains(&"user_message"), "UserMessage 必須");
    assert!(
        types.contains(&"tool_call_start"),
        "ToolCallStart 必須 (echo呼出)"
    );
    assert!(
        types.contains(&"tool_call_end"),
        "ToolCallEnd 必須 (echo呼出)"
    );
    assert!(
        types.contains(&"session_end"),
        "SessionEnd 必須 (extract_successful_trajectories の前提)"
    );

    // 軌跡抽出の最低限ガード（成功率100%、1ステップ以上）が通ることを確認
    let trajectories = es
        .extract_successful_trajectories(0.5, 1)
        .expect("extract_successful_trajectories");
    assert_eq!(trajectories.len(), 1, "1セッション分の軌跡が抽出される");
    let traj = &trajectories[0];
    assert!(
        traj.tool_sequence.contains(&"echo".to_string()),
        "tool_sequence に echo が含まれる"
    );
    assert!(
        (traj.tool_success_rate - 1.0).abs() < 1e-6,
        "成功率100% (失敗なし)"
    );
}
