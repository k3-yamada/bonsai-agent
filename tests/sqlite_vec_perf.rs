//! Phase 4 G-4.1 + G-4.4 synthetic perf benchmark for sqlite-vec activation.
//!
//! Run:
//!   cargo test --release --features embeddings --test sqlite_vec_perf -- --ignored --nocapture
//!
//! Gates (handoff 05-09g、plan §4):
//!   G-4.1: vec0 p50 ≦ linear p50 / 3 (3x 高速化)
//!   G-4.4: 観測値のみ (gate なし、規模感の参考データ)
//!
//! `--ignored` 必須 (CI に毎回走らせない、CLAUDE.md 「`#[ignore]` — 実サーバー/ネットワーク必要」と同方針で重い perf も opt-in)。

#![cfg(feature = "embeddings")]

use bonsai_agent::memory::store::MemoryStore;
use bonsai_agent::runtime::embedder::{
    DEFAULT_EMBEDDING_DIM, Embedder, SimpleEmbedder, cosine_similarity,
};
use std::time::Instant;

const DIM: usize = DEFAULT_EMBEDDING_DIM; // 256

/// 決定論的擬似乱数 (Numerical Recipes LCG) で `dim` 次元 L2 正規化ベクトルを生成。
/// seed 固定で再現性確保 (1bit 環境のばらつき分析と切り分けるため)。
fn pseudo_random_vec(seed: u64, dim: usize) -> Vec<f32> {
    let mut state = seed
        .wrapping_mul(2_862_933_555_777_941_757)
        .wrapping_add(3_037_000_493);
    let mut v = Vec::with_capacity(dim);
    for _ in 0..dim {
        state = state
            .wrapping_mul(2_862_933_555_777_941_757)
            .wrapping_add(3_037_000_493);
        let f = (state >> 11) as f32 / (1u64 << 53) as f32;
        v.push(f.mul_add(2.0, -1.0));
    }
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
    v
}

fn percentile(sorted_us: &[u128], p: f64) -> u128 {
    if sorted_us.is_empty() {
        return 0;
    }
    let idx = ((sorted_us.len() as f64 - 1.0) * p).round() as usize;
    sorted_us[idx]
}

#[test]
#[ignore = "G-4.1: synthetic perf benchmark (1000 vec × 100 query)"]
fn vec_perf_synthetic_g41_linear_vs_vec0() {
    const N_MEMORIES: usize = 1000;
    const N_QUERIES: usize = 100;
    const TOP_K: usize = 10;

    let store = MemoryStore::in_memory().expect("create in-memory store");

    let mut embeddings: Vec<(i64, Vec<f32>)> = Vec::with_capacity(N_MEMORIES);
    for i in 0..N_MEMORIES {
        let id = store
            .save_memory(&format!("synthetic memory {i}"), "perf", &[])
            .expect("save_memory");
        let v = pseudo_random_vec(i as u64 + 1, DIM);
        store
            .insert_memory_embedding(id, &v)
            .expect("insert_memory_embedding");
        embeddings.push((id, v));
    }
    eprintln!("[G-4.1] inserted {N_MEMORIES} memories + 256d embeddings");

    let queries: Vec<Vec<f32>> = (0..N_QUERIES)
        .map(|q| pseudo_random_vec(0xCAFE_0000 + q as u64, DIM))
        .collect();

    // 比較 A: linear_optimal — 事前計算済 embedding に対する theoretical best
    //   (algorithm 純粋比較、vec0 SQL overhead vs Rust HashMap scan)
    let mut linear_opt_us: Vec<u128> = Vec::with_capacity(N_QUERIES);
    for q in &queries {
        let t0 = Instant::now();
        let mut scored: Vec<(i64, f32)> = embeddings
            .iter()
            .map(|(id, v)| (*id, cosine_similarity(q, v)))
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(TOP_K);
        let elapsed = t0.elapsed().as_micros();
        std::hint::black_box(scored);
        linear_opt_us.push(elapsed);
    }
    linear_opt_us.sort_unstable();

    // 比較 B: linear_realistic — 各 memory を query 時に re-embed (production
    //   HybridSearch::vector_search non-embeddings path 等価、search.rs:108)。
    //   SimpleEmbedder hash embed 1000 回 + cosine + sort で N=1000 では支配項。
    let realistic_embedder = SimpleEmbedder::default();
    let memory_contents: Vec<(i64, String)> = (0..N_MEMORIES)
        .map(|i| (i as i64 + 1, format!("synthetic memory {i}")))
        .collect();
    let mut linear_real_us: Vec<u128> = Vec::with_capacity(N_QUERIES);
    for q in &queries {
        let t0 = Instant::now();
        let mut scored: Vec<(i64, f32)> = memory_contents
            .iter()
            .map(|(id, content)| {
                let v = realistic_embedder
                    .embed(&[content.as_str()])
                    .unwrap_or_default();
                let sim = if v.is_empty() {
                    0.0
                } else {
                    cosine_similarity(q, &v[0])
                };
                (*id, sim)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(TOP_K);
        let elapsed = t0.elapsed().as_micros();
        std::hint::black_box(scored);
        linear_real_us.push(elapsed);
    }
    linear_real_us.sort_unstable();

    let mut vec0_us: Vec<u128> = Vec::with_capacity(N_QUERIES);
    for q in &queries {
        let t0 = Instant::now();
        let res = store.vec_knn(q, TOP_K).expect("vec_knn");
        let elapsed = t0.elapsed().as_micros();
        std::hint::black_box(res);
        vec0_us.push(elapsed);
    }
    vec0_us.sort_unstable();

    let lin_opt_p50 = percentile(&linear_opt_us, 0.50);
    let lin_opt_p99 = percentile(&linear_opt_us, 0.99);
    let lin_real_p50 = percentile(&linear_real_us, 0.50);
    let lin_real_p99 = percentile(&linear_real_us, 0.99);
    let vec_p50 = percentile(&vec0_us, 0.50);
    let vec_p99 = percentile(&vec0_us, 0.99);

    eprintln!(
        "[G-4.1] linear_optimal     p50={lin_opt_p50:>7}us  p99={lin_opt_p99:>7}us  (precomputed)"
    );
    eprintln!(
        "[G-4.1] linear_realistic   p50={lin_real_p50:>7}us  p99={lin_real_p99:>7}us  (embed-at-query, production-equivalent)"
    );
    eprintln!(
        "[G-4.1] vec0               p50={vec_p50:>7}us  p99={vec_p99:>7}us  (sqlite-vec KNN)"
    );
    let ratio_opt = lin_opt_p50 as f64 / vec_p50.max(1) as f64;
    let ratio_real = lin_real_p50 as f64 / vec_p50.max(1) as f64;
    eprintln!("[G-4.1] speedup vs linear_optimal    p50 = {ratio_opt:.2}x");
    eprintln!("[G-4.1] speedup vs linear_realistic  p50 = {ratio_real:.2}x  (gate: ≥3.0x)");

    // gate は production-equivalent (linear_realistic) で判定。
    // linear_optimal vs vec0 は algorithm-only 参考値 (vec0 SQL overhead 可視化)。
    if vec_p50 * 3 > lin_real_p50 {
        eprintln!(
            "[G-4.1] WARN gate fail vs realistic: vec0 p50 ({vec_p50}us) > linear_realistic p50 / 3 ({}us)",
            lin_real_p50 / 3
        );
    } else {
        eprintln!("[G-4.1] PASS gate: vec0 ≤ linear_realistic / 3");
    }
}

#[test]
#[ignore = "G-4.4: backfill timing for 10K memories"]
fn vec_perf_synthetic_g44_backfill_10k() {
    const N_MEMORIES: usize = 10_000;

    let store = MemoryStore::in_memory().expect("create in-memory store");
    let embedder = SimpleEmbedder::default();

    let t_insert = Instant::now();
    for i in 0..N_MEMORIES {
        store
            .save_memory(&format!("backfill memory {i} content"), "perf", &[])
            .expect("save_memory");
    }
    let insert_ms = t_insert.elapsed().as_millis();
    eprintln!("[G-4.4] inserted {N_MEMORIES} memories in {insert_ms}ms");

    // ensure_vec_table は idempotent (count > 0 で skip)、in_memory store は
    // V13 で空 vec_memories が作成済 → 上記 save_memory ループは vec_memories
    // を populate しないので、ここでの ensure_vec_table 呼出が「初回 backfill」。
    let t_backfill = Instant::now();
    store.ensure_vec_table(&embedder).expect("ensure_vec_table");
    let backfill_ms = t_backfill.elapsed().as_millis();
    eprintln!("[G-4.4] ensure_vec_table backfill {N_MEMORIES} rows in {backfill_ms}ms");
    eprintln!(
        "[G-4.4] avg per-row: {:.3}ms",
        backfill_ms as f64 / N_MEMORIES as f64
    );

    // R-A2 軽減確認: 5 min 超で別 plan (lazy or background backfill) 起票判断。
    if backfill_ms > 300_000 {
        eprintln!("[G-4.4] WARN backfill > 5 min, R-A2 mitigation 別 plan 検討");
    }
}

/// Plan A1+A3 G-2.4: G-4.5 recall@k の k-parametric helper。
///
/// 戻り値: (avg_recall, zero_recall_queries)。
/// - vec0 brute-force exact KNN (HNSW ではない) なので recall は理論上 linear と一致
/// - 50 query × top-k で intersect 計上、tie-break ばらつき許容
fn run_recall_at_k(k: usize, n_memories: usize) -> (f64, usize) {
    const N_QUERIES: usize = 50;

    let store = MemoryStore::in_memory().expect("create in-memory store");

    let mut embeddings: Vec<(i64, Vec<f32>)> = Vec::with_capacity(n_memories);
    for i in 0..n_memories {
        let id = store
            .save_memory(&format!("recall memory {i}"), "perf", &[])
            .expect("save_memory");
        let v = pseudo_random_vec(i as u64 + 1, DIM);
        store
            .insert_memory_embedding(id, &v)
            .expect("insert_memory_embedding");
        embeddings.push((id, v));
    }

    let queries: Vec<Vec<f32>> = (0..N_QUERIES)
        .map(|q| pseudo_random_vec(0xBEEF_0000 + q as u64, DIM))
        .collect();

    let mut total_intersect = 0usize;
    let mut zero_recall_queries = 0usize;
    for q in &queries {
        // linear ground truth (cosine similarity 上位 k)
        let mut linear_scored: Vec<(i64, f32)> = embeddings
            .iter()
            .map(|(id, v)| (*id, cosine_similarity(q, v)))
            .collect();
        linear_scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let linear_top: std::collections::HashSet<i64> =
            linear_scored.iter().take(k).map(|(id, _)| *id).collect();

        // vec0 candidate (distance 昇順 = similarity 降順)
        let vec_scored = store.vec_knn(q, k).expect("vec_knn");
        let vec_top: std::collections::HashSet<i64> =
            vec_scored.iter().map(|(id, _)| *id).collect();

        let intersect = linear_top.intersection(&vec_top).count();
        total_intersect += intersect;
        if intersect == 0 {
            zero_recall_queries += 1;
        }
    }

    let avg_recall = total_intersect as f64 / (N_QUERIES * k) as f64;
    (avg_recall, zero_recall_queries)
}

#[test]
#[ignore = "G-4.5 (Step B 第 1 軸): recall@10 of vec0 vs linear (exact KNN parity)"]
fn vec_perf_synthetic_g45_recall_at_10() {
    // sqlite-vec vec0 は brute-force exact KNN (HNSW ではない) のため、recall@10
    // は理論上 linear scan と完全一致する。本 test は plan §8 Step B 判定の
    // 第 1 軸 (recall@10 axis) を実機で検証する。
    //
    // PASS 条件 (Step B 不要):  recall@10 ≥ 95% (tie-break ばらつき許容)
    // FAIL 条件 (Step B 検討):  recall@10 < 95% → ANN 採用検討
    const N_MEMORIES: usize = 1000;
    const TOP_K: usize = 10;
    const N_QUERIES: usize = 50;

    let (avg_recall, zero_recall_queries) = run_recall_at_k(TOP_K, N_MEMORIES);

    eprintln!(
        "[G-4.5] avg recall@10 = {avg_recall:.4} (over {N_QUERIES} queries, {N_MEMORIES} memories)"
    );
    eprintln!("[G-4.5] zero-recall queries: {zero_recall_queries}/{N_QUERIES}");

    // gate: vec0 と linear が exact KNN であれば recall ≥ 95% (tie-break 余地)
    if avg_recall >= 0.95 {
        eprintln!("[G-4.5] PASS Step B axis-1: vec0 recall@10 ≥ 95%, ANN 不要");
    } else {
        eprintln!(
            "[G-4.5] WARN Step B axis-1 fail: vec0 recall@10 ({avg_recall:.4}) < 95% → Milvus Lite 検討"
        );
    }
}

// --- Plan A1+A3 (`.claude/plan/sqlite-vec-a1-a3-impl.md`) Phase 1 Red T-1.8 ---
// 既存 G-4.5 の k=10 限定測定を k>10 に拡張する informational test (gate なし)。
// `run_recall_at_k(k, n_memories)` helper は Phase 1 では **未定義** のため
// compile error で Red 確証 (Phase 2 Green G-2.4 で helper 抽出)。

#[test]
#[ignore = "G-4.5 extended k=20 (informational、plan A3 D-4)"]
fn vec_perf_synthetic_g45_recall_at_20() {
    let (avg_recall, zero_recall_queries) = run_recall_at_k(20, 1000);
    eprintln!(
        "[G-4.5 k=20] avg recall@20 = {avg_recall:.4}, zero-recall queries: {zero_recall_queries}/50"
    );
    // gate なし、informational only (vec0 brute-force exact KNN なので 1.0 期待)
}

#[test]
#[ignore = "G-4.5 extended k=50 (informational、plan A3 D-4)"]
fn vec_perf_synthetic_g45_recall_at_50() {
    let (avg_recall, zero_recall_queries) = run_recall_at_k(50, 1000);
    eprintln!(
        "[G-4.5 k=50] avg recall@50 = {avg_recall:.4}, zero-recall queries: {zero_recall_queries}/50"
    );
    // gate なし、informational only
}
