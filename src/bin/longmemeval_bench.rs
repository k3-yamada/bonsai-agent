//! LongMemEval-S benchmark CLI.
//!
//! 使用例:
//! ```bash
//! # Dataset DL (one-time)
//! mkdir -p ~/.cache/bonsai-agent/longmemeval
//! curl -L "https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/main/longmemeval_s_cleaned.json" \
//!   -o ~/.cache/bonsai-agent/longmemeval/longmemeval_s_cleaned.json
//!
//! # Smoke (10 Q)
//! cargo run --release --bin longmemeval-bench -- --limit 10
//! # Full
//! cargo run --release --bin longmemeval-bench
//!
//! # 3-stream RRF (graph fusion) opt-in
//! BONSAI_GRAPH_FUSION_ENABLED=1 cargo run --release --bin longmemeval-bench -- --limit 100
//! # graph 重みカスタム (default 0.25)
//! BONSAI_GRAPH_FUSION_ENABLED=1 BONSAI_GRAPH_FUSION_WEIGHT=0.3 \
//!   cargo run --release --bin longmemeval-bench -- --limit 100
//! ```

use std::path::PathBuf;

use anyhow::{Context, Result};
use bonsai_agent::config::{AppConfig, ServerBackend};
use bonsai_agent::domain::llm::LlmBackend;
use bonsai_agent::eval::longmemeval::{BenchConfig, load_dataset, run_benchmark};
use bonsai_agent::runtime::llama_server::LlamaServerBackend;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "longmemeval-bench",
    about = "LongMemEval-S retrieval benchmark"
)]
struct Args {
    /// 評価する entries 上限 (省略時=全件)
    #[arg(long)]
    limit: Option<usize>,

    /// dataset JSON のパス (省略時=`~/.cache/bonsai-agent/longmemeval/longmemeval_s_cleaned.json`)
    #[arg(long)]
    dataset_path: Option<PathBuf>,

    /// retrieval top-k 上限 (default=20)
    #[arg(long, default_value_t = 20)]
    top_k: usize,

    /// 概念ページ ON arm (Phase 4b 証拠ゲート) を有効化。`BONSAI_CONCEPT_EVAL=1` でも可。
    /// ON 時は実 LLM server (MLX) が必要 (概念合成のため)。
    #[arg(long)]
    concept_eval: bool,
}

fn default_dataset_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| {
        d.join("bonsai-agent")
            .join("longmemeval")
            .join("longmemeval_s_cleaned.json")
    })
}

fn main() -> Result<()> {
    let args = Args::parse();
    let path = args
        .dataset_path
        .or_else(default_dataset_path)
        .context("dataset path 未指定かつ cache_dir 取得失敗")?;

    eprintln!("[longmemeval-bench] loading dataset: {}", path.display());
    let entries =
        load_dataset(&path).with_context(|| format!("dataset load 失敗: {}", path.display()))?;
    eprintln!("[longmemeval-bench] loaded {} entries", entries.len());

    // CLI flag は env (BONSAI_CONCEPT_EVAL) と OR。どちらかが立てば ON arm。
    let concept_eval = args.concept_eval || bonsai_agent::config::is_concept_eval_enabled();
    let cfg = BenchConfig {
        limit: args.limit,
        k_values: vec![5, 10, 20],
        top_k_retrieve: args.top_k,
        concept_eval,
        ..BenchConfig::default()
    };

    // concept ON arm は概念合成に実 LLM backend (MLX) を要する。OFF arm は backend 不要。
    let backend: Option<Box<dyn LlmBackend>> = if cfg.concept_eval {
        let app = AppConfig::load().context("concept ON arm: AppConfig load 失敗")?;
        let m = &app.model;
        let b = LlamaServerBackend::connect_with_params(
            &m.server_url,
            &m.model_id,
            m.inference.clone(),
        )
        .with_mlx_compatible(m.backend == ServerBackend::MlxLm)
        .with_sse_timeout(m.sse_chunk_timeout_secs);
        if !b.is_healthy() {
            eprintln!(
                "エラー: concept ON arm には LLM server が必要です ({})。MLX server を起動してください。",
                m.server_url
            );
            std::process::exit(1);
        }
        eprintln!(
            "[longmemeval-bench] concept ON arm: backend={} @ {}",
            m.model_id, m.server_url
        );
        Some(Box::new(b))
    } else {
        None
    };

    let report = run_benchmark(&entries, &cfg, backend.as_deref())?;

    eprintln!(
        "[longmemeval-bench] arm={} processed={} overall_n={} per_type={} concept_indexed={}",
        if cfg.concept_eval {
            "concept_ON"
        } else {
            "OFF"
        },
        report.processed,
        report.overall.n,
        report.per_type.len(),
        report.concept_memories_indexed,
    );

    // Console pretty summary
    println!("# LongMemEval-S report\n");
    println!(
        "arm = {}",
        if cfg.concept_eval {
            "concept_ON"
        } else {
            "OFF"
        }
    );
    println!(
        "concept_memories_indexed = {}",
        report.concept_memories_indexed
    );
    println!("processed = {}", report.processed);
    println!("\n## overall");
    let recall_avg = report.overall.recall_at_k_avg();
    for (k, v) in &recall_avg {
        println!("- recall_any@{k} = {v:.4}");
    }
    println!("- NDCG@10        = {:.4}", report.overall.ndcg_at_10_avg());
    println!("- MRR            = {:.4}", report.overall.mrr_avg());

    println!("\n## per_question_type");
    let mut types: Vec<&String> = report.per_type.keys().collect();
    types.sort();
    for t in types {
        let agg = &report.per_type[t];
        println!("\n### {t} (n={})", agg.n);
        let r = agg.recall_at_k_avg();
        for (k, v) in &r {
            println!("- recall_any@{k} = {v:.4}");
        }
        println!("- NDCG@10        = {:.4}", agg.ndcg_at_10_avg());
        println!("- MRR            = {:.4}", agg.mrr_avg());
    }

    // JSON dump (stdout)
    println!("\n## raw_json");
    println!("{}", serde_json::to_string_pretty(&report)?);

    Ok(())
}
