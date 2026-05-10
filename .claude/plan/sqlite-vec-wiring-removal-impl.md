# sqlite-vec wiring removal Plan: A1+A3 dead-code 削除 (G-4.2 REJECT 後経路)

**作成日**: 2026-05-12
**親 plan (immutable record)**: `.claude/plan/sqlite-vec-a1-a3-impl.md` (394 行、§5 ACCEPT/REJECT mapping の REJECT 経路)
**前提 plan**: `.claude/plan/sqlite-vec-activation-impl.md` §2 G-2.5「caller 配線不要確定」(handoff 05-09h)
**前提 handoff**: `session_2026_05_11_handoff.md` (G-4.2 paired smoke 4 軸 gate 判定 = 3/4 PASS / RSS NG → REJECT 確定 / user 承認 = A 採用)

**目的**: G-4.2 Lab paired smoke REJECT (3/4 PASS、RSS gate NG +99.9 MB > +50 MB) を受け、`index_memory_if_enabled` および関連 wiring を **purely subtractive** に削除する。vec0 infrastructure (`ensure_vec_table` / `insert_memory_embedding` / `knn_search` / recall@k synthetic perf benches) は **保持** (将来 chunk-level embedding 等の余地)。

**設計原則**: 項目 216 (ERL defaults OFF) と同経路 — 既に default OFF (env unset) で no-op であるため、削除は production 観測動作に影響しない (純減算)。Cerememory 三本柱 (項目 217/218/219) と env name 対称な opt-in pattern を完全撤去。

**TDD strict 5 phase + Phase 0 architecture confirmation**

---

## 0. Architecture Decisions (Phase 0、Phase 1 着手前必須)

### D-1: 削除方針 (purely subtractive)

| 案 | 内容 | trade-off |
|---|---|---|
| **A1 (採用)** | wiring を全削除、env toggle module も削除、新 toggle 追加なし | 項目 216 と同経路、純減算で意味論明快、再導入時は plan を新起票 |
| A2 | env toggle 残置で `index_memory_if_enabled` のみ削除 | dead env name が残り混乱、対称性破綻 |
| A3 | feature flag (`embeddings`) でガードして残置 | 既に default OFF と同義、無意味な間接参照 |

**理由**: G-4.2 paired smoke で score Δ=-0.0031 (noise) かつ RSS +99.9 MB が architectural constant と判明。再導入の合理性ゼロ。`.claude/plan/sqlite-vec-a1-a3-impl.md` に検証経路と REJECT 根拠が immutable record として残るため、コードからの除去は安全。

### D-2: 削除対象 surface (9 項目、確定)

| # | path:line | symbol | bytes ≈ |
|---|-----------|--------|---------|
| 1 | `src/memory/vec_index_toggle.rs` (whole) | env toggle module + `is_vec_index_enabled` + `VEC_INDEX_TEST_LOCK` + 4 unit tests | 96 行 |
| 2 | `src/memory/store.rs:559-586` | `MemoryStore::index_memory_if_enabled` | 28 行 |
| 3 | `src/memory/store.rs:589-603` | `VecIndexCtx` struct + `vec_index_ctx()` `OnceLock` | 15 行 |
| 4 | `src/memory/store.rs:969-1054` | 3 unit tests: `t_a1_2_*` / `t_a1_3_*` / `t_a1_6_*` | 86 行 |
| 5 | `src/memory/search.rs:123-139` | `HybridSearch::index_memory` delegate (R-5、0 caller) | 17 行 |
| 6 | `src/memory/evolution.rs:183-185, 253-258, 271-276` | 3 production callsite + 3 explanatory comment | 計 ~12 行 |
| 7 | `src/agent/compaction.rs:637-640` | 1 production callsite + 1 explanatory comment | ~4 行 |
| 8 | `src/agent/compaction.rs:1334-1365` | 1 integration test `t_a1_5_context_flush_populates_vec_memories_when_vec_index_enabled` | ~32 行 |
| 9 | `src/memory/mod.rs:13` | `pub(crate) mod vec_index_toggle;` | 1 行 |

**合計**: 1 module + 1 method + 1 struct + 1 fn + 1 delegate + 4 callsite + 4 unit test + 1 integration test + 1 module decl = ~290 行 net delete

### D-3: 保持 surface (vec0 infrastructure、recall@k 用に温存)

| path:line | symbol | 残置理由 |
|-----------|--------|----------|
| `src/memory/store.rs:447` | `MemoryStore::ensure_vec_table` | recall@k synthetic perf benches で必須、eager backfill 経路 |
| `src/memory/store.rs:473` | `MemoryStore::insert_memory_embedding` | `ensure_vec_table` 内部で利用、test で直接呼出 |
| `src/memory/store.rs:502` | `MemoryStore::knn_search` | recall@k benches `tests/sqlite_vec_perf.rs` で使用 |
| `src/memory/store.rs:854-961` | 6 vec0 unit tests (`t_1_1`〜`t_1_7`) | virtual table / 256d embedding / eager backfill 検証 |
| `tests/sqlite_vec_perf.rs` (whole) | G-4.1/4.4/4.5 synthetic recall@k benches | 将来 chunk-level embedding 検証で再利用 |
| `db/schema.rs` V13 migration (vec_memories virtual table) | schema | infrastructure として未来余地保持 |
| `Cargo.toml` `sqlite-vec = "0.1.9"` 依存 | crate | knn_search / recall@k が依存 |

### D-4: テスト件数予測

- 現状: 1158 lib tests passing (handoff 05-10、G-4.1 G-4.4 G-4.5 追加後)
- 削除: 4 (vec_index_toggle) + 3 (store.rs t_a1_2/3/6) + 1 (compaction.rs t_a1_5) = **-8 tests**
- 完了時: **1150 lib tests** (handoff 05-09g 完遂時の値と一致 = 削除前の baseline 復元)

### D-5: ACCEPT 判定 (4 gate、補助)

本 plan は subtractive のため成功条件は単純:
1. `cargo build --release --features embeddings` 成功
2. `cargo test --release` 1150 passed (-8 from current 1158、退行ゼロ)
3. `cargo test --features embeddings --test sqlite_vec_perf -- --ignored` recall@k benches PASS (vec0 infrastructure 健全性)
4. `cargo clippy -- -D warnings` 0 warning + `cargo fmt --check` clean

REJECT 条件: 1〜4 のいずれか fail → root cause 特定 → 修正再走 (削除取りやめではなく修正で前進)

---

## 1. Phase 1 Red — compile-driven failure surfacing

**目的**: 削除前に「dead code が production の compile graph に組み込まれている」ことを compile error で確証する (TDD strict の Red 相当、新規 test ではなく compile-time evidence)。

### 1.1 module declaration 削除で red 起動

```rust
// src/memory/mod.rs:13 (Before)
pub(crate) mod vec_index_toggle;
// (After: 行ごと削除)
```

**期待 red**:
- `src/memory/store.rs:568` `crate::memory::vec_index_toggle::is_vec_index_enabled()` → unresolved
- `src/memory/store.rs:975, 1001, 1036` `crate::memory::vec_index_toggle::VEC_INDEX_TEST_LOCK` → unresolved
- `src/agent/compaction.rs:1341` 同上 → unresolved
- `cargo check` で全 unresolved を一覧化 → 削除箇所網羅性の compile-time 検証

### 1.2 Phase 1 commit boundary

Phase 1 単独 commit は **不要** (compile error 状態を残さない)。Phase 1 〜 Phase 3 を 1 commit に統合 (純減算で意味論的に分割不要)。

---

## 2. Phase 2 Green — minimum subtractive removal

**目的**: D-2 の 9 項目を削除し、cargo build を green に戻す。

### 2.1 削除順序 (依存関係 bottom-up)

1. **callsites 削除** (4 箇所):
   - `src/memory/evolution.rs:185` `let _ = self.store.index_memory_if_enabled(memory_id, &content);` 削除 + 直前 2 行 comment 削除
   - `src/memory/evolution.rs:258` 同 pattern 削除 + 直前 2 行 comment 削除
   - `src/memory/evolution.rs:276` 同 pattern 削除 + 直前 2 行 comment 削除
   - `src/agent/compaction.rs:640` 同 pattern 削除 + 直前 1 行 comment (`// Plan A1+A3 G-2.3: env=1 で...`) 削除

2. **delegate 削除**:
   - `src/memory/search.rs:123-139` `HybridSearch::index_memory` method + R-5 docstring 全削除

3. **MemoryStore method + infrastructure 削除**:
   - `src/memory/store.rs:559-586` `index_memory_if_enabled` method 全削除
   - `src/memory/store.rs:589-603` `VecIndexCtx` struct + `vec_index_ctx()` fn 全削除 (コメント `/// Plan A1+A3 G-2.2 D-6: ...` 含む)

4. **テスト削除**:
   - `src/memory/store.rs:969-1054` 3 unit tests 全削除
   - `src/agent/compaction.rs:1334-1365` integration test 全削除

5. **module 削除**:
   - `src/memory/vec_index_toggle.rs` ファイル削除 (`rm src/memory/vec_index_toggle.rs`)
   - `src/memory/mod.rs:13` `pub(crate) mod vec_index_toggle;` 行削除

### 2.2 callsite 削除の擬似コード

```rust
// src/memory/evolution.rs (185 周辺、Before)
let memory_id = self.store.save_memory(&content, "knowledge", &tags)?;
// Plan A1+A3 G-2.3: BONSAI_VEC_INDEX_ENABLED=1 で vec_memories へ動的 populate
// (env unset で no-op、既存挙動 100% 維持)
let _ = self.store.index_memory_if_enabled(memory_id, &content);
saved.push(entry.id.clone());

// After (clean)
let memory_id = self.store.save_memory(&content, "knowledge", &tags)?;
saved.push(entry.id.clone());
```

```rust
// src/agent/compaction.rs:638-640 (Before)
match store.save_memory(&combined, "context_flush", &["compaction".to_string()]) {
    Ok(memory_id) => {
        let _ = store.index_memory_if_enabled(memory_id, &combined);
    }
    Err(e) => eprintln!("[flush] メモリ保存失敗: {e}"),
}

// After (clean、`memory_id` 未使用化)
if let Err(e) = store.save_memory(&combined, "context_flush", &["compaction".to_string()]) {
    eprintln!("[flush] メモリ保存失敗: {e}");
}
```

注: clippy `single_match` / `let_underscore_must_use` 警告を回避するため `if let Err(e) = ...` 簡素化を採用。

### 2.3 想定 cargo check 結果

Phase 2 完了時:
- `cargo check --release --features embeddings` 成功 (unresolved ゼロ)
- `cargo test --release` 1150 passed (1158 - 8)

---

## 3. Phase 3 Refactor — import scrub + comment cleanup

### 3.1 import 整理 (各ファイルで未使用検出)

- `src/memory/store.rs`: `use std::sync::OnceLock;` (vec_index_ctx 内のみ使用) を削除
- `src/agent/compaction.rs`: 削除した integration test の `use crate::memory::vec_index_toggle::VEC_INDEX_TEST_LOCK;` 削除 (削除済 module の参照)
- `src/memory/evolution.rs`: callsite 周辺に `vec_index` 系 import なし (確認のみ)
- `src/memory/search.rs`: delegate 削除後の embedder/Result import 利用継続性確認

### 3.2 comment scrubbing

- `src/memory/evolution.rs`:
  - L183-184 「Plan A1+A3 G-2.3: BONSAI_VEC_INDEX_ENABLED=1 で...」削除
  - L253 同 pattern 削除
  - L271 同 pattern 削除
- `src/memory/store.rs:559 周辺` (削除済 method の docstring) 自動消滅
- `src/agent/compaction.rs:637` 「Plan A1+A3 G-2.3: env=1 で vec_memories 動的 populate (env unset で no-op)」削除
- 残置 comment の方針: 削除痕跡の comment は **書かない** (CLAUDE.md `// removed comments for removed code` 禁止規定遵守)

### 3.3 grep 検証 (Phase 3 末尾、Phase 4 直前)

```bash
rtk rg -n "index_memory_if_enabled|VecIndexCtx|vec_index_ctx|vec_index_toggle|BONSAI_VEC_INDEX_ENABLED|VEC_INDEX_TEST_LOCK" src tests
```

**期待**: 0 hits (immutable plan record `.claude/plan/sqlite-vec-a1-a3-impl.md` は src/tests 外なので非該当)。

```bash
rtk rg -n "index_memory_if_enabled|VecIndexCtx|vec_index_ctx|vec_index_toggle|BONSAI_VEC_INDEX_ENABLED|VEC_INDEX_TEST_LOCK" .claude/plan/
```

**期待**: `sqlite-vec-a1-a3-impl.md` および本 plan のみ hit (記録目的のため正常)。

---

## 4. Phase 4 Smoke — 4 gate verification

### 4.1 実行順序 (fast → slow)

```bash
# Gate 1: format
rtk cargo fmt --check
# 期待: no diff

# Gate 2: build
rtk cargo build --release --features embeddings 2>&1 | tail -5
# 期待: Finished `release` profile in <Xs>、warning ゼロ

# Gate 3: lib test
rtk cargo test --release 2>&1 | tail -10
# 期待: test result: ok. 1150 passed; 0 failed; <N> ignored

# Gate 4: vec0 infrastructure 健全性 (recall@k)
rtk proxy cargo test --release --features embeddings --test sqlite_vec_perf -- --ignored --nocapture 2>&1 | grep -E "G-4\.|recall|test result|p50|p99" | head -20
# 期待: G-4.1 / G-4.4 / G-4.5 全て PASS、recall@10/20/50 = 1.0000、p50/p99 既存値域内

# Gate 5: clippy
rtk cargo clippy -- -D warnings 2>&1 | tail -3
# 期待: warning ゼロ、`cargo clippy` 終了 code 0
```

### 4.2 fail パターンと対応

| Gate | 失敗例 | 対応 |
|---|---|---|
| 1 | trailing whitespace | `cargo fmt` で再生成 |
| 2 | unused import | Phase 3.1 で見落とし → 該当 import 削除 |
| 3 | test count != 1150 | 削除対象 test の漏れ or 余剰削除 → D-4 と diff |
| 4 | recall@k != 1.0 / vec0 panic | `ensure_vec_table` / `insert_memory_embedding` / `knn_search` への副作用混入 → revert + 削除順序見直し |
| 5 | clippy warn | unused variable / dead_code → match arm 簡素化 (Phase 2.2 の擬似コード採用) |

### 4.3 副次検証 (任意、ACCEPT 判定外)

- `rtk cargo test --release --features embeddings` の追加 features build PASS (削除で feature ガード残骸が無いか)
- `git diff HEAD~1 --stat | tail -5` で 純減算であることを確認 (insertion ≪ deletion)
- 期待 stat: 約 `9 files changed, ~10 insertions(+), ~290 deletions(-)`

---

## 5. Phase 5 Commit — atomic boundary

### 5.1 commit 構造 (2 commit、項目 216 と同 pattern)

#### Commit 1: refactor 本体

```
refactor(vec_index): G-4.2 REJECT 後の dead-code 削除 (項目 216 経路)

G-4.2 Lab paired smoke (3/4 PASS / RSS gate NG +99.9 MB) を受け、
`index_memory_if_enabled` 系 wiring を全削除。vec0 infrastructure
(`ensure_vec_table` / `insert_memory_embedding` / `knn_search` /
recall@k benches) は将来余地のため保持。

削除内容:
- src/memory/vec_index_toggle.rs (whole, 96 行)
- MemoryStore::index_memory_if_enabled (store.rs:559-586)
- VecIndexCtx + vec_index_ctx() (store.rs:589-603)
- 3 unit tests in store.rs (t_a1_2/3/6)
- HybridSearch::index_memory delegate (search.rs:123-139, 0 caller)
- 4 production callsites: evolution.rs (3) + compaction.rs (1)
- 1 integration test in compaction.rs (t_a1_5)
- mod.rs declaration

参照:
- handoff: session_2026_05_11_handoff.md
- 親 plan: .claude/plan/sqlite-vec-a1-a3-impl.md (immutable record)
- 本 plan: .claude/plan/sqlite-vec-wiring-removal-impl.md
- 同 pattern 前例: 項目 216 (ERL defaults OFF)

検証: 1158→1150 passed / clippy 0 / fmt clean / vec0 G-4.1/4.4/4.5 PASS
```

#### Commit 2: CLAUDE.md 項目 222 追記

```
docs(claude-md): 項目 222 追加 — sqlite-vec wiring 削除 (G-4.2 REJECT 後)

CLAUDE.md「直近項目」セクションに項目 222 を追加。
1 行サマリー: vec_index_toggle 全削除 + 4 callsite + delegate + 3 unit test
+ 1 integration test、1158→1150 passed、項目 216 経路。
```

### 5.2 push 戦略

- 本 plan 完遂後、累計 2 commit が origin/master に未 push 状態 (handoff 05-11 終了時 13 commits ahead はすでに push 済確認、本 session で +2)
- user 承認後 `git push origin master`

### 5.3 handoff 作成

- `~/.claude/projects/-Users-keizo-bonsai-agent/memory/session_2026_05_12_handoff.md` を新規作成
- 含む情報: G-4.2 REJECT 経路完遂、項目 222、test 1150 復元、deferred 残 (RSS gate 緩和 / chunk-level embedding は本 plan REJECT で moot 化、再起票時は別 plan)

---

## 6. Risks and Mitigations

| ID | Risk | Severity | Mitigation |
|---|---|---|---|
| **R1** | vec0 infrastructure 誤削除 (`ensure_vec_table` 等) | CRITICAL | D-3 STAYS 表で明示 + Phase 3.3 grep で `index_memory_if_enabled` 系のみ確認、`ensure_vec_table` 等の grep は意図的に対象外 |
| **R2** | Test 件数 drift | HIGH | D-4 で 4+3+1=8 件を逐次列挙 + Phase 4 Gate 3 で `1150` を assertion |
| **R3** | clippy 警告 (unused import / dead_code / match arm) | MEDIUM | Phase 3.1 imports + Phase 2.2 擬似コードで match arm 簡素化、Phase 4 Gate 5 で `-D warnings` |
| **R4** | A1+A3 系 comment 残骸 | LOW | Phase 3.2 comment scrubbing + Phase 3.3 grep `BONSAI_VEC_INDEX_ENABLED` で網羅確認 |
| **R5** | 観測動作変化 (後方互換性) | LOW | default OFF 状態は env unset = no-op で既存と完全同等、削除は純減算 (項目 216 と同経路) |
| **R6** | future chunk-level embedding 等の再導入で混乱 | LOW | `.claude/plan/sqlite-vec-a1-a3-impl.md` を immutable record として保持、本 plan を completion record として残す。再導入時は新 plan 起票 |
| **R7** | 削除順序起因の transient compile error | LOW | Phase 2.1 順序 (callsites → delegate → method → infrastructure → tests → module) で bottom-up 解消、Phase 1〜3 を 1 commit に統合し中間状態 commit 回避 |
| **R8** | hello.txt 修正 (`Hello World` → JSON) との混入 | LOW | 本 plan の commit 範囲に hello.txt 含めない。別途 user 確認 (本 session の関心事は dead-code 削除のみ) |

---

## 7. 残 (将来) deferred follow-ups

handoff 05-11 で「(将来)」と記載された TODO は本 plan 完遂で次の通り扱い:

- **RSS gate 緩和 plan (+50 → +100 MB)**: **moot**。本 plan で wiring 削除のため architectural constant も消滅。再導入時は別 plan で再評価。
- **chunk-level embedding plan**: **moot 〜 candidate**。vec0 infrastructure は保持しているため再活性化の余地あり。再導入時は新 plan 起票 (本 plan を「reset record」として参照)。

---

## 8. 工数見積

| Phase | 内容 | 見積 |
|---|---|---|
| 0 | 本 plan の synthesis (本ドキュメント) | 完了 (本 turn) |
| 1+2 | 削除実装 (TDD strict、9 項目 ~290 行 net delete) | ~30 min |
| 3 | import scrub + comment scrubbing + grep | ~10 min |
| 4 | 4 gate smoke | ~5 min (recall@k benches は ignored 含み 30s 程度) |
| 5 | 2 commits + CLAUDE.md 222 + handoff | ~15 min |
| **合計** | | **~1 h** |

handoff 05-11 主張「~0.5 day TDD strict」より小さく見積もれる根拠: 削除手順が完全に静的解析可能で、Phase 1 Red を新規 test ではなく compile error で代替できるため。

---

## 9. SESSION_ID (for /ccg:execute use)

- **CODEX_SESSION**: `019e108a-438b-7021-b22f-46f3e2109581` (architect role による plan draft、本 plan に統合済)
- **GEMINI_SESSION**: skip (frontend.md/reviewer.md のみで backend dead-code 削除タスクには不適、Codex backend authority に委譲)

---

## 10. 関連参照

- 親 plan (immutable): `.claude/plan/sqlite-vec-a1-a3-impl.md` §5 ACCEPT/REJECT mapping (REJECT 経路 = 本 plan)
- 起源 plan: `.claude/plan/sqlite-vec-activation-impl.md` §2 G-2.5 (caller 配線不要確定)
- 判断 plan: `.claude/plan/external-memory-oss-integration-judgment.md` (Step A 採用 / Step B Milvus Lite REJECT)
- 同 pattern 前例: `.claude/plan/erl-defaults-off-switch-impl.md` (項目 216、env name 反転 + default OFF 切替の TDD strict 5 phase)
- Cerememory env opt-in 三本柱: `.claude/plan/cerememory-decay-port-impl.md` (項目 217) / `.claude/plan/cerememory-review-state-v12-impl.md` (項目 218) / Phase G working cap (項目 219)
- handoff: `session_2026_05_11_handoff.md` (G-4.2 paired smoke 4 軸 gate / RSS architectural constant 解釈 / user 承認 A 採用)
