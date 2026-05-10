# External Memory OSS Integration Judgment Plan

**作成日**: 2026-05-09
**起点**: handoff 05-09e task #6
**前提**: Cerememory 三本柱完遂 (項目 217/218/219)、886 tests + 1143 lib tests passing
**対象 OSS**: Milvus / Cognee / mem0ai
**fact source**: `memory/external_memory_oss_comparison_2026_05_09.md`
**mode**: research + plan only (production code 変更ゼロ)

---

## 0. 全体方針

bonsai は **「Scaffolding > Model」** 原則 + M2 16GB single-process + Bonsai-8B 1bit (1.28GB) という制約下で設計されており、外部 OSS の安易な port は天井 7 連続 (Lab v8-v17) と同じ「1bit に translate しない」現象を再生する risk が高い。

本 plan は **採用 / 却下 / 部分 port のいずれを推奨するか** を OSS 単位で判断し、部分 port 候補は別 plan に切り出す。

---

## 1. Milvus → **段階評価**: full Milvus は REJECT、**Milvus Lite は A→B 2-step 評価**

### 1.1 Milvus full (分散 K8s/Standalone) → **REJECT**
- M2 16GB single-process bonsai に対し scale 不適合、本セクション以下では Milvus Lite のみを評価対象とする

### 1.2 Milvus Lite → **段階評価候補**

#### 訂正された前提 (本 plan 初版の誤りを修正)
| 項目 | 事実 (2026-05-09 確認) |
|---|---|
| bonsai 現状 vector | **線形スキャン** (`src/memory/search.rs:60-75`、O(N×D)/query)、`sqlite-vec` は `Cargo.toml:65` で **コメントアウト未活性** |
| Milvus Lite 実体 | **Pure Python** (旧 C++/CGo から rebuilt)、PyPI `milvus-lite` |
| Rust integration | gRPC server mode (`milvus-lite server --port 19530`) + `milvus-sdk-rust` (alpha、looking for maintainers) のみ |
| 提供 index | FAISS HNSW / HNSW_SQ / IVF_FLAT / IVF_SQ8 / FLAT、BM25 (Function 経由 native)、scalar filter |
| 規模適性 | <1M vectors、prototyping/edge target |

#### 判断軸 (訂正後)
| 観点 | bonsai 現状 | Milvus Lite | 評価 |
|---|---|---|---|
| vector index | 線形 (ANN なし) | FAISS HNSW + 4 variant | **大幅改善余地** |
| sparse | FTS5 (BM25 系列) | BM25 native | 同等 |
| hybrid | RRF 後段融合 (項目 80) | dense+sparse+filter native | Milvus がより統合的 |
| process count | 2 (Rust + llama-server) | **3** (+ Python milvus-lite) | 運用 cost ↑ |
| Rust SDK | native (sqlite) | gRPC 経由、alpha | 成熟度懸念 |
| persistence | SQLite single-file | LSM + WAL + Parquet | bonsai の方が単純 |
| memory cost | Bonsai-8B 1.3GB + SQLite | + FAISS index in RAM + Python runtime | M2 16GB で要計測 |

#### Step A: sqlite-vec 活性化 (~0.5 day、低 risk、推奨先行)
- **対象**: `Cargo.toml:65` の `# sqlite-vec = "0.1"` コメント解除 + `MemoryStore` に vec0 virtual table 追加
- **既存影響**: `src/memory/search.rs::vector_search` (線形 scan) を vec0 query に置換 (linear 実装は削除、`sqlite-vec` 未ロードは build error で扱う)
- **deliverable**: 別 plan `.claude/plan/sqlite-vec-activation-impl.md` (TDD strict 5 phase、~5h)
- **gate (B 着手の必要性判定)**:
  - PASS: recall@10 と latency が現在の linear 比 +改善、かつ Lab core 22 で score 維持 (退行 ±0.02 以内)
  - **A だけで bottleneck 解消なら B 不要** = Milvus Lite REJECT 確定

#### Step B: Milvus Lite sidecar (~3-5 day、中 risk、A 不十分時のみ)
- **trigger**: A 完遂後、retrieval が bottleneck 残存と判明した場合のみ
- **対象**: llama-server と同 sidecar pattern で `milvus-lite server` を起動、Rust 側は `milvus-sdk-rust` で gRPC 接続
- **置換範囲**: `MemoryStore` の vector 部分のみ (5 store / KG / heuristics 等の primary storage は SQLite 維持、Milvus Lite は **vector-only secondary store**)
- **risk**:
  - R1: M2 16GB で 3 process (Rust + llama-server + python milvus-lite) の memory 競合 → Phase 0 で `/usr/bin/time -l` 計測 gate
  - R2: `milvus-sdk-rust` alpha 状態 → 上流 PR or fork 覚悟、production 採用前に維持コスト試算
  - R3: Lab v17 副次 finding (stability 軸) と矛盾する変更 risk → Phase 5 paired t-test で stability 軸も評価
  - R4: Python runtime 追加で Rust-only ethos 違反 → user 判断必須 gate
- **deliverable**: 別 plan `.claude/plan/milvus-lite-sidecar-impl.md` (Phase 0 計測 + Phase 1-4 TDD + Phase 5 Lab、~3-5 day)

#### 不採用部分
- Milvus full (K8s/Standalone): scale mismatch
- bonsai の 5 store + KG を Milvus に**統合移植**する案: Milvus は vector primary、bonsai 既存 schema (V12) と思想差、移植コスト > 利益

### 1.3 deliverable まとめ
- 本 plan §1 (本改訂) で段階評価方針を確定
- A 着手判断 = user 即決可、別 plan 起票必要 (`sqlite-vec-activation-impl.md`)
- B 着手判断 = A の gate 結果待ち、別 plan 起票は A 完遂後

---

## 2. Cognee → **PARTIAL PORT 候補** (cognify entity 抽出のみ、Lab v23 で検証)

### 判断根拠
| 観点 | bonsai 現状 | Cognee | 結論 |
|---|---|---|---|
| graph | **手動 edge 構築** (項目 13/77) | **LLM 自動 entity/relation 抽出** (cognify 6-stage) | **port 候補** |
| 4 ops API | 5 store API (semantic overlap) | remember/recall/forget/improve | API 思想差、強制統一は不要 |
| 2 階層 (session+permanent) | Session + MemoryStore (V5) | session_id scoping + KG sync | bonsai 既存設計と等価 |

### 推奨アクション
- **新 plan 起票**: `.claude/plan/cognee-cognify-port-impl.md`
  - cognify 6-stage pipeline のうち **stage 4 (LLM entity/relation 抽出)** のみ port
  - 既存 `KnowledgeGraph::add_edge` の自動化拡張 (manual + auto の併存)
  - `BONSAI_AUTO_ENTITY_ENABLED` env opt-in (Cerememory 三本柱と同 pattern)
  - **Phase 5 Lab 検証必須**: paired t-test (auto entity ON/OFF、core 22、k=3、5 cycle)
  - **risk R1**: Bonsai-8B 1bit で entity 抽出品質が GPT-class より大幅劣化 → mock LLM (Cognee 公式 = gpt-4o 推奨) との比較 baseline 確保必要
  - **risk R2**: KG edge 爆発 (cognify は incremental に entity 追加) → fingerprint dedup (項目 206 同方針) 必須
  - 見積もり: ~3 day (Phase 1-4 + Phase 5 Lab v23、~12-18h)

### 不採用部分
- 4 ops API (remember/recall/forget/improve): bonsai 5 store API と思想差、forget concept は ReviewState の Superseded status (項目 218) で代替可能
- Memify post-processing: ERL hindsight pass (項目 213/215) と機能 overlap、ERL は Lab v17 REJECT 確定なので教訓は反映済

### deliverable
- 本 plan の本セクション + 別 plan `cognee-cognify-port-impl.md` (~400 行、TDD strict 5 phase)

---

## 3. mem0 → **部分 PORT 候補** (entity boost のみ) + **思想 REJECT** (ADD-only)

### 判断根拠
| 観点 | bonsai 現状 | mem0 v3 | 結論 |
|---|---|---|---|
| ADD-only 思想 | 陳腐化対応 (項目 217/218) | **明示的に廃止** | **REJECT** (思想全面矛盾) |
| Multi-Level scoping | session_id + 5 store | User/Session/Agent 3 軸 | 現状で十分、3 軸採用は overkill |
| entity linking | KG (手動 edge) | **parallel collection + score boost** | **port 候補** |
| multi-signal hybrid | FTS5+vector RRF (2 signal) | semantic+BM25+**entity** (3 signal) | **項目 80 拡張候補** |

### 推奨アクション
- **新 plan 起票**: `.claude/plan/mem0-entity-boost-port-impl.md`
  - 既存 `hybrid_search` (項目 80) に **entity matching score** を 3rd signal として追加
  - 入力 query から entity 抽出 (proper nouns / quoted text、Cognee port と同 LLM call 共有検討) → KG entity match → score boost
  - mem0 同方針の adaptive: entity 抽出 fail 時は既存 2-signal RRF に degrade (mem0 公式が 3 mode adapt と明記している点を踏襲)
  - `BONSAI_ENTITY_BOOST_ENABLED` env opt-in
  - **Phase 5 Lab 検証**: paired t-test (entity boost ON/OFF、core 22、k=3、5 cycle)
  - **risk R1**: 1bit entity 抽出品質低下 → Cognee port (§2) と同 challenge、共通検証 framework 推奨
  - **risk R2**: BM25 への退行 (mem0 は BM25 を 2nd signal、bonsai は FTS5 = BM25 系列で既装備) → 純粋追加でなく置換不要を確認
  - 見積もり: ~1.5 day (Phase 1-4 + Phase 5 Lab v24、~6-10h)

### 不採用部分
- **v3 ADD-only 思想 (no UPDATE/DELETE)**: bonsai 設計の根幹否定、項目 217/218 と全面矛盾、Cerememory ADR-005/011 命題 (「frequently used memory が dangerous」) と論理的に対立 → REJECT
- Multi-Level Memory 3 軸 (User/Session/Agent): bonsai は session_id + 5 store で代替済、3 軸採用は API 複雑化のみで benefit 不明

### deliverable
- 本 plan の本セクション + 別 plan `mem0-entity-boost-port-impl.md` (~300 行、TDD strict 5 phase)

---

## 4. 推奨実装順序

| 順 | plan | 見積もり | 依存 | 期待 effect |
|---|---|---|---|---|
| 1 | Cognee cognify port (§2) | ~3 day | なし (KG 拡張) | KG edge 自動化、graph density 向上 |
| 2 | mem0 entity boost port (§3) | ~1.5 day | Cognee の entity 抽出 LLM call 共有可 | hybrid 3rd signal、retrieval 精度 |
| 3 | (将来) 統合 Lab v25 | ~12-18h | 1+2 完遂 | 両機構 ON/OFF 4 組合せ paired t-test |

**理由**: Cognee が KG 自動化の基盤、mem0 entity boost は Cognee の entity 抽出 logic を **再利用** できる (LLM call 共有)。順序逆だと entity 抽出 logic を 2 回実装することになる。

---

## 5. 全体 risk

### R-Global-1: 1bit Bonsai-8B での entity 抽出品質
- **影響**: Cognee/mem0 両 port の Phase 5 Lab で REJECT 連発 risk (天井 8 連続候補)
- **軽減策**:
  - Phase 1 Red 前に **smoke 4 task で entity 抽出単体評価** (Bonsai-8B vs gpt-4o-mini API mock を 10 文書で比較、F1 ≥ 0.5 を gate)
  - F1 < 0.5 で plan を **defer** (Bonsai-8B 改善 or 大型モデル切替を待つ)

### R-Global-2: Cerememory 三本柱との conflict
- **影響**: ReviewState (項目 218) と entity boost の優先順位 conflict (古い entity を boost する case)
- **軽減策**: entity boost score に `freshness` 係数を乗算 (review_state.freshness < 0.35 で boost = 0、項目 218 inject_heuristics と同 threshold)

### R-Global-3: env opt-in 増殖による設定空間爆発
- **影響**: `BONSAI_DECAY_ENABLED` / `REVIEW_ENABLED` / `WORKING_CAP_ENABLED` / `AUTO_ENTITY_ENABLED` / `ENTITY_BOOST_ENABLED` の 5 env で 32 組合せ
- **軽減策**: `BONSAI_MEMORY_PROFILE={off,baseline,full}` meta-env を後段で導入 (将来別 plan、本 plan 範囲外)

---

## 6. 次セッション着手判断

### 即着手可
- §2 Cognee port plan 起票 (~30 min、planner agent or 直接記述)
- §3 mem0 port plan 起票 (~30 min、planner agent or 直接記述)

### 着手前 gate (R-Global-1)
- entity 抽出 smoke test (Bonsai-8B vs gpt-4o-mini、10 文書、F1 ≥ 0.5) ~2h
- gate PASS で §2/§3 着手 / FAIL で defer + Bonsai 改善計画

### 着手後 effort
- §2 Cognee: ~3 day
- §3 mem0: ~1.5 day
- §1+§2+§3 統合 Lab: ~12-18h

---

## 7. 関連参照
- `memory/external_memory_oss_comparison_2026_05_09.md`: 5 軸 fact 比較表 (本 plan の判断根拠)
- 項目 13/77: KnowledgeGraph V5 (BFS 双方向、手動 edge)
- 項目 80: hybrid search (FTS5+vector RRF)
- 項目 215: Lab v17 stability 軸副次 finding (mem0 v3 ADD-only との対立証拠)
- 項目 217/218/219: Cerememory 三本柱
- handoff 05-09e: 本 plan の起点
