# Disk-Backed Compaction Checkpoint (antirez/ds4 inspired)

**状態**: planning-only (未起票)、推奨度 ★★ (Lab v18+ 長時間 run の中断耐性向上)
**推定工数**: ~5h (TDD strict 5 phase、SCHEMA migration なし、SQLite session_snapshot table 新規)
**起点**: antirez/ds4 server の "KV cache on disk, first-class disk citizen" 設計

## §1. 背景 — Bonsai Lab cycle の中断問題

### ds4 が解いた問題
DeepSeek V4 Flash の場合、長文 prefill (>10K token) は数秒〜数十秒かかる。server 再起動や
unrelated request による live KV eviction が起きると、その prefill が完全に無駄になる。
ds4 server は 4 trigger で `disk KV checkpoint` を save:

- `cold`: 長い first prompt が安定 prefix に到達 (generation 開始前)
- `continued`: prefill / generation が次の絶対 frontier に到達 (2048-token chunk alignment)
- `evict`: 関係ない request が live session を置換する直前
- `shutdown`: server 終了時

ファイル名は rendered text の SHA1、prefix matching で reuse 判定。

### Bonsai 現状 (項目 25 / 81 / 187)
- `src/agent/checkpoint.rs`: **git stash** ベースの session checkpoint (file-level)
- `src/agent/compaction.rs`: 4 段 context compaction (項目 6)、AI+Tool ペア保護 (項目 78)
- `src/agent/event_store.rs`: Event Sourcing で event stream 全永続化 (項目 209)
- **gap**: Lab cycle 中の compaction 結果 (中間 LoopState) が in-process のみで、cycle 終了で消失
- Lab v17 (15h 37min) / v22 想定 (~22-23h) で **途中中断 → 再開コスト = 0 (再走必須)**

### 必要性 (Lab v18+ 視点)
- Lab v18 wall ~22-23h を 24h サイクルで回すには、夜間 unattended run の resilience が必須
- 既存 git stash 経路は file system 状態のみ、agent runtime state (Session.messages / heuristics pool /
  hindsight queue) は cover していない
- ds4 思想 = **state の disk persistence は first-class** を Bonsai に持ち込む

## §2. 設計 (案 = SQLite-backed snapshot)

### スコープ
- 対象: `LoopState` (`session: Session`, `task_state: TaskState`, `compaction_history`, `heuristics: Option<HeuristicsCarry>`)
- 非対象: KV cache 自体 (llama-server 側責務、項目 167)
- 永続化先: 既存 `MemoryStore` SQLite に新規 table `loop_snapshots`

### Trigger (ds4 直訳)
- `cold`: 初回 compaction 直前 (Session.messages.len() > N_chat_min かつ first compaction)
- `continued`: 各 compaction 終了直後 (interval = N_steps、default 2048-eq)
- `evict`: 該当なし (Bonsai は in-process single session、外部 evict 発生しない)
- `shutdown`: agent_loop 正常終了 / cancel.rs 経由の graceful shutdown

3 trigger に絞る (`evict` skip)。

### Schema (SCHEMA_V16 で追加、本 plan 内では migration 必要)
```sql
CREATE TABLE loop_snapshots (
    id INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL,
    trigger TEXT NOT NULL,  -- 'cold' | 'continued' | 'shutdown'
    saved_at INTEGER NOT NULL,  -- unix epoch seconds
    step_count INTEGER NOT NULL,
    session_payload BLOB NOT NULL,  -- bincode-serialized LoopState
    rendered_text_hash TEXT NOT NULL,  -- SHA1 prefix-match key
    rendered_text_bytes INTEGER NOT NULL,
    cache_reason TEXT  -- optional human-readable reason
);
CREATE INDEX idx_loop_snapshots_session ON loop_snapshots(session_id);
CREATE INDEX idx_loop_snapshots_hash ON loop_snapshots(rendered_text_hash);
```

### Reuse 判定 (ds4 同 pattern)
1. Lab cycle 開始時に session_id 先頭 SHA1 hash を計算
2. `loop_snapshots WHERE rendered_text_hash = ?` で hit 確認
3. hit したら bincode deserialize で LoopState restore → 続きから resume
4. miss なら通常の fresh agent loop 起動

### Eviction policy
- `max_snapshots_per_session = 5` (env `BONSAI_SNAPSHOT_MAX_PER_SESSION`)
- LRU で古いものを削除 (saved_at desc 5 件残す)
- 全体 cap = 100 snapshot (db file size 抑制、env `BONSAI_SNAPSHOT_GLOBAL_CAP`)

## §3. TDD strict 5 phase 計画

### Phase 1 (Red) — 失敗 test 8 件
- `t_snapshot_table_v16_migration` (V15 → V16 で table 作成確認)
- `t_save_loop_snapshot_persists_payload` (Session + heuristics carry を BLOB 保存 → DB row 確認)
- `t_load_loop_snapshot_by_hash_matches` (SHA1 hash で hit、bincode round-trip で LoopState equality)
- `t_load_loop_snapshot_no_match_returns_none` (miss → None)
- `t_eviction_keeps_top_n_per_session` (5 件 cap、6 件目で oldest LRU 削除)
- `t_eviction_global_cap` (100 件 cap、global LRU)
- `t_trigger_cold_emitted_on_first_compaction` (`SnapshotTrigger::Cold` event)
- `t_trigger_continued_on_subsequent_compactions` (`SnapshotTrigger::Continued` event)

### Phase 2 (Green) — 実装
- `src/agent/snapshot.rs` 新規: `LoopSnapshot` struct + `save_snapshot` / `load_snapshot_by_hash` /
  `evict_oldest`、SQLite CRUD
- `src/agent/agent_loop/core.rs`: compaction hook 直後に `save_snapshot(Trigger::Cold | Continued)` 呼出
- `src/cancel.rs`: graceful shutdown 経由で `save_snapshot(Trigger::Shutdown)` 呼出
- `src/agent/agent_loop/core.rs`: `run_agent_loop_with_session` 冒頭で `load_snapshot_by_hash` 試行、hit
  なら LoopState 復元
- env: `BONSAI_SNAPSHOT_ENABLED` (default OFF、Cerememory 三本柱 pattern)
- env: `BONSAI_SNAPSHOT_MAX_PER_SESSION` (default 5)
- env: `BONSAI_SNAPSHOT_GLOBAL_CAP` (default 100)
- env: `BONSAI_SNAPSHOT_CONTINUED_INTERVAL_STEPS` (default 50、ds4 の chunk alignment 思想)

### Phase 3 (Refactor)
- bincode encode/decode を `LoopState::to_bytes()` / `from_bytes()` impl で encapsulate
- `LoopSnapshot::rendered_text_for_hash()` private helper で hash 生成統一
- SQLite CRUD は既存 store pattern (項目 218 ReviewState の `from_row` / `save_to_db` 同 pattern)

### Phase 4 (Smoke)
- G-4a (env unset = default OFF): 既存挙動完全互換、snapshot table 空のまま完走
- G-4b (env enabled、cold trigger): smoke 1 cycle で compaction 1 回 → SQLite row 1 件確認
- G-4c (env enabled、resume): smoke 1 cycle 完了 → 同 session_id 再起動 → snapshot 復元 → step_count 再開確認
- G-4d (eviction): max 5 設定で 6 件保存後に oldest 削除確認

### Phase 5 (Effectiveness)
- Lab v23 (将来) ~20-22h run を擬似的に中断 (SIGTERM at 50%) → 再起動で resume 確認
- ACCEPT 基準: resume 後の終了 score が中断なし baseline 比 ±0.02 以内 (再現性)
- 副次効果 = nightly Lab を安心して unattended で回せる (運用 win)

## §4. ds4 直接転用しない判断

### Tokenizer-decoded text を hash key にする (転用)
ds4 は KV checkpoint のファイル名 = rendered text SHA1。Bonsai でも session の prompt rendering
結果 (system + history) の SHA1 を hash key に採用。

### KV blob 形式 / RAW expert quant bits ヘッダは転用しない
ds4 のヘッダ (`u8 routed expert quant bits, currently 2 or 4`) は DS4 model 特有、
Bonsai は llama-server 側委譲なので不要。

### `evict` trigger は転用しない
Bonsai は in-process single session、外部 request による live state eviction が発生しない。

### `cold` boundary trim (32 tail tokens) は **不要**
ds4 は BPE boundary retokenization 回避のため tail trim するが、Bonsai は llama-server 側の
tokenizer に委譲、Session.messages の boundary は LLM 出力後の rendered text と一致するため
trim 不要。

## §5. 期待効果 (仮説、Phase 5 で検証)

| 仮説 | 反証条件 |
|---|---|
| H1: 中断後 resume で実行時間 ~50% 短縮 | resume 後 wall time delta < -30% |
| H2: 中断後 resume で score 再現性 ±0.02 以内 | resume 後 score delta > ±0.05 |
| H3: snapshot save overhead ≤ +2% | smoke wall delta > +5% |

H1+H2+H3 すべて成立で Lab v23+ default ON 候補。
H2 失敗なら snapshot に含まれていない state (LLM model internal / vec_memories 等) が原因 → 別 plan。

## §6. 起票候補項目

- **項目 231** = 本 plan の Phase 1-3 完遂 + Phase 4 G-4a/b/c/d smoke
- **項目 232** (将来) = Lab v23 paired t-test ACCEPT/REJECT

## §7. 依存 / 順序

- 項目 25 git stash checkpoint (済) — file-level checkpoint の前提、本 plan は agent state level で補完
- 項目 81 compaction interval (済) — `continued` trigger の interval 起点
- 項目 209 EventRepository trait (済) — snapshot store も同じ pattern で実装

## §8. リスク

| Risk | Mitigation |
|---|---|
| LoopState フィールド変更で bincode 後方互換破壊 | bincode version field + migration helper、または bincode 失敗時 `cold start` fallback |
| 大量 snapshot で SQLite db file 肥大化 | global_cap=100 + per-session=5 で hard cap |
| Resume 時の memory store / heuristics pool 不整合 | snapshot 保存時に `MemoryStore.path()` / `current_max_id()` も payload に含め、復元時に整合 check |
| 並列 Lab cycle で同 session_id 競合 | UNIQUE constraint なし、save は append-only、load は ORDER BY saved_at DESC LIMIT 1 で最新採用 |
