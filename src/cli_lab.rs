//! Lab実験ループおよびEvolve（自己改善）モードハンドラ

use anyhow::Result;
use bonsai_agent::agent::experiment::{ExperimentLoopConfig, run_experiment_loop};
use bonsai_agent::domain::llm::{LlmBackend, MockLlmBackend};
use bonsai_agent::memory::store::MemoryStore;
use bonsai_agent::runtime::cache::CachedBackend;

#[allow(clippy::too_many_arguments)]
pub fn handle_lab_mode(
    config: &bonsai_agent::agent::agent_loop::AgentConfig,
    tools: &bonsai_agent::tools::ToolRegistry,
    path_guard: &bonsai_agent::agent::validate::PathGuard,
    cancel: &bonsai_agent::cancel::CancellationToken,
    app_config: &bonsai_agent::config::AppConfig,
    mock: bool,
    max_experiments: usize,
    backend_factory: impl FnOnce() -> Box<dyn LlmBackend>,
    db_path: &str,
) -> Result<()> {
    let store = MemoryStore::open(db_path)?;

    // 非モック経路は backend_factory に委譲
    let backend: Box<dyn LlmBackend> = if mock {
        Box::new(MockLlmBackend::new(
            (0..10000).map(|_| "1024".to_string()).collect(),
        ))
    } else {
        backend_factory()
    };

    if bonsai_agent::config::is_lab_mlx_warmup() {
        let n = bonsai_agent::config::lab_mlx_warmup_count().unwrap_or(3);
        let succ = bonsai_agent::agent::experiment::lab_mlx_prewarm(backend.as_ref(), n, cancel);
        if n > 0 && succ == 0 {
            eprintln!(
                "[lab] WARN: pre-warm 全失敗 (succ=0/n={n}, BONSAI_LAB_MLX_WARMUP_COUNT={n}). \
                 MLX server 未起動か到達不可の可能性. \
                 Lab cycle は続行するが cold start latency 未消化."
            );
        }
    }

    let tsv_path = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("bonsai-agent")
        .join("experiments.tsv");
    let loop_config = ExperimentLoopConfig {
        tsv_path: Some(tsv_path),
        max_experiments: Some(max_experiments),
        dreamer_interval: app_config.experiment.dreamer_interval,
        enable_prescreening: app_config.experiment.enable_prescreening,
        prescreening_threshold: app_config.experiment.prescreening_threshold,
        task_timeout_secs: app_config.experiment.task_timeout_secs,
        judge_threshold: app_config.experiment.judge_threshold,
        judge_sample_size: app_config.experiment.judge_sample_size,
    };
    let backend = CachedBackend::new(backend, 200);

    if bonsai_agent::knowledge::vault_lint::is_vault_lint_lab_enabled() {
        let vault_root = dirs::data_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("bonsai-agent")
            .join("vault");
        let audit = bonsai_agent::observability::audit::AuditLog::new(store.conn());
        let strict = bonsai_agent::knowledge::vault_lint::is_vault_lint_strict();
        if let Err(e) = bonsai_agent::knowledge::vault_lint::run_vault_sanity_gate(
            &vault_root,
            bonsai_agent::knowledge::vault_lint::vault_lint_stale_days(),
            strict,
            Some(&audit),
        ) {
            if strict {
                return Err(e);
            }
            eprintln!("[lab.vault_lint] sanity gate failed (warn-only): {e}");
        }
    }

    let experiments = run_experiment_loop(
        config,
        &backend,
        tools,
        path_guard,
        cancel,
        &store,
        &loop_config,
    )?;
    println!("\n実験完了: {}件", experiments.len());
    Ok(())
}

pub fn handle_evolve_mode(db_path: &str) -> Result<()> {
    let store = MemoryStore::open(db_path)?;
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
