# sqlite-vec Activation Plan (Step A)

**作成日**: 2026-05-09
**親 plan**: `.claude/plan/external-memory-oss-integration-judgment.md` §1.2 Step A
**目的**: 線形 scan vector_search を sqlite-vec vec0 virtual table に置換し、ANN 検索を実現
**前提**: bonsai 1143 lib tests passing、Cerememory 三本柱完遂、production code 変更ゼロ (本 plan は plan のみ)
**TDD strict 5 phase + Phase 0 architecture decision**

---

## 0. Architecture Decisions (Phase 0、Phase 1 着手前必須)

### D-1: 埋め込み配置戦略
| 案 | 内容 | trade-off |
|---|---|---|
| A1 | `memories.embedding BLOB` 列追加 (V13)、insert/update 時に embed | schema migration 必要、vec0 と二重保持 |
| **A2 (採用)** | vec0 を **shadow table** として独立 (`vec_memories(memory_id, embedding)`) | memories table 不変、JOIN cost、別管理 |
| A3 | vec0 を primary、memories の content から都度 query 時に embed | 現状と同じ、却下 |

**理由**: A2 は memories schema 影響ゼロ、vec0 は orthogonal 拡張、SQLite JOIN は安価。

### D-2: feature flag 設計
- **default build (`cargo build`)**: hash-based `SimpleEmbedder` のみ、`HybridSearch::vector_search` は **線形 scan path を compile** (test/CI 用、semantic 意味は持たない)
- **embeddings build (`cargo build --features embeddings`)**: `fastembed` + `sqlite-vec` 両方を compile、`vector_search` は **vec0 KNN path のみ compile**
- **方式**: 2 path は `#[cfg(feature = "embeddings")]` による **コンパイル時排他選択** (= Rust 標準 conditional compilation、ランタイム分岐なし)
- **bundle**: `Cargo.toml` で `embeddings = ["dep:fastembed", "dep:sqlite-vec"]` (feature 1 つで両方 on)

**rationale**: hash mode で sqlite-vec を使っても hash 値を indexing するだけで benefit ゼロ → コンパイル時に path を分離するのが正しい設計。production deploy では `--features embeddings` が前提 (README 要追記)。

### D-3: sqlite-vec version
- **採用**: **`sqlite-vec = "0.1.9"`** (2026-03-31 stable、`Cargo.toml:65` の `"0.1"` を 0.1.9 に明示固定)
- alpha 0.1.10-alpha.x は不採用 (stable 範囲のみ)
- build: cc crate 経由で C source を static link、system dep なし

### D-4: 次元統一
- 現状: EmbeddingGemma 768d Matryoshka→256d、AllMiniLML6V2 384d→256d (両 embedder とも 256d 出力に統一済)
- vec0 schema: `vec0(memory_id INTEGER PRIMARY KEY, embedding float[256])`
- 別次元への変更は schema V14 別 plan で扱う

### D-5: backfill 戦略 (eager 採用)
- `MemoryStore::ensure_vec_table()` 初回呼出時に **既存 memories を一括 embed + insert** (eager)
- 進捗 log: 100 row ごと
- 想定規模: bonsai 開発環境で典型 <10K row、M2 16GB で 10K × 256d embed ~ 数分以内
- 大規模 DB (>100K row) 対応は別 plan (本 plan 範囲外、必要性が確認されてから検討)
- Phase 4 smoke で実 DB の backfill 時間を計測

---

## 1. Phase 1 Red (失敗テスト先行、~1.5h)

> 以下 T-1.1 〜 T-1.7 はすべて `#[cfg(feature = "embeddings")]` 配下で記述。default build では vec0 path を compile しないので test 自体が compile されない。

### T-1.1 vec0 virtual table 存在確認
- `MemoryStore::ensure_vec_table()` 呼出後、`SELECT name FROM sqlite_master WHERE name='vec_memories'` で 1 row 返ること
- Red 期待: 関数未定義 = compile error

### T-1.2 insert_memory_embedding でエンベディング保存
- 256d Vec<f32> を渡して insert → `SELECT memory_id FROM vec_memories WHERE memory_id=?` で 1 row 返ること
- Red 期待: メソッド未定義 = compile error

### T-1.3 vec0 KNN クエリ
- 5 件 insert 後、query embedding に対し top-3 を vec0 distance order で返すこと
- Red 期待: `MemoryStore::vec_knn` 未定義 = compile error

### T-1.4 schema V13 migration
- V12 → V13 で `vec_memories` virtual table が作成、`SCHEMA_VERSION=13`
- Red 期待: migration 未追加

### T-1.5 eager backfill
- ensure_vec_table 呼出前に memories を 3 件 insert → ensure_vec_table 実行 → vec_memories に 3 row 存在
- Red 期待: backfill ロジック未実装

### T-1.6 caller compatibility
- `HybridSearch::search` の signature 不変、existing 6 test (search.rs:171-214) + 1 production caller (context_inject.rs:274) が無修正で pass
- 既存 test は default build (linear scan path) で実行されるため signature 変更だけ Red、実装変更は Green で

### T-1.7 dimension validation
- 256d 以外 (例: 128d) を渡したら Err 返却
- Red 期待: 検証ロジック未実装

---

## 2. Phase 2 Green (~3h)

### Step G-2.1: Cargo.toml
```toml
[dependencies]
sqlite-vec = { version = "0.1.9", optional = true }

[features]
embeddings = ["dep:fastembed", "dep:sqlite-vec"]
```

### Step G-2.2: schema.rs V13 migration
- `SCHEMA_V13_VEC_MEMORIES`: `CREATE VIRTUAL TABLE IF NOT EXISTS vec_memories USING vec0(memory_id INTEGER PRIMARY KEY, embedding float[256])`
- `SCHEMA_VERSION` 12→13、Migration list に V13 追加
- 注: vec0 virtual table は `embeddings` feature が必要なので、V13 migration も `#[cfg(feature = "embeddings")]` で gate

### Step G-2.3: store.rs (`#[cfg(feature = "embeddings")]` 配下)
```rust
pub fn ensure_vec_table(&self, embedder: &dyn Embedder) -> Result<()>
pub fn insert_memory_embedding(&self, memory_id: i64, embedding: &[f32]) -> Result<()>
pub fn vec_knn(&self, query: &[f32], limit: usize) -> Result<Vec<(i64, f32)>>
```
- `ensure_vec_table`: V13 migration 適用 + 既存 memories を全 embed + insert (eager backfill、進捗 log)
- 256d 検証: `embedding.len() != 256` で `bail!`

### Step G-2.4: search.rs `vector_search` 改修
```rust
#[cfg(feature = "embeddings")]
fn vector_search(&self, query_embedding: &[f32], limit: usize) -> Result<Vec<(MemoryRecord, f32)>> {
    let knn = self.store.vec_knn(query_embedding, limit)?;
    // memory_id batch fetch (IN clause で N+1 回避)
    let ids: Vec<i64> = knn.iter().map(|(id, _)| *id).collect();
    let memories = self.store.get_memories_by_ids(&ids)?;
    // distance score 付きで返却
    ...
}

#[cfg(not(feature = "embeddings"))]
fn vector_search(&self, query_embedding: &[f32], limit: usize) -> Result<Vec<(MemoryRecord, f32)>> {
    // 既存 linear scan を維持
    ...
}
```

### Step G-2.5: insert path 統合
- `HybridSearch::index_memory(record_id, content)`: 内部で `embedder.embed(&[content])` → `store.insert_memory_embedding(record_id, &emb[0])`
- 上位 caller (context_inject.rs ほか) の memory 投入箇所に hook
- `MemoryStore::insert_memory` 自体は変更しない (D-1 A2 = orthogonal 設計の維持)

---

## 3. Phase 3 Refactor (~1h)

- `vector_search` 両 path に docstring (vec0 path = production、linear path = test/CI build)
- `MemoryStore::get_memories_by_ids` を IN clause batch 実装 (N+1 回避)
- `Cargo.toml` の `# sqlite-vec = "0.1"` コメント行削除
- clippy 0 / fmt 0 維持

---

## 4. Phase 4 Smoke (~2h)

### G-4.1: 単体測定 (synthetic、`--features embeddings`)
- 1000 memories + 256d random vector を insert → 100 query で latency p50/p99 計測
- 比較: linear scan path (default build) vs vec0 path (embeddings build)
- **gate**: vec0 p50 が linear p50 の **1/3 以下** (3x 高速化以上)

### G-4.2: Lab core 22 cycle (実機、`--features embeddings`)
- `cargo run --release --features embeddings -- --lab --lab-experiments 0 --lab-tier core` 1 cycle (~15-20 min)
- **gate**: score が直近 baseline (handoff 05-09e の Cerememory 三本柱 ON 値) の **±0.02 以内**
- 退行 > 0.02 で REJECT、原因調査

### G-4.3: memory cost 計測
- `/usr/bin/time -l` で peak RSS を旧/新で比較
- **gate**: vec0 path の peak RSS 増加が **+200MB 以下** (M2 16GB 余裕内)

### G-4.4: backfill 時間計測
- 既存 開発 DB の memories 全件に対する `ensure_vec_table` 実行時間
- 観測値のみ、gate なし (規模感の参考データ)

---

## 5. Phase 5 Docs (~30 min)

- CLAUDE.md 項目 220 候補追加 (本 plan 完遂時)
- session handoff `session_2026_05_09f_handoff.md` (本 session ↔ 実装 session ブリッジ)
- README に `--features embeddings` 推奨を追記
- MEMORY.md index 不変

---

## 6. Risks

### R-A1: hash mode build で vector_search が semantic 意味を持たない
- **影響**: ユーザーが feature flag を理解せず default build を production に deploy
- **軽減**: D-2 で path を compile time 排他に分離、README + run-time log で `embeddings` feature 推奨を明示

### R-A2: eager backfill 時間が大規模 DB で許容外
- **影響**: 起動 hang
- **軽減**: G-4.4 で実 DB の時間計測、>5 min で別 plan (lazy or background backfill) を起票
- 想定規模 <10K row では問題化しない見込み

### R-A3: 既存 1143 test の linear path が壊れる
- **影響**: CI 全部赤
- **軽減**: T-1.6 で signature 不変を gate、Phase 1 Red で 6 既存 test pass を確認
- default build と embeddings build の **両方で test 実行**を Phase 2 G-A2 gate に追加

### R-A4: sqlite-vec 0.1.9 の C build 失敗 (M2/aarch64 互換性)
- **影響**: ビルド不可
- **軽減**: Phase 0 着手前に `cargo add sqlite-vec --features embeddings` 試行 build (~5 min)、失敗時は plan 一時 hold

### R-A5: vec0 backend の persistence 単位
- **影響**: virtual table の `.db` file 持続性確認なし
- **軽減**: T-1.4 migration test で V13 適用後も再 open で vec_memories が存在することを assert

---

## 7. Gates 一覧

| Gate ID | Phase | 内容 | PASS 条件 |
|---|---|---|---|
| G-A0 | Phase 0 | sqlite-vec 0.1.9 build OK | `cargo add sqlite-vec` + `cargo build --features embeddings` |
| G-A1 | Phase 1 | Red 全件確証 | T-1.1〜T-1.5/T-1.7 fail、T-1.6 既存 test pass |
| G-A2 | Phase 2 | Green 全件 + 既存退行ゼロ (両 build) | new test pass + 1143 既存 test 維持 (default + embeddings) + clippy 0 + fmt 0 |
| G-A3 | Phase 3 | Refactor で動作不変 | Phase 2 と同 test pass |
| G-A4 | Phase 4 | 性能 + 退行 + memory | G-4.1 (3x 高速)、G-4.2 (±0.02)、G-4.3 (+200MB 内) |
| G-A5 | Phase 5 | docs + handoff | CLAUDE.md/handoff 整合 |

---

## 8. Step B (Milvus Lite) 着手判断

Phase 4 smoke 完遂後の **2 軸 evaluation** で B 必要性を決定:

| 評価軸 | A 結果 | B 不要判定 | B 検討判定 |
|---|---|---|---|
| recall@10 (synthetic) | linear vs vec0 | vec0 が +5% 以上 | vec0 ≦ linear (要因調査) |
| Lab core 22 score | 旧 baseline 比 | ±0.02 以内 (退行なし) | -0.02 超 (劣化) or +0.02 超 (改善余地大) |

- **B 不要 = Milvus Lite REJECT 確定** (本 plan で bottleneck 解消、Lab v17 副次 finding 「stability > score」と整合)
- **B 検討 = `.claude/plan/milvus-lite-sidecar-impl.md` 別 plan 起票** (Phase 0 計測 + Phase 1-5)

---

## 9. 見積もり総計

| Phase | 見積もり |
|---|---|
| Phase 0 | 30 min |
| Phase 1 Red | 1.5h |
| Phase 2 Green | 3h |
| Phase 3 Refactor | 1h |
| Phase 4 Smoke | 2h |
| Phase 5 Docs | 30 min |
| **合計** | **~8.5h ≒ 1 day** |

実装 session で連続消化推奨 (Cerememory 三本柱と同 pattern)。

---

## 10. 関連参照

- `src/memory/search.rs:60-75` (現状 vector_search 線形 scan)
- `src/runtime/embedder.rs:1-100` (Embedder trait + feature gate)
- `Cargo.toml:65` (sqlite-vec コメントアウト箇所)
- `src/db/schema.rs` (V12 = ReviewState 最新、V13 が本 plan)
- 項目 80: hybrid search RRF 融合 (本 plan は vector 部分のみ更新、RRF 不変)
- 項目 217/218/219: Cerememory 三本柱 (env opt-in pattern を踏襲、ただし本 plan は feature flag、env ではない)
- 親 plan: `.claude/plan/external-memory-oss-integration-judgment.md` §1.2
