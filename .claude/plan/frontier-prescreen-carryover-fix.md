# Frontier Pre-Screen Carry-over Fix — 項目 229 副次 finding (a) follow-up

**状態**: planning-only (2026-05-15 起票)、推奨度 ★、推定工数: ~1.6h active + 1.5h smoke wall
**起点**: CLAUDE.md 項目 229 §3 副次 finding (a) = pre-screen REJECT 経路で `[INFO][lab.frontier]` log emit 成功するが DB `frontier_bucket_scores='[]'` / `frontier_inject_scores='[]'` で永続化 gap

---

## §1. 背景

### 1.1 副次 finding (a) 詳細 (Phase 4 G-4b/c v3 実機 evidence)
- id=226 (G-4b, `BONSAI_FRONTIER_INJECT_ENABLED=1`): log line 260-261 で `inject: (no T6 tasks populated)` emit、DB `'[]'`/`'[]'`
- id=227 (G-4c, `BONSAI_FRONTIER_ENABLED=1`): log line 376-377 で `bucket 1 [2048, 4096): 0.6313` emit、DB `'[]'`/`'[]'`
- **真因**: `build_prescreen_reject_experiment` helper (experiment.rs:975-1021、項目 224 抽出済) が `frontier_bucket_scores: Vec::new()` / `frontier_inject_scores: Vec::new()` で空 Vec 固定、baseline からの carry-over なし

### 1.2 項目 224 vs 本 plan の対比 (1:1 mapping)
| 軸 | 項目 224 (tier carry-over) | 本 plan (frontier carry-over) |
|---|---|---|
| 対象 fields | `tier_t1..t6` (6 列) | `frontier_bucket_scores`, `frontier_inject_scores` (2 列) |
| 修正対象 source | helper 抽出前 inline literal | `build_prescreen_reject_experiment` helper 内 2 行 |
| carry-over semantics | "no improvement" なら baseline と同等が論理的に正しい | 同 (env-gated `is_frontier_enabled()` で OFF 時空 Vec 維持) |
| wiring fix の最終結節点 | tier_t1..t6 3 段配線完遂 (項目 224) | run_k populate (Sub-Phase 2F) + from_multi_results transfer (Sub-Phase 2D) + pre-screen carry-over (本 plan) の 3 段配線最終完遂 |

### 1.3 真因コード位置 (experiment.rs:975-1021)
```rust
fn build_prescreen_reject_experiment(...) -> Experiment {
    Experiment {
        // 既存 fields...
        tier_t1: tiers.and_then(|t| t[0]),  // 項目 224 で carry-over 済
        // ...
        frontier_bucket_scores: Vec::new(),   // ← 本 plan 対象
        frontier_inject_scores: Vec::new(),   // ← 本 plan 対象
    }
}
```

---

## §2. 設計

### 2.1 Option 比較 (推奨 = 案 A)
| 案 | 概要 | 採否 |
|---|---|---|
| **A** | helper signature に 2 引数追加 + env-gated carry-over | ✓ |
| B | 無条件 baseline carry-over | ✗ 結果同等だが意図不明 |
| C | 全列 carry-over (tier + frontier + RDC/VAF/GDS + pass@(k,T)) | ✗ scope creep |
| D | NULL 維持 + ドキュメント化 | ✗ Lab v19 解析で REJECT 行分析不能 |

### 2.2 Option A 実装案
```rust
fn build_prescreen_reject_experiment(
    experiment_id: String,
    mutation_type: MutationType,
    mutation_detail: String,
    baseline_score: f64,
    baseline_tier_avg_scores: Option<[Option<f64>; 6]>,
    baseline_frontier_bucket_scores: &[(usize, f64)],  // 追加
    baseline_frontier_inject_scores: &[(usize, f64)],  // 追加
    estimated_delta: f64,
    snapshot: HashMap<String, String>,
) -> Experiment {
    let frontier_bucket = if crate::agent::frontier::is_frontier_enabled() {
        baseline_frontier_bucket_scores.to_vec()
    } else {
        Vec::new()
    };
    let frontier_inject = if crate::agent::frontier::is_frontier_inject_enabled() {
        baseline_frontier_inject_scores.to_vec()
    } else {
        Vec::new()
    };
    Experiment {
        // 既存...
        frontier_bucket_scores: frontier_bucket,
        frontier_inject_scores: frontier_inject,
    }
}
```

### 2.3 caller 修正 (1 箇所)
`run_experiment_loop` 内 pre-screen REJECT branch で `&baseline.frontier_bucket_scores` / `&baseline.frontier_inject_scores` 引数追加。

---

## §3. TDD strict 5 phase

### Phase 1 (Red) — 4 failing test
1. `t_prescreen_reject_carries_baseline_frontier_when_bucket_enabled` (FRONTIER_ENABLED=1 で carry-over)
2. `t_prescreen_reject_carries_baseline_frontier_when_inject_enabled` (INJECT_ENABLED=1 で carry-over)
3. `t_prescreen_reject_frontier_empty_when_env_off` (両 env unset で空 Vec、**後方互換 100% 確証**)
4. `t_prescreen_reject_frontier_empty_when_baseline_empty` (env ON でも baseline 空なら空 Vec)

**env mutex**: `FRONTIER_TEST_LOCK` を frontier.rs `pub(crate)` 化または experiment.rs 独立 mutex (項目 226 CRITIC_TEST_LOCK pattern)

### Phase 2 (Green)
1. helper signature 2 引数追加
2. helper body で env-gated carry-over
3. caller 1 箇所更新
4. cargo test 4 PASS、1245 → **1249 passed**
5. clippy 0 / fmt 0

### Phase 3 (Refactor)
- 既存「V16 (frontier benchmark) も同じ理由で pre-screen REJECT 経路では計測なし」コメント置換
- mutex visibility 判断

### Phase 4 (Smoke G-4)
| Gate | env | 期待 |
|------|-----|------|
| **G-4a** | unset | 1249 pass 維持、DB `'[]'`/`'[]'` (env OFF 後方互換) |
| **G-4b** | `INJECT_ENABLED=1` + SMOKE | wiring 動作 (SMOKE で T6 ゼロ = baseline 空 → 結果空、helper env-gating path 実行確認) |
| **G-4c** | `FRONTIER_ENABLED=1` + SMOKE | SQLite で pre-screen REJECT row の frontier_bucket_scores が非空 JSON (例: `[[1, 0.6313]]`) |

### Phase 5 (Commit) — 4 commits

---

## §4. 期待効果

### Lab v19 解析 sample size 拡張
- 現状: pre-screen REJECT row は `'[]'` で `bucket_variance.py` 解析対象外
- 本 plan 後: pre-screen REJECT row も baseline frontier carry-over で解析可、**sample size +20-30% 増加**見込

### 項目 229 wiring 最終完遂
- run_k populate (Sub-Phase 2F) + from_multi_results transfer (Sub-Phase 2D) + pre-screen carry-over (本 plan) の **3 段配線完成**

---

## §5. risks / mitigations
| # | Risk | Mitigation |
|---|------|-----------|
| **R1** | baseline.frontier_* 空で carry-over 後も空 | `.to_vec()` で preserve、test 4 で確証、`WHERE != '[]'` で filter 可 |
| **R2** | env 並列実行 race | `FRONTIER_TEST_LOCK` Mutex (項目 225 PASS_K_T pattern) |
| **R3** | "no improvement" 仮定の論理妥当性 | `prescreened: true` flag で区別、別 plan で pre-screen 拡張可 |
| **R4** | helper signature 変更で caller 漏れ | 1 callsite のみ、cargo build で網羅検証 |
| **R5** | test count 期待値ズレ | 本 plan を他 plan の前に消化、handoff で明示 |

---

## §6. 起票候補項目
**項目 234** = 完遂時 (frontier pre-screen carry-over fix、test 1245→1249、Lab v19 sample +20-30%)

---

## §7. 依存
- ✅ 項目 224 helper 抽出 (commit `a52edc6`)
- ✅ 項目 229 frontier Phase 1-4 (8 commits)
- ✅ SCHEMA_V16
- ❌ Lab v19 起動 (~20h wall): 完走前後どちらでも可、ただし起動前完遂で解析 sample +20-30%

---

## §8. ロールバック戦略
- env opt-in default OFF (env unset で空 Vec 維持 = 100% 後方互換)
- git revert 1 commit (Phase 2 Green) で clean rollback

---

## §9. Quick Start
```bash
grep -n "build_prescreen_reject_experiment" src/agent/   # 1 caller 確認

# Phase 1 Red
$EDITOR src/agent/experiment.rs   # 4 test 追加
cargo test --lib t_prescreen_reject_carries_baseline_frontier 2>&1 | tail -10  # 4 FAIL

# Phase 2 Green
cargo test --lib                  # 1249 passed

# Phase 3 Refactor
cargo clippy --lib -- -D warnings && cargo fmt --check

# Phase 4 Smoke (background)
cargo build --release
./target/release/bonsai --lab --lab-experiments 1 | tee /tmp/g4a.log &
BONSAI_LAB_SMOKE=1 BONSAI_FRONTIER_INJECT_ENABLED=1 ./target/release/bonsai --lab --lab-experiments 1 | tee /tmp/g4b.log &
BONSAI_LAB_SMOKE=1 BONSAI_FRONTIER_ENABLED=1 ./target/release/bonsai --lab --lab-experiments 1 | tee /tmp/g4c.log &

# Verification
sqlite3 "$HOME/Library/Application Support/bonsai-agent/bonsai.db" \
  "SELECT id, prescreened, frontier_bucket_scores FROM experiments WHERE id > 227"
# 期待: G-4c row で frontier_bucket_scores='[[1, 0.6313]]' 非空
```

---

## §10. 不要転用 (rejected)
| 案 | 棄却理由 |
|---|---|
| pre-screen REJECT 全列 carry-over | scope creep、各 metric semantics 異なる |
| pre-screen 経路で frontier 新規計測 | コスト ↑、4 task × k=1 で T6 サンプル不足 |
| frontier schema 変更 (Vec → struct) | V16 確立済、本 plan は populate 経路のみ |
| helper 解体 inline literal | 項目 224 helper 再利用性毀損 |
| env-gating 撤廃 | 意図不明、Cerememory 三本柱 pattern 維持 |

---

## §11. 参考
- CLAUDE.md 項目 229 §3 副次 finding (a) (本 plan 起点)
- `.claude/plan/frontier-benchmark-impl.md` (項目 229 親 plan)
- `.claude/plan/agentfloor-prescreen-tier-fix.md` (項目 224 = tier carry-over、TDD pattern 流用元)
- `src/agent/experiment.rs:975-1021` (`build_prescreen_reject_experiment` helper)
- `src/agent/experiment_log.rs::from_multi_results` (full-cycle 参照実装、Sub-Phase 2D `2bbb83d`)
- `src/agent/benchmark.rs` (`MultiRunBenchmarkResult.frontier_*`、Sub-Phase 2F `35eb648`)
- env opt-in pattern: 項目 217-219 Cerememory、項目 225 PASS@(k,T)、項目 229 `BONSAI_FRONTIER_*`
