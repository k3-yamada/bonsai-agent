use std::io;

use anyhow::Result;
use clap::Parser;

use bonsai_agent::agent::agent_loop::{
    AgentConfig, ReplIo, new_session_with_system, run_agent_loop, run_repl,
};
use bonsai_agent::agent::validate::PathGuard;
use bonsai_agent::cancel::CancellationToken;
use bonsai_agent::config::{AppConfig, ServerBackend, model_id_env_overrides};
use bonsai_agent::domain::llm::{LlmBackend, MockLlmBackend};
use bonsai_agent::domain::model_profile::resolve_model_id;
use bonsai_agent::memory::store::MemoryStore;
use bonsai_agent::runtime::inference::FallbackBackend;
use bonsai_agent::runtime::llama_server::LlamaServerBackend;
use bonsai_agent::tools::ToolRegistry;
use bonsai_agent::tools::arxiv::ArxivTool;
use bonsai_agent::tools::file::{FileReadTool, FileWriteTool, MultiEditTool};
use bonsai_agent::tools::git::GitTool;
use bonsai_agent::tools::memory::{RecallTool, RememberTool};
use bonsai_agent::tools::repomap::RepoMapTool;
use bonsai_agent::tools::shell::ShellTool;
use bonsai_agent::tools::web::{WebFetchTool, WebSearchTool};

mod cli_admin;
mod cli_args;
mod cli_diagnose;
mod cli_init;
mod cli_lab;

use cli_args::Cli;

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
    let mut app_config = AppConfig::load()?;

    if let Some(ref backend_str) = cli.backend {
        match backend_str.to_lowercase().as_str() {
            "unsloth" => app_config.model.backend = ServerBackend::Unsloth,
            "mlx" | "mlx-lm" => app_config.model.backend = ServerBackend::MlxLm,
            "bitnet" => app_config.model.backend = ServerBackend::BitNet,
            "llama" | "llama-server" => app_config.model.backend = ServerBackend::LlamaServer,
            other => eprintln!("[warn] 不明なバックエンド: {other}"),
        }
    }
    if let Some(ref key) = cli.api_key {
        app_config.model.api_key = Some(key.clone());
    }
    let env = model_id_env_overrides();
    app_config.model.model_id = resolve_model_id(
        cli.model.as_deref(),
        env.bonsai_model.as_deref(),
        env.bonsai_model_id.as_deref(),
        env.unsloth_model.as_deref(),
        app_config.model.backend == ServerBackend::Unsloth,
        &app_config.model.model_id,
    );
    // M-3: `AppConfig::load()` 適用済み。ここでは model_id 変更時 (`--model`/env) 用に再適用
    // (同じ model_id なら idempotent no-op、doc: config.rs `AppConfig::load`)。
    if let Some(w) = app_config.model.apply_profile_defaults_checked() {
        eprintln!("[warn] {w}"); // HIGH-1 (doc: config.rs ModelConfig::apply_profile_defaults_checked)
    }
    // Lab (`--lab`) 起動時のみの env override 3 種の適用 + 表示。適用ロジックは
    // config.rs `apply_lab_overrides` に集約 (項目 247/249、SIZE-001 800 行制約対応、
    // doc は config.rs 側参照)。eprintln! はここ (main.rs、LOG-001 whitelist 対象) の責務。
    if cli.lab {
        let report = bonsai_agent::config::apply_lab_overrides(&mut app_config);
        if let Some((prev, new)) = report.temp_override {
            eprintln!("[lab] BONSAI_LAB_TEMP override: temperature {prev:.3} -> {new:.3}");
        }
        if let Some(prev_sse) = report.long_sse_applied {
            eprintln!(
                "[lab] BONSAI_LAB_LONG_SSE=1 → sse_chunk_timeout_secs {prev_sse} -> 180 (MLX cold start catch)"
            );
        }
        if let Some(m) = report.mlx_only_applied {
            eprintln!(
                "[lab] BONSAI_LAB_MLX_ONLY=1 → primary backend {}({}) → MlxLm(8000) + model_id {} → {} + fallback_chain.entries cleared ({} → 0)",
                m.prev_backend,
                m.prev_url,
                m.prev_model_id,
                m.new_model_id,
                m.prev_fallback_entries
            );
        }
    }

    let server_url = if cli.server_url != "http://localhost:8080" {
        cli.server_url.clone()
    } else if app_config.model.backend == ServerBackend::MlxLm
        && app_config.model.server_url == "http://localhost:8080"
    {
        "http://localhost:8000".to_string()
    } else if app_config.model.backend == ServerBackend::BitNet
        && app_config.model.server_url == "http://localhost:8080"
    {
        "http://localhost:8090".to_string()
    } else if app_config.model.backend == ServerBackend::Unsloth
        && app_config.model.server_url == "http://localhost:8080"
    {
        "http://localhost:8888".to_string()
    } else {
        app_config.model.server_url.clone()
    };

    // B-3: BONSAI_MLX_AUTO_CLAMP=1 または Unsloth バックエンドのとき context 上限を取得し
    // context_length を min(configured, server_n_ctx) にクランプ (LocalAI fit_params 思想)。
    // llama.cpp は /props、Unsloth は /v1/models、MLX は config.json (max_position_embeddings)
    // から取得する多段 fallback。
    if bonsai_agent::config::is_mlx_auto_clamp()
        || app_config.model.backend == ServerBackend::Unsloth
    {
        let configured = app_config.model.context_length;
        let server_n_ctx = bonsai_agent::runtime::server_props::resolve_server_n_ctx_with_key(
            &server_url,
            &app_config.model.model_id,
            app_config.model.api_key.as_deref(),
        );
        let clamped =
            bonsai_agent::runtime::server_props::clamp_context_to_server(configured, server_n_ctx);
        if clamped != configured {
            eprintln!(
                "[auto-clamp] context_length {} → {} (server n_ctx={:?})",
                configured, clamped, server_n_ctx
            );
            app_config.model.context_length = clamped;
        }
    }

    let mut tools = setup_tools(&app_config);
    // プラグインツールの登録
    for plugin_tool in bonsai_agent::tools::plugin::load_plugin_tools(&app_config.plugins.tools) {
        tools.register(plugin_tool);
    }

    let cancel = CancellationToken::new();
    ctrlc_handler(cancel.clone());

    let autonomy_level = if let Some(ref a) = cli.autonomy {
        a.parse::<bonsai_agent::safety::autonomy::AutonomyLevel>()
            .unwrap_or_else(|e| {
                eprintln!("警告: {e}。デフォルトの supervised を使用します。");
                bonsai_agent::safety::autonomy::AutonomyLevel::Supervised
            })
    } else {
        app_config
            .safety
            .autonomy
            .unwrap_or(bonsai_agent::safety::autonomy::AutonomyLevel::Supervised)
    };

    let ctx = AppContext {
        tools,
        path_guard: PathGuard::new(app_config.safety.deny_paths.clone()),
        config: AgentConfig {
            max_iterations: app_config.agent.max_iterations,
            max_retries: app_config.agent.max_retries,
            max_tools_selected: app_config.agent.max_tools_selected,
            max_tool_output_chars: app_config.agent.max_tool_output_chars,
            max_tools_in_context: app_config.agent.max_tools_in_context,
            max_mcp_tools_in_context: app_config.agent.max_mcp_tools_in_context,
            base_inference: app_config.model.inference.clone(),
            advisor: app_config.advisor.to_runtime().with_cancel(cancel.clone()),
            task_timeout: (app_config.experiment.task_timeout_secs > 0)
                .then(|| std::time::Duration::from_secs(app_config.experiment.task_timeout_secs)),
            soul_path: app_config.agent.soul_path.clone(),
            n_ctx_budget: (app_config.model.context_length > 0)
                .then_some(app_config.model.context_length),
            memory_blocks: app_config.memory.blocks.clone(),
            autonomy: autonomy_level,
            ..Default::default()
        },
        cancel,
        server_url,
        mock: cli.mock,
        app_config,
    };

    // 早期リターンモード（DB不要）
    if cli.diagnose {
        return cli_diagnose::handle_diagnose_mode(&ctx.server_url, &ctx.app_config);
    }
    if cli.lab {
        return cli_lab::handle_lab_mode(
            &ctx.config,
            &ctx.tools,
            &ctx.path_guard,
            &ctx.cancel,
            &ctx.app_config,
            ctx.mock,
            cli.lab_experiments,
            || create_backend(&ctx),
            &get_db_path(),
        );
    }
    if cli.evolve {
        return cli_lab::handle_evolve_mode(&get_db_path());
    }
    if cli.init {
        return cli_init::handle_init_mode();
    }
    if cli.manifest {
        println!("{}", bonsai_agent::safety::manifest::format_manifest());
        return Ok(());
    }
    if cli.list_tools {
        // whitelist 適用後の live registry (ctx.tools は setup_tools で filter 済み)
        print!("{}", bonsai_agent::tools::format_tool_listing(&ctx.tools));
        return Ok(());
    }
    if cli.vault {
        return cli_admin::handle_vault_mode();
    }

    // DB必要モード
    let store = MemoryStore::open(&get_db_path())?;

    if cli.serve {
        let env_token = std::env::var("BONSAI_API_KEY").ok();
        let api_token = cli.api_token.as_deref().or(env_token.as_deref());
        println!(
            "REST API サーバーを起動します (ポート: {})...",
            cli.api_port
        );
        bonsai_agent::server::start_api_server(&store, cli.api_port, api_token);
        return Ok(());
    }
    if cli.mcp_server {
        bonsai_agent::mcp_server::run_mcp_server_with_guard(&store, &ctx.path_guard);
        return Ok(());
    }

    if let Some(path) = &cli.ingest {
        let n = bonsai_agent::memory::ingest::ingest_path(&store, path)?;
        println!("ingest 完了: {n} chunk を保存しました ({})", path.display());
        if cli.ingest_prune {
            if path.is_dir() {
                let purged = bonsai_agent::memory::ingest::reconcile_ingested_files(&store, path)?;
                println!("prune 完了: 削除済ファイルの孤児 chunk {purged} 件を掃除しました");
            } else {
                eprintln!("警告: --ingest-prune はディレクトリ取込時のみ有効です (スキップ)");
            }
        }
        return Ok(());
    }
    if cli.skills_export {
        return cli_admin::handle_skills_export_mode(&store);
    }
    if cli.visualize {
        return cli_admin::handle_visualize_mode(
            &store,
            &cli.visualize_output,
            cli.visualize_open,
            !cli.no_privacy,
        );
    }
    if cli.sessions {
        return cli_admin::handle_sessions_mode(&store);
    }
    if cli.tasks {
        return cli_admin::handle_tasks_mode(&store);
    }
    if cli.audit {
        return cli_admin::handle_audit_mode(&store);
    }
    if cli.dashboard {
        return cli_admin::handle_dashboard_mode(&store);
    }
    if cli.checkpoints {
        return cli_admin::handle_checkpoints_mode(&store);
    }
    if let Some(cp_id) = cli.rollback {
        return cli_admin::handle_rollback_mode(&store, cp_id);
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
    tools.register(Box::new(MultiEditTool));
    tools.register(Box::new(GitTool));
    tools.register(Box::new(WebSearchTool));
    tools.register(Box::new(WebFetchTool));
    tools.register(Box::new(ArxivTool));
    tools.register(Box::new(RepoMapTool));
    // 能動的記憶ツール (①パーソナル知識デーモン Phase 1)
    tools.register(Box::new(RememberTool::new(get_db_path())));
    tools.register(Box::new(RecallTool::new(get_db_path())));

    // MCPサーバー起動・ツール登録
    for server_cfg in &app_config.mcp.servers {
        match setup_mcp_server(server_cfg) {
            Ok(mcp_tools) => {
                let count = mcp_tools.len();
                for t in mcp_tools {
                    tools.register(t);
                }
                bonsai_agent::observability::logger::log_event(
                    bonsai_agent::observability::logger::LogLevel::Info,
                    "mcp",
                    &format!(
                        "MCPサーバー '{}' 起動: {}ツール登録",
                        server_cfg.name, count
                    ),
                );
            }
            Err(e) => {
                bonsai_agent::observability::logger::log_event(
                    bonsai_agent::observability::logger::LogLevel::Warn,
                    "mcp",
                    &format!("MCPサーバー '{}' スキップ: {e}", server_cfg.name),
                );
            }
        }
    }

    // deny-by-default whitelist (Z-NEW-E): BONSAI_ENABLED_TOOLS 明示列挙 or BONSAI_LAB_SMOKE=1 時のみ
    // readonly tool に絞る。env 未設定 (= None) で全 tool 維持 (backward compat)。
    let tools = match bonsai_agent::tools::whitelist::effective_tool_whitelist() {
        Some(whitelist) => {
            let filtered = tools.apply_whitelist(&whitelist);
            bonsai_agent::observability::logger::log_event(
                bonsai_agent::observability::logger::LogLevel::Info,
                "tools",
                &format!(
                    "tool whitelist 適用: {} tool に制限 ({})",
                    filtered.len(),
                    whitelist.join(",")
                ),
            );
            filtered
        }
        None => tools,
    };

    // ツール数上限警告（whitelist適用時等、常時アクティブなツールがコンテキスト上限を超えている場合のみ）
    if bonsai_agent::tools::whitelist::is_tool_whitelist_enabled() {
        tools.warn_if_exceeded(app_config.agent.max_tools_in_context);
    }

    tools
}

/// MCPサーバーを起動しツールリストを取得
fn setup_mcp_server(
    cfg: &bonsai_agent::tools::mcp_client::McpServerConfig,
) -> anyhow::Result<Vec<Box<dyn bonsai_agent::tools::Tool>>> {
    use bonsai_agent::tools::mcp_client::{McpConnection, McpToolWrapper};
    use std::sync::{Arc, Mutex};

    let mut conn = McpConnection::spawn(cfg)?;
    let tool_infos = conn.list_tools()?;
    let connection = Arc::new(Mutex::new(conn));
    let tools: Vec<Box<dyn bonsai_agent::tools::Tool>> = tool_infos
        .into_iter()
        .map(|info| {
            Box::new(McpToolWrapper::new(info, &cfg.name, connection.clone()))
                as Box<dyn bonsai_agent::tools::Tool>
        })
        .collect();
    Ok(tools)
}

/// バックエンド生成（モック/実機の分岐を統合）
fn create_backend(ctx: &AppContext) -> Box<dyn LlmBackend> {
    if ctx.mock {
        return Box::new(MockLlmBackend::new(
            (0..10000)
                .map(|_| "モックモードです。".to_string())
                .collect(),
        ));
    }

    // Step 12: フォールバックチェーンが設定されていれば FallbackBackend で wrap
    if let Some(chain) = ctx.app_config.fallback_chain.build_chain() {
        let sse_timeout = ctx.app_config.model.sse_chunk_timeout_secs;
        let inference = &ctx.app_config.model.inference;
        let mut backends: std::collections::HashMap<String, Box<dyn LlmBackend>> =
            std::collections::HashMap::new();
        let mut at_least_one_healthy = false;
        for entry in chain.entries() {
            let mlx_compat = entry.backend == ServerBackend::MlxLm;
            let mut b = LlamaServerBackend::connect_with_params(
                &entry.server_url,
                &entry.model_id,
                inference.clone(),
            )
            .with_mlx_compatible(mlx_compat)
            .with_sse_timeout(sse_timeout);
            if let Some(key) = &ctx.app_config.model.api_key {
                b = b.with_api_key(key.clone());
            }
            if b.is_healthy() {
                at_least_one_healthy = true;
            } else {
                eprintln!(
                    "[fallback] 警告: エントリ {:?}/{} ({}) は応答していません。フォールバック対象として登録のみ続行。",
                    entry.backend, entry.model_id, entry.server_url
                );
            }
            backends.insert(FallbackBackend::key_for(entry), Box::new(b));
        }
        if !at_least_one_healthy {
            eprintln!("エラー: フォールバックチェーン内のどのバックエンドにも接続できません。");
            eprintln!("--mock を指定するか、少なくとも 1 つのサーバーを起動してください。");
            std::process::exit(1);
        }
        eprintln!(
            "[fallback] FallbackBackend を構築しました（{} entries、threshold={}）",
            chain.entries().len(),
            ctx.app_config.fallback_chain.max_failures.unwrap_or(2),
        );
        return maybe_supervise(ctx, Box::new(FallbackBackend::new(chain, backends)));
    }

    // 単一バックエンド経路（既存）
    let backend = &ctx.app_config.model.backend;
    let mut b = LlamaServerBackend::connect_with_params(
        &ctx.server_url,
        &ctx.app_config.model.model_id,
        ctx.app_config.model.inference.clone(),
    )
    .with_mlx_compatible(*backend == ServerBackend::MlxLm)
    .with_sse_timeout(ctx.app_config.model.sse_chunk_timeout_secs);
    if let Some(key) = &ctx.app_config.model.api_key {
        b = b.with_api_key(key.clone());
    }
    if !b.is_healthy() {
        let backend_name = match backend {
            ServerBackend::LlamaServer => "llama-server",
            ServerBackend::MlxLm => "mlx-lm",
            ServerBackend::BitNet => "bitnet.cpp",
            ServerBackend::Unsloth => "unsloth",
        };
        eprintln!(
            "エラー: {} ({}) に接続できません。",
            backend_name, ctx.server_url
        );
        eprintln!("--mock フラグでモックモードを使用するか、サーバーを起動してください。");
        std::process::exit(1);
    }
    maybe_supervise(ctx, Box::new(b))
}

/// B-1: `BONSAI_MLX_IDLE_TIMEOUT_SEC>0` の場合のみ backend を SupervisedBackend で wrap し、
/// idle watchdog thread を起動する。env unset/0 (default) では backend をそのまま返す
/// = 既存挙動 100% 保持。
fn maybe_supervise(ctx: &AppContext, backend: Box<dyn LlmBackend>) -> Box<dyn LlmBackend> {
    let idle = bonsai_agent::config::mlx_idle_timeout_sec();
    if idle == 0 {
        return backend;
    }

    let health_url = format!("{}/health", ctx.server_url.trim_end_matches('/'));
    let port = ctx
        .server_url
        .rsplit(':')
        .next()
        .and_then(|p| p.trim_end_matches('/').parse::<u16>().ok())
        .unwrap_or(8000);
    let supervisor = std::sync::Arc::new(
        bonsai_agent::runtime::process_supervisor::ProcessSupervisor::with_spawn(
            health_url,
            idle,
            bonsai_agent::config::mlx_spawn_program(),
            ctx.app_config.model.model_id.clone(),
            port,
        ),
    );

    // watchdog thread: 30s 毎に kill_if_idle を poll、idle なら kill (次 request で respawn)。
    let sup_watch = std::sync::Arc::clone(&supervisor);
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(30));
            if sup_watch.kill_if_idle() {
                eprintln!(
                    "[watchdog] MLX server idle timeout — killed (will respawn on next request)"
                );
            }
        }
    });
    eprintln!("[watchdog] MLX idle supervisor active (timeout={idle}s)");

    Box::new(bonsai_agent::runtime::supervised_backend::SupervisedBackend::new(backend, supervisor))
}

// --- モードハンドラ ---

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

    let msg_len = session.messages.len();
    let msg_start = msg_len.saturating_sub(4);
    for msg in &session.messages[msg_start..] {
        let role = match msg.role {
            bonsai_agent::domain::conversation::Role::User => "\x1b[36mあなた\x1b[0m",
            bonsai_agent::domain::conversation::Role::Assistant => "\x1b[32mBonsai\x1b[0m",
            _ => continue,
        };
        let preview: String = msg.content.chars().take(80).collect();
        println!("{role}: {preview}");
    }
    println!("\n--- 続きからどうぞ ---\n");

    let backend: std::sync::Arc<dyn LlmBackend> = create_backend(ctx).into();
    run_repl_stdio(ctx, store, backend, &mut session)
}

fn handle_exec_mode(ctx: &AppContext, store: &MemoryStore, input: &str) -> Result<()> {
    let mut fast_path = bonsai_agent::agent::fast_path::FastPathDispatcher::with_default_rules();
    if let Some(fast_resp) = fast_path.try_handle(input) {
        println!("{fast_resp}");
        return Ok(());
    }

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
    // ストリーミング出力で正常回答は表示済み。[中断]のみ追加表示。
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

    let backend: std::sync::Arc<dyn LlmBackend> = create_backend(ctx).into();
    // [会話継続] fresh session を 1 つ生成し全ターンで共有する。
    // 旧実装は毎ターン独立実行 (None) で履歴を失っていた。
    let mut session = new_session_with_system(&ctx.config);
    run_repl_stdio(ctx, store, backend, &mut session)
}

/// stdin/stdout を lib 側の会話継続 REPL (`run_repl`) へ橋渡しする glue。
/// REPL / resume の双方が単一 `Session` をターン間で共有する。
fn run_repl_stdio(
    ctx: &AppContext,
    store: &MemoryStore,
    backend: std::sync::Arc<dyn LlmBackend>,
    session: &mut bonsai_agent::domain::conversation::Session,
) -> Result<()> {
    let stdin = io::stdin();
    let mut reader = io::BufReader::new(stdin);
    let stdout = io::stdout();
    let mut writer = stdout.lock();

    let is_busy = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let db_path = store.path().map(|s| s.to_string());

    // 外部知覚センサー (SensorHub) の常駐起動
    let sensors_cfg = &ctx.app_config.sensors;
    if sensors_cfg.enabled {
        let mut sensors: Vec<std::sync::Arc<dyn bonsai_agent::agent::sensors::Sensor>> = Vec::new();

        if sensors_cfg.file_watch {
            let watch_dir =
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            sensors.push(std::sync::Arc::new(
                bonsai_agent::agent::sensors::FileWatchSensor::new("workspace", watch_dir),
            ));
        }

        if sensors_cfg.idle_detection {
            sensors.push(std::sync::Arc::new(
                bonsai_agent::agent::sensors::IdleSensor::new(std::time::Duration::from_secs(
                    sensors_cfg.idle_threshold_secs,
                )),
            ));
        }

        // フェーズ2: ウィンドウ監視（config または env override で明示有効化された場合）
        let window_enabled = bonsai_agent::config::is_window_sensor_enabled_env()
            .unwrap_or(sensors_cfg.window_focus);
        if window_enabled {
            sensors.push(std::sync::Arc::new(
                bonsai_agent::agent::sensors::WindowChangedSensor::new(
                    std::time::Duration::from_secs(sensors_cfg.window_cooldown_secs),
                    bonsai_agent::safety::sensor_filter::AppDenylist::new(
                        &sensors_cfg.window_denylist,
                    ),
                    bonsai_agent::safety::sensor_filter::PrivacyFilter::default(),
                ),
            ));
        }

        let hub = bonsai_agent::agent::sensors::SensorHub::new(sensors);
        let (sensor_rx, _sensor_handles) = hub.spawn_all_with_permission_check(ctx.cancel.clone());

        let sensor_cancel = ctx.cancel.clone();
        let sensor_db_path = db_path.clone();
        std::thread::spawn(move || {
            let sensor_store = sensor_db_path
                .as_deref()
                .and_then(|p| MemoryStore::open(p).ok());
            let handler = bonsai_agent::agent::sensors::SensorEventHandler::new();
            while !sensor_cancel.is_cancelled() {
                if let Ok(event) = sensor_rx.recv_timeout(std::time::Duration::from_millis(200)) {
                    handler.handle_event(&event, sensor_store.as_ref());
                }
            }
        });
    }

    // DMN常駐バックグラウンドスレッドを起動（アイドル時に自発的思考・内省）
    let vault_path = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("bonsai-agent")
        .join("vault");
    let dream_threshold = std::env::var("BONSAI_DMN_DREAM_THRESHOLD")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(300.0);
    let auto_persist = std::env::var("BONSAI_DMN_AUTO_PERSIST")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let worker = bonsai_agent::agent::dmn::worker::DmnWorker::new(120.0, 30.0, 0.75)
        .with_vault(vault_path)
        .with_dream_threshold(dream_threshold)
        .with_auto_persist(auto_persist);
    let dmn_backend = backend.clone();
    let dmn_inbox = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let dmn_inbox_clone = dmn_inbox.clone();
    let mut dmn_runner = bonsai_agent::agent::dmn::worker::DmnRunner::spawn(
        worker,
        is_busy.clone(),
        ctx.cancel.clone(),
        db_path,
        move |store| {
            bonsai_agent::agent::dmn::generator::DmnGenerator::generate_reflection_with_llm(
                store,
                Some(&*dmn_backend),
            )
        },
        move |msg| {
            if let Ok(mut inbox) = dmn_inbox_clone.lock() {
                inbox.push(msg);
            }
        },
    );

    let mut repl_config = ctx.config.clone();
    if matches!(
        repl_config.autonomy,
        bonsai_agent::safety::autonomy::AutonomyLevel::Supervised
    ) && repl_config.confirm_callback.is_none()
    {
        repl_config.confirm_callback =
            Some(std::sync::Arc::new(|name: &str, args: &str| -> bool {
                use std::io::Write;
                let display_args = if args.len() > 200 {
                    let end = args.floor_char_boundary(200);
                    format!("{}...", &args[..end])
                } else {
                    args.to_string()
                };
                eprintln!("\n⚠️  [確認要求] ツール '{name}' を実行しますか？");
                eprintln!("引数: {display_args}");
                eprint!("実行を許可しますか？ [y/N]> ");
                let _ = std::io::stderr().flush();

                let mut input = String::new();
                if std::io::stdin().read_line(&mut input).is_ok() {
                    let trimmed = input.trim();
                    trimmed.eq_ignore_ascii_case("y") || trimmed.eq_ignore_ascii_case("yes")
                } else {
                    false
                }
            }));
    }

    let repl_io = ReplIo {
        backend: &*backend,
        tools: &ctx.tools,
        path_guard: &ctx.path_guard,
        config: &repl_config,
        cancel: &ctx.cancel,
        store: Some(store),
        is_busy: Some(is_busy),
        dmn_inbox: Some(dmn_inbox),
        dmn_notify: None,
    };
    let res = run_repl(&mut reader, &mut writer, session, &repl_io);
    dmn_runner.stop();
    res
}

// --- ユーティリティ ---

fn get_db_path() -> String {
    let env_override = std::env::var("BONSAI_DB_PATH").ok();
    let path = bonsai_agent::config::resolve_db_path(env_override.as_deref(), dirs::data_dir());
    // 親ディレクトリを作成して open を成功させる (env override 指定パスにも適用)。
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    path.to_string_lossy().to_string()
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
