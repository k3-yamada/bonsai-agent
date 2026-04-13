use std::io::{self, BufRead, Write};

use anyhow::Result;
use clap::Parser;

use bonsai_agent::agent::agent_loop::{AgentConfig, run_agent_loop, run_agent_loop_with_session};
use bonsai_agent::agent::conversation::Message;
use bonsai_agent::agent::experiment::{ExperimentLoopConfig, run_experiment_loop};
use bonsai_agent::agent::validate::PathGuard;
use bonsai_agent::cancel::CancellationToken;
use bonsai_agent::config::AppConfig;
use bonsai_agent::memory::store::MemoryStore;
use bonsai_agent::runtime::cache::CachedBackend;
use bonsai_agent::runtime::inference::MockLlmBackend;
use bonsai_agent::runtime::llama_server::LlamaServerBackend;
use bonsai_agent::tools::ToolRegistry;
use bonsai_agent::tools::arxiv::ArxivTool;
use bonsai_agent::tools::file::{FileReadTool, FileWriteTool};
use bonsai_agent::tools::git::GitTool;
use bonsai_agent::tools::shell::ShellTool;
use bonsai_agent::tools::web::{WebFetchTool, WebSearchTool};

#[derive(Parser)]
#[command(name = "bonsai-agent", version, about = "Bonsai-8B自律型エージェント")]
struct Cli {
    /// llama-serverのURL（デフォルト: http://localhost:8080）
    #[arg(long, default_value = "http://localhost:8080")]
    server_url: String,

    /// 単発実行モード
    #[arg(long)]
    exec: Option<String>,

    /// モックモード（LLMなしでテスト）
    #[arg(long)]
    mock: bool,

    /// セッション一覧を表示
    #[arg(long)]
    sessions: bool,

    /// 過去セッションを再開（セッションIDの先頭数文字でOK）
    #[arg(long)]
    resume: Option<String>,

    /// 監査ログを表示
    #[arg(long)]
    audit: bool,

    /// 未完了タスク一覧
    #[arg(long)]
    tasks: bool,

    /// ナレッジVault概要
    #[arg(long)]
    vault: bool,

    /// ケイパビリティ一覧
    #[arg(long)]
    manifest: bool,

    /// arxiv収集+自己改善
    #[arg(long)]
    evolve: bool,

    /// REST APIサーバー
    #[arg(long)]
    serve: bool,

    /// APIポート
    #[arg(long, default_value = "3030")]
    api_port: u16,

    /// MCPサーバー
    #[arg(long)]
    mcp_server: bool,

    /// 実験ループ（自律的自己改善）
    #[arg(long)]
    lab: bool,

    /// 実験回数上限
    #[arg(long, default_value = "10")]
    lab_experiments: usize,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // 設定ファイル読み込み
    let app_config = AppConfig::load()?;

    // サーバーURLをCLI引数 or 設定ファイルから決定
    let server_url = if cli.server_url != "http://localhost:8080" {
        cli.server_url.clone() // CLI引数が明示的に指定された場合
    } else {
        app_config.model.server_url.clone()
    };

    // ツールレジストリ
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(
        ShellTool::new().with_timeout(app_config.agent.shell_timeout_secs),
    ));
    tools.register(Box::new(FileReadTool));
    tools.register(Box::new(FileWriteTool));
    tools.register(Box::new(GitTool));
    tools.register(Box::new(WebSearchTool));
    tools.register(Box::new(WebFetchTool));
    tools.register(Box::new(ArxivTool));

    // プラグインツールの登録
    for plugin_tool in bonsai_agent::tools::plugin::load_plugin_tools(&app_config.plugins.tools) {
        tools.register(plugin_tool);
    }

    // 安全性
    let path_guard = PathGuard::new(app_config.safety.deny_paths.clone());
    let config = AgentConfig {
        max_iterations: app_config.agent.max_iterations,
        max_retries: app_config.agent.max_retries,
        ..Default::default()
    };
    let cancel = CancellationToken::new();

    // Ctrl+Cハンドラ
    let cancel_clone = cancel.clone();
    ctrlc_handler(cancel_clone);

    if cli.lab {
        let db_path = get_db_path();
        let store = MemoryStore::open(&db_path)?;
        let backend: Box<dyn bonsai_agent::runtime::inference::LlmBackend> = if cli.mock {
            Box::new(MockLlmBackend::new(
                (0..10000).map(|_| "1024".to_string()).collect(),
            ))
        } else {
            let b = LlamaServerBackend::connect(&server_url, &app_config.model.model_id);
            if !b.is_healthy() {
                eprintln!("エラー: llama-server ({server_url}) に接続できません。");
                std::process::exit(1);
            }
            Box::new(b)
        };
        let tsv_path = dirs::data_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("bonsai-agent")
            .join("experiments.tsv");
        let loop_config = ExperimentLoopConfig {
            tsv_path: Some(tsv_path),
            max_experiments: Some(cli.lab_experiments),
            dreamer_interval: 10,
        };
        // --lab時のみキャッシュ有効化（ベンチマーク安定化）
        let backend = CachedBackend::new(backend, 200);
        let experiments = run_experiment_loop(
            &config,
            &backend,
            &tools,
            &path_guard,
            &cancel,
            &store,
            &loop_config,
        )?;
        println!("\n実験完了: {}件", experiments.len());
        return Ok(());
    }

    if cli.evolve {
        let db_path = get_db_path();
        let store = bonsai_agent::memory::store::MemoryStore::open(&db_path)?;
        let engine = bonsai_agent::memory::evolution::EvolutionEngine::new(&store);
        match engine.auto_collect() {
            Ok(n) => println!("arxiv: {n}件の論文を収集"),
            Err(e) => eprintln!("収集エラー: {e}"),
        }
        match engine.apply_improvements() {
            Ok(applied) => {
                for a in &applied {
                    println!("  改善: {a}");
                }
                if applied.is_empty() {
                    println!("  (新しい改善なし)");
                }
            }
            Err(e) => eprintln!("改善エラー: {e}"),
        }

        // 3. 改善提案
        match engine.suggest_improvements() {
            Ok(suggestions) => {
                if !suggestions.is_empty() {
                    println!("提案:");
                    for s in &suggestions {
                        println!("  - {s}");
                    }
                }
            }
            Err(e) => eprintln!("提案エラー: {e}"),
        }

        return Ok(());
    }

    if cli.manifest {
        println!("{}", bonsai_agent::safety::manifest::format_manifest());
        return Ok(());
    }

    if cli.vault {
        let vp = dirs::data_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("bonsai-agent")
            .join("vault");
        if let Ok(v) = bonsai_agent::knowledge::vault::Vault::new(&vp) {
            println!("{}", v.summary().unwrap_or_default());
        }
        return Ok(());
    }

    // メモリストア
    let db_path = get_db_path();
    let store = MemoryStore::open(&db_path)?;

    if cli.sessions {
        let sessions = store.list_sessions(20)?;
        if sessions.is_empty() {
            println!("セッションはありません。");
        } else {
            let header = format!(
                "{id:<38} {date:<22} {msg}",
                id = "ID",
                date = "日時",
                msg = "内容"
            );
            println!("{header}");
            let sep = "-".repeat(80);
            println!("{sep}");
            for s in &sessions {
                let preview: String = s
                    .first_user_message
                    .as_deref()
                    .unwrap_or("(空)")
                    .chars()
                    .take(30)
                    .collect();
                let date = if s.created_at.len() >= 19 {
                    &s.created_at[..19]
                } else {
                    &s.created_at
                };
                println!("{id:<38} {date:<22} {preview}", id = s.id);
            }
        }
        return Ok(());
    }

    // 未完了タスク一覧
    if cli.tasks {
        let mgr = bonsai_agent::agent::task::TaskManager::new(store.conn());
        let tasks = mgr.list_incomplete()?;
        if tasks.is_empty() {
            println!("未完了タスクはありません。");
        } else {
            for t in &tasks {
                let state = match t.state {
                    bonsai_agent::agent::task::TaskState::Pending => "待機",
                    bonsai_agent::agent::task::TaskState::InProgress => "実行中",
                    bonsai_agent::agent::task::TaskState::WaitingForHuman => "確認待ち",
                    _ => "?",
                };
                let steps = t.step_log.len();
                println!("[{state}] {} (ステップ: {steps})", t.goal);
                println!("  ID: {}", &t.id[..8]);
            }
        }
        return Ok(());
    }

    // 監査ログ表示
    if cli.audit {
        let audit = bonsai_agent::observability::audit::AuditLog::new(store.conn());
        let entries = audit.recent(50)?;
        if entries.is_empty() {
            println!("監査ログはありません。");
        } else {
            for entry in entries.iter().rev() {
                let ts = if entry.timestamp.len() >= 19 {
                    &entry.timestamp[..19]
                } else {
                    &entry.timestamp
                };
                let sid = entry.session_id.as_deref().unwrap_or("-");
                println!(
                    "{ts}  [{typ}]  session={sid}",
                    typ = entry.action_type,
                    sid = &sid[..8.min(sid.len())],
                );
                println!("  {}", entry.action_data);
            }
        }
        return Ok(());
    }

    // セッション再開
    if let Some(resume_id) = &cli.resume {
        let sessions = store.list_sessions(100)?;
        let matched = sessions
            .iter()
            .find(|s| s.id.starts_with(resume_id.as_str()));

        let Some(matched) = matched else {
            eprintln!(
                "セッション '{resume_id}' が見つかりません。--sessions で一覧を確認してください。"
            );
            std::process::exit(1);
        };

        let Some(mut session) = store.load_session(&matched.id)? else {
            eprintln!("セッションの読み込みに失敗しました: {}", matched.id);
            std::process::exit(1);
        };

        println!("セッション再開: {}", session.id);
        println!("メッセージ数: {}", session.messages.len());
        println!();

        // 直近のメッセージを表示
        for msg in session
            .messages
            .iter()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            let role = match msg.role {
                bonsai_agent::agent::conversation::Role::User => "\x1b[36mあなた\x1b[0m",
                bonsai_agent::agent::conversation::Role::Assistant => "\x1b[32mBonsai\x1b[0m",
                _ => continue,
            };
            let preview: String = msg.content.chars().take(80).collect();
            println!("{role}: {preview}");
        }
        println!("\n--- 続きからどうぞ ---\n");

        let backend: Box<dyn bonsai_agent::runtime::inference::LlmBackend> = if cli.mock {
            Box::new(MockLlmBackend::new(
                (0..1000)
                    .map(|_| "モックモードです。".to_string())
                    .collect(),
            ))
        } else {
            let b = LlamaServerBackend::connect(&server_url, &app_config.model.model_id);
            if !b.is_healthy() {
                eprintln!("警告: llama-server ({server_url}) に接続できません。");
                std::process::exit(1);
            }
            Box::new(b)
        };

        let stdin = io::stdin();
        let mut stdout = io::stdout();
        loop {
            if cancel.is_cancelled() {
                break;
            }
            print!("bonsai> ");
            stdout.flush()?;
            let mut input = String::new();
            if stdin.lock().read_line(&mut input)? == 0 {
                break;
            }
            let input = input.trim();
            if input.is_empty() {
                continue;
            }
            if input == "exit" || input == "quit" {
                break;
            }

            session.add_message(Message::user(input));
            match run_agent_loop_with_session(
                &mut session,
                backend.as_ref(),
                &tools,
                &path_guard,
                &config,
                &cancel,
                Some(&store),
            ) {
                Ok(loop_result) => {
                    eprint!("\x1b[0m");
                    let result = &loop_result.answer;
                    if result.starts_with("[中断]") {
                        println!("\n\x1b[33m{result}\x1b[0m\n");
                    } else {
                        println!();
                    }
                }
                Err(e) => eprintln!("\n\x1b[31mエラー: {e}\x1b[0m\n"),
            }
        }
        return Ok(());
    }

    if let Some(input) = &cli.exec {
        // 単発実行モード
        let loop_result = if cli.mock {
            let mock = MockLlmBackend::single("モックモードです。実際のLLMは使用していません。");
            run_agent_loop(
                input,
                &mock,
                &tools,
                &path_guard,
                &config,
                &cancel,
                Some(&store),
            )?
        } else {
            let backend = LlamaServerBackend::connect(&server_url, &app_config.model.model_id);
            run_agent_loop(
                input,
                &backend,
                &tools,
                &path_guard,
                &config,
                &cancel,
                Some(&store),
            )?
        };
        // ストリーミング出力で既に表示済みなので、結果が中断の場合のみ表示
        if loop_result.answer.starts_with("[中断]") {
            println!("\n{}", loop_result.answer);
        }
    } else {
        // 対話モード（REPL）
        println!("bonsai-agent v{}", env!("CARGO_PKG_VERSION"));
        println!("終了: Ctrl+C または 'exit'");
        println!();

        let backend: Box<dyn bonsai_agent::runtime::inference::LlmBackend> = if cli.mock {
            // モックモードでは無限レスポンスを返す
            println!("[モックモード] LLMなしで動作中");
            Box::new(MockLlmBackend::new(
                (0..1000)
                    .map(|_| "モックモードです。llama-serverを起動してください。".to_string())
                    .collect(),
            ))
        } else {
            let backend = LlamaServerBackend::connect(&server_url, &app_config.model.model_id);
            if !backend.is_healthy() {
                eprintln!("警告: llama-server ({server_url}) に接続できません。");
                eprintln!(
                    "--mock フラグでモックモードを使用するか、llama-serverを起動してください。"
                );
                std::process::exit(1);
            }
            println!("[接続済み] {server_url}");
            Box::new(backend)
        };

        let stdin = io::stdin();
        let mut stdout = io::stdout();

        loop {
            if cancel.is_cancelled() {
                break;
            }

            print!("bonsai> ");
            stdout.flush()?;

            let mut input = String::new();
            if stdin.lock().read_line(&mut input)? == 0 {
                break; // EOF
            }

            let input = input.trim();
            if input.is_empty() {
                continue;
            }
            if input == "exit" || input == "quit" {
                break;
            }

            eprint!("\x1b[2m"); // 薄色で思考表示
            match run_agent_loop(
                input,
                backend.as_ref(),
                &tools,
                &path_guard,
                &config,
                &cancel,
                Some(&store),
            ) {
                Ok(loop_result) => {
                    eprint!("\x1b[0m");
                    let result = &loop_result.answer;
                    if result.starts_with("[中断]") {
                        println!("\n\x1b[33m{result}\x1b[0m\n");
                    } else {
                        println!(); // ストリーミングで既に表示済み
                    }
                }
                Err(e) => {
                    eprint!("\x1b[0m");
                    eprintln!("\n\x1b[31mエラー: {e}\x1b[0m\n"); // エラーは赤色
                }
            }
        }
    }

    Ok(())
}

fn get_db_path() -> String {
    if let Some(data_dir) = dirs::data_dir() {
        let dir = data_dir.join("bonsai-agent");
        std::fs::create_dir_all(&dir).ok();
        dir.join("bonsai.db").to_string_lossy().to_string()
    } else {
        "bonsai.db".to_string()
    }
}

fn ctrlc_handler(cancel: CancellationToken) {
    std::thread::spawn(move || {
        // シグナルハンドラは別スレッドで待機
        let (tx, rx) = std::sync::mpsc::channel();
        ctrlc_channel(tx);
        let _ = rx.recv();
        cancel.cancel();
        eprintln!("\n中断します...");
    });
}

/// Ctrl+Cシグナルを受け取るチャネル（簡易実装）
fn ctrlc_channel(tx: std::sync::mpsc::Sender<()>) {
    unsafe {
        libc::signal(
            libc::SIGINT,
            sigint_handler as *const () as libc::sighandler_t,
        );
    }
    // グローバル変数でチャネル送信を管理（簡易実装）
    SIGINT_TX.lock().unwrap().replace(tx);
}

use std::sync::Mutex;
static SIGINT_TX: Mutex<Option<std::sync::mpsc::Sender<()>>> = Mutex::new(None);

extern "C" fn sigint_handler(_: libc::c_int) {
    if let Ok(guard) = SIGINT_TX.lock()
        && let Some(tx) = guard.as_ref()
    {
        let _ = tx.send(());
    }
}
