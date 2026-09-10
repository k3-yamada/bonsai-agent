//! Issue #22 B3-4: セッション境界の回帰ゲート。
//!
//! bonsai-agent は「1プロセス=1ユーザーセッション」を前提とする実行モデルであり、
//! プロセス内で複数の `Session` が並存するのは `SubAgentExecutor::execute_parallel`
//! （`src/agent/subagent.rs`）ただ1箇所に限定される（詳細は
//! `docs/decisions/ADR-014-single-session-execution-model.md` 参照）。
//!
//! 本ファイルはその境界を固定する回帰テスト:
//! - AC-4-1: `ToolResultCache` はセッション間で共有されない
//! - AC-4-2: Working Memory (`Session.messages`) はセッション間で共有されない
//! - AC-4-3: 並列サブエージェントはそれぞれ相異なる `session_id` を持つ
//! - AC-4-4: 親に `confirm_callback` がある場合、サブエージェントは順次実行される
//!
//! `tests/` 配下は `tests/structural.rs::walk_src()` の走査対象外
//! （`Path::new("src")` のみを走査するため）であり、DEP-001/SIZE-001/LOG-001の
//! いずれの対象でもない。既存 `tests/e2e_extension_flow.rs` と同様、
//! `bonsai_agent::agent::*` / `bonsai_agent::tools::*` を自由に use してよい。

use bonsai_agent::agent::agent_loop::{AgentConfig, ConfirmCallback};
use bonsai_agent::agent::event_store::EventStore;
use bonsai_agent::agent::subagent::{SubAgentConfig, SubAgentExecutor};
use bonsai_agent::agent::validate::PathGuard;
use bonsai_agent::cancel::CancellationToken;
use bonsai_agent::domain::conversation::{Message, Session};
use bonsai_agent::domain::llm::{GenerateResult, LlmBackend, MockLlmBackend, TokenUsage};
use bonsai_agent::domain::tool_schema::ToolSchema;
use bonsai_agent::memory::store::MemoryStore;
use bonsai_agent::tools::file::FileReadTool;
use bonsai_agent::tools::shell::ShellTool;
use bonsai_agent::tools::{Tool, ToolRegistry, ToolResultCache};
use std::sync::{Arc, Mutex};

fn tmp_path_guard() -> PathGuard {
    PathGuard::new(vec![std::env::temp_dir().to_string_lossy().to_string()])
}

/// AC-4-3 強化用: `MockLlmBackend` と同じスクリプト化レスポンス方式だが、
/// `generate()` が呼ばれた `std::thread::ThreadId` を記録する。
///
/// `should_parallelize` が実際に並列（別スレッド）経路を通ったことを、
/// 「session_idが相異なる」という間接的な証拠ではなく、
/// 「generate() が複数の異なるスレッドから呼ばれた」という直接的な証拠で固定する
/// （qa-reviewer MEDIUM-1: `should_parallelize` を `false` 固定にするミューテーションで
/// 実際にテストが落ちることを保証する）。
struct ThreadIdRecordingBackend {
    responses: Mutex<Vec<String>>,
    thread_ids: Arc<Mutex<Vec<std::thread::ThreadId>>>,
}

impl ThreadIdRecordingBackend {
    fn new(responses: Vec<String>, thread_ids: Arc<Mutex<Vec<std::thread::ThreadId>>>) -> Self {
        // MockLlmBackend と同様、逆順にしてpop()で先頭から取り出す
        let mut reversed = responses;
        reversed.reverse();
        Self {
            responses: Mutex::new(reversed),
            thread_ids,
        }
    }
}

impl LlmBackend for ThreadIdRecordingBackend {
    fn model_id(&self) -> &str {
        "thread-id-recording-mock"
    }

    fn generate(
        &self,
        _messages: &[Message],
        _tools: &[ToolSchema],
        on_token: &mut dyn FnMut(&str),
        _cancel: &CancellationToken,
    ) -> anyhow::Result<GenerateResult> {
        self.thread_ids
            .lock()
            .unwrap()
            .push(std::thread::current().id());

        let mut responses = self.responses.lock().unwrap();
        let text = responses
            .pop()
            .unwrap_or_else(|| "（モックのレスポンスが枯渇しました）".to_string());
        for word in text.split_whitespace() {
            on_token(word);
            on_token(" ");
        }
        Ok(GenerateResult {
            text: text.clone(),
            usage: TokenUsage {
                prompt_tokens: 10,
                completion_tokens: text.split_whitespace().count(),
                duration: std::time::Duration::from_millis(0),
            },
            model_id: self.model_id().to_string(),
        })
    }
}

/// AC-4-1: 新しいセッション（＝新しい `ToolResultCache` インスタンス）は、
/// 前のセッションでキャッシュされたツール結果を一切引き継がないこと。
#[test]
fn t_tool_cache_is_not_shared_across_sessions() {
    let tmp = tempfile::NamedTempFile::new().expect("tempfile作成失敗");
    std::fs::write(tmp.path(), "session-a-content").expect("初期書き込み失敗");

    let tool = FileReadTool;
    let args = serde_json::json!({"path": tmp.path().to_str().unwrap()});

    // --- セッションA相当: 読み取り、キャッシュへ格納 ---
    let mut cache_a = ToolResultCache::new();
    let result_a = tool.call(args.clone()).expect("file_read失敗");
    assert!(result_a.output.contains("session-a-content"));
    cache_a.put("file_read", &args, result_a);
    assert!(
        cache_a.get("file_read", &args).is_some(),
        "同一セッション内キャッシュは有効であるべき"
    );

    // ファイル内容を変更（セッション境界をまたぐ変化をシミュレート）
    std::fs::write(tmp.path(), "session-b-content").expect("上書き失敗");

    // --- セッションB相当: 新規 ToolResultCache（旧キャッシュを一切参照しない）---
    let mut cache_b = ToolResultCache::new();
    assert!(
        cache_b.get("file_read", &args).is_none(),
        "新セッションのキャッシュに旧セッションの結果が漏れてはならない"
    );

    let result_b = tool.call(args.clone()).expect("file_read失敗");
    assert!(result_b.output.contains("session-b-content"));
    assert!(!result_b.output.contains("session-a-content"));
    cache_b.put("file_read", &args, result_b);
    assert!(cache_b.get("file_read", &args).is_some());
}

/// AC-4-2: `Session` はそれぞれ独立した `messages` を持ち、
/// `SubAgentExecutor` 実行の前後で親 `Session` は変化しないこと。
#[test]
fn t_working_memory_is_not_shared_across_sessions() {
    let mut session_a = Session::new();
    session_a.add_message(Message::user("Aの発言1"));
    session_a.add_message(Message::user("Aの発言2"));

    let session_b = Session::new();
    assert_ne!(session_a.id, session_b.id);
    assert_eq!(
        session_b.messages.len(),
        0,
        "新規SessionはセッションAの状態に関わらず空であるべき"
    );

    let before = session_a.messages.len();

    // SubAgentExecutor はサブタスクごとに新規 Session を内部生成する
    // （new_session_with_system 経由）。親 Session A には一切触れない構造を検証。
    let store = MemoryStore::in_memory().expect("in-memory store作成失敗");
    let backend = MockLlmBackend::new(vec!["サブタスク完了".to_string()]);
    let tools = ToolRegistry::default();
    let path_guard = tmp_path_guard();
    let cancel = CancellationToken::new();
    let sub_config = SubAgentConfig::from_parent(&AgentConfig::default(), 0);

    let executor = SubAgentExecutor::new(
        &backend,
        &tools,
        &path_guard,
        &cancel,
        Some(&store),
        sub_config,
    );
    let _ = executor
        .execute("parent-task", &["サブタスクゴール".to_string()])
        .expect("execute失敗");

    assert_eq!(
        session_a.messages.len(),
        before,
        "サブエージェント実行の前後で親Sessionのメッセージ数は不変であるべき"
    );
}

/// AC-4-3: file-backed `MemoryStore`（`store_clonable=true`）+ 独立な複数goalで
/// 並列実行された場合、各サブエージェントの `session_id` がすべて相異なること。
/// in-memory store は `store_clonable=false` で順次実行に落ちるため使えない。
///
/// qa-reviewer MEDIUM-1 対応: session_id が相異なることは逐次実行でも成立してしまい
/// 「並列経路を通ったこと」の証明にならない。`ThreadIdRecordingBackend` で
/// `generate()` の呼び出し元 `ThreadId` を記録し、**実際に複数スレッドから
/// 呼ばれたこと**を直接検証する（`should_parallelize` を `false` 固定にする
/// ミューテーションで本テストが確実に落ちることを保証する）。
#[test]
fn t_parallel_subagents_get_distinct_session_ids() {
    let tmp = tempfile::NamedTempFile::new().expect("tempfile作成失敗");
    let db_path = tmp.path().to_str().unwrap();
    let store = MemoryStore::open(db_path).expect("file-backed store作成失敗");
    assert!(
        store.path().is_some(),
        "file-backed storeはpathを公開するべき"
    );

    let thread_ids: Arc<Mutex<Vec<std::thread::ThreadId>>> = Arc::new(Mutex::new(Vec::new()));
    let backend = ThreadIdRecordingBackend::new(
        vec!["タスクAの結果".to_string(), "タスクBの結果".to_string()],
        thread_ids.clone(),
    );
    let tools = ToolRegistry::default();
    let path_guard = tmp_path_guard();
    let cancel = CancellationToken::new();
    let sub_config = SubAgentConfig::from_parent(&AgentConfig::default(), 0);

    let executor = SubAgentExecutor::new(
        &backend,
        &tools,
        &path_guard,
        &cancel,
        Some(&store),
        sub_config,
    );

    let result = executor
        .execute(
            "parent-1",
            &[
                "README.mdを読む".to_string(),
                "Cargo.tomlの内容を確認".to_string(),
            ],
        )
        .expect("execute失敗");
    assert_eq!(result.results.len(), 2);

    let es = EventStore::new(store.conn());
    let sessions = es.list_sessions().expect("list_sessions失敗");
    let unique: std::collections::HashSet<&String> = sessions.iter().collect();
    assert_eq!(
        unique.len(),
        sessions.len(),
        "並列サブエージェントのsession_idはすべて相異なるべき: {sessions:?}"
    );
    assert!(
        sessions.len() >= 2,
        "少なくとも2件の相異なるサブエージェントsessionが記録されるべき: {sessions:?}"
    );

    // --- 本強化の核: generate() が実際に複数スレッドから呼ばれたことを検証 ---
    let recorded = thread_ids.lock().unwrap();
    assert_eq!(
        recorded.len(),
        2,
        "2件のサブタスクそれぞれでgenerate()が1回ずつ呼ばれるべき: {recorded:?}"
    );
    let unique_threads: std::collections::HashSet<std::thread::ThreadId> =
        recorded.iter().copied().collect();
    assert_eq!(
        unique_threads.len(),
        2,
        "should_parallelizeがtrueならgenerate()は2つの異なるワーカースレッドから呼ばれるべき\
         （false固定ミューテーションではexecute_sequentialが呼び出し元スレッドから\
         2回呼ぶため、ここが1件に潰れて検出できる）: {recorded:?}"
    );
    let caller_thread = std::thread::current().id();
    assert!(
        !recorded.contains(&caller_thread),
        "並列経路ではgenerate()はテスト自身のスレッドではなくscope.spawnされた\
         ワーカースレッドから呼ばれるべき: caller={caller_thread:?}, recorded={recorded:?}"
    );
}

/// AC-4-4: 親に `confirm_callback` が設定されている場合、`should_parallelize` が
/// 強制的に `false` を返し（Issue #22 AC-3-5）、file-backed store かつ独立goalの
/// 組み合わせでも並列実行（別スレッド）が選ばれないこと。
/// `ThreadId` を捕捉する confirm_callback で検証する。
#[test]
fn t_confirm_callback_forces_sequential_subagents() {
    let tmp = tempfile::NamedTempFile::new().expect("tempfile作成失敗");
    let db_path = tmp.path().to_str().unwrap();
    let store = MemoryStore::open(db_path).expect("file-backed store作成失敗");
    assert!(store.path().is_some());

    let thread_ids: Arc<Mutex<Vec<std::thread::ThreadId>>> = Arc::new(Mutex::new(Vec::new()));
    let thread_ids_clone = thread_ids.clone();
    let confirm_cb: ConfirmCallback = Arc::new(move |_name: &str, _args: &str| {
        thread_ids_clone
            .lock()
            .unwrap()
            .push(std::thread::current().id());
        true
    });

    let parent = AgentConfig {
        confirm_callback: Some(confirm_cb),
        ..Default::default()
    };
    let sub_config = SubAgentConfig::from_parent(&parent, 0);

    let mut tools = ToolRegistry::new();
    tools.register(Box::new(ShellTool::new()));

    let backend = MockLlmBackend::new(vec![
        r#"<tool_call>{"name": "shell", "arguments": {"command": "echo hi"}}</tool_call>"#
            .to_string(),
        r#"<tool_call>{"name": "shell", "arguments": {"command": "echo hi"}}</tool_call>"#
            .to_string(),
    ]);
    let path_guard = tmp_path_guard();
    let cancel = CancellationToken::new();

    let executor = SubAgentExecutor::new(
        &backend,
        &tools,
        &path_guard,
        &cancel,
        Some(&store),
        sub_config,
    );

    let result = executor
        .execute(
            "parent-1",
            &[
                "README.mdを読む".to_string(),
                "Cargo.tomlの内容を確認".to_string(),
            ],
        )
        .expect("execute失敗");
    assert_eq!(result.results.len(), 2);

    let ids = thread_ids.lock().unwrap();
    assert!(
        !ids.is_empty(),
        "confirm_callbackが1度も呼ばれていない。シナリオ不成立"
    );
    let first = ids[0];
    assert!(
        ids.iter().all(|id| *id == first),
        "confirm_callback呼び出しはすべて同一スレッド（順次実行）であるべき: {ids:?}"
    );
}
