# Dynamic Budget Ratio Tune (項目 261 Phase 5 follow-up、項目 263 候補)

**状態**: planning-only (2026-05-22 起票)
**推奨度**: ★★★ (項目 261 G-RT2 拡張 -9% regression の即時対応、Lab v22+ paired blocker 解消)
**推定工数**: ~3-4h plan + Phase 1-3 TDD strict + ~45 min Phase 4 smoke (G-DB-R-1/2/3)
**起点**:
- 項目 261 Dynamic Budget Phase 5 axis-priority prune 実装完遂 (commit `da42c5d` / `761f3d8` / `9d2164f`)
- G-RT2 拡張 (`BONSAI_DYNAMIC_BUDGET=1` + smoke 5 task × k=3) **score 0.8298 → 0.7542 = -9.1% regression**
- compaction.budget emit 13 回 = 実機で axis-priority prune 発火確認、しかし score 劣化
- rust-reviewer M-1 / L-1 / L-2 note (overflow_axes `>=` 判定、float precision)
- 参照 SSOT: `docs/architecture/module-layer-rules.md`

## §1. Motivation + 観察 finding

### 1.1 G-RT2 拡張 観察値

| metric | env=0 baseline | env=1 (Phase 5 default) | Δ |
|--------|----------------|-------------------------|---|
| smoke score (5 task × k=3) | 0.8298 | 0.7542 | **-0.0756 (-9.1%)** |
| compaction.budget emit | 0 | 13 | +13 (prune 発火実証) |

### 1.2 仮説 4 件

- **H1**: kg=10% allocation が smoke の KG 使用量に低すぎる (KG 1228 token = `[memory_search]` 1-2 件で overflow)
- **H2**: overflow_axes `>=` 判定が Buffer overflow を不要発火 (rust-reviewer M-1)
- **H3**: candidate sort で overflow priority > score 強すぎ
- **H4**: smoke task vs T6 long-horizon で KG 使用量分布が異なる、default は T6 向け

### 1.3 ACCEPT 条件

- **G-DB-R-2**: smoke 5 task × k=3 で score regression ≤ 1% (Δ ≥ -0.008、≥ 0.8218)
- **G-DB-R-3**: T6 long-horizon で score 維持 or 改善 (Δ ≥ 0)
- 副軸: overflow log axis-balanced

## §2. 案比較

### 2.1 4 案

| 軸 | 案 A: kg 10→25% | 案 B: α 0.2→0.4 | 案 C: tier-aware ratio | 案 D: `>=` → `>` |
|---|---|---|---|---|
| 実装影響 | ◎ | ○ | △ | ◎ |
| H1/H2/H3/H4 対応 | H1◎ H2△ H3× H4△ | 全△ | H1○ H2△ H3× H4◎ | H1△ H2◎ H3× H4△ |
| backward compat | ◎ | ◎ | ◎ | ◎ |
| test 追加 | +2-3 | +2-3 | +5-7 | +2-3 |
| Lab 再 build | ○ 1 cycle | ○ 1 cycle | △ 2 cycle | ○ 1 cycle |
| **総合** | **smoke 即対応** | 動的調整 | 構造的 | M-1 解消 |

### 2.2 CCG synthesis

- **Codex**: 案 D + 案 A atomic combo (M-1 解消 + KG 救済)
- **Gemini**: 案 A 単独 (user 観測 -9% に直接効く)
- **Claude final = 案 A + 案 D atomic combo**: H1 直撃 + M-1 解消 + float tolerance 別途吸収

採用 default ratio: **`buffer 0.30, summary 0.30, entities 0.15, kg 0.25`**
- buffer 0.40 → 0.30 / summary 0.30 維持 / entities 0.20 → 0.15 / kg 0.10 → 0.25

### 2.3 棄却案

- **案 B 単独**: relevance MVP では効果保証なし
- **案 C 単独**: tier resolver test cost + Lab paired 2 cycle 再起動

## §3. TDD strict 3-phase outline

### Phase 1 Red (5 failing test)

1. `t_budget_ratios_default_is_kg_heavy` (kg == 0.25)
2. `t_budget_ratios_default_sum_is_one` (4 軸 sum = 1.0)
3. `t_overflow_axes_strict_greater_than` (境界等量 = 不含、案 D)
4. `t_overflow_axes_epsilon_tolerance` (float tolerance 範囲内 不含)
5. `t_default_ratio_no_kg_overflow_in_smoke_fixture` (KG 2400 token に 3000 allocate で no overflow)

```rust
#[test]
fn t_overflow_axes_strict_greater_than() {
    let usage = AxisUsage { buffer: 100, summary: 50, entities: 30, kg: 20, unclassified: 0 };
    let allocated = AllocatedBudget { total: 200, buffer: 100, summary: 50, entities: 30, kg: 20 };
    let result = overflow_axes(&usage, &allocated);
    assert!(result.is_empty(), "境界等量は overflow ではない (案 D)");
}
```

### Phase 2 Green

```rust
impl Default for BudgetRatios {
    fn default() -> Self {
        // 項目 263: 30/30/15/25 (smoke KG 救済 + buffer 譲渡)
        Self {
            recent_buffer: 0.30,
            conversation_summary: 0.30,
            relevant_entities: 0.15,
            knowledge_graph: 0.25,
        }
    }
}

fn overflow_tolerance(total: usize) -> usize {
    (total / 1000).max(1)  // 0.1% tolerance
}

pub(crate) fn overflow_axes(usage: &AxisUsage, allocated: &AllocatedBudget) -> Vec<(MemoryKind, usize)> {
    let tol = overflow_tolerance(allocated.total);
    let mut result = Vec::new();
    if usage.buffer > allocated.buffer + tol { result.push((MemoryKind::Buffer, usage.buffer - allocated.buffer)); }
    if usage.summary > allocated.summary + tol { result.push((MemoryKind::Summary, usage.summary - allocated.summary)); }
    if usage.entities > allocated.entities + tol { result.push((MemoryKind::Entities, usage.entities - allocated.entities)); }
    if usage.kg > allocated.kg + tol { result.push((MemoryKind::Kg, usage.kg - allocated.kg)); }
    result.sort_by(|a, b| b.1.cmp(&a.1));
    result
}
```

### Phase 3 Refactor

- `OVERFLOW_TOLERANCE_PROMILLE` const SSOT 化
- rustdoc 強化 (項目 263 由来 + 観察 -9% finding)
- clippy / fmt clean

## §4. Phase 4 Smoke acceptance

| Gate | env | 期待 |
|------|-----|------|
| G-DB-R-1 | env=1 + 旧 ratio 強制 `BONSAI_DYNAMIC_BUDGET_RATIOS="0.4,0.3,0.2,0.1"` | score ≈ 0.7542 ± 0.005 |
| G-DB-R-2 | env=1 (新 default) | **score ≥ 0.8218** (回帰 1% 以内) |
| G-DB-R-3 | env=1 + T6 (lh_*) × k=3 | T6 score 維持 (Δ ≥ 0) |
| G-DB-R-4 副軸 | log 解析 | KG overflow 発火率低下 |

**G-DB-R-2 FAIL 時の分岐**:
- 0.78-0.82: kg 25%→30% 微 tune
- < 0.78: 案 A revert、案 D 単独再測
- > 0.83: 完遂、Lab v22 paired 起動許可

## §5. 既存資産との整合

- 項目 248 Phase 1-4 wiring: 変更なし
- 項目 261 Phase 5 axis-priority: 直接対象
- 項目 262 案 A T6 augment: env 重複なし、direct paired 可能
- 項目 247 Lab v22 metric: paired Δscore + Wilcoxon 流用
- rust-reviewer M-1 解消、L-1/L-2 は Phase 6 で別途

## §6. Rollback strategy

### 6.1 即時 rollback (env 経路)

```bash
BONSAI_DYNAMIC_BUDGET=1 \
  BONSAI_DYNAMIC_BUDGET_RATIOS="0.4,0.3,0.2,0.1" \
  ./target/release/bonsai --lab ...
```

### 6.2 段階 rollback decision tree

```
G-DB-R-2 FAIL?
├── 0.78-0.82 → kg ratio 微 tune (反復)
├── < 0.78 → 案 A revert、案 D 単独で再測
└── G-DB-R-3 FAIL → kg 25% → 20% へ後退
```

### 6.3 完全 rollback

3 commits revert で default 40/30/20/10 + overflow `>=` 復元。

## §7. 次手

1. Phase 1 Red: 5 failing test、1343 passed
2. Phase 2 Green: default 30/30/15/25 + `>` + tolerance、1353 passed
3. Phase 3 Refactor: rustdoc + const SSOT + clippy/fmt clean
4. `cargo build --release` 完走
5. G-DB-R-1/2/3 smoke 実機
6. 3 gate 全 PASS → 項目 262 案 A T6 augment と direct paired 可能化
7. Lab v22+ paired blocker 解消

## §8. metadata

- 起点 commit: `da42c5d` / `761f3d8` / `9d2164f` (項目 261 Phase 5)
- 関連 plan: `dynamic-token-budget-compaction.md` / `dynamic-token-budget-phase5-axis-prune.md` / `lab-v22-metric-redesign.md`
- 想定推定項目番号: **項目 263**
- 起票根拠: G-RT2 拡張 -9.1% regression / rust-reviewer M-1 解消 / Lab v22+ paired blocker 解消
