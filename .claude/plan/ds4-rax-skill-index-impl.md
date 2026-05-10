# Plan: ds4 Stage 2 — rax (Redis Adaptive Radix tree) を skill / heuristic prefix index 化

> **由来**: 親 plan `.claude/plan/ds4-insights-port-impl.md` §4.2 で Stage 2 として概要のみ記載されていた「ds4 同梱 rax (Redis Adaptive Radix tree、antirez 単独著作、103+14 KB) を bonsai SkillStore / HeuristicStore の prefix index に転用」を独立 plan として起票する。Stage 1 (KV cache wiring) と独立着手可、依存なし。
>
> **由来 research**: 本 session の ds4 deep dive (rax は ds4 内 `tool_id_replay_map` の `by_id` + `by_block` 2 つの rax として DSML ツール呼び出しブロックの byte-for-byte 再現に使用)
>
> **bonsai 文脈**: SkillStore (項目 13/179) は SQLite + LIKE 'prefix%' 経路、HeuristicStore (項目 213) は Lab v17 で pool 134 件達成 (項目 215)、tool_chain 文字列 prefix 検索が hot path。rax は **O(N)→O(key_len)** で scaling 線形性を保証。
>
> **関連 plan**:
> - `.claude/plan/ds4-insights-port-impl.md` (Stage 2 §4.2、本 plan の親)
> - `.claude/plan/cerememory-decay-port-impl.md` (Plan A、外部 OSS port pattern + env opt-in 確立)
> - `.claude/plan/cerememory-review-state-v12-impl.md` (Plan B、`BONSAI_*_ENABLED` env opt-in 詳細)
> - `.claude/plan/erl-heuristics-pool-impl-v2.md` (項目 213、HeuristicStore 前提実装)
> - `.claude/plan/event-repository-trait-impl.md` (項目 209、Repository trait 化前例 = 本 plan の SkillRepository / HeuristicRepository trait 化と同 pattern)
> - `.claude/plan/sqlite-vec-activation-impl.md` (項目 220-221、Lab paired smoke で REJECT → 項目 222 wiring 削除前例)

## Task Type
- [ ] Frontend
- [x] Backend (`memory/` 新規 `src/memory/rax_index.rs` + `SkillRepository` / `HeuristicRepository` trait 抽出 + 既存 `SkillStore` / `HeuristicStore` の find 系メソッドに rax 経路追加)
- [ ] Fullstack
- [x] Docs (CLAUDE.md 項目 224 候補 / `docs/THIRD_PARTY_LICENSES.md` rax 由来 + radix_trie crate ライセンス追記)

## 1. 背景

### 1.1 rax 概念 (antirez 1-pager)
- **Adaptive Radix tree** = 圧縮 prefix tree、共通 prefix を物理的に共有する radix tree の最適化版
- 同一 prefix の key 群を 1 ノード内 byte 配列で表現、子 pointer は分岐位置のみで保持 (空間効率 O(unique_prefix_len))
- ds4 同梱版 (`rax.c` 103 KB + `rax.h` 14 KB、antirez 単独著作 2017-2018) は **leaf bitmap 最適化** 含む (子が値のみで pointer 不要なら inline、最大 13 children まで)
- **計算量** (rax.h 269-287 行 Exported API):
  - `raxInsert(s, len, data)` / `raxFind(s, len)` / `raxRemove(s, len)`: **O(key_len)**、tree 内要素数 N に独立
  - `raxSeek("^", prefix, plen) → raxNext(...)`: prefix 一致範囲 iteration **O(plen + matches)**
  - `raxSize(rt)` / `raxFree(rt)`: O(1) / O(N) (count cache あり)

### 1.2 SQLite + LIKE 経路の特性 (現状の bonsai)
| 経路 | 計算量 | 想定 (heuristics N=134) | 想定 N=1000 |
|---|---|---|---|
| `WHERE name LIKE '?%'` (skill.rs:69) | O(N) full scan | 線形 | 線形 |
| `find_top_k_for_task` (heuristics.rs:206) | O(N) full scan + JSON parse | 線形 | 線形 |
| **rax (本 plan)** | **O(key_len)** | N 独立 | N 独立 |

**注**: 上記の「想定」は asymptotic な計算量議論であり、実測値は Phase 4 G-4.1 / G-4.2 smoke で初めて取得する。Phase 1 で benchmark 数値の事前 hardcode は行わない (R8 score 退行の検出は recall@10 と paired duration で行う)。

### 1.3 Lab v17 で pool 134 件達成 → scaling 想定
- Lab v17 = 12 cycle × 平均 75.4 min wall (項目 215、CLAUDE.md "Lab v17 結果" section)
- HeuristicStore.find_top_k_for_task は session 開始時 + 失敗 recovery 時 + post-cycle reflection で複数回呼出 (heuristics.rs:206 + agent_loop の inject hook)
- 134 row × cycle 内呼出回数 = 線形 scan の累積、N=1000 想定で線形比率は ~7.5x
- rax 化で **scaling 線形性を構造的に断つ** (実装難易度に対し効果は inject 頻度と N 依存)

### 1.4 bonsai 既存 prefix 検索 hot path
| ファイル | 行 | 関数 | 役割 |
|---|---|---|---|
| `src/memory/skill.rs` | 63-88 | `find_matching(query, limit)` | skill name + description LIKE prefix |
| `src/memory/heuristics.rs` | 206-265 | `find_top_k_for_task(task_context, k)` | trigger_patterns 部分一致 ranking |
| `src/memory/heuristics.rs` | 401-416 | `review_tick(now)` | next_review_at scheduler、prefix 不要 |
| `src/memory/skill.rs` | 170-215 | `promote_with_prefix` | dedup 用 tool_chain UNIQUE 検索 |

本 plan は `find_matching` / `find_top_k_for_task` の **既存 signature を不変**に保ちつつ、新規 method `find_by_prefix` / `find_by_tool_chain_prefix` を追加 (legacy SQL 経路は env unset で維持)。

## 2. 目的

1. **prefix 検索 O(N)→O(key_len) 化** — N 増大時の inject 線形性を構造的に断つ
2. **HeuristicStore pool scaling 対応** — Lab v17 134 件達成 (項目 215) → 想定 N=1000 への運用余裕確保
3. **SkillStore 拡張** — promote_with_prefix dedup 高速化 (副次)、項目 220-221 で sqlite-vec が REJECT された経緯と同様 Lab paired smoke で実効性検証

### 非目標
- **SQLite 削除しない** — rax は in-memory cache、SQLite は source-of-truth、両者並存で 2-way sync (legacy 経路は env unset で完全互換)
- **Lab paired smoke で REJECT 時は項目 222 (sqlite-vec wiring 削除) と同経路で wiring 削除** (§7 R4 + §15 で詳述)
- **rax の C コード自前 port は Phase 1 範囲外** (Phase 1 では `radix_trie` crate 採用、自前 port は §16 別 plan 候補として記録のみ)
- **VaultRepository / ExperienceStore への横展開は本 plan 範囲外** (Phase 5 SkillStore + HeuristicStore 限定、横展開は別 plan で Lab ACCEPT 後)
- **rax の Defrag iterator / RandomWalk / Compare** は port 対象外 (bonsai は insert/find/seek/iterate の 4 操作のみ必要)

## 3. 既存項目との関係

| 項目 | 関係 | 改修要否 |
|---|---|---|
| **13** SkillStore Phase 1 | trait 化 + find_by_prefix 新規追加 (signature 不変) | 拡張 |
| **179** SkillStore Phase 2 (promote_from_trajectory) | promote_with_prefix の dedup SELECT を rax 化候補 | Phase 5 任意 |
| **209** EventRepository trait 化 | 同 pattern: `SkillRepository` / `HeuristicRepository` trait 抽出 + Mock | 設計踏襲 |
| **213** ERL Heuristics Pool | HeuristicStore に find_by_tool_chain_prefix 新規追加 | 拡張 |
| **215** Lab v17 完走 (REJECT) | pool 134 件達成 = 本 plan の scaling 必要性根拠 | 参照のみ |
| **216** ERL defaults OFF 切替 | env opt-in pattern (`BONSAI_*_ENABLED`) を踏襲 | 設計踏襲 |
| **217** Cerememory decay port | `BONSAI_DECAY_ENABLED` env name 形式踏襲 | 設計踏襲 |
| **218** Cerememory ReviewState port | `BONSAI_REVIEW_ENABLED` env name 形式踏襲 | 設計踏襲 |
| **219** Working Memory Cap | `BONSAI_WORKING_CAP_ENABLED` env name 形式踏襲 | 設計踏襲 |
| **220** sqlite-vec Step A 採用 | 本 plan の Lab paired smoke ACCEPT 基準は同 pattern | 設計踏襲 |
| **221** sqlite-vec G-4.2 REJECT | RSS Δ+99.9 MB の architectural constant 学習 — rax index も同様の RSS 増を smoke で計測 | 教訓踏襲 |
| **222** sqlite-vec wiring 削除 | 本 plan REJECT 時の dead-code 削除手順は項目 222 と同経路 | 削除 pattern 踏襲 |

## 4. 設計

### 4.1 crate 選定

#### 候補比較
| crate | 由来 | API 適合 | 採用判定 |
|---|---|---|---|
| **`radix_trie`** | community 純 Rust | `Trie<String, V>` で `get_raw_descendant(prefix)` あり | **★ Phase 1 採用** |
| `qp-trie` | community | bytes-only、API 異なる | △ 次候補 |
| `patricia_tree` | community | `iter_prefix(prefix)` 直接、Generic | △ 次候補 |
| `rax-rs` | (未検証) | — | ✗ Phase 1 範囲外 |
| 自前 port (rax.c → Rust idiomatic) | antirez/rax MIT | full feature parity | ✗ Phase 1 範囲外 (§16) |

**Phase 1 では `radix_trie` を採用候補**。Phase 2 Green 着手時に上流 maintenance 状況・最新 version・downloads を再確認 (R1 mitigation step 1)。

#### 採用 API (`radix_trie`)
```rust
use radix_trie::{Trie, TrieCommon};

let mut trie: Trie<String, i64> = Trie::new();        // alloc
trie.insert("file_read -> shell".to_string(), 42);     // O(key_len)
let id = trie.get("file_read -> shell");               // O(key_len) → Option<&i64>
let removed = trie.remove("file_read -> shell");       // O(key_len)
let subtrie = trie.get_raw_descendant("file_read");    // prefix iter root、Option<SubTrie>
for (key, value) in subtrie.unwrap().iter() {          // O(plen + matches)
    /* ... */
}
let n = trie.len();                                     // O(1)
```

依存追加 (Cargo.toml):
```toml
[dependencies]
radix_trie = "0.2"  # MIT/Apache-2.0 dual、API stable since 0.1
```

クレート license = MIT/Apache-2.0 dual = bonsai に互換。`docs/THIRD_PARTY_LICENSES.md` に追記 (項目 217 で先行作成済 file)。

### 4.2 `SkillRepository` trait 抽出 (項目 209 EventRepository pattern と同形)

```rust
// src/memory/skill.rs に追加 (既存 struct はそのまま、trait 経由で拡張)
pub trait SkillRepository {
    fn save(&self, name: &str, description: &str, tool_chain: &str, trigger_patterns: &str) -> Result<i64>;
    fn find_matching(&self, query: &str, limit: usize) -> Result<Vec<Skill>>;       // 既存
    fn find_by_prefix(&self, prefix: &str, limit: usize) -> Result<Vec<Skill>>;     // **新規**、rax 経路
    fn list_all(&self) -> Result<Vec<Skill>>;
    fn promote_from_trajectory(&self, c: &TrajectoryCandidate) -> Result<Option<i64>>;
    // ... 既存 method (省略、heuristics.rs の HeuristicRepository と同形)
}

impl<'a> SkillRepository for SkillStore<'a> {
    fn find_by_prefix(&self, prefix: &str, limit: usize) -> Result<Vec<Skill>> {
        if rax_index::is_rax_enabled() {
            self.find_by_prefix_rax(prefix, limit)
        } else {
            self.find_by_prefix_legacy(prefix, limit)  // 内部で LIKE 'prefix%' SQL
        }
    }
    /* ... */
}
```

- 既存 `find_matching` (LIKE pattern) は触らない (signature 不変、API additive)
- `find_by_prefix` は **新規 method**、env=enabled で rax / env=unset で SQL `LIKE 'prefix%'` legacy
- caller は `find_matching` を継続使用、新規 hot path 出現時のみ `find_by_prefix` に移行

### 4.3 `HeuristicRepository::find_by_tool_chain_prefix` 追加

```rust
// src/memory/heuristics.rs に追加
pub trait HeuristicRepository {
    fn save(&self, advice: &str, triggers: &[String], src_session: Option<&str>, src_task: &str, category: &str) -> Result<i64>;
    fn find_top_k_for_task(&self, task_context: &str, k: usize) -> Result<Vec<Heuristic>>;
    fn find_by_tool_chain_prefix(&self, prefix: &str, limit: usize) -> Result<Vec<Heuristic>>;  // **新規**
    fn record_outcome(&self, id: i64, success: bool) -> Result<()>;
    fn prune(&self) -> Result<usize>;
    fn review_tick(&self, now: chrono::DateTime<chrono::Utc>) -> Result<Vec<i64>>;
    fn record_review(&self, id: i64, outcome: ReviewOutcome, now: chrono::DateTime<chrono::Utc>) -> Result<()>;
    // ... 項目 213/217/218 既存
}
```

`find_by_tool_chain_prefix` の用途:
- inject_heuristics で過去 cycle の similar tool_chain heuristic を retrieval
- promote_with_prefix の dedup (同一 tool_chain 検索) を skill 側と heuristic 側で共通化

### 4.4 SQLite との 2-way sync 設計

#### 4.4.1 新規 module `src/memory/rax_index.rs` (~250 行)

```rust
//! In-memory rax (Adaptive Radix tree) prefix index for SkillStore / HeuristicStore.
//!
//! rax 概念 由来: antirez/ds4 同梱 rax.c (Redis Adaptive Radix tree、MIT、2017-2018、
//! Salvatore Sanfilippo 単独著作)。本 module は radix_trie crate を採用、rax の
//! C 実装は未 port (Phase 1 範囲外、§4.1 比較表参照)。
//!
//! # 同期方針
//! - SQLite が source-of-truth (永続化)
//! - rax は in-memory cache (起動時再構築、insert/delete で 2-way sync)
//! - env unset で legacy SQL 経路維持 (default 観測動作完全互換)
//!
//! # 起動時再構築コスト (R2)
//! - 想定 skills N=200 / heuristics N=1000 の rebuild 時間は Phase 4 G-4.1 で計測
//! - benchmark 結果が +50 ms 超なら OnceLock → background thread に変更検討

use std::sync::Mutex;
use radix_trie::{Trie, TrieCommon};
use anyhow::Result;

/// `BONSAI_RAX_INDEX_ENABLED=1` (or "true"、case-insensitive) で rax 経路 opt-in。
///
/// production default = env unset = false 返却 = OFF (Cerememory 三本柱 + 項目 220
/// sqlite-vec と同 pattern)。Lab paired smoke ACCEPT 後の defaults 化は別 plan で
/// 1 commit (~30 min)、REJECT 時は項目 222 と同経路で全 wiring 削除 (§15)。
pub(crate) fn is_rax_enabled() -> bool {
    std::env::var("BONSAI_RAX_INDEX_ENABLED")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Skill 用 rax index (key = name、value = skill id)。
/// 起動時に SQLite から全 row 再構築、save/delete で 2-way sync。
pub struct SkillRaxIndex {
    inner: Mutex<Trie<String, i64>>,
}

impl SkillRaxIndex {
    /// 空 index 作成 (rebuild_from_db で一括 populate 想定)。
    pub fn new() -> Self { /* ... */ }

    /// SQLite から全 row 読み込んで rax 再構築 (起動時 1 回)。
    pub fn rebuild_from_db(&self, conn: &rusqlite::Connection) -> Result<usize> {
        let mut trie = self.inner.lock().expect("rax mutex poisoned");
        trie.clear();
        let mut stmt = conn.prepare("SELECT id, name FROM skills")?;
        let mut count = 0;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?;
        for row in rows {
            let (id, name) = row?;
            trie.insert(name, id);
            count += 1;
        }
        Ok(count)
    }

    /// 1 row 追加 (SkillStore::save 経由で呼出)。
    pub fn insert(&self, name: &str, id: i64) -> Result<()> { /* ... */ }

    /// 1 row 削除 (SkillStore::purge_expired 経由で呼出)。
    pub fn remove(&self, name: &str) -> Result<()> { /* ... */ }

    /// prefix 一致 key を返す (limit 上限)。
    /// O(prefix.len + matches.len)、SQL LIKE O(N) と比較。
    pub fn find_by_prefix(&self, prefix: &str, limit: usize) -> Result<Vec<i64>> {
        let trie = self.inner.lock().expect("rax mutex poisoned");
        let Some(subtrie) = trie.get_raw_descendant(prefix) else {
            return Ok(Vec::new());
        };
        let ids: Vec<i64> = subtrie.iter().take(limit).map(|(_, &id)| id).collect();
        Ok(ids)
    }

    /// 検証 / smoke 用、現在 row 数。
    pub fn len(&self) -> usize { /* ... */ }
}

/// Heuristic 用 rax index (key = tool_chain、value = heuristic id)。
/// SkillRaxIndex と同 API、value type も同 i64 (DB row id)。
pub struct HeuristicRaxIndex {
    inner: Mutex<Trie<String, i64>>,
}

// SkillRaxIndex と同 method 群 (rebuild_from_db, insert, remove, find_by_prefix, len)
// trigger_patterns JSON のうち "tool_chain" key を index に投入
```

#### 4.4.2 2-way sync 経路
| SQLite 操作 | rax 操作 | 呼出箇所 |
|---|---|---|
| `INSERT INTO skills` | `SkillRaxIndex::insert(name, id)` | `SkillStore::save` (env=enabled) |
| `UPDATE skills SET name = ...` | `remove(old) + insert(new, id)` | (現状 name 更新経路なし、Phase 4 でも未実装) |
| `DELETE FROM skills` | `SkillRaxIndex::remove(name)` | `SkillStore::purge_expired` |
| 起動時 / 初回 access | `rebuild_from_db(conn)` | `MemoryStore` 初期化 (`OnceLock` で lazy)|
| `INSERT INTO heuristics` | `HeuristicRaxIndex::insert(tool_chain, id)` | `HeuristicStore::save` |
| `DELETE FROM heuristics` | `HeuristicRaxIndex::remove(tool_chain)` | `HeuristicStore::prune` (3 経路、§4.4.3) |

#### 4.4.3 prune 経路の 3 ルート対応
HeuristicStore::prune (heuristics.rs:309) は (1) low-score + min-used / (2) 30 day idle / (3) excess > 200 の 3 SQL を発行。各削除に対し rax 側も同期削除する必要がある:
```rust
// Phase 2 Green 実装イメージ
fn prune(&self) -> Result<usize> {
    if rax_index::is_rax_enabled() {
        // 削除前に DELETE 対象 row の tool_chain を SELECT
        let to_remove_chains: Vec<String> = self.conn.prepare(
            "SELECT tool_chain FROM heuristics WHERE <legacy 3 経路と同条件>"
        )?.query_map(...)?.collect();
        // SQL DELETE 実行 (legacy と同 SQL)
        // rax remove
        for chain in &to_remove_chains {
            self.heuristic_rax.remove(chain)?;
        }
    } else {
        // 既存 SQL DELETE のみ (heuristics.rs:309-342 の 3 SQL そのまま)
    }
    /* ... */
}
```

### 4.5 env opt-in: `BONSAI_RAX_INDEX_ENABLED=1`

Cerememory 三本柱と同形:
| env value | 意味 |
|---|---|
| unset / empty | OFF (default、legacy SQL 経路) |
| `0` / `false` / `no` | OFF (明示的に無効化) |
| `1` / `true` / `True` / `TRUE` | ON (rax index 経路) |

`is_rax_enabled()` で 1 箇所集約、`SkillStore::find_by_prefix` / `HeuristicStore::find_by_tool_chain_prefix` / `save` / `prune` / `rebuild_from_db` の各 hot path で評価。

### 4.6 env OFF で legacy SQL 経路維持 (default、既存挙動 100% 互換)

env unset で:
- `find_by_prefix` → SQL `WHERE name LIKE ?prefix||'%'` legacy
- `find_by_tool_chain_prefix` → SQL legacy
- `save` / `prune` → rax index への insert/delete を skip (no-op)
- `rebuild_from_db` → 呼出されない (lazy init guarded by env)

**観測動作完全互換** = production default 安全性 (Cerememory 三本柱 + 項目 220 sqlite-vec と同設計)。

### 4.7 R2 lazy init の OnceLock 採用

```rust
use std::sync::OnceLock;

pub struct MemoryStore {
    /* 既存 fields */
    skill_rax: OnceLock<SkillRaxIndex>,
    heuristic_rax: OnceLock<HeuristicRaxIndex>,
}

impl MemoryStore {
    /// rax index への遅延初期化 access (初回呼出で rebuild_from_db、以降 cache hit)。
    /// env unset では呼ばれない (caller が is_rax_enabled で gate)。
    pub fn skill_rax(&self) -> Result<&SkillRaxIndex> {
        Ok(self.skill_rax.get_or_init(|| {
            let idx = SkillRaxIndex::new();
            let _ = idx.rebuild_from_db(self.conn());  // 起動時 1 回
            idx
        }))
    }
    pub fn heuristic_rax(&self) -> Result<&HeuristicRaxIndex> { /* ... */ }
}
```

項目 221 G-4.2 REJECT で OnceLock embedder の +99.9 MB RSS が architectural constant として観測された前例あり (sqlite-vec)。本 plan は `Trie<String, i64>` × 2 = N=2000 row で **想定 ~200 KB** (key 平均 30 chars × 2000 row × 2 trees + node overhead)、実測は G-4.3 で確証必須 (+50 MB 超で要 architectural review)。

## 5. TDD strict 5 phase (test ≥ 6 件)

### Phase 1 — Red (新規 ~12 test、当初要件 6 件超達成)

**`src/memory/rax_index.rs` 純関数 / index 単体 5 test**:
1. `t_is_rax_enabled_default_false` — env unset で false (production default)
2. `t_is_rax_enabled_explicit_true` — env=1 で true
3. `t_skill_rax_insert_then_find` — insert + find_by_prefix で見つかる
4. `t_skill_rax_remove_then_not_found` — remove 後に見つからない
5. `t_skill_rax_rebuild_from_empty_db` — 空 SQLite から rebuild → len=0

**`SkillStore` 統合 4 test**:
6. `t_skill_find_by_prefix_legacy_when_disabled` — env unset で SQL LIKE 経路、N=3 row で正常動作
7. `t_skill_find_by_prefix_rax_when_enabled` — env=1 で rax 経路、N=3 row で正常動作
8. `t_skill_save_syncs_to_rax_when_enabled` — env=1 で save 後 rax にも入る
9. `t_skill_purge_removes_from_rax` — purge_expired で rax 側も削除される

**`HeuristicStore` 統合 3 test**:
10. `t_heuristic_find_by_tool_chain_prefix_rax` — env=1 で N=5 heuristic から prefix 一致 2 件返却
11. `t_heuristic_prune_removes_from_rax` — prune 経路 (1)(2)(3) 全てで rax 同期確証
12. `t_heuristic_rebuild_idempotent` — rebuild_from_db を 2 回呼出して同 len、重複なし

env mutation race を避けるため module-local `Mutex` で serialize (項目 217/218 と同 pattern、heuristics.rs:1127 `ERL_TEST_LOCK` を template)。

期待: compile error (新規 module / `find_by_prefix` 未定義 / `SkillRepository` trait 未定義) → Red 確認。

### Phase 2 — Green
1. `Cargo.toml` に `radix_trie = "0.2"` 追加 (Phase 2 着手時に最新 version 再確認、R1)
2. `src/memory/rax_index.rs` 新規 (~250 行、SkillRaxIndex + HeuristicRaxIndex + is_rax_enabled)
3. `src/memory/skill.rs` に `SkillRepository` trait + `find_by_prefix` 実装 (rax / legacy 2 経路)
4. `src/memory/heuristics.rs` に `HeuristicRepository` trait + `find_by_tool_chain_prefix` 実装
5. `src/memory/store.rs` に `OnceLock<SkillRaxIndex>` + `OnceLock<HeuristicRaxIndex>` field 追加
6. `SkillStore::save` / `purge_expired` / `HeuristicStore::save` / `prune` に rax sync hook 追加 (env-gate)
7. `docs/THIRD_PARTY_LICENSES.md` に `radix_trie` MIT/Apache-2.0 + rax 由来コメント追記

期待: 既存 1150 + 新規 12 = **1162 passed** / clippy 0 / fmt 0 / Cerememory 三本柱と同 pattern (env unset で観測動作完全互換)

### Phase 3 — Refactor
- `SkillRepository` / `HeuristicRepository` trait を `pub` に昇格 (項目 209 EventRepository と同 visibility)
- `MockSkillRepository` / `MockHeuristicRepository` 追加 (Phase 5 dividend、項目 209 と同 pattern、`src/memory/mocks/` 既存 module 拡張)
- `find_by_prefix` の `Mutex<Trie>` 取得 race を test-local Mutex で serialize (項目 217/218 と同 pattern)
- docstring に rax 由来 + radix_trie crate 由来 + Lab paired smoke ACCEPT 後の defaults 化条件明記

### Phase 4 — Smoke 検証 (Lab paired smoke、~80 min)

**G-4.1 起動時再構築 latency (R2)**:
```bash
# heuristics 1000 row 投入後の rebuild 計測
cargo bench --bench rax_rebuild  # heuristics 1000 row → rebuild_from_db elapsed
# 期待: < 50 ms (起動時間想定 ~2 s に対し 2.5% 以下)
```

**G-4.2 Lab paired smoke (core 22 / k=3、~80 min)**:
```bash
# OFF (legacy SQL)
./target/release/bonsai --lab --lab-experiments 0 --core 2>&1 | tee /tmp/rax_off.log
# ON (rax)
BONSAI_RAX_INDEX_ENABLED=1 ./target/release/bonsai --lab --lab-experiments 0 --core 2>&1 | tee /tmp/rax_on.log
# 比較
python3 scripts/compare_paired_smoke.py /tmp/rax_off.log /tmp/rax_on.log
```

ACCEPT 基準 (sqlite-vec G-4.2 と同基準、項目 220):
- ✅ score Δ ±0.02 以内 (退行なし)
- ✅ duration -50% 以上 (find_top_k_for_task hot path で測定)
- ✅ stability Δ ±0.05 以内
- ✅ utilization (rax index hit > 0)

**G-4.3 RSS 計測** (項目 221 教訓踏襲):
- 起動直後 RSS と heuristics 1000 row rebuild 後の RSS 差を計測
- 期待: +1 MB 以下 (Trie<String,i64> × 2、N=2000、key 平均 30 chars、想定値)
- +50 MB 超で **要 architectural review** (項目 221 と同基準)

**G-4.4 recall@10** (項目 220 G-4.5 と同):
- SQL LIKE 結果と rax 結果の重複率 = 1.0000 期待 (rax は exact prefix match)

### Phase 5 — Lab paired smoke ACCEPT 判定 + commit/handoff

ACCEPT 時 (G-4.2 全条件 PASS):
- 5 commits:
  1. `test(rax-index): Phase 1 Red — SkillRaxIndex + HeuristicRaxIndex test`
  2. `feat(rax-index): Phase 2 Green — radix_trie + 2-way sync + env opt-in`
  3. `refactor(rax-index): Phase 3 — trait pub + Mock + test mutex + docstring`
  4. `docs(rax-index): docs/THIRD_PARTY_LICENSES.md + memory/ds4_alignment.md 追記`
  5. `docs(claude.md): 項目 224 — ds4 Stage 2 rax skill index ACCEPT`
- handoff 起票 + CLAUDE.md 項目 224 + INDEX.md "🆕 外部 OSS 取込み" 行追加

REJECT 時 (G-4.2 score Δ < -0.02 or duration -50% 未達 or RSS Δ > +50 MB or recall < 1.0):
- §15 失敗時 handling 経路: 項目 222 (sqlite-vec wiring 削除) と同 pattern
- `BONSAI_RAX_INDEX_ENABLED` env-gate 経路 + sync hook + rax_index module を全削除
- `radix_trie` crate dep を Cargo.toml から削除
- legacy SQL 経路 + SkillRepository / HeuristicRepository trait は保持判断 (Mock 利用済 test ≥ 1 件で残置、項目 209 dividend pattern)

## 6. API 影響 (additive 確証)

| modulo path | 関数 / 構造体 | 種別 |
|---|---|---|
| `crate::memory::rax_index::is_rax_enabled` | pub(crate) fn | 新規 |
| `crate::memory::rax_index::SkillRaxIndex` | pub struct + 5 method | 新規 |
| `crate::memory::rax_index::HeuristicRaxIndex` | pub struct + 5 method | 新規 |
| `crate::memory::skill::SkillRepository` | pub trait | 新規 (項目 209 EventRepository pattern) |
| `crate::memory::heuristics::HeuristicRepository` | pub trait | 新規 |
| `SkillStore::find_by_prefix` | pub method | 新規 |
| `HeuristicStore::find_by_tool_chain_prefix` | pub method | 新規 |
| `MemoryStore::skill_rax` / `heuristic_rax` | pub(crate) fn (lazy OnceLock) | 新規 |
| `SkillStore::find_matching` | signature 不変 | **既存維持** |
| `HeuristicStore::find_top_k_for_task` | signature 不変 | **既存維持** |
| `SkillStore::save` / `purge_expired` | signature 不変、内部で env-gate sync | 拡張 |
| `HeuristicStore::save` / `prune` | signature 不変、内部で env-gate sync | 拡張 |
| env `BONSAI_RAX_INDEX_ENABLED` | 新規 | ✅ default 未設定で既存挙動 |
| SQLite | 変更なし (rax は in-memory cache、SQLite source-of-truth 不変) | — |
| TSV / Lab metric | 変更なし (新規 metric 追加せず、既存 duration_secs で観測) | — |

**signature 変更ゼロ** = 全 additive、項目 205 のような必須化はなし、Cerememory 三本柱 + sqlite-vec wiring と同 pattern。既存 caller 無変更。

## 7. Risks / Mitigations

| # | Risk | 影響 | Mitigation |
|---|---|---|---|
| **R1** | `radix_trie` crate 依存追加 (上流 maintenance 不安定リスク) | 上流 maintenance 切れ時に migration 必要 | (i) Phase 2 Green 着手時に crate downloads / 最新 version / commit history を再確認 (ii) MIT/Apache-2.0 dual で fork 可能 (iii) API 表面が薄く (insert/get/remove/get_raw_descendant) `qp-trie` / `patricia_tree` への置換コスト低 (iv) 自前 port (rax.c → Rust) は §16 別 plan で起票可 |
| **R2** | 起動時 rax 再構築コスト (heuristics N=1000 で想定 ~25 ms、実測は G-4.1 で取得) | 起動 latency 増 | (i) `OnceLock` lazy init で env unset 時は再構築 skip (ii) G-4.1 smoke で計測、+50 ms 超で OnceLock → background thread に変更検討 (iii) bonsai 起動時間に対する許容比は G-4.1 結果で判定 |
| **R3** | SQLite と rax の sync ズレ (insert SQL 成功 + rax insert 失敗等) | find_by_prefix が stale 結果返却 | (i) `Mutex<Trie>` で 1-by-1 排他 (ii) SQL 実行 → rax 更新の順序、rax 更新失敗は anyhow Err で伝播 (Phase 2 Green 実装方針) (iii) 整合性疑い時に `rebuild_from_db` で全再構築可能 (iv) 統合 test (#9, #11) で 4 経路 sync 確証 (v) `purge_expired` の rax remove は SQL DELETE 後、失敗時は次 rebuild で復旧 |
| **R4** | Lab paired smoke REJECT 時の wiring 削除手順 | 削除残骸が dead-code 化 | (i) 項目 222 (sqlite-vec wiring 削除) と同経路 (ii) §15 失敗時 handling で 4 段階削除手順明記 (iii) trait 抽出は Mock dividend として保持判断可 |
| **R5** | `Mutex<Trie>` の lock 競合 (find_by_prefix と save の同時呼出) | レイテンシ尖端増 | (i) Phase 1 では `Mutex` 採用、`RwLock<Trie>` への切替は Phase 4 latency 計測後に判断 (ii) lock 内処理は O(key_len + matches) で短時間、競合は Phase 1 想定範囲 (iii) Phase 4 G-4.2 で latency p99 計測候補 |
| **R6** | rax key 衝突 (skill name 重複 = SkillStore::save の UPDATE 経路) | 古い id が rax に残る | (i) SkillStore::save の UPDATE 経路では rax に既存 entry あり = `Trie::insert` で値更新 (ii) `Trie::insert` は same-key で値置換 (iii) test #8 で確証 |
| **R7** | RSS 増加 (Trie<String, i64> × 2、N=2000) | 項目 221 と同様 architectural constant 化 | (i) 想定 ~200 KB (key 平均 30 chars × 2000 row × 2 trees + node overhead) (ii) +50 MB 超で REJECT (項目 221 基準踏襲) (iii) G-4.3 で計測必須 |
| **R8** | Lab paired smoke で score 退行 (env=1 で誤一致 / 順序差) | ACCEPT 失敗 | (i) `find_by_prefix` は exact prefix match (LIKE 'prefix%' と数学的等価) (ii) recall@10 = 1.0000 を G-4.4 で確証 (iii) 順序保証は legacy SQL の `ORDER BY score DESC` を rax 経路でも post-fetch sort 適用 |

## 8. Quality Gates

- **G-1 Phase 1 Red**: 12 新規 test compile error or 全 fail (`cargo test rax_index skill heuristics 2>&1 | grep "test result"`)
- **G-2 Phase 2 Green**: 12 新規 test PASS + 既存 1150 維持 = **1162 passed** / clippy 0 / fmt 0 / env unset で既存全 test 退行ゼロ
- **G-3 Phase 3 Refactor**: trait pub + Mock 追加 + docstring 完備 + test mutex + 既存 test 退行ゼロ
- **G-4 Phase 4 Smoke 4 段**:
  - **G-4.1**: 起動時 rebuild latency < 50 ms (heuristics N=1000 想定、実測必須)
  - **G-4.2 Lab paired smoke** (core 22, k=3): score Δ ±0.02 以内 + **duration -50% 以上** (rax hot path) + stability Δ ±0.05 以内 + utilization > 0
  - **G-4.3 RSS**: env=1 で +1 MB 以下、+50 MB 超で要 architectural review (項目 221 教訓)
  - **G-4.4 recall@10**: SQL LIKE と rax 結果の重複率 = 1.0000
- **G-5 Final**: handoff 起票 + CLAUDE.md 項目 224 候補 + `docs/THIRD_PARTY_LICENSES.md` 追記 + INDEX.md 行追加 + Phase 5 commit 完遂

## 9. 完了条件 (10 項目)

1. ✅ `Cargo.toml` に `radix_trie = "0.2"` 追加 (Phase 2 着手時に最新 version 再確認)
2. ✅ `src/memory/rax_index.rs` 新規 (SkillRaxIndex + HeuristicRaxIndex + is_rax_enabled)
3. ✅ `BONSAI_RAX_INDEX_ENABLED=1` env reader 実装 + `is_rax_enabled()` helper
4. ✅ `SkillRepository` / `HeuristicRepository` trait 抽出 + `find_by_prefix` 系新規 method 実装 (rax / legacy 2 経路)
5. ✅ `SkillStore::save` / `purge_expired` / `HeuristicStore::save` / `prune` に rax sync hook 追加 (env-gate、observable 不変)
6. ✅ `MemoryStore` に `OnceLock<SkillRaxIndex>` / `OnceLock<HeuristicRaxIndex>` field 追加 + lazy init accessor
7. ✅ smoke G-4.1〜G-4.4 全 PASS (G-4.2 paired smoke で score ±0.02 + duration -50% + RSS +1 MB 以下 + recall=1.0000)
8. ✅ 1162+ passed 維持 / clippy 0 / fmt 0
9. ✅ CLAUDE.md 項目 224 + handoff 起票 + INDEX.md 行追加 + `docs/THIRD_PARTY_LICENSES.md` 追記
10. ✅ Lab paired smoke REJECT 時は §15 失敗時 handling で項目 222 同経路 wiring 削除

## 10. 見積もり

| Phase | 内容 | 時間 |
|-------|------|------|
| Phase 1 | Red — 12 test 追加 (rax_index 5 + skill 4 + heuristic 3)、cargo test Red 確認 | 1h |
| Phase 2 | Green — radix_trie crate 追加 + rax_index.rs 250 行 + trait 抽出 + sync hook | 4h |
| Phase 3 | Refactor — trait pub + Mock 追加 + test mutex + docstring | 1h |
| Phase 4 | Smoke 4 段 (G-4.1 bench + G-4.2 paired smoke 80 min + G-4.3 RSS + G-4.4 recall) | 3h (実機 wall 1.5h) |
| Phase 5 | Commit (5 件) + handoff + CLAUDE.md 項目 224 + INDEX.md + license docs | 1.5h |
| Buffer | radix_trie API 検証 + sync race 検出 + RSS variance 確認 | 1.5h |
| **合計** | | **~12h ≈ 1.5 day** |

ACCEPT 時の defaults 化 (production default ON 切替) は別 plan / 1 commit (~30 min、後続セッション)。
REJECT 時の wiring 削除は別 plan で実施 (~2h、項目 222 と同 pattern、§15 で詳述)。

## 11. Quick Start

```bash
# 0. 着手前 verify
cargo test --lib skill heuristics 2>&1 | tail -5  # baseline 1150 passed
rtk grep -rn "BONSAI_.*_ENABLED" src/  # Cerememory 三本柱 + ERL の env pattern 確認
rtk grep -rn "radix_trie\|rax_index" src/  # 期待 0 件
ls src/memory/  # rax_index.rs 不存在確認

# 1. Phase 1 Red
$EDITOR Cargo.toml                    # radix_trie = "0.2" 追加
$EDITOR src/memory/rax_index.rs       # SkillRaxIndex + HeuristicRaxIndex (todo!() panic)
$EDITOR src/memory/skill.rs           # SkillRepository trait + find_by_prefix test (todo!())
$EDITOR src/memory/heuristics.rs      # HeuristicRepository trait + find_by_tool_chain_prefix test (todo!())
$EDITOR src/memory/mod.rs             # pub mod rax_index;
cargo test --lib --release rax_index skill heuristics 2>&1 | grep "test result"
# 期待: compile error or test fail (Red 確認)

# 2. Phase 2 Green
# rax_index.rs の todo!() を radix_trie::Trie 経由で実装
# skill.rs / heuristics.rs に find_by_prefix 系メソッド + sync hook
$EDITOR src/memory/store.rs           # OnceLock<SkillRaxIndex> + lazy accessor
cargo test --lib --release && cargo clippy --lib --tests -- -D warnings && cargo fmt --check
# 期待: 1150 → 1162 passed

# 3. Phase 3 Refactor
$EDITOR src/memory/skill.rs           # SkillRepository trait pub + MockSkillRepository
$EDITOR src/memory/heuristics.rs      # HeuristicRepository trait pub + MockHeuristicRepository
$EDITOR src/memory/rax_index.rs       # docstring + Cerememory 三本柱 reference

# 4. Phase 4 Smoke
cargo bench --bench rax_rebuild       # G-4.1 起動時 rebuild latency
# G-4.2 paired smoke (要 user 操作: llama-server 起動)
./target/release/bonsai --lab --lab-experiments 0 --core 2>&1 | tee /tmp/rax_off.log
BONSAI_RAX_INDEX_ENABLED=1 ./target/release/bonsai --lab --lab-experiments 0 --core 2>&1 | tee /tmp/rax_on.log
python3 scripts/compare_paired_smoke.py /tmp/rax_off.log /tmp/rax_on.log
# G-4.3 RSS 計測 (Activity Monitor or `ps -o rss`)
# G-4.4 recall@10
cargo test --lib --release rax_recall

# 5. Phase 5 Commit (ACCEPT 時、5 commits)
git add Cargo.toml src/memory/rax_index.rs src/memory/skill.rs src/memory/heuristics.rs src/memory/store.rs src/memory/mod.rs
git commit -m "test(rax-index): Phase 1 Red"
git commit -m "feat(rax-index): Phase 2 Green — radix_trie + 2-way sync + env opt-in"
git commit -m "refactor(rax-index): Phase 3 — trait pub + Mock + test mutex"
git commit -m "docs(rax-index): THIRD_PARTY_LICENSES.md + memory/ds4_alignment.md 追記"
$EDITOR /Users/keizo/bonsai-agent/CLAUDE.md  # 項目 224
$EDITOR /Users/keizo/bonsai-agent/.claude/plan/INDEX.md  # rax 行追加

# 5'. REJECT 時 (§15 失敗時 handling)
# 項目 222 と同経路で wiring 削除 plan を別途起票
# .claude/plan/ds4-rax-skill-index-removal-impl.md
```

## 12. 参考

- antirez/ds4 (https://github.com/antirez/ds4) — DeepSeek V4 Flash inference engine、rax 同梱元
- antirez/rax (https://github.com/antirez/rax) — Redis Adaptive Radix tree 単独 repo (2017-2018、MIT)
- `rax.h` API 一覧 (本 session fetch 済 `/tmp/rax_h.h`、本 plan §1.1)
- `radix_trie` crate (https://crates.io/crates/radix_trie) — MIT/Apache-2.0、Phase 1 採用候補
- bonsai 親 plan: `.claude/plan/ds4-insights-port-impl.md` §4.2 (Stage 2 概要)
- bonsai 既存 plan: `cerememory-decay-port-impl.md` (Plan A、外部 OSS port pattern)
- bonsai 既存 plan: `cerememory-review-state-v12-impl.md` (Plan B、env opt-in pattern)
- bonsai 既存 plan: `event-repository-trait-impl.md` (項目 209、trait 抽出 pattern)
- bonsai 既存 plan: `sqlite-vec-activation-impl.md` (項目 220、Lab paired smoke pattern)
- bonsai CLAUDE.md 項目 13/179 (SkillStore 前提実装)
- bonsai CLAUDE.md 項目 209 (EventRepository trait 化、Mock dividend pattern)
- bonsai CLAUDE.md 項目 213 (HeuristicStore Phase 2 Green、前提実装)
- bonsai CLAUDE.md 項目 215 (Lab v17 完走、pool 134 件達成 = 本 plan の scaling 必要性根拠)
- bonsai CLAUDE.md 項目 217-219 (Cerememory 三本柱、env opt-in pattern 手本)
- bonsai CLAUDE.md 項目 220-222 (sqlite-vec 採否経緯、Lab paired smoke + 失敗時 wiring 削除前例)
- 派生 plan 候補 (本 plan ACCEPT 後起票):
  - `ds4-tool-id-replay-impl.md` (Stage 3、親 plan §4.3 で Stage 1 完遂後起票予定)
  - `rax-vault-experience-extension.md` (Phase 5 横展開、VaultRepository / ExperienceStore への rax 適用)

## 13. SESSION_ID (for /ccg:execute use)

- CODEX_SESSION: 新規取得 (本 plan は ds4 Stage 2 専用、親 plan + Cerememory 三本柱とは独立 session)
- GEMINI_SESSION: 任意

## 14. 着手前チェックリスト

1. [ ] 親 plan `ds4-insights-port-impl.md` Stage 1 ACCEPT 確認 (Stage 2 は独立着手可だが、親 plan の方針確定推奨)
2. [ ] `cargo test --lib skill heuristics --release` で 1150 passed baseline
3. [ ] `radix_trie` crate 最新版 / 上流 maintenance 状況 確認 (R1)
4. [ ] `docs/THIRD_PARTY_LICENSES.md` 既存確認 (項目 217 で先行作成済 file 拡張)
5. [ ] CODEX_SESSION 新規取得 (`/ccg:execute` 経由実装時)

## 15. ★ 失敗時 (Phase 4 Smoke G-4.2 REJECT) handling

Lab paired smoke で以下のいずれか NG:
- score Δ < -0.02 (退行)
- duration -50% 未達 (rax 効果なし)
- RSS Δ > +50 MB (項目 221 architectural constant 超過)
- recall@10 < 1.0000 (rax と SQL LIKE 結果不一致)

→ 項目 222 (sqlite-vec wiring 削除) と同経路で **段階的 wiring 削除**:

### 15.1 削除対象 (4 段階)
| 段階 | 対象 | 理由 |
|---|---|---|
| **P1** | env-gate 経路 (`if rax_index::is_rax_enabled()` 全箇所) | rax 動作停止、legacy SQL 経路のみ残置 |
| **P2** | sync hook (`SkillStore::save` / `purge_expired` / `HeuristicStore::save` / `prune` の rax insert/remove call) | 2-way sync 撤去 |
| **P3** | `SkillRaxIndex` / `HeuristicRaxIndex` struct + `rax_index.rs` module + `MemoryStore::skill_rax/heuristic_rax` field | 全 wiring 撤去 |
| **P4** | `Cargo.toml` から `radix_trie` 依存削除 | 依存 pruning |

### 15.2 削除しない (D-3 STAYS、項目 222 と同 pattern)
- **`SkillRepository` / `HeuristicRepository` trait** — Mock 経由 SQLite なし unit test の dividend (Mock 利用済 test が trait に依存していれば残置、項目 209 EventRepository と同)
- **legacy SQL 経路 (`find_by_prefix_legacy` 等)** — 既存 caller の API 互換維持、trait method 経由で SQL 実装のみ残す
- **`MockSkillRepository` / `MockHeuristicRepository`** — Phase 3 で追加した Mock は 項目 209 dividend として継続利用

### 15.3 削除手順
1. 別 plan `.claude/plan/ds4-rax-skill-index-removal-impl.md` 起票 (TDD strict 5 phase、項目 222 plan を template)
2. P1〜P4 の各削除を 1 commit ずつ (累計 4 commits、項目 222 は 6 commits)
3. 削除後 cargo test で 1150 → 1162 → 1150 passed (新規 12 test も削除、退行ゼロ)
4. CLAUDE.md 項目 225 候補 (REJECT 後経路) + handoff 起票
5. INDEX.md "🆕 外部 OSS 取込み" の rax 行を撤去 (sqlite-vec wiring 行と同 pattern)

### 15.4 trait 残置の判断基準
- Mock test が 1 件以上残っていれば trait 残置 (項目 209 dividend)
- Mock test ゼロなら trait も削除 (`SkillRepository` / `HeuristicRepository` の存在意義喪失)

## 16. 補足: 自前 rax port (Phase 1 範囲外、別 plan 候補)

Phase 1 で `radix_trie` crate 採用が REJECT された場合 (R1 上流不安定 / API 不適合) の代替案:
- antirez/rax C 実装 ~3500 行 (rax.c) を Rust idiomatic に逐語 port (`unsafe` 局所化、Box<RaxNode> 中心)
- license MIT (rax/LICENSE)、bonsai 取込 OK、Cerememory 三本柱と同 attribution pattern
- 工数 ~3 day (rax.c の defrag / iterator / compare 全機能 port、bonsai は前 4 機能のみ必要なら ~1.5 day)
- 別 plan `ds4-rax-port-from-c.md` 起票候補 (Phase 1 ACCEPT 時は不要、REJECT で `radix_trie` 不採用時のみ)
- 本 plan 範囲外、Phase 1 採否確定後の判断

---

**Status**: ★ 起票直後 (2026-05-10)、production code 変更ゼロ、実装は別 session
**Trigger**: 親 plan `ds4-insights-port-impl.md` Stage 1 完遂時または Lab v17 完走後の手空き時
**Owner**: Stage 2 独立 plan、Stage 1 / 3 と並行着手可
