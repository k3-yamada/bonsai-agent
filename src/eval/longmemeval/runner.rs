//! LongMemEval per-question benchmark runner.
//!
//! 各 entry に対し isolated `MemoryStore::in_memory()` を起こし、
//! haystack_sessions を 1 session = 1 memory として index、
//! `HybridSearch::search(question, top_k_retrieve)` で retrieval、
//! recall/NDCG/MRR を per_type + overall で集計する。

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use serde::Serialize;

use crate::domain::embedder::create_embedder;
use crate::eval::longmemeval::dataset::LongMemEvalEntry;
use crate::eval::longmemeval::metrics::{mrr, ndcg_at_k, recall_any_at_k};
use crate::memory::search::HybridSearch;
use crate::memory::store::MemoryStore;

#[derive(Debug, Clone)]
pub struct BenchConfig {
    pub limit: Option<usize>,
    pub k_values: Vec<usize>,
    pub top_k_retrieve: usize,
    pub progress_every: usize,
    /// Graph stream の重み (0.0-1.0、0.0 = 2-stream legacy)。
    /// `BONSAI_GRAPH_FUSION_ENABLED=1` で `BONSAI_GRAPH_FUSION_WEIGHT` (default 0.25) から populate される。
    pub graph_weight: f32,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            limit: None,
            k_values: vec![5, 10, 20],
            top_k_retrieve: 20,
            progress_every: progress_every_from_env(50),
            graph_weight: graph_weight_from_env(),
        }
    }
}

fn progress_every_from_env(default_val: usize) -> usize {
    std::env::var("BONSAI_LONGMEMEVAL_PROGRESS_INTERVAL")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(default_val)
}

/// `BONSAI_GRAPH_FUSION_ENABLED=1` のとき `BONSAI_GRAPH_FUSION_WEIGHT` (default 0.25) を返す。
/// 未設定なら 0.0 (2-stream legacy 経路、production default OFF / Cerememory 三本柱 pattern)。
fn graph_weight_from_env() -> f32 {
    let enabled = std::env::var("BONSAI_GRAPH_FUSION_ENABLED").ok().as_deref() == Some("1");
    if !enabled {
        return 0.0;
    }
    std::env::var("BONSAI_GRAPH_FUSION_WEIGHT")
        .ok()
        .and_then(|s| s.parse::<f32>().ok())
        .filter(|w| w.is_finite() && *w > 0.0 && *w <= 1.0)
        .unwrap_or(0.25)
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct EntryMetrics {
    pub recall_at_k: BTreeMap<usize, f64>,
    pub ndcg_at_10: f64,
    pub mrr: f64,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct TypeAgg {
    pub n: usize,
    pub recall_at_k_sum: BTreeMap<usize, f64>,
    pub ndcg_at_10_sum: f64,
    pub mrr_sum: f64,
}

impl TypeAgg {
    pub fn absorb(&mut self, m: &EntryMetrics) {
        self.n += 1;
        for (k, v) in &m.recall_at_k {
            *self.recall_at_k_sum.entry(*k).or_insert(0.0) += v;
        }
        self.ndcg_at_10_sum += m.ndcg_at_10;
        self.mrr_sum += m.mrr;
    }

    pub fn recall_at_k_avg(&self) -> BTreeMap<usize, f64> {
        if self.n == 0 {
            return BTreeMap::new();
        }
        self.recall_at_k_sum
            .iter()
            .map(|(k, v)| (*k, v / self.n as f64))
            .collect()
    }

    pub fn ndcg_at_10_avg(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.ndcg_at_10_sum / self.n as f64
        }
    }

    pub fn mrr_avg(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.mrr_sum / self.n as f64
        }
    }
}

#[derive(Debug, Serialize)]
pub struct BenchReport {
    pub overall: TypeAgg,
    pub per_type: HashMap<String, TypeAgg>,
    pub processed: usize,
}

pub fn run_benchmark(entries: &[LongMemEvalEntry], cfg: &BenchConfig) -> Result<BenchReport> {
    let mut per_type: HashMap<String, TypeAgg> = HashMap::new();
    let mut overall = TypeAgg::default();
    let mut processed = 0usize;

    let take_n = cfg.limit.unwrap_or(entries.len()).min(entries.len());
    let embedder = create_embedder();

    for (idx, entry) in entries.iter().take(take_n).enumerate() {
        let store = MemoryStore::in_memory()?;

        // 1 session = 1 memory として narrative 化、tags[0] に session_id を埋め込む
        let mut indexed: Vec<(i64, String)> = Vec::with_capacity(entry.haystack_sessions.len());
        for (sess_idx, turns) in entry.haystack_sessions.iter().enumerate() {
            let sess_id = entry
                .haystack_session_ids
                .get(sess_idx)
                .cloned()
                .unwrap_or_else(|| format!("auto-{sess_idx}"));
            let narrative = turns
                .iter()
                .map(|t| format!("{}: {}", t.role, t.content))
                .collect::<Vec<_>>()
                .join("\n");
            let mid = store.save_memory(&narrative, "session", &[sess_id])?;
            indexed.push((mid, narrative));
        }

        // vec0 KNN 経路を有効化するため、save_memory の後に embedding を別途 insert する
        // (save_memory は memories table のみ書き込み、vec_memories は別 op)。
        // SimpleEmbedder fallback / FastEmbedder どちらも DEFAULT_EMBEDDING_DIM=256 を返す。
        #[cfg(feature = "embeddings")]
        {
            let texts: Vec<&str> = indexed.iter().map(|(_, t)| t.as_str()).collect();
            let embs = embedder.embed(&texts)?;
            for ((mid, _), emb) in indexed.iter().zip(embs.iter()) {
                store.insert_memory_embedding(*mid, emb)?;
            }
        }

        let mut search = HybridSearch::new(&store, &*embedder);
        if cfg.graph_weight > 0.0 {
            search = search.with_graph_weight(cfg.graph_weight);
            // Graph stream indexing: 各 session narrative を token 化して
            // KnowledgeGraph に (token -[appears_in]-> memory:{id}) edge を張る。
            for (mid, narrative) in &indexed {
                search.index_memory_tokens(*mid, narrative)?;
            }
        }
        let results = search.search(&entry.question, cfg.top_k_retrieve)?;

        let retrieved_ids: Vec<String> = results
            .iter()
            .filter_map(|r| {
                serde_json::from_str::<Vec<String>>(&r.memory.tags)
                    .ok()
                    .and_then(|v| v.into_iter().next())
            })
            .collect();

        let mut entry_metrics = EntryMetrics::default();
        for &k in &cfg.k_values {
            let r = recall_any_at_k(&retrieved_ids, &entry.answer_session_ids, k);
            entry_metrics.recall_at_k.insert(k, r);
        }
        entry_metrics.ndcg_at_10 = ndcg_at_k(&retrieved_ids, &entry.answer_session_ids, 10);
        entry_metrics.mrr = mrr(&retrieved_ids, &entry.answer_session_ids);

        let agg = per_type.entry(entry.question_type.clone()).or_default();
        agg.absorb(&entry_metrics);
        overall.absorb(&entry_metrics);
        processed += 1;

        if cfg.progress_every > 0 && (idx + 1) % cfg.progress_every == 0 {
            eprintln!("[longmemeval-bench] {}/{}", idx + 1, take_n);
        }
    }

    Ok(BenchReport {
        overall,
        per_type,
        processed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::longmemeval::dataset::HaystackTurn;

    fn entry_single_session_user(
        question_id: &str,
        gold_idx: usize,
        n_sessions: usize,
    ) -> LongMemEvalEntry {
        let mut haystack_sessions = Vec::new();
        let mut haystack_session_ids = Vec::new();
        let mut haystack_dates = Vec::new();
        for i in 0..n_sessions {
            let sess_id = format!("s-{i:03}");
            let content = if i == gold_idx {
                "deadline is Friday".to_string()
            } else {
                format!("filler chitchat {i}")
            };
            haystack_sessions.push(vec![HaystackTurn {
                role: "user".to_string(),
                content,
                has_answer: Some(i == gold_idx),
            }]);
            haystack_session_ids.push(sess_id);
            haystack_dates.push("2024-01-01".to_string());
        }
        LongMemEvalEntry {
            question_id: question_id.to_string(),
            question_type: "single-session-user".to_string(),
            question: "deadline".to_string(),
            question_date: "2024-01-15".to_string(),
            answer: "Friday".to_string(),
            answer_session_ids: vec![format!("s-{gold_idx:03}")],
            haystack_dates,
            haystack_session_ids,
            haystack_sessions,
        }
    }

    #[test]
    fn test_runner_indexes_53_sessions_returns_topk() {
        let entry = entry_single_session_user("q-001", 7, 53);
        let cfg = BenchConfig {
            limit: None,
            k_values: vec![5, 10, 20],
            top_k_retrieve: 20,
            progress_every: 9999,
            graph_weight: 0.0,
        };
        let report = run_benchmark(&[entry], &cfg).unwrap();
        assert_eq!(report.processed, 1);
        // 53 session を index して top-20 取得できれば良い (recall 数値は問わない)
        assert_eq!(report.overall.n, 1);
        assert!(report.overall.recall_at_k_sum.contains_key(&5));
        assert!(report.overall.recall_at_k_sum.contains_key(&10));
        assert!(report.overall.recall_at_k_sum.contains_key(&20));
    }

    #[test]
    fn test_graph_weight_env_default_off() {
        // 環境変数未設定なら 0.0 (2-stream legacy 経路)
        // Safety: env var を unset するため `unsafe` block 必要 (Rust 2024)
        unsafe { std::env::remove_var("BONSAI_GRAPH_FUSION_ENABLED") };
        unsafe { std::env::remove_var("BONSAI_GRAPH_FUSION_WEIGHT") };
        let cfg = BenchConfig::default();
        assert_eq!(cfg.graph_weight, 0.0);
    }

    #[test]
    fn test_runner_3stream_graph_fusion_smoke() {
        // graph_weight > 0 で 3-stream 経路を indexing + search する smoke。
        // 1bit retrieval 数値は assert しない (SimpleEmbedder hash 経路は不安定)。
        let entry = entry_single_session_user("q-001", 7, 53);
        let cfg = BenchConfig {
            limit: None,
            k_values: vec![5, 10, 20],
            top_k_retrieve: 20,
            progress_every: 9999,
            graph_weight: 0.25,
        };
        let report = run_benchmark(&[entry], &cfg).unwrap();
        assert_eq!(report.processed, 1);
        assert_eq!(report.overall.n, 1);
        // graph 経路が crash せず top-K を返却することを確認
        assert!(report.overall.recall_at_k_sum.contains_key(&5));
    }

    #[test]
    fn test_per_question_type_aggregation() {
        let mut e1 = entry_single_session_user("q-001", 0, 5);
        e1.question_type = "single-session-user".to_string();
        let mut e2 = entry_single_session_user("q-002", 1, 5);
        e2.question_type = "multi-session".to_string();
        let cfg = BenchConfig {
            limit: None,
            k_values: vec![5],
            top_k_retrieve: 20,
            progress_every: 9999,
            graph_weight: 0.0,
        };
        let report = run_benchmark(&[e1, e2], &cfg).unwrap();
        assert_eq!(report.processed, 2);
        assert!(report.per_type.contains_key("single-session-user"));
        assert!(report.per_type.contains_key("multi-session"));
        assert_eq!(report.per_type["single-session-user"].n, 1);
        assert_eq!(report.per_type["multi-session"].n, 1);
    }
}
