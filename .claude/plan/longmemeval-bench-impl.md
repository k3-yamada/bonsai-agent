# LongMemEval-S 外部 Benchmark 移植 plan

## 起点 / Motivation

- **Lab 天井 7 連続確定** (Lab v8/v9/v10/v14/v15/v16/v17、p=0.5072) — 内部 core 22 + AgentFloor 30 task の retrieval 評価軸が**飽和**
- `rohitg00/agentmemory` (TypeScript、6,554★、Apache-2.0) が LongMemEval-S で **R@5=95.2% / R@10=98.6% / MRR=88.2%** を達成 (mem0 68.5% / Letta 83.2% を凌駕)
- bonsai retrieval stack (`HybridSearch`: FTS5 + vec0 + RRF α=0.5 k=60、`KnowledgeGraph::bfs`、Cerememory 三本柱) を**外部 well-known benchmark で初めて客観評価**
- agentmemory ベンチ実装は BM25×0.4 + Vector×0.6 の**加重線形和** (RRF ではない、誇大マーケ実態)、**bonsai 既存 RRF α=0.5 で同等以上は十分到達可能** と判断
- 期待値:
  - **超過時**: 「Cerememory 三本柱 + bonsai RRF は paper-grade」を実証
  - **下回り時**: 改善方向 (graph 軸合流による 3-stream RRF、query expansion、accessCount feedback) が**明示**

**Lab 軸とは別 dimension** で打開: Lab は内部 task の improvement、本 plan は外部 standard の retrieval quality 評価。

---

## Scope

### 含む

- LongMemEval-S dataset (HuggingFace `xiaowu0162/longmemeval-cleaned`、264 MB JSON、500 Q × ~53 sessions、MIT) ローダ
- `LongMemEvalEntry` struct (serde Deserialize)
- Per-question isolated `MemoryStore::in_memory()` index → `HybridSearch::search` → 集計 loop
- Metric: `recall_any@K` (K ∈ {5, 10, 20})、`NDCG@10`、`MRR`
- Per-question_type 集計 (single-session-user/assistant/preference、multi-session、temporal-reasoning、knowledge-update)
- CLI binary: `cargo run --bin longmemeval-bench -- [--limit N] [--dataset-path PATH]`
- JSON report + console summary 出力
- **production code 変更ゼロ** (新規 binary + 既存 `HybridSearch` 呼出のみ)

### 含まない

- dataset commit (264 MB なので `.gitignore` + env で path 指定)
- M / oracle split 対応 (S のみ、後 plan で extend)
- agentmemory との実機並走比較 (numbers のみ静的比較)
- ACCEPT 判定 (informational only、Lab gate なし)

---

## ファイル構成

```
src/bin/longmemeval_bench.rs            ← 新規 binary entry
src/eval/longmemeval/mod.rs             ← 新規 module
src/eval/longmemeval/dataset.rs         ← LongMemEvalEntry + loader
src/eval/longmemeval/metrics.rs         ← recall_any@K / NDCG / MRR
src/eval/longmemeval/runner.rs          ← per-question index + query loop
src/eval/mod.rs                          ← 既存 (新規 longmemeval 追加)
Cargo.toml                              ← `[[bin]] name = "longmemeval-bench"` 追加
```

---

## Phase 1 Red (TDD strict)

### Tests 追加

1. `test_dataset_parse_single_entry` — fixture 1 entry の JSON parse → schema field 全 populate
2. `test_dataset_parse_empty_array` — `[]` → empty Vec
3. `test_recall_any_at_k_hit_at_top` — `retrieved=[gold]`、`k=5` → 1.0
4. `test_recall_any_at_k_miss` — `retrieved=[other]`、`k=5` → 0.0
5. `test_recall_any_at_k_hit_outside_k` — `retrieved=[..10 others, gold]`、`k=5` → 0.0、`k=20` → 1.0
6. `test_ndcg_at_10_perfect_ranking` — gold at rank 0 → 1.0
7. `test_ndcg_at_10_partial` — gold at rank 5 → 1/log₂(7) ≒ 0.356
8. `test_mrr_first_hit_rank_3` — gold at rank 2 (0-indexed) → 1/3
9. `test_mrr_no_hit` — 0.0
10. `test_runner_indexes_53_sessions_returns_topk` — fixture 1 question × 53 dummy sessions → `HybridSearch::search` 呼出で top-K 取得
11. `test_per_question_type_aggregation` — 2 question (single-session-user / multi-session) → type 別 metric が分離集計

全部 `#[test]` で初期 `todo!()` 実装 → cargo test 走らせ 11 件 fail 確認。

---

## Phase 2 Green

### `dataset.rs`

```rust
#[derive(Debug, Deserialize)]
pub struct LongMemEvalEntry {
    pub question_id: String,
    pub question_type: String,
    pub question: String,
    pub question_date: String,
    pub answer: String,
    pub answer_session_ids: Vec<String>,
    pub haystack_dates: Vec<String>,
    pub haystack_session_ids: Vec<String>,
    pub haystack_sessions: Vec<Vec<HaystackTurn>>,
}

#[derive(Debug, Deserialize)]
pub struct HaystackTurn {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub has_answer: Option<bool>,
}

pub fn load_dataset(path: &Path) -> Result<Vec<LongMemEvalEntry>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let entries: Vec<LongMemEvalEntry> = serde_json::from_reader(reader)?;
    Ok(entries)
}
```

### `metrics.rs`

```rust
pub fn recall_any_at_k(retrieved: &[String], gold: &[String], k: usize) -> f64 {
    let top_k: HashSet<&String> = retrieved.iter().take(k).collect();
    if gold.iter().any(|g| top_k.contains(g)) { 1.0 } else { 0.0 }
}

pub fn ndcg_at_k(retrieved: &[String], gold: &[String], k: usize) -> f64 {
    let gold_set: HashSet<&String> = gold.iter().collect();
    let dcg: f64 = retrieved.iter().take(k).enumerate()
        .filter(|(_, id)| gold_set.contains(id))
        .map(|(i, _)| 1.0 / ((i as f64 + 2.0).log2()))
        .sum();
    let ideal_size = gold.len().min(k);
    let idcg: f64 = (0..ideal_size).map(|i| 1.0 / ((i as f64 + 2.0).log2())).sum();
    if idcg > 0.0 { dcg / idcg } else { 0.0 }
}

pub fn mrr(retrieved: &[String], gold: &[String]) -> f64 {
    let gold_set: HashSet<&String> = gold.iter().collect();
    retrieved.iter().position(|id| gold_set.contains(id))
        .map(|i| 1.0 / (i as f64 + 1.0))
        .unwrap_or(0.0)
}
```

### `runner.rs`

```rust
pub struct BenchConfig {
    pub limit: Option<usize>,
    pub k_values: Vec<usize>,  // [5, 10, 20]
    pub top_k_retrieve: usize, // 20 で十分
}

pub fn run_benchmark(entries: &[LongMemEvalEntry], cfg: &BenchConfig) -> BenchReport {
    let mut per_type: HashMap<String, TypeAgg> = HashMap::new();
    let mut overall = TypeAgg::default();

    for (idx, entry) in entries.iter().take(cfg.limit.unwrap_or(usize::MAX)).enumerate() {
        // Per-question isolated store
        let store = MemoryStore::in_memory()?;
        let embedder = SimpleEmbedder::default();  // or HashEmbedder

        // Index each session as 1 memory (1 session = 1 narrative)
        for (sess_idx, turns) in entry.haystack_sessions.iter().enumerate() {
            let sess_id = &entry.haystack_session_ids[sess_idx];
            let narrative = turns.iter()
                .map(|t| format!("{}: {}", t.role, t.content))
                .collect::<Vec<_>>().join("\n");
            // session_id を tag に格納
            store.save_memory(&narrative, "session", &[sess_id.clone()])?;
        }

        let search = HybridSearch::new(&store, &embedder);
        let results = search.search(&entry.question, cfg.top_k_retrieve)?;

        // retrieved session_ids を tag から復元
        let retrieved_ids: Vec<String> = results.iter()
            .filter_map(|r| r.memory.tags.first().cloned())
            .collect();

        // metric 計算
        let mut entry_metrics = EntryMetrics::default();
        for &k in &cfg.k_values {
            let r = recall_any_at_k(&retrieved_ids, &entry.answer_session_ids, k);
            entry_metrics.recall_at_k.insert(k, r);
        }
        entry_metrics.ndcg_at_10 = ndcg_at_k(&retrieved_ids, &entry.answer_session_ids, 10);
        entry_metrics.mrr = mrr(&retrieved_ids, &entry.answer_session_ids);

        // aggregate
        let agg = per_type.entry(entry.question_type.clone()).or_default();
        agg.absorb(&entry_metrics);
        overall.absorb(&entry_metrics);

        if (idx + 1) % 50 == 0 {
            eprintln!("[bench] {}/{}", idx + 1, entries.len());
        }
    }
    BenchReport { overall, per_type }
}
```

### `bin/longmemeval_bench.rs`

```rust
fn main() -> Result<()> {
    let args = Args::parse();
    let path = args.dataset_path
        .unwrap_or_else(|| dirs::cache_dir().unwrap()
            .join("bonsai-agent/longmemeval/longmemeval_s_cleaned.json"));
    let entries = load_dataset(&path)?;
    let cfg = BenchConfig {
        limit: args.limit,
        k_values: vec![5, 10, 20],
        top_k_retrieve: 20,
    };
    let report = run_benchmark(&entries, &cfg)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
```

### Cargo.toml

```toml
[[bin]]
name = "longmemeval-bench"
path = "src/bin/longmemeval_bench.rs"
```

11 test 全 PASS 確認。

---

## Phase 3 Refactor

- `EntryMetrics::absorb_into(&self, agg: &mut TypeAgg)` 抽出 → `TypeAgg::absorb` 重複削減
- `BenchReport::to_pretty_console` で per-type table 整形
- magic constant `top_k_retrieve = 20` → `BenchConfig::default()` 経由
- `progress_every = 50` env 化 (`BONSAI_LONGMEMEVAL_PROGRESS_INTERVAL`)
- clippy / fmt clean

---

## Phase 4 Smoke

### 4-1: 10 Q sample (~5 min wall)

```bash
# Dataset DL (one-time, manual)
mkdir -p ~/.cache/bonsai-agent/longmemeval
curl -L "https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/main/longmemeval_s_cleaned.json" \
  -o ~/.cache/bonsai-agent/longmemeval/longmemeval_s_cleaned.json
# Smoke: 10 Q only
cargo run --release --bin longmemeval-bench -- --limit 10
```

期待: 10 entries 処理完了、recall_any@5 が 0.0-1.0 範囲、JSON pretty print

### 4-2: 100 Q sample (~12 min wall)

```bash
cargo run --release --bin longmemeval-bench -- --limit 100
```

期待: per-type breakdown 6 種類分離、各 metric 計算済

### 4-3: 500 Q full run (~60 min wall)

```bash
cargo run --release --bin longmemeval-bench -- > /tmp/longmemeval-full.json
```

期待: 500 entries 処理、overall recall_any@5 ≥ 0.85 (agentmemory BM25-only baseline)、stretch goal ≥ 0.95

### Informational metric 観測 (no gate)

| Metric | agentmemory | bonsai 目標 |
|---|---|---|
| R@5 (overall) | 95.2% | ≥ 85% baseline / ≥ 92% stretch |
| R@10 | 98.6% | ≥ 92% |
| R@20 | 99.4% | ≥ 96% |
| NDCG@10 | 87.9% | ≥ 80% |
| MRR | 88.2% | ≥ 80% |

**ACCEPT 判定なし** (informational)。下回りでも改善方向 (graph 軸合流 / query expansion / accessCount) が明示されるので **失敗しない設計**。

---

## Phase 5 Verify & Commit

### Verify

- `cargo test --release` 全 PASS、回帰ゼロ
- `cargo clippy -- -D warnings` clean
- `cargo fmt -- --check` clean
- production code 変更ゼロ (新規 src/eval/longmemeval/ + src/bin/ のみ追加)

### Commit 構成 (TDD 5 phase に沿う)

1. `test(longmemeval): Phase 1 Red — schema + metric formula + runner skeleton (11 tests fail)`
2. `feat(longmemeval): Phase 2 Green — dataset loader + recall_any/NDCG/MRR + per-question runner`
3. `refactor(longmemeval): TypeAgg::absorb 抽出 + env progress interval`
4. `test(longmemeval): Phase 4 Smoke — 100 Q sample report 付与`
5. `docs(claude-md): 項目 227 LongMemEval-S 移植完遂 + 500 Q baseline 数値`

### CLAUDE.md 項目 227 (commit 5 で追加)

```
227. **LongMemEval-S 外部 benchmark 移植完遂 (★★★ Lab 天井 7 連続打破第 4 軸)** — `.claude/plan/longmemeval-bench-impl.md` TDD strict 5 phase: 500 Q × ~53 sessions × MIT (HuggingFace `xiaowu0162/longmemeval-cleaned`、264 MB)、bonsai HybridSearch (RRF k=60 α=0.5) で R@5=X.XXX / R@10=X.XXX / MRR=X.XXX (agentmemory 95.2%/98.6%/88.2% 比 +/-X.X) 計測、production code 変更ゼロ・1190→1201 passed (+11)、informational only no-gate、外部 well-known benchmark での bonsai retrieval stack 初評価
```

---

## リスク & 緩和

| Risk | 緩和 |
|---|---|
| Dataset DL の network 必要 | smoke で fixture 10 entry を `tests/fixtures/longmemeval_sample.json` に commit、CI 互換 |
| 264 MB JSON が serde_json で OOM | `serde_json::Deserializer::from_reader().into_iter()` で stream parse fallback (Phase 3 で実装) |
| 500 Q × 53 session embed で long run | release build + `--limit` flag で incremental、`#[ignore]` で CI から除外 |
| SimpleEmbedder の sparse vector が semantic に弱い | `HashEmbedder` or `cargo run --features embeddings` で MiniLM 等の実 embedder 検討 (defer to follow-up plan) |
| session_id を tag 第一要素で復元する設計が脆い | dedicated `metadata` JSON column 案も検討、Phase 3 で評価 |

---

## ACCEPT 判定なし (informational metric)

本 plan は Lab v17 などの paired t-test ACCEPT 判定**を持たない**。500 Q baseline 数値の取得自体が成果。

Follow-up plan で:
- **3-stream RRF 拡張**: `KnowledgeGraph::bfs` を 3rd stream に合流 (agentmemory の名目主張を bonsai が**実装で実現**) → Lab v19 でも paired 評価
- **Query expansion**: LLM-rewrite で recall +5-10pp 期待 → Lab effectiveness 検証
- **AccessCount feedback**: ReviewState に `access_count` 列追加 → retrieval hit で Strength boost

---

## 実装コスト

- Phase 1 Red: ~1h (11 tests)
- Phase 2 Green: ~2.5h (loader + metrics + runner)
- Phase 3 Refactor: ~30 min
- Phase 4 Smoke: ~1.5h (10/100/500 Q 実機 + report 検証)
- Phase 5 Verify & Commit: ~1h

**Total ~6.5h** (見積もり ~6-8h と一致)

---

## 並行性 / 依存

- Lab v18 (G1 Critic effectiveness、wall ~22-23h) と**完全並行可能** (本 plan は production code 変更ゼロ、Lab 結果に依存しない)
- Crystallize plan (`.claude/plan/crystallize-action-digest-impl.md`) とも独立、並列実装可
