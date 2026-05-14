# AgentFloor LADDER Mode Wiring Fix — 項目 224 副次 finding (b) follow-up

**状態**: planning-only (2026-05-15 起票)、推奨度 ★★、推定工数: ~3h (TDD strict 5 phase、SCHEMA migration ゼロ、API additive)
**起点**: CLAUDE.md 項目 224 §3 副次 finding (b) = `BONSAI_BENCH_LADDER=1` env wiring 完全ゼロ、`is_ladder_mode_enabled()` 定義済だが experiment.rs:1042-1090 suite 選択経路から call site 不在

---

## §1. 背景

### 1.1 副次 finding (b) 詳細
CLAUDE.md 項目 224 末尾より: `BONSAI_BENCH_LADDER=1` env wiring 完全ゼロ、`is_ladder_mode_enabled()` 定義済だが experiment.rs:1042-1090 suite 選択経路から呼出ゼロ、tier_t1..t6 集計は default_tasks(40) 上で post-hoc 動作のため tier 出力には影響ないが、agentfloor_tasks の curated 5/tier balance は未活用。

### 1.2 問題
| 軸 | 現状 | 期待 |
|----|------|------|
| `is_ladder_mode_enabled()` | benchmark.rs:945 定義済 | suite 選択経路から **call site ゼロ** |
| `agentfloor_tasks()` | 30 task curated 5/tier balance 実装済 | production Lab 経路で **未活用** |
| sample 数 | T1=4 / T2=8 / T3=10 / T4=12 / T5=5 / **T6=1** | T1-T6 全 **5 統一** |

---

## §2. 設計

### 2.1 優先順位設計 (3 env 競合時の固定順位)
```
1. BONSAI_LAB_SMOKE=1     → smoke_tasks()       [最優先、既存維持]
2. BONSAI_BENCH_LADDER=1  → agentfloor_tasks()  [★本 plan で追加]
3. BONSAI_BENCH_TIER=core → core/extended_tasks() [既存維持]
4. default                → default_tasks()      [既存維持]
```

**根拠**: SMOKE は dev iteration 用で最優先、LADDER は paper 比較用 curated suite で BENCH_TIER より上位、LADDER + TIER 同時設定は LADDER 優先 + warn log (silent ignore 回避)。

### 2.2 改修後コード
```rust
} else if crate::agent::benchmark::is_ladder_mode_enabled() {
    if std::env::var("BONSAI_BENCH_TIER").ok().is_some_and(|v| !v.trim().is_empty()) {
        log_event(LogLevel::Warn, "lab", "LADDER=1 + BENCH_TIER 同時設定 → LADDER 優先");
    }
    log_event(LogLevel::Info, "lab.agentfloor",
        "BONSAI_BENCH_LADDER=1 → agentfloor_tasks() (30 task、5/tier balance、arxiv 2605.00334)");
    BenchmarkSuite::agentfloor_tasks()
}
```

### 2.3 副作用ゼロ確証
| 観点 | 確証 |
|------|------|
| env unset | 既存 4 経路完全互換 |
| API signature | `run_experiment_loop` 不変 |
| SQLite schema | V16 維持 |
| TSV | 25 列維持 |

---

## §3. TDD strict 5 phase

### Phase 1 (Red) — 5 failing test
1. `test_lab_suite_selection_ladder_mode_returns_30_tasks` (LADDER=1 で 30 task)
2. `test_lab_suite_selection_default_unchanged_when_ladder_unset` (env unset で 40 task)
3. `test_lab_suite_selection_smoke_takes_priority_over_ladder` (SMOKE + LADDER で SMOKE 優先)
4. `test_lab_suite_selection_ladder_takes_priority_over_bench_tier` (LADDER + TIER=core で LADDER + warn)
5. `test_lab_suite_selection_ladder_off_with_tier_extended` (LADDER unset + TIER=extended で既存挙動)

**設計**: `fn select_lab_suite() -> BenchmarkSuite` private helper (~40 行) に抽出して test 経由カバー、`LADDER_TEST_LOCK: Mutex<()>` で env 隔離。

### Phase 2 (Green)
1. `select_lab_suite()` helper 抽出 (~40 行)
2. `run_experiment_loop` §1042-1090 を helper call 1 行に置換
3. `is_ladder_mode_enabled` import
4. log channel = `"lab.agentfloor"` (項目 223 と統一)

**期待**: 1245 → 1250 passed

### Phase 3 (Refactor)
- helper docstring + 優先順位 4 段表 + paper 参照 arxiv 2605.00334
- clippy 0 / fmt 0

### Phase 4 (Smoke G-4)
| Gate | env | task | wall | 検証 |
|------|-----|------|------|------|
| **G-4a** | (unset) | default (40) | ~50 min | 既存挙動互換、`[INFO][lab]` 経路 |
| **G-4b** | `BONSAI_BENCH_LADDER=1` | agentfloor (30) | ~38 min | `[INFO][lab.agentfloor]` log、tier 集計 = 30 task 由来、T1=T6=5 balance |

### Phase 5 (commit) — 3 commits + handoff

---

## §4. 期待効果 (Phase 4 G-4b で検証)

### H1: tier balance 改善
| metric | default (40) | agentfloor (30) |
|---|---|---|
| T1 | 4 | **5 ★** |
| T6 | **1** | **5 ★** |

### H2: T6 variance 計算可能化
- 現状 T6 = 1 sample で variance 不能
- LADDER ON = 5 sample で variance 計算可、tier-targeted 変異 (Lab v22+) の前提精度向上

---

## §5. risks / mitigations
| # | Risk | Mitigation |
|---|------|-----------|
| **R1** | baseline shift (40→30、Lab v15 0.7812 比較不可) | env opt-in default OFF、log channel で識別 |
| **R2** | BENCH_TIER 競合 | LADDER 優先 + warn (silent ignore 回避) |
| **R3** | SMOKE + LADDER 競合 | SMOKE 優先 = 既存挙動継続 |
| **R4** | test 並列 env race | `LADDER_TEST_LOCK` Mutex (項目 226 CRITIC pattern) |

---

## §6. 起票候補項目
**項目 233** = 本 plan 完遂時 (LADDER mode env 配線 + smoke 2/2 PASS + T1/T6 balance 4→5/1→5)

---

## §7. 依存
- ✅ 項目 223 AgentFloor 30 task suite
- ✅ 項目 224 Pre-Screen Tier Persistence Fix
- ⏳ Lab v22+ HypothesisGenerator tier-targeted 改修 (本 plan H3 を前提とする別 plan)

---

## §8. ロールバック戦略
- env opt-in default OFF
- `unset BONSAI_BENCH_LADDER` で即時 disable
- git revert 1 commit (Phase 2 Green)

---

## §9. Quick Start
```bash
# Phase 1 Red
$EDITOR src/agent/experiment.rs
cargo test --lib lab_suite_selection 2>&1 | tail -10  # 5 FAIL

# Phase 2 Green
cargo test --lib                                       # 1250 passed

# Phase 3 Refactor
cargo clippy --tests -- -D warnings && cargo fmt --check

# Phase 4 Smoke (release build)
cargo build --release
unset BONSAI_BENCH_LADDER && cargo run --release -- --lab --lab-experiments=1 | tee /tmp/g4a.log
BONSAI_BENCH_LADDER=1 cargo run --release -- --lab --lab-experiments=1 | tee /tmp/g4b.log
```

---

## §10. 不要転用 (rejected)
| 案 | 棄却理由 |
|---|---|
| LADDER + PASS@(k,T) 統合 | `pass-k-t-agentfloor-3d-impl.md` (項目 231) で別途扱う |
| curated task suite 動的生成 | paper baseline 一致性毀損 |
| SCHEMA migration (suite_name column) | log channel で代替可能 |

---

## §11. 参考
- CLAUDE.md 項目 223 / 224 / 229
- `src/agent/benchmark.rs:945-952` `is_ladder_mode_enabled()`
- `src/agent/benchmark.rs:1019-1029` `agentfloor_tasks()`
- `src/agent/experiment.rs:1042-1090` (改修対象)
- arxiv 2605.00334 AgentFloor
