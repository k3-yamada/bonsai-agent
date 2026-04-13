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
use bonsai_agent::runtime::inference::{LlmBackend, MockLlmBackend};
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

/// 共有コンテキスト（各モードハンドラに渡す）
struct AppContext {
    tools: ToolRegistry,
    path_guard: PathGuard,
    config: AgentConfig,
    cancel: CancellationToken,
    server_url: String,
    app_config: AppConfig,
    mock: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let app_config = AppConfig::load()?;
    let server_url = if cli.server_url != "http://localhost:8080" {
        cli.server_url.clone()
    } else {
        app_config.model.server_url.clone()
    };

    let mut tools = setup_tools(&app_config);
    // プラグインツールの登録
    for plugin_tool in bonsai_agent::tools::plugin::load_plugin_tools(&app_config.plugins.tools) {
        tools.register(plugin_tool);
    }

    let cancel = CancellationToken::new();
    ctrlc_handler(cancel.clone());

    let ctx = AppContext {
        tools,
        path_guard: PathGuard::new(app_config.safety.deny_paths.clone()),
        config: AgentConfig {
            max_iterations: app_config.agent.max_iterations,
            max_retries: app_config.agent.max_retries,
            max_tools_selected: app_config.agent.max_tools_selected,
            ..Default::default()
        },
        cancel,
        server_url,
        mock: cli.mock,
        app_config,
    };

    // 早期リターンモード（DB不要）
    if cli.lab {
        return handle_lab_mode(&ctx, cli.lab_experiments);
    }
    if cli.evolve {
        return handle_evolve_mode();
    }
    if cli.manifest {
        println!("{}", bonsai_agent::safety::manifest::format_manifest());
        return Ok(());
    }
    if cli.vault {
        return handle_vault_mode();
    }

    // DB必要モード
    let store = MemoryStore::open(&get_db_path())?;

    if cli.sessions {
        return handle_sessions_mode(&store);
    }
    if cli.tasks {
        return handle_tasks_mode(&store);
    }
    if cli.audit {
        return handle_audit_mode(&store);
    }
    if let Some(resume_id) = &cli.resume {
        return handle_resume_mode(&ctx, &store, resume_id);
    }
    if let Some(input) = &cli.exec {
        return handle_exec_mode(&ctx, &store, input);
    }

    handle_repl_mode(&ctx, &store)
}

// --- ツール初期化 ---

fn setup_tools(app_config: &AppConfig) -> ToolRegistry {
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
    tools
}

/// バックエンド生成（モック/実機の分岐を統合）
fn create_backend(ctx: &AppContext) -> Box<dyn LlmBackend> {
    if ctx.mock {
        Box::new(MockLlmBackend::new(
            (0..10000)
                .map(|_| "モックモードです。".to_string())
                .collect(),
        ))
    } else {
        let b = LlamaServerBackend::connect(&ctx.server_url, &ctx.app_config.model.model_id);
        if !b.is_healthy() {
            eprintln!(
                "エラー: llama-server ({}) に接続できません。",
                ctx.server_url
            );
            eprintln!("--mock フラグでモックモードを使用するか、llama-serverを起動してください。");
            std::process::exit(1);
        }
        Box::new(b)
    }
}

// --- モードハンドラ ---

fn handle_lab_mode(ctx: &AppContext, max_experiments: usize) -> Result<()> {
    let store = MemoryStore::open(&get_db_path())?;
    let backend: Box<dyn LlmBackend> = if ctx.mock {
        Box::new(MockLlmBackend::new(
            (0..10000).map(|_| "1024".to_string()).collect(),
        ))
    } else {
        let b = LlamaServerBackend::connect(&ctx.server_url, &ctx.app_config.model.model_id);
        if !b.is_healthy() {
            eprintln!(
                "エラー: llama-server ({}) に接続できません。",
                ctx.server_url
            );
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
        max_experiments: Some(max_experiments),
        dreamer_interval: 10,
    };
    let backend = CachedBackend::new(backend, 200);
    let experiments = run_experiment_loop(
        &ctx.config,
        &backend,
        &ctx.tools,
        &ctx.path_guard,
        &ctx.cancel,
        &store,
        &loop_config,
    )?;
    println!("\n実験完了: {}件", experiments.len());
    Ok(())
}

fn handle_evolve_mode() -> Result<()> {
    let store = MemoryStore::open(&get_db_path())?;
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
    Ok(())
}

fn handle_vault_mode() -> Result<()> {
    let vp = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("bonsai-agent")
        .join("vault");
    if let Ok(v) = bonsai_agent::knowledge::vault::Vault::new(&vp) {
        println!("{}", v.summary().unwrap_or_default());
    }
    Ok(())
}

fn handle_sessions_mode(store: &MemoryStore) -> Result<()> {
    let sessions = store.list_sessions(20)?;
    if sessions.is_empty() {
        println!("セッションはありません。");
    } else {
        println!(
            "{id:<38} {date:<22} {msg}",
            id = "ID",
            date = "日時",
            msg = "内容"
        );
        println!("{}", "-".repeat(80));
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
    Ok(())
}

fn handle_tasks_mode(store: &MemoryStore) -> Result<()> {
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
            println!("[{state}] {} (ステップ: {})", t.goal, t.step_log.len());
            println!("  ID: {}", &t.id[..8]);
        }
    }
    Ok(())
}

fn handle_audit_mode(store: &MemoryStore) -> Result<()> {
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
    Ok(())
}

fn handle_resume_mode(ctx: &AppContext, store: &MemoryStore, resume_id: &str) -> Result<()> {
    let sessions = store.list_sessions(100)?;
    let matched = sessions.iter().find(|s| s.id.starts_with(resume_id));

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

    let backend = create_backend(ctx);
    run_repl_loop(ctx, store, &*backend, Some(&mut session))
}

fn handle_exec_mode(ctx: &AppContext, store: &MemoryStore, input: &str) -> Result<()> {
    let backend = create_backend(ctx);
    let loop_result = run_agent_loop(
        input,
        &*backend,
        &ctx.tools,
        &ctx.path_guard,
        &ctx.config,
        &ctx.cancel,
        Some(store),
    )?;
    if loop_result.answer.starts_with("[中断]") {
        println!("\n{}", loop_result.answer);
    }
    Ok(())
}

fn handle_repl_mode(ctx: &AppContext, store: &MemoryStore) -> Result<()> {
    println!("bonsai-agent v{}", env!("CARGO_PKG_VERSION"));
    println!("終了: Ctrl+C または 'exit'");
    println!();

    if ctx.mock {
        println!("[モックモード] LLMなしで動作中");
    } else {
        println!("[接続済み] {}", ctx.server_url);
    }

    let backend = create_backend(ctx);
    run_repl_loop(ctx, store, &*backend, None)
}

/// REPLループ（REPL/resume共通）
fn run_repl_loop(
    ctx: &AppContext,
    store: &MemoryStore,
    backend: &dyn LlmBackend,
    session: Option<&mut bonsai_agent::agent::conversation::Session>,
) -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    // セッション再開モードの場合はrun_agent_loop_with_sessionを使う
    if let Some(session) = session {
        loop {
            if ctx.cancel.is_cancelled() {
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
                session,
                backend,
                &ctx.tools,
                &ctx.path_guard,
                &ctx.config,
                &ctx.cancel,
                Some(store),
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

    // 通常REPLモード
    loop {
        if ctx.cancel.is_cancelled() {
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
        eprint!("\x1b[2m");
        match run_agent_loop(
            input,
            backend,
            &ctx.tools,
            &ctx.path_guard,
            &ctx.config,
            &ctx.cancel,
            Some(store),
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
            Err(e) => {
                eprint!("\x1b[0m");
                eprintln!("\n\x1b[31mエラー: {e}\x1b[0m\n");
            }
        }
    }

    Ok(())
}

// --- ユーティリティ ---

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
    use std::sync::atomic::{AtomicBool, Ordering};

    static SIGNALED: AtomicBool = AtomicBool::new(false);

    // SAFETY: AtomicBoolのstore()はasync-signal-safe
    extern "C" fn sigint_handler(_: libc::c_int) {
        SIGNALED.store(true, Ordering::Relaxed);
    }

    unsafe {
        libc::signal(
            libc::SIGINT,
            sigint_handler as *const () as libc::sighandler_t,
        );
    }

    std::thread::spawn(move || {
        while !SIGNALED.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        cancel.cancel();
        eprintln!("\n中断します...");
    });
}
