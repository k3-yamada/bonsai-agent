# Frontier-Based Benchmark for Bonsai (antirez/ds4 inspired)

**状態**: planning-only (未起票)、推奨度 ★★★ (Lab 天井 7 連続打破候補 = 第 6 軸 = context-length axis)
**推定工数**: ~6h (TDD strict 5 phase、SCHEMA V16 migration 必要)
**起点**: antirez/ds4 `ds4-bench` 思想 + handoff §227 LongMemEval 移植完遂

## §1. 背景 — 第 6 軸 = context-length axis の必要性

### 現状 Bonsai の評価軸 (5 軸)
1. **Score 軸**: `pass@k` / `pass_consecutive_k` (項目 11) — overall mean
2. **Capability 軸**: AgentFloor 6-tier (項目 223/224) — 能力プロファイル
3. **Efficiency 軸**: `PASS@(k,T)` T_steps / T_seconds (項目 225) — 試行コスト
4. **Stability 軸**: RDC/VAF/GDS (項目 200) — 信頼性
5. **Retrieval 軸**: LongMemEval R@K / NDCG / MRR (項目 227) — 記憶検索

**Lab 天井 7 連続 (v8/v9/v10/v14/v15/v16/v17)** で打破できなかった理由 = 上記 5 軸はすべて
**入力 context 長を fixed と仮定** している。実際の Bonsai-8B は長文入力で劇的に劣化する可能性
があるが、現行 benchmark では detect できない (項目 187 ContextOverflowGuard が補正している
だけで定量化されていない)。

### antirez/ds4 `ds4-bench` の知見
- **whole-run 平均ではなく context frontier 毎の incremental throughput を測定**
- 2048 / 4096 / 6144 / 8192 ... の境界で `prefill_tokens/s` / `generation_tokens/s` / `kvcache_bytes` を CSV 出力
- 「次の frontier に到達するまでの差分」を測ることで、長文での性能劣化曲線が描ける
- `KV snapshot save → fixed greedy probe → snapshot restore` で context state を保持しながら frontier 毎に probe

### Bonsai への応用
ds4 は **推論 throughput** を測るが、Bonsai では **score 品質** を context 長で bucket 化する:
- task 実行終了時の累積 token count を frontier bucket に振り分け
- bucket 毎の mean score / pass@k / fail rate を report
- 長文 → score 劣化曲線が見えれば、第 6 軸 = context-length axis 確立

## §2. 設計 (3 案、推奨 = 案 C)

### 案 A: Fixed-context probe injection (ds4 直訳)
各 task に対し 2K/4K/8K/16K の filler context を inject して 4 回実行 → 4 score の degradation 曲線。
- ✅ ds4 思想に忠実
- ❌ 4x 実行コスト = Lab cycle が 4x、現実的でない (~3-4 day/cycle)
- ❌ filler の合成方法 (random / repetitive / coherent) で結果が変動

### 案 B: Per-task post-hoc bucketing
各 task 実行終了時の累積 context token count を計測 → bucket {[0,2K), [2K,4K), [4K,8K), [8K,16K)+} に振り分けて mean score を report。
- ✅ Lab cycle 増加なし
- ✅ 既存 22 task / k=3 の 66 runs で各 bucket に十分なサンプル
- ❌ task 自体が context 長を決めるので bucket 偏り発生
- ❌ task 間で本質的に異なる難易度が context 長と confound する

### 案 C (推奨): Task-aware frontier injection + bucketing 併用
1. **Frontier injection** = AgentFloor 30 task のうち T6-LongHorizon を選んで、`<filler_context>` block size を {0, 4K, 8K, 16K} で 4 variant 化 → 4 score
2. **Post-hoc bucketing** = 既存 22 task / k=3 を 4 bucket に振り分け (mean score reporting のみ)
3. **2 種の frontier metric** (`frontier_inject_*` + `frontier_bucket_*`) を独立 TSV column に persist

- ✅ inject は控えめ (T6 4 task × 4 size = 16 runs 増、cycle ~+15%)
- ✅ bucketing は既存 runs に重畳でゼロコスト
- ✅ 2 metric の trianglulation で confound を相互打ち消し可能
- ❌ 実装複雑度がやや上がる (T6 task に `<filler_context>` slot 追加)

## §3. TDD strict 5 phase 計画

### Phase 1 (Red) — 失敗 test 8 件
- `t_frontier_bucket_assignment_correct` (0..2K → bucket 0, 2K..4K → 1, etc)
- `t_frontier_bucket_empty_when_disabled` (env unset / cfg default で None)
- `t_frontier_inject_filler_block_grows_with_size` (4K filler vs 0K で token count 差確認)
- `t_frontier_inject_t6_only` (T1-T5 は inject 対象外)
- `t_frontier_metrics_populate_from_results` (`MultiRunBenchmarkResult::frontier_*` field)
- `t_frontier_persist_v16_schema` (SCHEMA_V15 → V16 migration、5 TEXT 列 JSON encode)
- `t_frontier_tsv_columns_extended` (TSV 23 → 28 列、frontier 5 fields 追加)
- `t_frontier_backward_compat_v15_json` (旧 JSON で frontier 空、`#[serde(default)]`)

### Phase 2 (Green) — 実装
- `src/agent/benchmark.rs`: `frontier_bucket_for(token_count: usize) -> Option<usize>` helper
- `src/agent/benchmark.rs`: `compute_frontier_bucket_scores(...) -> BTreeMap<usize, f64>` aggregator
- `src/agent/benchmark.rs`: `inject_filler_context(task: &mut BenchmarkTask, size_kb: usize)` helper (T6 のみ)
- `src/agent/experiment.rs`: `Experiment` に 2 Vec field 追加 + `from_multi_results` で populate
- `src/agent/experiment.rs`: SCHEMA_V16 migration (`frontier_inject_scores` TEXT / `frontier_bucket_scores` TEXT)
- `src/agent/experiment.rs`: `save_to_db` / `recent_experiments` SQL 21→23 列 + JSON encode/decode
- env: `BONSAI_FRONTIER_ENABLED=1` opt-in (default OFF / Cerememory 三本柱 pattern)
- env: `BONSAI_FRONTIER_BUCKETS=2048,4096,8192,16384` (default、カスタム可)
- env: `BONSAI_FRONTIER_INJECT_SIZES_KB=0,4,8,16` (T6 inject variant、default OFF で 0 のみ)

### Phase 3 (Refactor)
- `frontier_bucket_for` を `pure fn` として `utils/frontier.rs` に切出 (項目 200 RDC/VAF helper と同居)
- `inject_filler_context` の filler 生成は coherent (gutenberg snippet) より repetitive (deterministic seed) を採用、re-run reproducibility 優先

### Phase 4 (Smoke)
- G-4a (env unset = default OFF): wall ≈ baseline、frontier_* fields = empty、既存挙動互換確証
- G-4b (LADDER + FRONTIER + smoke=1): T6 4 task × 4 size = 16 inject runs 確認、frontier_inject_scores populate
- G-4c (full cycle、LADDER + FRONTIER + bucketing): 既存 22 task / k=3 = 66 runs を 4 bucket 振り分け、`[INFO][lab.frontier]` log emit 確認

### Phase 5 (Effectiveness — 別 plan)
- Lab v19 paired t-test で `BONSAI_FRONTIER_ENABLED` ON/OFF 5 paired cycle
- ACCEPT 基準: bucket [8K, 16K)+ で OFF baseline 比 score variance 拡大 (= 長文劣化検出能力獲得)
- Lab 天井 7 連続打破の **第 6 軸 baseline 確立** が成果 (本 plan の主成果)

## §4. ds4 直接転用しない判断

### ds4-bench の `ds4-bench` 自体は転用しない
- ds4 は **インプロセス Metal/CUDA graph に直接アクセス** して KV snapshot save/restore が可能
- Bonsai は llama-server HTTP API 経由 (項目 167)、KV snapshot 直接制御は llama-server 側責務
- Bonsai 側で context frontier 単位の KV reuse は項目 81 で部分的に実装済 (compaction.rs)

### 転用する思想 = "frontier 毎の incremental metric"
- 全 run 平均は context-length axis を平滑化してしまう
- Bonsai では score を frontier bucket 化する形で「discrete frontier 軸」を導入

## §5. 期待効果 (仮説、Phase 5 で検証)

| 仮説 | 反証条件 (Phase 5) |
|---|---|
| H1: Bonsai-8B は 8K+ context で score 劣化する | bucket [8K, 16K)+ mean score が [0, 2K) と差 <0.02 |
| H2: 第 6 軸 baseline で Lab 天井打破候補が見える | 6 軸どこでも他軸顕著優位の新規 variant 0 件 |
| H3: T6-LongHorizon タスクは inject 0K → 16K で線形劣化 | inject 16K score / 0K score が 0.8 未満 |

H1 反証なら Bonsai-8B は context 長に対し robust と確証、scaffolding 不要が示唆される
(項目 117 と整合)。H1 成立なら長文 compaction の優先度が上がる (項目 187 ContextOverflowGuard
を再評価)。

## §6. 起票候補項目

- **項目 229** = 本 plan の Phase 1-3 完遂 + Phase 4 G-4a/b/c smoke
- **項目 230** (将来) = Lab v19 paired t-test ACCEPT/REJECT 判定

## §7. 依存 / 順序

- 項目 223 AgentFloor 6-tier (済) — T6 task identification の前提
- 項目 227 LongMemEval 移植 (済) — 第 5 軸先例として API additive 流儀
- 項目 228 (本 session 完遂候補) 3-stream RRF — n=500 ACCEPT/REJECT 後に着手 (frontier と直交)

## §8. 不要転用 (rejected)

- ds4 directional steering — llama-server backend で activation 編集不可
- ds4 disk KV cache — llama-server 側責務、Bonsai 既存 compaction.rs で代替
- ds4 tool ID radix tree replay — session-local Bonsai では過剰、parse robustness で代替
