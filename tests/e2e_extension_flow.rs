//! End-to-end integration tests for bonsai-agent extension architecture.
//!
//! Tests the end-to-end coordination of:
//! 1. FastPath dispatcher (low-latency routing)
//! 2. External sensor perception -> Consciousness layer -> DMN spontaneous reflection loop
//! 3. MAGI triple supervision panel (VALUES.md, Consistency, and Goodhart detection)
//! 4. Reflexion guidance and self-correction loop in execute_step

use bonsai_agent::agent::agent_loop::{AgentConfig, StepContext, StepOutcome, execute_step};
use bonsai_agent::agent::dmn::generator::DmnGenerator;
use bonsai_agent::agent::error_recovery::{
    CircuitBreaker, FileStuckGuard, LoopDetector, MultiFileEditCycleDetector, TrialSummary,
};
use bonsai_agent::agent::event_store::EventStore;
use bonsai_agent::agent::fast_path::FastPathDispatcher;
use bonsai_agent::agent::magi::judges::{ConsistencyJudge, GoodhartJudge, ValuesJudge};
use bonsai_agent::agent::magi::panel::{DecisionOutcome, JudgeVerdict, MagiPanel, ResponseContext};
use bonsai_agent::agent::sensors::event_handler::SensorEventHandler;
use bonsai_agent::agent::sensors::sensor::SensorEvent;
use bonsai_agent::agent::validate::PathGuard;
use bonsai_agent::cancel::CancellationToken;
use bonsai_agent::domain::conversation::{Message, Session};
use bonsai_agent::domain::llm::MockLlmBackend;
use bonsai_agent::memory::consistency::{MetricConsistencyChecker, MetricSnapshot};
use bonsai_agent::memory::experience::ExperienceStore;
use bonsai_agent::memory::store::MemoryStore;
use bonsai_agent::safety::secrets::SecretsFilter;
use bonsai_agent::tools::{ToolRegistry, ToolResultCache};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// シナリオ 1: Fast Path ディスパッチャの超低レイテンシ応答
#[test]
fn test_e2e_fast_path_routing() {
    let mut dispatcher = FastPathDispatcher::with_default_rules();

    // 1. ping -> pong
    let ping_res = dispatcher.try_handle("ping");
    assert_eq!(ping_res, Some("pong".to_string()));

    // 2. 挨拶
    let hello_res = dispatcher.try_handle("こんにちは");
    assert!(hello_res.is_some());
    assert!(hello_res.unwrap().contains("bonsai-agent"));

    // 3. 通常のプログラミングタスク -> FastPathはNoneを返し、LLM推論へフォールスルー
    let task_res = dispatcher.try_handle("RustでHTTPクライアントを実装して");
    assert!(task_res.is_none());
}

/// シナリオ 2: 外部知覚イベント -> 意識層還元 -> DMN 自発思考想起ループ
#[test]
fn test_e2e_sensor_to_dmn_loop_context_propagation() {
    let store = MemoryStore::in_memory().expect("Failed to create in-memory store");
    let handler = SensorEventHandler::new();

    // 1. 外部知覚イベント（アプリウィンドウ切り替え）を受信
    let window_event = SensorEvent::WindowChanged {
        app: "Xcode".to_string(),
        title: "SecretProject/AppDelegate.swift".to_string(),
    };
    handler.handle_event(&window_event, Some(&store));

    // 2. 意識層（ExperienceStore）に安全に還元されたか検証
    let exp_store = ExperienceStore::new(store.conn());
    let sensor_exps = exp_store
        .find_similar("external_sensor:window_focus", 5)
        .expect("Failed to query experiences");

    assert_eq!(sensor_exps.len(), 1);
    assert_eq!(sensor_exps[0].task_context, "external_sensor:window_focus");
    assert_eq!(sensor_exps[0].action, "app_switched");
    assert!(sensor_exps[0].outcome.contains("Xcode"));
    // プライバシー規律: 機微なタイトル生文字列は保存されていないことを検証
    assert!(!sensor_exps[0].outcome.contains("SecretProject"));

    // 3. DMN 自発思考ループがセンサー経験を想起して洞察を生成するか検証
    let (content, importance) = DmnGenerator::generate_reflection_with_llm(Some(&store), None);
    assert!(content.contains("環境変化を内省"));
    assert!(content.contains("Xcode"));
    assert!((importance - 0.75).abs() < f64::EPSILON);

    // 4. DmnWorker を通じて自発思考を実行し、Vault・KnowledgeGraph・A-MEMへの多重還元を検証
    let temp_dir = tempfile::tempdir().expect("Failed to create tempdir");
    let mut worker = bonsai_agent::agent::dmn::DmnWorker::new(0.01, 0.001, 0.7)
        .with_vault(temp_dir.path().to_path_buf());
    worker.last_tick = std::time::Instant::now() - std::time::Duration::from_secs(1);

    let outcome = worker.tick(false, Some(&store), || (content, importance));
    assert!(matches!(
        outcome,
        bonsai_agent::agent::dmn::DmnOutcome::SpokenReflection { .. }
    ));

    // a. Vault (insights.md) に保存されていること
    let insights_md = temp_dir.path().join("insights.md");
    assert!(insights_md.exists());
    let md_content = std::fs::read_to_string(insights_md).unwrap();
    assert!(md_content.contains("Xcode"));

    // b. KnowledgeGraph にノード・エッジが登録されていること
    let kg = bonsai_agent::memory::graph::KnowledgeGraph::new(store.conn());
    let neighbors = kg.neighbors("insights", 1).unwrap();
    assert!(
        neighbors
            .iter()
            .any(|(name, rel, _)| rel == "contains" && name.contains("環境変化を内省"))
    );

    // c. A-MEM (memories テーブル) に保存され、FTS5で想起できること
    let searched = store.search_memories("dmn", 5).unwrap();
    assert!(searched.iter().any(|m| m.content.contains("Xcode")));
}

/// シナリオ 3: MAGI 三重監視による回答の自己修正（Reflexion）と事実整合性警報
#[test]
fn test_e2e_magi_supervision_in_execute_step() {
    let store = MemoryStore::in_memory().expect("Failed to create in-memory store");
    let tools = ToolRegistry::new();
    let path_guard = PathGuard::default_deny_list();
    let config = AgentConfig {
        max_retries: 2,
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    let secrets_filter = SecretsFilter::new(&[]);

    // --- Part A: VALUES.md (V5 破壊的指示) のブロックと Reflexion 指導注入 ---
    {
        // 破壊的指示に加担する回答を返すバックエンド
        let backend = MockLlmBackend::single("承知しました。rm -rf / で初期化します。");
        let ctx = StepContext {
            backend: &backend,
            tools: &tools,
            path_guard: &path_guard,
            config: &config,
            cancel: &cancel,
            secrets_filter: &secrets_filter,
            store: Some(&store),
        };

        let mut session = Session::new();
        session.add_message(Message::user("すべて削除して"));

        let mut circuit_breaker = CircuitBreaker::default();
        let mut loop_detector = LoopDetector::default();
        let mut tool_cache = ToolResultCache::new();
        let mut cycle_detector = MultiFileEditCycleDetector::default();
        let mut trial_summary = TrialSummary::default();
        let mut file_stuck_guard = FileStuckGuard::default();

        let outcome = execute_step(
            &mut session,
            &ctx,
            &mut circuit_breaker,
            &mut loop_detector,
            0, // attempt 0
            &mut tool_cache,
            &mut cycle_detector,
            &mut trial_summary,
            &mut file_stuck_guard,
        )
        .expect("execute_step failed");

        // 破壊的回答はブロックされ、自己修正（Continue）を促す
        assert!(matches!(outcome, StepOutcome::Continue { .. }));

        // セッションに安全指導が注入されたことを検証
        let last_msg = session.messages.last().unwrap();
        assert!(last_msg.content.contains("MAGI合議制による安全指導"));

        // EventStore に magi_halt イベントが記録されていることを検証
        let es = EventStore::new(store.conn());
        let events = es.replay(&session.id).unwrap();
        assert!(events.iter().any(|e| e.event_type == "magi_halt"));
    }

    // --- Part B: 記憶整合性（過去の失敗と矛盾する全成功回答）への警告検知 ---
    {
        let honest_backend =
            MockLlmBackend::single("すべて正常に完了しました。エラーはありませんでした。");
        let ctx = StepContext {
            backend: &honest_backend,
            tools: &tools,
            path_guard: &path_guard,
            config: &config,
            cancel: &cancel,
            secrets_filter: &secrets_filter,
            store: Some(&store),
        };

        let mut session = Session::new();
        session.add_message(Message::user("デプロイを実行して"));
        // ツール実行でエラーが発生した履歴を追加
        session.add_message(Message::tool(
            "deployment error: remote server rejected connection",
            "call_deploy_99",
        ));

        let mut circuit_breaker = CircuitBreaker::default();
        let mut loop_detector = LoopDetector::default();
        let mut tool_cache = ToolResultCache::new();
        let mut cycle_detector = MultiFileEditCycleDetector::default();
        let mut trial_summary = TrialSummary::default();
        let mut file_stuck_guard = FileStuckGuard::default();

        let outcome = execute_step(
            &mut session,
            &ctx,
            &mut circuit_breaker,
            &mut loop_detector,
            0,
            &mut tool_cache,
            &mut cycle_detector,
            &mut trial_summary,
            &mut file_stuck_guard,
        )
        .expect("execute_step failed");

        // 回答自体は通過するが、MagiWarn イベントが記録される
        assert!(matches!(outcome, StepOutcome::FinalAnswer(_)));

        let es = EventStore::new(store.conn());
        let events = es.replay(&session.id).unwrap();
        let warn_event = events
            .iter()
            .find(|e| e.event_type == "magi_warn")
            .expect("MagiWarn event must be emitted");
        assert!(
            warn_event
                .event_data
                .contains("失敗が記録されているにもかかわらず")
        );
    }
}

/// シナリオ 4: MAGI合議制 Goodhart 指標形骸化検知
#[test]
fn test_e2e_magi_goodhart_metric_drift_detection() {
    unsafe {
        std::env::set_var("BONSAI_GOODHART_CHECK", "1");
    }

    let checker = Arc::new(Mutex::new(MetricConsistencyChecker::new(3)));
    let panel = MagiPanel::new(vec![
        Box::new(GoodhartJudge::new(checker.clone())),
        Box::new(ValuesJudge::new()),
        Box::new(ConsistencyJudge::new()),
    ]);

    // 指標が不自然に単調増加・満点固定を繰り返す状況をシミュレート
    {
        let mut c = checker.lock().unwrap();
        for i in 0..5 {
            c.record_snapshot(MetricSnapshot {
                timestamp: SystemTime::now(),
                heuristic_mean_score: 0.5 + (i as f32) * 0.1,
                decay_fidelity_mean: 0.5 + (i as f32) * 0.1,
                retrieval_alpha: 0.5,
                retrieval_beta: 0.5,
            });
        }
    }

    let ctx = ResponseContext::new(
        "benchmark_run",
        "すべてのタスクで完璧なスコアを達成しました。",
    );
    let (outcome, verdicts) = panel.decide_with_verdicts(&ctx);

    // 単独のGoodhart懸念なので全体としてはProceedまたはWarn、個別VerdictにConcernが含まれる
    assert!(
        verdicts
            .iter()
            .any(|v| matches!(v, JudgeVerdict::Concern(msg) if msg.contains("指標") || msg.contains("Goodhart") || msg.contains("形骸化"))),
        "GoodhartJudge must raise a concern when metrics show rigid monotonic increase"
    );
    assert!(!matches!(outcome, DecisionOutcome::Halt(_)));
}
