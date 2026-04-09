use std::io::{self, BufRead, Write};

use anyhow::Result;
use clap::Parser;

use bonsai_agent::agent::agent_loop::{run_agent_loop, AgentConfig};
use bonsai_agent::agent::validate::PathGuard;
use bonsai_agent::cancel::CancellationToken;
use bonsai_agent::memory::store::MemoryStore;
use bonsai_agent::runtime::inference::MockLlmBackend;
use bonsai_agent::runtime::llama_server::LlamaServerBackend;
use bonsai_agent::tools::file::{FileReadTool, FileWriteTool};
use bonsai_agent::tools::git::GitTool;
use bonsai_agent::tools::shell::ShellTool;
use bonsai_agent::tools::ToolRegistry;

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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // ツールレジストリ
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(ShellTool::new()));
    tools.register(Box::new(FileReadTool));
    tools.register(Box::new(FileWriteTool));
    tools.register(Box::new(GitTool));

    // 安全性
    let path_guard = PathGuard::default_deny_list();
    let config = AgentConfig::default();
    let cancel = CancellationToken::new();

    // Ctrl+Cハンドラ
    let cancel_clone = cancel.clone();
    ctrlc_handler(cancel_clone);

    // メモリストア
    let db_path = get_db_path();
    let store = MemoryStore::open(&db_path)?;

    if let Some(input) = &cli.exec {
        // 単発実行モード
        let result = if cli.mock {
            let mock = MockLlmBackend::single("モックモードです。実際のLLMは使用していません。");
            run_agent_loop(input, &mock, &tools, &path_guard, &config, &cancel, Some(store.conn()))?
        } else {
            let backend = LlamaServerBackend::connect(&cli.server_url, "bonsai-8b");
            run_agent_loop(input, &backend, &tools, &path_guard, &config, &cancel, Some(store.conn()))?
        };
        println!("{result}");
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
            let backend = LlamaServerBackend::connect(&cli.server_url, "bonsai-8b");
            if !backend.is_healthy() {
                eprintln!("警告: llama-server ({}) に接続できません。", cli.server_url);
                eprintln!("--mock フラグでモックモードを使用するか、llama-serverを起動してください。");
                std::process::exit(1);
            }
            println!("[接続済み] {}", cli.server_url);
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
                Some(store.conn()),
            ) {
                Ok(result) => {
                    eprint!("\x1b[0m"); // 色リセット
                    println!("\n\x1b[1m{result}\x1b[0m\n"); // 最終回答は太字
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
        libc::signal(libc::SIGINT, sigint_handler as *const () as libc::sighandler_t);
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
