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
use bonsai_agent::eval::longmemeval::{BenchConfig, load_dataset, run_benchmark};
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

    let cfg = BenchConfig {
        limit: args.limit,
        k_values: vec![5, 10, 20],
        top_k_retrieve: args.top_k,
        ..BenchConfig::default()
    };

    let report = run_benchmark(&entries, &cfg)?;

    eprintln!(
        "[longmemeval-bench] processed={} overall_n={} per_type={}",
        report.processed,
        report.overall.n,
        report.per_type.len()
    );

    // Console pretty summary
    println!("# LongMemEval-S report\n");
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
