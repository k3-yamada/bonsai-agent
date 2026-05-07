# Plan: AgentHER Event Flow Fix — benchmark の ephemeral store events を persistent store に流す

> **由来**: handoff 05-07g (Phase 4 smoke) で発見した architectural disconnect の修復。`run_experiment_loop` 末尾の `run_hindsight_pass(store)` (experiment.rs:1194、項目 201-202) は **persistent MemoryStore** を読むが、benchmark の event 発生源 (`benchmark.rs:1009` および `:1136` の `MemoryStore::in_memory()?`) は **ephemeral store** に閉じ込められ、events が AgentHER 入力に到達しない。CLAUDE.md 項目 202 / handoff 05-07g で「Phase 5 案件 = `agenther-event-flow-fix`」として TODO 化。

## Task Type

- [ ] Frontend
- [x] Backend (event flow 修復、benchmark.rs / experiment.rs / event_store.rs)
- [ ] Fullstack

## Background

### 現状の disconnect

```
run_experiment_loop(store: &MemoryStore)   ← persistent (main.rs:529 で MemoryStore::open)
 ├─ baseline = suite.run_k(...)             ← 内部で MemoryStore::in_memory() を毎タスク作成
 │   └─ 各 task で store_eph (in-memory) を作り、k=3 回 run_agent_loop に渡す
 │       └─ agent_loop emit_event → store_eph.events に書き込み
 │   ※ task 終了時に store_eph がスコープを抜けて DROP → events 全消失
 └─ run_hindsight_pass(store)                ← persistent.events を読む → 0 件 → AgentHER 空振り
```

### 関連事実 (smoke 検証で確証)

- `benchmark.rs:1009` (BenchmarkSuite::run_k): `let store = MemoryStore::in_memory()?;` を **task ごとに新規作成**
- `benchmark.rs:1136` (BenchmarkSuite::run, 単発版): 同上、k=1 経路でも独立 ephemeral
- `benchmark.rs:1016` (k loop 内): `store.reset_session_data()` を呼ぶ → `DELETE FROM messages; sessions; memories;` のみで **events テーブルは保護** (memory/store.rs:83-87 で確認)
  - → 1 task 内の k=3 run 間では events が累積する
  - → ただし task が終わると store DROP で全 events 消失
- AgentHER 4 unit test (experiment.rs:2289-2360) は in-memory store で動作確認済 → AgentHER ロジック自体は問題なし
- 実機 Phase 4 smoke (handoff 05-07g、12 min 実行): `[INFO][lab.agenther] AgentHER post-Lab: failed=0 successful=0` で hook の wiring は PASS、events 0 件で effectiveness FAIL

### 修復後の理想 flow

```
run_experiment_loop(store: &MemoryStore)
 ├─ baseline = suite.run_k(..., store)       ← persistent を引き回し
 │   └─ 各 task で run_agent_loop に persistent store 渡す
 │       └─ agent_loop emit_event → persistent.events に直接書き込み
 │   ※ task 終了時にも events は persistent に保持
 └─ run_hindsight_pass(store)                ← persistent.events から失敗 trajectory 抽出 → relabel/promote 発動
```

## Architecture: Option A / B / C

### Option A — signature 変更 (persistent store 引き回し)

**変更**:
- `BenchmarkSuite::run_k` の signature に `store: &MemoryStore` 追加
- `BenchmarkSuite::run` も同様
- 内部で `MemoryStore::in_memory()?` を削除、引数 store を `Some(&store)` で `run_agent_loop` に渡す
- `reset_session_data()` の呼び出しは維持 (k loop 間 message/session reset、events 保護のまま)

**利点**:
- 意味論的に最もクリーン (event 発生源と読み取り先が同一 store)
- export logic 不要、event flow が直線的
- 将来 EventStore に対する別機能 (例: events ベースの metric) も自然に動く

**欠点**:
- signature 変更 = 全 caller 更新必要 (caller list は Phase 1 で grep で網羅、想定 5-10 箇所)
- `MemoryStore::in_memory()` 前提のテスト test_run_k_*** が persistent store でも動くか要検証 (`MemoryStore::in_memory()` で渡せば既存テスト互換、新規 test だけ persistent + 後段 cleanup)
- persistent.events に **過去 Lab cycle の events が累積** → AgentHER pass が古い session も拾う risk → **scoping が必要** (後述)

### Option B — export hook (ephemeral → persistent bulk copy)

**変更**:
- `BenchmarkSuite::run_k` の signature に `persistent_store: Option<&MemoryStore>` を **追加** (default `None` で後方互換)
- 各 task の k loop 終了直後 (drop 直前) に、persistent が Some なら ephemeral.events から persistent.events へ **bulk INSERT**
- 新規 EventStore method: `pub fn export_to(&self, dest: &MemoryStore) -> Result<usize>` を追加
- benchmark の既存 `MemoryStore::in_memory()?` 経路は維持

**利点**:
- signature 後方互換 (`Option<&MemoryStore>`)、既存 caller を壊さない
- ephemeral 内で events を作る現行アーキ温存 → 並列 task 化したいときも干渉なし
- export 範囲が「この task の k=3 run の events だけ」と明示的に限定 → scoping が自然

**欠点**:
- export logic 追加 (~30 行) + EventStore method 1 つ追加
- データの copy that's redundant (ephemeral と persistent に同 events 一時並存)
- 将来別機能 (skills の persistent 反映など) も同様に export が必要なら scale しない

### Option C — AgentHER pass に Vec<&MemoryStore> 渡す

**変更**:
- benchmark cycle 終了時に ephemeral store list を集約して保持
- `run_hindsight_pass` の signature を `(stores: &[&MemoryStore])` に拡張

**利点**:
- benchmark 構造そのままで、AgentHER pass だけ複数 store 対応

**欠点**:
- store のライフタイム管理が複雑 (ephemeral を experiment_loop 全体で生存させる必要)
- 5 task × cycle 数で store 数が増え memory 圧迫
- events 集約コストが multi-store 操作分散で大きい

### 採用: **Option B → A 段階移行**

- **Phase 5 (本 plan)**: Option B 実装 (最小侵襲、event flow を最速で開通)
- **Phase 6 (将来)**: Option A へリファクタ (signature 変更、過渡 export logic 削除)

理由: Option B で event flow を通せば、smoke 再走で AgentHER の真の効力を **24-48h 以内に実機実証**できる。Option A は signature 変更で 5-10 caller 更新必要、TDD strict で 4-6h、実機実証まで合計遅い。Option B → A 段階移行なら、Phase 5 完遂後に AgentHER 効果検証を並行しつつ Option A をのんびり追える。

## Phase 5 詳細 (TDD strict 5 phase)

### Phase 1 — Red (failing integration test)

**新規 test** (experiment.rs テストモジュール末尾):
```rust
#[test]
fn t_benchmark_events_propagate_to_persistent_store() {
    let persistent = MemoryStore::in_memory().unwrap();  // (本番では disk)
    let suite = BenchmarkSuite::smoke_tasks();
    let backend = mock_backend_with_failures();  // tool_success_rate < 0.8 を確実に出すモック
    let result = suite.run_k(
        &test_config(),
        &backend,
        &test_tools(),
        &test_path_guard(),
        &CancellationToken::new(),
        &MultiRunConfig { k: 3, jitter_seed: false },
        0.5,
        Some(&persistent),  // ← 新規パラメータ
    ).unwrap();

    let es = EventStore::new(persistent.conn());
    let count = es.list_sessions().unwrap().len();
    assert!(count >= 5 * 3, "5 task × k=3 = 15 sessions が persistent に積まれること、got={count}");

    let failed = es.extract_failed_trajectories(0.8, 2).unwrap();
    assert!(!failed.is_empty(), "failed >= 1 (mock backend が確実に失敗を出すよう設計)");
}
```

**期待**: コンパイルエラー (signature 不一致、`Some(&persistent)` 引数なし)。期待動作の合意確定 = Red 成立。

**追加で end-to-end test** (任意、cost 大なら Phase 4 で実機 smoke で代替):
- `t_run_experiment_loop_emits_agenther_with_failed`: MockLlmBackend で `run_experiment_loop` を 1 cycle、AgentHER pass の log capture で `failed >= 1` を assert

### Phase 2 — Green (Option B 実装)

#### 2a. EventStore::export_to method 追加

**新規** (event_store.rs、`extract_failed_trajectories` の隣):
```rust
/// 自 store の events を別 store に bulk copy する。
/// 戻り値は copy した event 数。重複検出は呼出側責務 (本 method は冪等性を保証しない)。
/// AgentHER post-Lab pass で benchmark ephemeral → experiment_loop persistent への
/// event flow を確立するために使用 (handoff 05-07g、項目 202-203)。
pub fn export_to(&self, dest: &MemoryStore) -> Result<usize> {
    // self.conn から SELECT、dest.conn へ INSERT
    // session_id / event_type / event_data / timestamp / step_index は保持
    // id は dest 側の auto_increment に再付与 (持ち込まない)
}
```

#### 2b. BenchmarkSuite::run_k signature 拡張

**変更** (benchmark.rs:945 付近):
```rust
pub fn run_k(
    &self,
    config: &AgentConfig,
    backend: &dyn LlmBackend,
    tools: &ToolRegistry,
    path_guard: &PathGuard,
    cancel: &CancellationToken,
    multi: &MultiRunConfig,
    pass_threshold: f64,
    persistent_store: Option<&MemoryStore>,  // ← 新規
) -> Result<MultiRunBenchmarkResult> {
    // ...
    for task in &self.tasks {
        // ...
        let store = MemoryStore::in_memory()?;
        for run_idx in 0..multi.k { /* k loop 既存 */ }

        // ★ k loop 完了直後、ephemeral から persistent へ events を export
        if let Some(dest) = persistent_store {
            let es = EventStore::new(store.conn());
            let copied = es.export_to(dest)?;
            log_event(LogLevel::Debug, "benchmark", &format!("events exported task={} count={copied}", task.id));
        }
        // ※ store はこの後 drop される (既存挙動)
    }
}
```

#### 2c. BenchmarkSuite::run signature 拡張 (run_k と同じ patten)

#### 2d. caller 更新

- `experiment.rs:855` (baseline 計測): `suite.run_k(..., Some(store))` に変更
- `experiment.rs` 内の他の `run_k` caller (実験 run): 同上
- `experiment.rs:566` の `BenchmarkSuite::default_tasks()` 周辺: None で互換
- benchmark.rs 内テスト caller: None 維持 (既存挙動互換)
- 新規 test: Some 渡し

### Phase 3 — Refactor

- Phase 2 で重複した logging を `emit_export_log` ヘルパー化
- `run_k` の引数増加に伴う docstring 更新 (param table)
- `export_to` の SQL を prepared statement にして efficiency 確保
- Codex / Gemini review 推奨 (CCG / multi-plan)

### Phase 4 — smoke 再走

```bash
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 \
  > /tmp/phase4-smoke-rerun.log 2>&1
grep "lab.agenther" /tmp/phase4-smoke-rerun.log
# 期待: AgentHER post-Lab: failed=N successful=M ... で N >= 1 (理想は N >= 3 / M >= 5)
```

判定:
- ✅ **PASS**: failed >= 1 → AgentHER 真の効力実機実証完了 → handoff 05-07h 起票
- ⚠️ **PARTIAL**: failed=0 / successful=N (>0) → ephemeral→persistent flow は通ったが Bonsai-8B smoke は happy run しか出ない → failure-inducing task 1 件追加 (SMOKE_TASK_IDS) → 再走
- ❌ **FAIL**: failed=0 successful=0 → export_to が動いていない → diagnose

### Phase 5 — Option A 移行 (任意、別 commit / plan)

Phase 4 で PASS 確定後、満を持して signature 変更:
- `Option<&MemoryStore>` → `&MemoryStore` (None 廃止)
- benchmark 内 `MemoryStore::in_memory()?` を削除し persistent のみ使用
- `export_to` は不要になり削除 → -50 行
- 過渡期で events 累積問題は **scoping 機構** (後述) で解決

## Scoping (重要)

Option B / A 共通で、persistent.events は Lab cycle 跨ぎで累積する。AgentHER pass が **「今回の Lab cycle 分の events だけ」** を見る scoping が必要。

### 推奨 scoping 機構

`run_experiment_loop` 開始時に `MAX(id) FROM events` を snapshot:
```rust
let lab_start_event_id: i64 = {
    let mut stmt = store.conn().prepare("SELECT COALESCE(MAX(id), 0) FROM events")?;
    stmt.query_row([], |row| row.get(0))?
};
// ... baseline + 実験 loop
match run_hindsight_pass(store, lab_start_event_id) {  // ← signature 変更
    // ...
}
```

EventStore に新規 method:
```rust
pub fn extract_failed_trajectories_since_id(
    &self,
    since_event_id: i64,
    max_tool_success_rate: f64,
    min_steps: usize,
) -> Result<Vec<TrajectoryCandidate>>
```

`extract_failed_trajectories` (既存) は薄いラッパに変える: `extract_failed_trajectories_since_id(0, ...)`。

**Phase 5 では scoping を Phase 2 に組み込む** (Phase 4 smoke で過去 events 0 件想定だから単 cycle なら不要だが、繰り返し実行で必要)。`since_event_id=0` 渡しで既存挙動も保証可能。

## Risks / Mitigations

| # | Risk | 影響 | Mitigation |
|---|------|------|------------|
| R1 | persistent.events accumulation で AgentHER pass が遅延 | 数百 cycle 後の遅延 | Phase 2 で scoping (since_id) を組み込み、AgentHER は cycle 分だけスキャン |
| R2 | export_to 中の SQL ロック競合 | benchmark 並列化で write-write 競合 | benchmark は逐次処理 (現行)、並列化は別 plan |
| R3 | events table の disk 肥大化 | 長期で SQLite ファイル増大 | 定期 purge (例: 30 日超 events 削除) は別 plan、本 plan ではログのみ |
| R4 | 既存 4 unit test (run_hindsight_pass) が壊れる | TDD 退行 | scoping signature 変更時、既存 test は `since_event_id=0` で互換維持 |
| R5 | `reset_session_data()` が persistent では危険 | 本番 messages 全消去 | benchmark 内ループは ephemeral 維持 (Option B) → 影響なし。Option A で要再設計 |
| R6 | Option A 移行で signature 変更が広範 | caller 修正コスト | 段階的 (Option B → A)、Phase 5 移行時に grep + 全 caller 一括更新 |
| R7 | 既存 events 累積で AgentHER pass の duration 増加 | AgentHER 終端で最大 +Ns | scoping (since_id) で解消、または extract に LIMIT/INDEX |
| R8 | Phase 4 smoke で failed=0 (effectiveness 不足) | 実機実証不完全 | failure-inducing task 1 件追加 (SMOKE_TASK_IDS) で対応 |

## Quality Gates

- **G-1 (Phase 1 Red)**: 新規 integration test がコンパイルエラー or 期待 assert 失敗で Red 確認
- **G-2 (Phase 2 Green)**: 新規 test PASS + 既存 4 unit test (`t_hindsight_pass_*`) 維持 + clippy 0 warning + fmt clean
- **G-3 (Phase 3 Refactor)**: 重複削除、docstring 整備、Codex / Gemini review (任意)
- **G-4 (Phase 4 smoke 実機)**: `[INFO][lab.agenther] AgentHER post-Lab: failed >= 1 ...` を実機 log で確認 (PASS / PARTIAL いずれか)
- **G-5 (Phase 5 任意)**: Option A 移行後、smoke baseline score 退行ゼロ (±0.005)

## Test Strategy

### Phase 1-3 unit test (TDD strict 必須)

- `t_event_store_export_to_basic` (event_store.rs): 5 events を src→dest copy、count=5 / event_type 配列同一を assert
- `t_event_store_export_to_dedupe_via_session_id` (event_store.rs): src と dest に同 session_id がある場合の挙動を確認 (本 plan は "重複は呼出側責務"、export_to 自体は dup を許容)
- `t_benchmark_run_k_with_persistent_store` (benchmark.rs): 新規 test、persistent_store=Some 経路で events が積まれることを assert
- `t_benchmark_run_k_without_persistent_store` (benchmark.rs): None 経路で既存挙動 (events が persistent に到達しない) を維持確認 = 後方互換 gate
- `t_extract_failed_trajectories_since_id` (event_store.rs): scoping 動作確認 (since=0 / since=N の両方)

### Phase 4 integration smoke (実機)

- `BONSAI_LAB_SMOKE=1 --lab-experiments 0` で baseline 5 task × k=3 = 15 run
- 期待 log: `[INFO][lab.agenther] AgentHER post-Lab: failed=N successful=M ...` で N >= 1
- 副次観測: SQLite `SELECT COUNT(*) FROM events;` >= 15 (Lab cycle 後)

### 既存 test 保護

- 4 unit test (`t_hindsight_pass_*`) を改変しない
- benchmark.rs 既存 test (`test_smoke_tasks_subset_of_default` 等) を改変しない
- 全体 1055 passed (handoff 05-07f baseline) からの増減: 期待 +5-7 (Phase 1-3 新規 test 分)

## Success Criteria

1. ✅ Phase 1-3 完了で `cargo test` PASS、新規 5+ test 追加、退行ゼロ
2. ✅ Phase 4 smoke で AgentHER 実機発火 (failed >= 1 観測)
3. ✅ Production code 行数増分 ≤ 100 行 (export_to + signature 拡張のみ)
4. ✅ commit ≤ 4 (Phase 1 Red / Phase 2 Green / Phase 3 Refactor / Phase 4 smoke 結果 doc)
5. ✅ scoping 機構 (since_event_id) を Phase 2 に組み込み、繰り返し Lab cycle で過去 events に汚染されない

## 推定工数

- Phase 1 Red: 30 min (test 1-2 件)
- Phase 2 Green (export_to + signature + scoping): 60-80 min
- Phase 3 Refactor: 20-30 min
- Phase 4 smoke: 15 min (compile 5 + run 12)
- Phase 5 Option A 移行: defer (別 plan、別セッション)

**合計**: 2.0-2.5h (handoff 05-07g の Phase 5 案件 ~2h 想定と一致)

## 次セッション着手手順

```bash
# 1. Phase 1 Red — 新規 test を 1-2 件追加
$EDITOR src/agent/experiment.rs  # t_benchmark_events_propagate_to_persistent_store
$EDITOR src/agent/event_store.rs  # t_event_store_export_to_basic
rtk cargo test t_benchmark_events_propagate t_event_store_export 2>&1 | grep -E "FAIL|error" | head -10
# Red 確認

# 2. Phase 2 Green — export_to + signature 拡張 + scoping
$EDITOR src/agent/event_store.rs  # export_to + extract_failed_trajectories_since_id
$EDITOR src/agent/benchmark.rs    # run_k / run signature 拡張
$EDITOR src/agent/experiment.rs   # caller 更新 + lab_start_event_id snapshot
rtk cargo test 2>&1 | tail -5
# 1055 → 1062+ passed 期待

# 3. Phase 3 Refactor + commit
rtk cargo clippy -- -D warnings && rtk cargo fmt -- --check
rtk git add -A
rtk git commit -m "test(agenther-event-flow): Phase 1 Red — ..."
rtk git commit -m "feat(agenther-event-flow): Phase 2 Green — Option B export_to + scoping"
rtk git commit -m "refactor(agenther-event-flow): Phase 3 — ..."

# 4. Phase 4 smoke
rtk cargo build --release
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 \
  > /tmp/phase4-smoke-rerun.log 2>&1
grep "lab.agenther" /tmp/phase4-smoke-rerun.log
# failed >= 1 を観測

# 5. handoff 05-07h 起票 + CLAUDE.md 項目 203 追記
```

## 参照

- handoff 05-07g (Phase 4 smoke の結果 + 真因特定)
- CLAUDE.md 項目 201 (AgentHER hindsight relabel API 実装)
- CLAUDE.md 項目 202 (AgentHER runtime 組込 + Phase 4 smoke wiring PASS)
- experiment.rs:1194 (run_hindsight_pass の hook 配線)
- experiment.rs:2289-2360 (既存 4 unit test)
- benchmark.rs:1009 / 1136 (`MemoryStore::in_memory()?` 真因箇所)
- memory/store.rs:83-87 (`reset_session_data` の DELETE 範囲、events は保護)
- event_store.rs:139-178 (extract_successful/failed_trajectories の既存 API)
