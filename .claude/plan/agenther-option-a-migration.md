# Plan: AgentHER Option A 移行 — `Option<&MemoryStore>` → `&MemoryStore` (必須化)

> **由来**: handoff 05-07h で Phase 5 Phase 1-4 完遂 (Option B export hook + scoping)、AgentHER 中核 production-ready 状態。本 plan は handoff 05-07h「★★ Phase 5 Option A 移行 (~2h)」と既存 plan `agenther-event-flow-fix.md` Phase 5 (line 220-226) の **完全版実装**。
>
> **目的**: 過渡期 Option B の冗長性 (export_to の bulk copy) を解消し、event 発生源と読み取り先を **直接同一 store** にすることで意味論をクリーン化、コード -50 行 net 削減。

## Task Type

- [ ] Frontend
- [x] Backend (signature 変更、benchmark.rs / experiment.rs / event_store.rs)
- [ ] Fullstack

## Background

### 現状 (Option B、handoff 05-07h)

```
run_experiment_loop(store: &MemoryStore)
 ├─ baseline = suite.run_k(..., Some(store))   ← Option<&MemoryStore>
 │   └─ task ごとに store_eph (in-memory) を作成
 │       └─ k=3 run の events を ephemeral に蓄積
 │   └─ task k loop 完了後、ephemeral.events を persistent に bulk INSERT (export_to)
 └─ run_hindsight_pass(store, lab_start_event_id)  ← scoping で今 cycle 分のみ抽出
```

### 移行後 (Option A、本 plan)

```
run_experiment_loop(store: &MemoryStore)
 ├─ baseline = suite.run_k(..., store)   ← &MemoryStore (必須)
 │   └─ persistent store を直接 run_agent_loop に渡す
 │       └─ events が persistent に直接書き込み (ephemeral 廃止)
 └─ run_hindsight_pass(store, lab_start_event_id)  ← scoping で今 cycle 分のみ抽出
```

### 削除対象

- `EventStore::export_to(&self, dest: &MemoryStore) -> Result<usize>` 完全削除 (~50 行)
- benchmark.rs 内の `MemoryStore::in_memory()?` 2 箇所削除 (line 1065 + 1210)
- `Option<&MemoryStore>` 引数を全 caller (4 箇所) で `&MemoryStore` に変更

## 重要な設計論点: pre-screen の events 汚染

**現状の caller 4 箇所** (handoff 05-07h で確認):

| line | 関数 | 現状 store 引数 | 用途 |
|------|------|-----------------|------|
| 582 | `estimate_mutation_effect_with_baseline` (pre-screen baseline) | `None` | 変異 delta 推定 |
| 595 | 同上 (pre-screen experiment) | `None` | 変異 delta 推定 |
| 868 | `run_experiment_loop` baseline | `Some(store)` | 本番計測 (AgentHER 対象) |
| 1017 | `run_experiment_loop` experiment | `Some(store)` | 本番計測 (AgentHER 対象) |

Option A で `&MemoryStore` 必須化すると、**pre-screen (582/595) も persistent.events を汚染** する:
- pre-screen は変異 delta 推定用 (sample 4 task × k=1)、AgentHER 対象外
- pre-screen events が persistent に書き込まれると、AgentHER `run_hindsight_pass` の対象に紛れ込む
- 影響: skill / insight が pre-screen 由来 (= sample task の貧弱な trajectory) で汚染される risk

### 解決策の選択肢

| Option | 概要 | 利点 | 欠点 |
|--------|------|------|------|
| **A1**: scoping 強化 | pre-screen 直前にも lab_start_event_id を snapshot し、AgentHER pass は 1 度だけ実行する | 既存 scoping 機構を活用、追加ロジック最小 | 現状 pass は 1 度のみで pre-screen の前後で event 累積を許容 (= pre-screen events が混入) |
| **A2**: pre-screen tag | events に `is_pre_screen: bool` 列を追加、`extract_failed_trajectories_*` で除外 | 意味論的にクリーン | events table schema 変更 (V9 → V10) + migration 必要 |
| **A3**: pre-screen ephemeral 維持 | pre-screen のみ `MemoryStore::in_memory()` を内部で作成し続ける (Option<&MemoryStore> を完全廃止せず private にする) | 後方互換、persistent 汚染ゼロ | Option B 比でコード削減幅が縮小 (-30 行程度) |
| **A4**: events delete | pre-screen 完了時に persistent から該当 events を DELETE | シンプル | DELETE は scoping snapshot を破壊する risk + 順序依存 |

**推奨: A3 (pre-screen ephemeral 維持)** — 安全側、リスク最小、削減幅 -30 行で十分

実装案:
```rust
// benchmark.rs (signature は &MemoryStore に変更、ephemeral fallback は内部判定で消す)
// Option A 後:
pub fn run_k(
    &self,
    config: &AgentConfig,
    backend: &dyn LlmBackend,
    tools: &ToolRegistry,
    path_guard: &PathGuard,
    cancel: &CancellationToken,
    multi: &MultiRunConfig,
    pass_threshold: f64,
    store: &MemoryStore,   // ← 必須
) -> Result<MultiRunBenchmarkResult> {
    // 既存の MemoryStore::in_memory()?; を削除し、引数 store を直接 run_agent_loop へ
    // ...
}

// experiment.rs estimate_mutation_effect の pre-screen は内部で local in_memory store を作る:
pub fn estimate_mutation_effect_with_baseline(...) -> Result<f64> {
    // pre-screen 専用 throwaway store (persistent 汚染回避)
    let scratch_store = MemoryStore::in_memory()?;
    // ...
    let baseline = sample_suite.run_k(..., &scratch_store)?;
    let experiment = sample_suite.run_k(..., &scratch_store)?;
    // scratch_store はこの関数 scope 抜けで DROP → events も消える (現行 Option B None と同等挙動)
}
```

→ pre-screen は完全に persistent と隔離、AgentHER 対象は 868/1017 経由の persistent.events のみ。

## Phase 詳細 (TDD strict 5 phase)

### Phase 1 — Red

新規 integration test:
- `test_run_k_requires_store_signature`: `run_k` を `Option<&MemoryStore>` で呼ぶとコンパイルエラー (signature 変更を検出)
- `test_pre_screen_does_not_pollute_persistent_events`: pre-screen 後に `event_store.extract_failed_trajectories_since_id(0, 1.0, 0)` で 0 件を確認

期待: 変更前にコンパイルエラー (signature 不一致) and / or assertion 失敗。

### Phase 2 — Green

1. `benchmark.rs::run_k` signature: `Option<&MemoryStore>` → `&MemoryStore` (line ~1040)
2. `benchmark.rs::run` 同様 (line ~1170)
3. `benchmark.rs::run_k` 内部の `let store = MemoryStore::in_memory()?;` 削除 (line 1065)、引数 store を直接使用
4. `benchmark.rs::run` 同様 (line 1210)
5. `experiment.rs:582 + 595`: pre-screen に `let scratch_store = MemoryStore::in_memory()?;` を追加し、`Some(&scratch_store)` ではなく `&scratch_store` で渡す
6. `experiment.rs:868 + 1017`: `Some(store)` → `store` に変更
7. `event_store.rs::export_to` 削除 (~50 行)
8. `experiment.rs::run_experiment_loop` 内の `export_to` 呼び出し箇所削除 (handoff 05-07h で `run_k` 内 hook に組み込まれているはず — 削除箇所は実装時 grep で特定)

### Phase 3 — Refactor

- benchmark.rs の `reset_session_data()` 安全性確認 (R5):
  - `reset_session_data` は `messages`, `sessions`, `memories` のみ DELETE、events は保護 (memory/store.rs:83-87)
  - persistent store 上で呼ぶと **本番 messages も消える** が、bonsai-agent は実時間で persistent.messages を使わない (Lab cycle 中のみ) → 影響なし
  - ただし将来 persistent.messages を生かしたい場合に備え、`reset_session_data_for_lab()` のような名前にリネーム
- docstring 更新 (Option B → Option A 移行記録)
- handoff 05-07i 後継で `lab_start_event_id` snapshot を scoping 専用の helper 関数に抽出

### Phase 4 — Smoke 検証

```bash
# core 22+2 (= 24) で 1 cycle baseline 取得
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0
# 期待:
# [INFO][lab.agenther] AgentHER post-Lab: failed=N successful=M relabels=K skills=I insights=J
# (handoff 05-07h と同等以上、特に relabels >= 3 / skills >= 1 / insights >= 3)
```

判定基準:
- ✅ AgentHER metric が Option B 時と同等以上
- ✅ events DB 増分が「今 cycle 分のみ」(scoping 機構が機能)
- ✅ pre-screen (estimate_mutation_effect_with_baseline) 経由の events が persistent に紛れ込まない

### Phase 5 — Commit + handoff

5 commits 想定 (Phase 1-3 各 1 + Phase 4 検証 1 + handoff 1):
1. `test(agenther-opt-a): Phase 1 Red — signature + pre-screen pollution test`
2. `feat(agenther-opt-a): Phase 2 Green — &MemoryStore 必須化 + scratch_store 分離`
3. `refactor(agenther-opt-a): Phase 3 — export_to 削除 (-50 行) + reset rename`
4. `(smoke 検証)`
5. `docs(claude.md): 項目 N — Option A 移行完遂`

## Risks / Mitigations

| # | Risk | 影響 | Mitigation |
|---|------|------|------------|
| R1 | pre-screen events が persistent.events に流入 | AgentHER 対象に sample task 軌跡が混入し insight 品質低下 | A3 採用 (scratch_store 分離)、Phase 1 test で検出 |
| R2 | reset_session_data() が persistent で危険 | 本番 messages 消去 (現状 Lab 専用なので影響なし) | Phase 3 で `reset_session_data_for_lab` にリネーム + docstring 警告 |
| R3 | signature 変更で既存 4 unit test (`t_hindsight_pass_*`) が壊れる | TDD 退行 | 既存 test は in_memory store でも `&store` 渡しで互換維持 |
| R4 | benchmark.rs の他 caller 漏れ | 未追従 caller でコンパイルエラー | Phase 1 で `grep -n "\.run_k\|\.run("` で全 caller を網羅 |
| R5 | export_to 削除で既存 test が依存 | 削除箇所近傍の test red | Phase 1 で `grep -n "export_to"` で test 依存を網羅、不要なら test も削除 |
| R6 | smoke (Phase 4) で Option B 比 metric 退行 | effectiveness 退行 | Phase 4 判定で同等以上を確認、不一致なら Phase 5 で revert |

## Quality Gates

- **G-1 (Phase 1 Red)**: 新規 test がコンパイルエラー or 期待 assert 失敗で Red 確認
- **G-2 (Phase 2 Green)**: 新規 test PASS + 既存 4 unit test (`t_hindsight_pass_*`) 維持 + clippy 0 warning + fmt clean + 1057+ passed 維持
- **G-3 (Phase 3 Refactor)**: 重複削除、docstring 整備、`reset_session_data_for_lab` リネーム
- **G-4 (Phase 4 Smoke)**: AgentHER metric `relabels >= 3 / skills >= 1 / insights >= 3` を Option B 同等以上で再現
- **G-5 (Final)**: net -30 行以上 (handoff 05-07h からの差分) + handoff 05-07i 起票

## 完了条件

1. ✅ `Option<&MemoryStore>` 完全削除 (production code)
2. ✅ `export_to` 削除 (-50 行 net)
3. ✅ pre-screen ephemeral 隔離 (scratch_store)
4. ✅ smoke G-4 PASS
5. ✅ 1057+ passed 維持

## Quick Start (実装着手手順)

```bash
# 1. caller 全網羅
rtk grep -rn "\.run_k\b" /Users/keizo/bonsai-agent/src/  # 4 caller 期待
rtk grep -rn "\.run\b.*BenchmarkSuite" /Users/keizo/bonsai-agent/src/  # run() caller 確認
rtk grep -rn "export_to" /Users/keizo/bonsai-agent/src/    # 削除対象網羅

# 2. Phase 1 Red — signature 変更 test
$EDITOR src/agent/benchmark.rs   # test_run_k_requires_store_signature 追加
$EDITOR src/agent/event_store.rs # test_pre_screen_does_not_pollute_persistent_events 追加
rtk cargo test --lib 2>&1 | tail -3  # Red 確認

# 3. Phase 2 Green — signature 変更 + scratch_store
$EDITOR src/agent/benchmark.rs   # run_k / run signature: Option<&MemoryStore> → &MemoryStore
$EDITOR src/agent/experiment.rs  # 4 caller 更新 (582 + 595 = scratch_store / 868 + 1017 = store)
$EDITOR src/agent/event_store.rs # export_to 削除 (-50 行)
rtk cargo test --lib 2>&1 | tail -3  # Green 確認

# 4. Phase 3 Refactor + Phase 4 smoke
rtk cargo build --release && BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0
grep "lab.agenther" /tmp/...

# 5. Commit + handoff
```

## 参考

- 既存 plan: `.claude/plan/agenther-event-flow-fix.md` (Phase 5 概要 line 220-226)
- handoff 05-07h: Phase 1-4 完遂状態、本 plan が Phase 5 完遂版
- CLAUDE.md 項目 203 (Phase 5 Phase 1-4)
