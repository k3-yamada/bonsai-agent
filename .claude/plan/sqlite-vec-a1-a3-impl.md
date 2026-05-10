# sqlite-vec A1+A3 Joint Plan: caller wiring + recall@k extension

**作成日**: 2026-05-10
**親 plan**: `.claude/plan/sqlite-vec-activation-impl.md` §2 G-2.5 / §8
**目的**:
- A1: `HybridSearch::index_memory` の caller-side 配線で **vec_memories の動的 populate** を実現（現状: ensure_vec_table eager backfill 後は新規 memory 不投入 = 機構休眠）
- A3: `tests/sqlite_vec_perf.rs::vec_perf_synthetic_g45_recall_at_10` を recall@{10,20,50} 拡張し、A1 ACCEPT 判定の補助 informational signal として組込む
**前提**: bonsai 1150 lib tests passing、sqlite-vec Phase 0-5 完遂 (CLAUDE.md 項目 220)、production code 変更ゼロ（本 plan は plan のみ）
**TDD strict 5 phase + Phase 0 architecture decision**

---

## 0. Architecture Decisions (Phase 0、Phase 1 着手前必須)

### D-1: 配線スイッチ方式（env opt-in、Cerememory 三本柱踏襲）

| 案 | 内容 | trade-off |
|---|---|---|
| **B1 (採用)** | `BONSAI_VEC_INDEX_ENABLED=1` env opt-in、default OFF で観測動作完全互換 | 項目 217/218/219 と同 pattern、Cerememory 三本柱と env name 対称、cold path で短絡 |
| B2 | feature flag のみ（`embeddings` build で常時 ON） | A1 効果が観測されるまで always-on にする根拠が弱い、Lab paired t-test で ON/OFF 切替し難い |
| B3 | `AdvisorConfig` field 化 | 設定空間爆発、env と二重 source-of-truth |

**理由**: A1 は Lab core 22 cycle で **未検証**（項目 220 で「不要確定」とした判断が「未配線で score 退行ゼロ」観測に依拠）。env opt-in なら paired t-test で fairness の高い ON/OFF 比較が可能、ACCEPT/REJECT 確証後に default ON 切替を別 plan で扱える。

### D-2: 配線対象 callsite（6 件、production のみ）

`save_memory` の production 呼出箇所（test 配下除く）:

| # | file:line | context | 期待 vec_memories 投入頻度 |
|---|---|---|---|
| 1 | `src/memory/evolution.rs:182` | auto-evolution loop の knowledge 保存 | 中（arxiv 自己進化 cycle ごと） |
| 2 | `src/memory/evolution.rs:252` | insight 保存（auto-improve） | 中（improvement 検出ごと） |
| 3 | `src/memory/evolution.rs:266` | insight 保存（auto-improve） | 中（improvement 検出ごと） |
| 4 | `src/memory/evolution.rs:380` | auto-improve 周辺 | 低 |
| 5 | `src/memory/evolution.rs:435` | auto-improve 周辺 | 低 |
| 6 | `src/agent/compaction.rs:637` | context_flush summary | **高**（compaction ごと、Lab cycle 内で頻発） |

**配線方式**: 各 callsite の `save_memory(...)?;` 直後に `index_memory_if_enabled(memory_id, content)` 呼出を追加。返り値 `Result<()>` は warn log で握り潰す（D-3 参照）。

### D-3: エラーハンドリング戦略（non-fatal）

- `index_memory` 失敗で `save_memory` 自体は **成功扱い**（vec0 投入失敗で memory 本体まで失われる事を防ぐ）
- 失敗時は `eprintln!("[warn] vec_index failed for memory_id={id}: {e}");` で stderr に warn 出力のみ
- 既存 `context_inject.rs:278-280` の `[warn] メモリ検索エラー` パターンに統一

### D-4: A3 recall@k 拡張範囲

| k | 現状 | 本 plan 拡張 | gate |
|---|---|---|---|
| 10 | recall=1.0 perfect (PASS) | 既存維持 | 既存 ≥0.95 PASS gate 維持 |
| 20 | 未測定 | **追加** | informational only（gate なし） |
| 50 | 未測定 | **追加** | informational only（gate なし） |

**N_MEMORIES = 1000 維持**（vec0 brute-force exact KNN なので k 拡張で recall は理論上 1.0 維持を確認するのみ）。50 query × 50 top_k = 2500 sample で十分な統計性。

### D-5: A1 ACCEPT 判定軸（Phase 4 Smoke gate）

| 軸 | metric | PASS 条件 | Lab v17 副次 finding (項目 215) との整合 |
|---|---|---|---|
| score | core 22 baseline (handoff 05-09h: 0.7420) | **±0.02 以内** で退行なし | 主軸維持 |
| vec_memories 利用率 | Lab 1 cycle 中の `vector_search` non-empty 結果回数 | **>0**（少なくとも 1 cycle で vec0 path が non-empty 結果返却） | 機構が休眠でないことを実証 |
| stability | run 間 variance（pass@k） | OFF baseline より ±0.05 以内 | 項目 215 で ON 優位観測、悪化していないこと確認 |

**REJECT 条件**: score 退行 > 0.02 OR vec_memories 利用率 = 0 (= 機構が事実上動作せず) OR stability std 0.05 超劣化。

### D-6: search ctx 同一化（A1 配線で必要）

- `HybridSearch::new(store, embedder)` を 6 callsite で都度生成すると embedder 再 init cost が発生
- **採用方式**: 各 callsite に lazy 生成 helper を導入する代わりに、`MemoryStore::index_memory_if_enabled(memory_id, content)` を `MemoryStore` の inherent method として追加（`store.index_memory_if_enabled(id, content)` の 1 行で完結、env-disabled で no-op）
- `MemoryStore` 内部で `OnceLock<Box<dyn Embedder>>` を保持し、初回呼出時のみ embedder を生成
- env=disabled の hot path では OnceLock 触らず短絡

---

## 1. Phase 1 Red (失敗テスト先行、~1.5h)

### T-1.1 env-gate `is_vec_index_enabled` (新規)
- `BONSAI_VEC_INDEX_ENABLED` env unset → `false`
- `=1`/`=true` (case-insensitive) → `true`
- `=0`/`=false`/空文字/`no` → `false`
- 既存 4 toggle と同 test pattern（項目 214/216/217/218 module-local Mutex で env mutation race serialize）

### T-1.2 `MemoryStore::index_memory_if_enabled` env-disabled で no-op
- env unset で 1000 memory save → vec_memories は ensure_vec_table eager backfill 分のみ（追加投入ゼロ）
- Red 期待: method 未定義 = compile error

### T-1.3 `MemoryStore::index_memory_if_enabled` env=1 で vec_memories 投入
- env=1 で 5 memory save → vec_memories に 5 row 追加（eager backfill 0 件 case を起点）
- Red 期待: method 未定義 = compile error

### T-1.4 配線済 evolution.rs save_memory 後の vec_memories 状態
- 専用 test helper 経由で env=1 + save → vec_memories に投入確認
- Red 期待: 配線がまだ無いため vec_memories 空のまま

### T-1.5 配線済 compaction.rs save_memory 後の vec_memories 状態
- 専用 test helper 経由で env=1 + flush → vec_memories に投入確認
- Red 期待: 同上、配線が無い

### T-1.6 non-fatal error handling
- mock embedder で embed 失敗を強制 → save_memory は **成功** 返却 + vec_memories は空
- Red 期待: エラー握り潰しロジック未実装

### T-1.7 既存 1150 test の signature 不変
- 既存 6 search test (search.rs:171-214) + 1 production caller (context_inject.rs:274) は **無修正で pass**
- 既存 sqlite_vec_perf.rs 3 test (G-4.1/G-4.4/G-4.5) は **無修正で pass**

### T-1.8 A3 recall@20 拡張
- `tests/sqlite_vec_perf.rs::vec_perf_synthetic_g45_recall_at_k(k: usize)` パラメータ化
- 既存 G-4.5 を `vec_perf_synthetic_g45_recall_at_10` (= `recall_at_k(10)` ラッパー) に保持
- 新規 `vec_perf_synthetic_g45_recall_at_20`、`vec_perf_synthetic_g45_recall_at_50` を `#[ignore]` で追加
- Red 期待: パラメータ化 helper 未定義 = compile error

---

## 2. Phase 2 Green (~2.5h)

### Step G-2.1: `src/memory/vec_index_toggle.rs` 新規 (~50 行)
```rust
pub(crate) fn is_vec_index_enabled() -> bool {
    std::env::var("BONSAI_VEC_INDEX_ENABLED")
        .map(|v| matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

#[cfg(test)]
pub(crate) static VEC_INDEX_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
```

### Step G-2.2: `MemoryStore` 拡張 (`src/memory/store.rs`、`#[cfg(feature = "embeddings")]` 配下)

擬似コード:
```rust
use std::sync::OnceLock;
use crate::runtime::embedder::{create_embedder, Embedder};

#[cfg(feature = "embeddings")]
struct VecIndexCtx { embedder: Box<dyn Embedder> }

#[cfg(feature = "embeddings")]
static VEC_INDEX_CTX: OnceLock<VecIndexCtx> = OnceLock::new();

impl MemoryStore {
    /// env=1 で memory_id に対する 256d embedding を vec_memories に投入。
    /// env unset で no-op (cold path 短絡)。embed 失敗は warn log + Ok で握り潰し。
    pub fn index_memory_if_enabled(&self, memory_id: i64, content: &str) -> Result<()> {
        #[cfg(not(feature = "embeddings"))]
        { let _ = (memory_id, content); return Ok(()); }

        #[cfg(feature = "embeddings")]
        {
            if !crate::memory::vec_index_toggle::is_vec_index_enabled() {
                return Ok(());
            }
            let ctx = VEC_INDEX_CTX.get_or_init(|| VecIndexCtx {
                embedder: create_embedder(),
            });
            match ctx.embedder.embed(&[content]) {
                Ok(emb) => {
                    if let Some(first) = emb.first() {
                        if let Err(e) = self.insert_memory_embedding(memory_id, first) {
                            eprintln!("[warn] vec_index insert failed for memory_id={memory_id}: {e}");
                        }
                    }
                }
                Err(e) => eprintln!("[warn] vec_index embed failed for memory_id={memory_id}: {e}"),
            }
            Ok(())
        }
    }
}
```

### Step G-2.3: 6 callsite 配線（最小修正パターン統一）
各 callsite を以下 1 行追加に統一:
```rust
let id = self.store.save_memory(&content, "knowledge", &tags)?;
let _ = self.store.index_memory_if_enabled(id, &content);  // ← 1 行追加
```

具体的修正:
- `src/memory/evolution.rs:182,252,266,380,435`: 5 callsite に上記 hook 追加（id 受取 + index_memory_if_enabled 呼出）
- `src/agent/compaction.rs:637`: `if let Err(e) = store.save_memory(...)` 後に `Ok(id) =>` arm で hook 追加

### Step G-2.4: A3 recall@k パラメータ化
`tests/sqlite_vec_perf.rs` を以下のように再構成:
```rust
fn run_recall_at_k(k: usize, n_memories: usize) -> (f64, usize) {
    // 既存 G-4.5 body の k/N_MEMORIES パラメータ化
    // 戻り値: (avg_recall, zero_recall_queries)
}

#[test]
#[ignore = "G-4.5 (recall@10 / Step B axis-1)"]
fn vec_perf_synthetic_g45_recall_at_10() {
    let (recall, zero) = run_recall_at_k(10, 1000);
    eprintln!("[G-4.5 k=10] recall={recall:.4} zero={zero}/50");
    assert!(recall >= 0.95, "recall@10 ≥ 0.95 PASS gate");
}

#[test]
#[ignore = "G-4.5 extended k=20 (informational)"]
fn vec_perf_synthetic_g45_recall_at_20() {
    let (recall, zero) = run_recall_at_k(20, 1000);
    eprintln!("[G-4.5 k=20] recall={recall:.4} zero={zero}/50");
    // gate なし、informational
}

#[test]
#[ignore = "G-4.5 extended k=50 (informational)"]
fn vec_perf_synthetic_g45_recall_at_50() {
    let (recall, zero) = run_recall_at_k(50, 1000);
    eprintln!("[G-4.5 k=50] recall={recall:.4} zero={zero}/50");
}
```

### Step G-2.5: `src/memory/mod.rs` で module 公開
```rust
pub(crate) mod vec_index_toggle;
```

---

## 3. Phase 3 Refactor (~30 min)

- `index_memory_if_enabled` の docstring に「Cerememory 三本柱と同 env opt-in pattern」明記
- 6 callsite の hook 形を git diff で全件確認、grep で漏れチェック (`grep -n "save_memory" src/ | grep -v "index_memory_if_enabled"` で他に hook 必要箇所が無いか確認)
- clippy 0 / fmt 0 維持
- `OnceLock` の thread safety を docstring 明示
- **R-5 解消**: `HybridSearch::index_memory` (search.rs:129) を `MemoryStore::index_memory_if_enabled` の delegate ラッパーに書き換え、observable behavior 不変、内部 1 source-of-truth 化

---

## 4. Phase 4 Smoke (~2h)

### G-4.1: 単体測定（synthetic、`--features embeddings`）
- 既存 G-4.1 維持（変更不要、A1 wiring 自体の latency 影響を測定）
- 追加測定: `BONSAI_VEC_INDEX_ENABLED=1` で 100 save_memory の latency p50/p99（embed cost を含む新規 overhead）
- **gate**: env=1 の save_memory p50 が env unset の **2x 以下**（embed cost 1 回分の許容上限）

### G-4.2: A1 Lab paired smoke（**user 起動 llama-server 必要**）

**ON cycle**:
```bash
BONSAI_VEC_INDEX_ENABLED=1 BONSAI_BENCH_TIER=core \
  /usr/bin/time -l cargo run --release --features embeddings -- \
  --lab --lab-experiments 0 2>&1 | tee lab-vec-on.log
```

**OFF cycle** (baseline):
```bash
unset BONSAI_VEC_INDEX_ENABLED
BONSAI_BENCH_TIER=core /usr/bin/time -l cargo run --release --features embeddings -- \
  --lab --lab-experiments 0 2>&1 | tee lab-vec-off.log
```

**観測項目**:
- score / pass@k / pass_consec
- vec_memories 行数（cycle 完了時 `SELECT COUNT(*) FROM vec_memories`）
- maximum resident set size（peak RSS、項目 220 G-4.3 比較）
- duration

**gate (D-5 参照)**:

| 軸 | PASS | NG |
|---|---|---|
| score | ON が OFF baseline (handoff 05-09h: 0.7420) ±0.02 | -0.02 超劣化 |
| 利用率 | ON cycle で vec_memories 行数 > eager backfill 分 | 増加なし = 配線失敗 |
| stability | ON pass@k と OFF pass@k の絶対差 ≤ 0.10 | 0.10 超 = 不安定化 |

### G-4.3: A3 recall@k 拡張測定
```bash
cargo test --release --features embeddings --test sqlite_vec_perf \
  vec_perf_synthetic_g45_recall_at_10 \
  vec_perf_synthetic_g45_recall_at_20 \
  vec_perf_synthetic_g45_recall_at_50 \
  -- --ignored --nocapture
```

**期待**: 全 k で recall ≥ 0.95（vec0 = brute-force exact KNN なので理論上 1.0）。
**判断**: recall@20/50 が perfect なら ANN 移行不要を再確認、< 0.95 なら sqlite-vec 内部実装に未知の優先度 sort 動作あり = bug report 候補。

### G-4.4: peak RSS 比較
- ON cycle peak RSS - OFF cycle peak RSS の差分
- **gate**: +50 MB 以下（embed model はすでに OFF cycle でも load 済 = ensure_vec_table backfill 経由、本 plan の追加 overhead は OnceLock + 1 embed/save のみ）

---

## 5. Phase 5 Docs (~30 min)

- CLAUDE.md 項目 221 候補追加（plan 完遂時、A1 ACCEPT/REJECT の Lab 結果込み）
- session handoff `session_2026_05_10_handoff.md`
- README の `BONSAI_VEC_INDEX_ENABLED` env を「Cerememory 三本柱 + sqlite-vec 配線」セクションで言及
- MEMORY.md index 不変（plan 系 file は plan/ 配下のみ）

### ACCEPT 後の defaults 化判断（次 plan 候補）
- G-4.2 ACCEPT で score +0.01 以上、stability 改善 → **default ON 切替 plan 起票**（env name 維持で「unset → ON」へ flip、項目 216 と逆方向）
- ACCEPT だが score ±0.02 維持のみ → env opt-in のまま据置
- REJECT → CLAUDE.md に「A1 Lab v18 REJECT 確定」記録、`index_memory_if_enabled` を dead-code 候補化（別 plan）

---

## 6. Risks

### R-1: env=1 でも save_memory hot path への embed cost 影響
- **影響**: Lab cycle duration regression（embed model は 256d 出力、1 inference あたり数 ms ～ 数十 ms、save_memory 6 callsite × Lab cycle 中の頻度）
- **軽減**: G-4.1 で latency 2x 以下を gate、超過時は async insert (background thread) を別 plan で検討

### R-2: vec_memories 行数増加で SQLite ファイルサイズ膨張
- **影響**: 1 row = 256d × 4 byte = 1KB + metadata、Lab 1 cycle で 100-500 row 増加 = 数 MB
- **軽減**: project lifetime で年単位の蓄積でも数 GB 程度、bonsai 開発環境で問題化しない見込み。production 長期運用想定の prune は別 plan（`vec_memories_prune_oldest_n` API）

### R-3: OnceLock embedder の冷起動 race
- **影響**: 並行 save_memory で embedder init を複数 thread が試行、`OnceLock` は内部で synchronize するが embed model load 自体は重い
- **軽減**: `OnceLock` は `get_or_init` が冪等、初回 init は serialized、2 回目以降は lock-free read（標準 library 保証）

### R-4: ensure_vec_table eager backfill との semantic 重複
- **影響**: 既存 ensure_vec_table が起動時に全 memory を backfill するため、A1 配線後は eager backfill が冗長（lazy hook で十分）
- **軽減**: 本 plan 範囲外、ensure_vec_table の lazy 化は別 plan（`ensure_vec_table_lazy` 切出 + 起動 backfill skip option）。本 plan は **eager backfill を維持** したまま hook 追加（既存挙動 100% 互換）

### R-5: HybridSearch::index_memory との二重実装
- **影響**: search.rs:129 の `HybridSearch::index_memory` と本 plan の `MemoryStore::index_memory_if_enabled` が semantic 同義
- **軽減**: HybridSearch::index_memory を `MemoryStore::index_memory_if_enabled` の **delegate ラッパー** に書き換え（observable behavior 不変、内部 1 source-of-truth 化）。Phase 3 Refactor に組込

### R-6: A3 で recall@20/50 が <0.95 だった場合の解釈
- **影響**: vec0 brute-force exact KNN の前提が崩れる、ground truth (cosine similarity) と vec0 distance の sort が一致しない unknown 動作
- **軽減**: informational only に格下げ済（gate なし）、観測されれば sqlite-vec 0.1.9 GitHub issue + 詳細な reproduction を別 plan

### R-7: Lab v17 副次 finding (stability 軸 ON 優位、項目 215) との conflict
- **影響**: A1 ON で stability 悪化（pool 成熟 = vec_memories 蓄積による検索ゆらぎ）が起きる risk
- **軽減**: D-5 に stability 軸を gate に組込（pass@k variance ±0.05 以内）、悪化観測時は ReviewState (項目 218) を vec_memories にも適用する別 plan を起票

---

## 7. Gates 一覧

| Gate ID | Phase | 内容 | PASS 条件 |
|---|---|---|---|
| G-A0 | Phase 0 | env name + callsite 確定 | D-1〜D-6 user 確認、`save_memory` 6 callsite 完全リスト |
| G-A1 | Phase 1 | Red 全件確証 | T-1.1〜T-1.6/T-1.8 fail、T-1.7 既存 test pass |
| G-A2 | Phase 2 | Green 全件 + 既存退行ゼロ | new test pass + 1150 既存 test 維持 (default + embeddings 両 build) + clippy 0 + fmt 0 |
| G-A3 | Phase 3 | Refactor で動作不変 + R-5 解消 | Phase 2 と同 test pass + HybridSearch::index_memory delegate 化確認 |
| G-A4 | Phase 4 | Lab paired + recall + memory | G-4.1 (latency 2x 内)、G-4.2 (score ±0.02 + 利用率 >0 + stability ±0.05)、G-4.3 (recall@{20,50} 観測値記録)、G-4.4 (RSS +50MB 内) |
| G-A5 | Phase 5 | docs + handoff + ACCEPT/REJECT 判定 | CLAUDE.md/handoff 整合、defaults 化 or dead-code 候補 plan の方向性記録 |

---

## 8. 見積もり総計

| Phase | 見積もり |
|---|---|
| Phase 0 | 30 min |
| Phase 1 Red | 1.5h |
| Phase 2 Green | 2.5h |
| Phase 3 Refactor | 30 min |
| Phase 4 Smoke | 2h（うち Lab paired 1.5h、user 起動 llama-server 必要） |
| Phase 5 Docs | 30 min |
| **合計** | **~7h ≒ 1 day** |

実装 session で連続消化推奨（Cerememory 三本柱 + sqlite-vec activation と同 pattern）。Phase 4 G-4.2 で llama-server を user 起動する必要があるため、**Phase 1-3 + G-4.1/G-4.3/G-4.4** までを 1 session（~5h）、**G-4.2 + Phase 5** を別 session（~2h）に分割が現実的。

---

## 9. Step B (Milvus Lite) との関係

本 plan の G-4.2 ACCEPT は項目 220 で確定済 Step B REJECT を **覆す動機を提供しない**:
- A1 ACCEPT = sqlite-vec 機構が稼働 → Milvus Lite の追加価値を再評価する root 条件は **Lab cycle で score 改善**（≥+0.02）の場合のみ
- A1 score 維持（±0.02）= Milvus Lite REJECT 確定の追加証拠（vec0 brute-force exact KNN で十分）
- A1 score 悪化（<-0.02）= Milvus Lite REJECT 維持（A1 が機構として不適合 → ANN への置換も同 root 課題）

**結論**: 本 plan の Phase 4 結果に関わらず、Step B Milvus Lite REJECT は維持。

---

## 10. 関連参照

- `src/memory/search.rs:129` (HybridSearch::index_memory、本 plan で delegate ラッパー化)
- `src/memory/store.rs:447,473,502` (ensure_vec_table / insert_memory_embedding / vec_knn)
- `src/agent/context_inject.rs:274` (production HybridSearch caller、無修正)
- `src/memory/evolution.rs:182,252,266,380,435` (A1 配線対象 5 件)
- `src/agent/compaction.rs:637` (A1 配線対象 1 件、頻度高)
- `tests/sqlite_vec_perf.rs:206-278` (G-4.5 recall@10、本 plan で k 拡張)
- 親 plan: `.claude/plan/sqlite-vec-activation-impl.md` §2 G-2.5 / §8
- CLAUDE.md 項目 220: sqlite-vec Phase 0-5 完遂、G-2.5 caller 配線「不要確定」（本 plan で再評価）
- 項目 215: Lab v17 副次 finding（stability 軸 ON 優位、本 plan G-4.2 で stability gate に組込）
- 項目 217/218/219: Cerememory 三本柱（本 plan の env opt-in pattern を踏襲）

---

## 11. SESSION_ID（multi-plan 規約）

- CODEX_SESSION: **N/A**（本 plan は scope 明確 + template 完備のため Codex/Gemini 並列呼出を skip、Claude 単独で起票）
- GEMINI_SESSION: **N/A**（同上、pure backend Rust task で Gemini frontend role が non-applicable）

dual-model 相談が必要な場合は Phase 0.5 として `/ccg:plan` 単独で D-1〜D-6 の architecture decision を Codex review にかけることを推奨（~30 min、本 plan の Phase 0 を再検証する形）。
