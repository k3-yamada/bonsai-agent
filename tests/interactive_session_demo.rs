//! REPL 実対話セッションの動作シミュレーションドライバ
//!
//! ユーザーリクエスト「動作させて貴方が使ってみて」を実証するため、
//! 以下の統合シナリオを対話型 REPL 環境で実行し、一連の UX を検証・出力します。
//!
//! 1. [Turn 1] FastPath 超低レイテンシ応答 ("ping" -> "pong")
//! 2. [Turn 2] 通常対話（bonsai-agentの思想質問） -> エージェント応答
//! 3. [Background] ターン間のアイドル中に DMN 自発思考ループ発火 -> Vault/KG/A-MEM還元 -> inbox蓄積
//! 4. [Turn 3] REPLプロンプト直前での非侵入型インサイト提示 (💡 [DMN]: ...)
//! 5. [Turn 4] "exit" による安全なセッション終了

use std::io::BufReader;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bonsai_agent::agent::agent_loop::{AgentConfig, ReplIo, run_repl};
use bonsai_agent::agent::dmn::generator::DmnGenerator;
use bonsai_agent::agent::dmn::worker::{DmnRunner, DmnWorker};
use bonsai_agent::agent::validate::PathGuard;
use bonsai_agent::cancel::CancellationToken;
use bonsai_agent::domain::conversation::Session;
use bonsai_agent::domain::llm::MockLlmBackend;
use bonsai_agent::memory::experience::{ExperienceStore, ExperienceType, RecordParams};
use bonsai_agent::memory::store::MemoryStore;
use bonsai_agent::tools::ToolRegistry;

/// ユーザーの入力・タイピングの間を再現する遅延リーダー
struct DelayedReader {
    lines: Vec<String>,
    current: usize,
    delay: Duration,
}

impl std::io::Read for DelayedReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.current >= self.lines.len() {
            return Ok(0);
        }
        // 2ターン目以降、ユーザーが入力するまでの「思考・タイピング時間」を再現
        if self.current > 0 {
            std::thread::sleep(self.delay);
        }
        let line = &self.lines[self.current];
        self.current += 1;
        let bytes = line.as_bytes();
        let len = bytes.len().min(buf.len());
        buf[..len].copy_from_slice(&bytes[..len]);
        Ok(len)
    }
}

#[test]
fn test_interactive_session_walkthrough() {
    println!("\n========================================================");
    println!("  bonsai-agent 自律対話セッション実証ドライバ");
    println!("========================================================\n");

    let temp_dir = std::env::temp_dir().join("bonsai_interactive_demo");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).unwrap();

    let db_path_buf = temp_dir.join("test_session.db");
    let db_path = db_path_buf.to_string_lossy().to_string();
    let store = MemoryStore::open(&db_path).expect("Failed to create MemoryStore");

    // 事前記憶（直前の試行での失敗と教訓）の投入（VALUES.md V3: 経験からの自己更新）
    let exp_store = ExperienceStore::new(store.conn());
    let _ = exp_store.record(&RecordParams {
        exp_type: ExperienceType::Failure,
        task_context: "tool_execution_timeout",
        action: "run_external_script",
        outcome: "外部プロセスのタイムアウトにより処理中断",
        lesson: Some("外部プロセス呼び出し時はタイムアウトガードと足場制御を徹底すべき"),
        tool_name: Some("shell"),
        error_type: Some("timeout"),
        error_detail: None,
    });

    // Agent メイン推論用バックエンド
    let agent_backend = Arc::new(MockLlmBackend::new(vec![
        // Turn 2: 思想の質問に対する回答
        "bonsai-agent は 1-bit 量子化モデル（Bonsai-8B）を「Scaffolding > Model」原則で支える自律エージェントです。外部ハーネスと多層記憶が知能を担保します。".to_string(),
        // Turn 3: 安全な代替案
        "ユーザーの意図を尊重し、安全かつ堅牢な実装方針をご案内します。".to_string(),
    ]));

    // DMN 専用バックエンド
    let dmn_backend = Arc::new(MockLlmBackend::single(
        "直前の対話から、モデル能力依存ではなく外部足場の強化方針をナレッジVaultに蓄積すべきと気付きました。",
    ));

    let tools = ToolRegistry::new();
    let path_guard = PathGuard::new(vec![]);
    let config = AgentConfig::default();
    let cancel = CancellationToken::new();
    let is_busy = Arc::new(AtomicBool::new(false));
    let dmn_inbox = Arc::new(Mutex::new(Vec::new()));

    // DMN ワーカーのセットアップ
    let temp_vault = temp_dir.join("vault");
    let _ = std::fs::create_dir_all(&temp_vault);
    let mut worker = DmnWorker::new(5.0, 5.0, 0.5)
        .with_vault(temp_vault.clone())
        .with_auto_persist(true);
    worker.last_tick = std::time::Instant::now() - Duration::from_secs(10);

    // ユーザー入力ストリーム
    let delayed_reader = DelayedReader {
        lines: vec![
            "ping\n".to_string(),
            "bonsai-agentの設計思想について教えてください\n".to_string(),
            "/help\n".to_string(),
            "/dmn\n".to_string(),
            "/dream\n".to_string(),
            "/graph /tmp/test_session_graph.html\n".to_string(),
            "安全な代替案をお願いします\n".to_string(),
            "exit\n".to_string(),
        ],
        current: 0,
        delay: Duration::from_millis(200),
    };
    let mut reader = BufReader::new(delayed_reader);
    let mut writer = Vec::new();

    let dmn_inbox_for_worker = dmn_inbox.clone();

    // DMN Runner を起動（高速ループ設定）
    let mut dmn_runner = DmnRunner::spawn(
        worker,
        is_busy.clone(),
        cancel.clone(),
        Some(db_path.clone()),
        move |s| DmnGenerator::generate_reflection_with_llm(s, Some(&*dmn_backend)),
        move |msg| {
            if let Ok(mut inbox) = dmn_inbox_for_worker.lock() {
                inbox.push(msg);
            }
        },
    );

    let repl_io = ReplIo::new(&*agent_backend, &tools, &path_guard, &config, &cancel)
        .with_store(Some(&store))
        .with_is_busy(Some(is_busy.clone()))
        .with_dmn_inbox(Some(dmn_inbox.clone()))
        .with_dmn_notify(true);

    let mut session = Session::new();

    // REPL 実行
    let res = run_repl(&mut reader, &mut writer, &mut session, &repl_io);
    assert!(res.is_ok(), "REPL execution should succeed");

    // DMN を安全に停止
    dmn_runner.stop();

    let output_str = String::from_utf8_lossy(&writer);
    println!("--- REPL 端末出力ログ ---");
    println!("{}", output_str);
    println!("-------------------------\n");

    // 検証アサーション
    assert!(
        output_str.contains("pong"),
        "Turn 1 FastPath 'pong' が出力されていること"
    );
    assert!(
        output_str.contains("[DMN]"),
        "DMN からの自発的インサイト通知が含まれていること"
    );
    assert!(
        output_str.contains("利用可能な内省・知能コマンド"),
        "/help コマンドの出力が含まれていること"
    );
    assert!(
        output_str.contains("[DMN 直近内省ログ]"),
        "/dmn コマンドの出力が含まれていること"
    );
    assert!(
        output_str.contains("Deep Dreaming メタ認知レポート"),
        "/dream コマンドの出力が含まれていること"
    );
    assert!(
        output_str.contains("力学グラフビューアを出力しました"),
        "/graph コマンドの出力が含まれていること"
    );
    assert!(
        std::path::Path::new("/tmp/test_session_graph.html").exists(),
        "/graph で HTML ファイルが出力されていること"
    );
    let _ = std::fs::remove_file("/tmp/test_session_graph.html");

    // 1. Vault への自動還元検証
    let insight_vault_file = temp_dir.join("vault").join("insights.md");
    assert!(
        insight_vault_file.exists(),
        "ナレッジVaultにinsights.mdが自動生成されていること"
    );
    let vault_content = std::fs::read_to_string(&insight_vault_file).unwrap();
    assert!(
        vault_content.contains("直前の対話から"),
        "VaultにDMN内省が還元されていること"
    );

    // 2. A-MEM への自動還元検証
    let all_mems = store.all_memories().unwrap();
    assert!(
        all_mems
            .iter()
            .any(|m| m.content.contains("直前の対話から")),
        "A-MEMにDMN内省が保存されていること"
    );

    // 一時ディレクトリの清掃
    let _ = std::fs::remove_dir_all(&temp_dir);
    println!("✅ 自律対話セッション実証ドライバ: 全ステップ検証完了\n");
}
