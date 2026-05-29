# Dynamic Budget Ratio Phase 6 — KG Micro-Tune (項目 263 follow-up 候補)

**状態**: planning-only (2026-05-30 起票)
**推奨度**: ★★ (項目 263 G-DB-R-2 borderline -0.32% 解消、Lab v22+ paired blocker の最終 polish)
**推定工数**: ~30 min plan review + ~45 min TDD strict Phase 1-3 + ~30 min Phase 4 smoke (G-DB-R-2-v2 / G-DB-R-3-v2)
**起点**:
- 項目 263 Dynamic Budget Ratio Tune (案 A+D atomic combo) 実装完遂 (commits `9d2164f` Red / `761f3d8` Green / `da42c5d` Refactor、default 40/30/20/10 → 30/30/15/25)
- 項目 263 Phase 4 Smoke G-DB-R-2/3 実機検証完了:
  - **G-DB-R-3 (T6)**: score 0.7671 → 0.8396 = **+9.5% strong ACCEPT**
  - **G-DB-R-2 (mixed smoke)**: score 0.8298 → 0.8266 = **-0.32% borderline** (strict target 0.8218 ≥ クリアだが micro miss)
- compaction.budget log emit で kg=25% allocation 実機確認済 (Phase 5 axis-priority prune 発火)
- 参照 plan: `.claude/plan/dynamic-budget-ratio-tune.md`
- 参照 SSOT: `docs/architecture/module-layer-rules.md` / `OVERFLOW_TOLERANCE_DIVISOR=1000` / `OVERFLOW_TOLERANCE_FLOOR=1`

## §1. Motivation + 観察 finding

### 1.1 項目 263 Phase 4 Smoke 観察値

| metric | env=0 baseline | env=1 (263 default 30/30/15/25) | Δ | 判定 |
|--------|----------------|----------------------------------|---|---|
| G-DB-R-2 mixed smoke score | 0.8298 | 0.8266 | **-0.0032 (-0.32%)** | borderline (strict 0.8218 ≥ クリア、micro miss) |
| G-DB-R-3 T6 score | 0.7671 | 0.8396 | **+0.0725 (+9.5%)** | strong ACCEPT |
| compaction.budget emit | 0 | 多数 | + | KG 25% allocation 発火確認 |

### 1.2 仮説 4 件 (項目 263 H1-H4 を踏まえた micro-tune 軸)

- **H6-1**: kg=25% は smoke の KG 使用量に対しまだ若干低く、`[memory_search]` 高頻度 task で marginal overflow が残存
- **H6-2**: entities=15% は smoke で marginal overprovision、kg へ 5pt 譲渡可能 (entities 圧縮可能性)
- **H6-3**: buffer=30% は recent_buffer recall に必要最小限、これ以上譲渡不可 (G-DB-R-2 score 維持 risk)
- **H6-4**: T6 +9.5% は kg 比率敏感度が高く、kg を 30% に引き上げると T6 がさらに改善 or 悪化する可能性が両方ある (smoke fixture vs T6 KG 分布の差)

### 1.3 ACCEPT 条件

- **G-DB-R-2-v2** (mixed smoke 5 task × k=3): Δ ≥ +0.005 vs 項目 263 (T6=0.8266) → score ≥ 0.8316 で ACCEPT
- **G-DB-R-3-v2** (T6 lh_* × k=3): T6 score 維持 (Δ ≥ -0.005 vs 263 T6=0.8396) → score ≥ 0.8346 で ACCEPT
- 副軸: compaction.budget log で entities overflow 発火率減少 (5pt 救済 = entities→kg trade-off 確証)

## §2. 案比較

### 2.1 4 案 (5 軸採点 = 効果見込み / 工数 / リスク / rollback 容易性 / G-DB-R-3 T6 +9.5% 維持)

| 軸 | 案 A: 30/30/10/30 (entities→kg) | 案 B: 25/30/15/30 (buffer→kg) | 案 C: 28/28/14/30 (全軸 micro) | 案 D: status quo + 別軸 (圧縮率改善) |
|---|---|---|---|---|
| 効果見込み (G-DB-R-2 +0.005) | ◎ (kg 30% は H6-1 直撃) | ○ (kg 30% 同等だが buffer recall risk) | ○ (全軸 5% 圧縮で smoothing) | △ (圧縮率は別軸、ratio 軸では未触) |
| 工数 | ◎ 5 LOC (default 値変更のみ) | ◎ 5 LOC | ○ 5 LOC + sum normalize 注意 | △ 別箇所 (compact_level1/2) 改修 |
| リスク (G-DB-R-2 score) | ○ entities 縮小、recent buffer 不変で recall 維持 | △ buffer 縮小は recent_buffer recall risk (recent N-2 件超過時) | ○ 全軸 5% 縮小、各軸 marginal | ◎ ratio 不変、blast radius 別箇所 |
| rollback 容易性 | ◎ default 値再変更のみ (1 commit revert) | ◎ 同 | ○ 同 | △ 別軸への影響範囲広 |
| G-DB-R-3 T6 +9.5% 維持 | ○ kg 25→30 は T6 にさらに positive 推定 (項目 262 augment と独立) | ○ 同 | △ buffer/summary 縮小で T6 long-horizon の context recall に影響可能 | ◎ 不変 |
| **総合** | **★★★ (smoke 救済 + T6 維持期待)** | ★★ (buffer risk) | ★★ (smoothing 中庸) | ★ (別軸、本 plan scope 外) |

### 2.2 推奨案 = 案 A (★★★)

採用 default ratio: **`buffer 0.30, summary 0.30, entities 0.10, kg 0.30`**
- buffer 0.30 維持 / summary 0.30 維持 / entities 0.15 → 0.10 / kg 0.25 → 0.30

**根拠**:
- entities=15% は項目 263 plan 起票時点で「buffer→entities 縮小」の trade-off 余地として残置、Phase 6 では entities が次の縮小候補
- kg=30% は smoke fixture KG 使用量 (`[memory_search]` 1-2 件 ≈ 25-30% 帯) を完全 cover
- G-DB-R-2 borderline -0.32% は micro miss、large refactor (案 D) は ROI 不適切
- kg ratio 単軸 5pt 引き上げで H6-1 直接対応

### 2.3 棄却案

- **案 B 単独**: buffer 縮小は recent_buffer recall への直接影響、G-DB-R-2 score 改善見込みが案 A と同等で risk profile 劣後
- **案 C 単独**: 全軸 micro 調整は smoothing 効果限定的、各軸 5% 縮小で marginal、test 期待値変更箇所も増加
- **案 D 単独**: 圧縮率改善は本 plan scope 外 (項目 248 Phase 5 axis-prune の延長線)、Phase 6 plan の最小変更原則に反する

## §3. TDD strict 3-phase outline

### Phase 1 Red

項目 263 Phase 1 で追加された 5 test のうち、default ratio 依存箇所の expected 値を新 default (30/30/10/30) に書換。新規 test 1 件追加 (entities 10% 下限 sanity)。

1. `t_budget_ratios_default_is_kg_heavy`: kg expected 0.25 → 0.30 (案 A 確証)
2. `t_budget_ratios_default_sum_is_one`: 不変 (sum=1.0 維持確証)
3. `t_overflow_axes_strict_greater_than`: 不変 (overflow logic 触らず、Phase 6 scope 外)
4. `t_overflow_axes_epsilon_tolerance`: 不変 (同上)
5. `t_default_ratio_no_kg_overflow_in_smoke_fixture`: KG fixture token 数と新 kg=30% allocation の no-overflow 関係再確認

**追加 test** (Phase 6 専用):
- `t_budget_ratios_default_entities_minimum_10pct`: entities ratio が 10% 下限を割らない sanity guard

```rust
#[test]
fn t_budget_ratios_default_is_kg_heavy() {
    let r = BudgetRatios::default();
    assert!((r.knowledge_graph - 0.30).abs() < 0.001, "kg ratio = 0.30 (Phase 6)");
    assert!((r.relevant_entities - 0.10).abs() < 0.001, "entities ratio = 0.10 (Phase 6)");
}

#[test]
fn t_budget_ratios_default_entities_minimum_10pct() {
    let r = BudgetRatios::default();
    assert!(
        r.relevant_entities >= 0.10 - f32::EPSILON,
        "entities ratio は 10% 下限維持 (Phase 6 sanity、将来 drift 防止)"
    );
}
```

期待 = `t_budget_ratios_default_is_kg_heavy` + `t_budget_ratios_default_entities_minimum_10pct` 2 件 FAIL (新 test 追加分含む)、他は PASS 維持。

### Phase 2 Green

```rust
impl Default for BudgetRatios {
    fn default() -> Self {
        // 項目 263 Phase 6 micro-tune: plan §2.2 案 A — entities→kg 5pt 移動 (30/30/10/30).
        // 起点: 項目 263 Phase 4 Smoke G-DB-R-2 borderline -0.32% (0.8298→0.8266) 解消狙い.
        //   G-DB-R-3 T6 +9.5% は kg 比率敏感性が高い実機証拠、kg 25→30% でさらに余裕確保.
        // 仮説 H6-1: kg=25% は smoke fixture の `[memory_search]` 高頻度 task で
        //   marginal overflow 残存、30% に引き上げて完全 cover.
        // 仮説 H6-2: entities=15% は smoke で marginal overprovision、kg へ 5pt 譲渡可.
        // buffer/summary は 30% 維持 (recent_buffer recall 保護 + summary 圧縮率不変).
        Self {
            recent_buffer: 0.30,
            conversation_summary: 0.30,
            relevant_entities: 0.10,
            knowledge_graph: 0.30,
        }
    }
}
```

cargo test --lib で Phase 1 Red の 2 件 + 既存全 PASS。

### Phase 3 Refactor

- rustdoc 強化: `BudgetRatios::default()` の comment block を Phase 6 由来 + G-DB-R-2 -0.32% borderline 解消根拠 + 仮説 H6-1/H6-2 込みに書換
- `OVERFLOW_TOLERANCE_DIVISOR` / `OVERFLOW_TOLERANCE_FLOOR` SSOT は不変 (項目 263 Phase 3 で確立済、Phase 6 では参照のみ)
- clippy / fmt clean

production code 変更箇所:
- `src/agent/compaction.rs::impl Default for BudgetRatios` (3 行 = entities 0.15→0.10, kg 0.25→0.30, comment 更新)

## §4. Phase 4 Smoke acceptance

| Gate | env | command sketch | 期待 |
|------|-----|----------------|------|
| G-DB-R-2-v2 | env=1 (新 default 30/30/10/30) | `BONSAI_DYNAMIC_BUDGET=1 BONSAI_LAB_TASK_LIMIT=5 ./scripts/lab_v22_aa_test.sh` | **score ≥ 0.8316** (Δ ≥ +0.005 vs 263 T6=0.8266 で ACCEPT) |
| G-DB-R-3-v2 | env=1 + T6 (lh_*) × k=3 | `SMOKE_TASK_IDS="lh_*" BONSAI_DYNAMIC_BUDGET=1 ...` | **score ≥ 0.8346** (Δ ≥ -0.005 vs 263 T6=0.8396 で 維持確認) |
| G-DB-R-4-v2 副軸 | compaction.budget log 解析 | log emit grep | entities overflow 発火率減少、kg overflow ゼロ |

**G-DB-R-2-v2 FAIL 時の分岐**:
- 0.8266-0.8316 (改善幅 0.0-0.005、micro miss): Phase 6 効果限定、案 C (28/28/14/30) 再試行 or status quo 確定
- 0.82-0.8266 (改悪): entities=10% 不足、entities=12% へ再 tune (entities=15→12、kg=25→28)
- < 0.82: 案 A revert、項目 263 default (30/30/15/25) 復元

**G-DB-R-3-v2 FAIL 時の分岐**:
- T6 0.83-0.8346 (-1pt 内): 許容範囲、smoke 救済優先で ACCEPT
- T6 < 0.83 (-2pt 超): T6 で kg 過剰 (30% > optimal)、kg=27% に再 tune (entities=13、kg=27)
- T6 < 0.82 (-4pt 超): 案 A revert 必須、Phase 6 中止

## §5. 既存資産との整合

- 項目 248 Phase 1-4 wiring: 変更なし (env-gated path 不変)
- 項目 261 Phase 5 axis-priority prune: 直接対象 (default ratio 値のみ tune、prune logic 不変)
- 項目 262 案 A T6 augment (`BONSAI_T6_PROMPT_AUGMENT=1`): env 独立、direct paired 可能 (combined ACCEPT 期待)
- 項目 263 案 A+D atomic combo: 本 plan は 263 の Phase 6 微調整、A+D combo logic は不変
- 項目 264 Phase 2a KG-augmented T6: env 独立、本 plan と直交 (Phase 6 ACCEPT 後 paired 可能)
- 項目 247 Lab v22 metric: paired Δscore + Wilcoxon 流用
- rust-reviewer 過去 finding (M-1/L-1/L-2): 項目 263 Phase 2/3 で解消済、本 plan で再発生なし
- SSOT 参照: `OVERFLOW_TOLERANCE_DIVISOR=1000` / `OVERFLOW_TOLERANCE_FLOOR=1` (compaction.rs:929/932) は不変、`docs/architecture/module-layer-rules.md` 不変

## §6. Rollback strategy

### 6.1 即時 rollback (env 経路)

```bash
# 完全 disable (項目 263 Phase 4 と同等の env unset 経路)
unset BONSAI_DYNAMIC_BUDGET
./target/release/bonsai --lab ...

# 旧 default (項目 263) を env override で復元
BONSAI_DYNAMIC_BUDGET=1 \
  BONSAI_DYNAMIC_BUDGET_RATIOS="0.30,0.30,0.15,0.25" \
  ./target/release/bonsai --lab ...
```

### 6.2 段階 rollback decision tree

```
G-DB-R-2-v2 FAIL?
├── 0.8266-0.8316 (改善 < 0.005) → status quo (263 default 30/30/15/25) 確定、Phase 6 中止
├── < 0.8266 → 案 A revert、entities=12% へ再 tune (28/30/12/30)
└── G-DB-R-3-v2 FAIL → kg 30% → 27% へ後退 (28/30/15/27) で T6 救済優先
```

### 6.3 完全 rollback

1 commit revert (Phase 2 Green の default 値書換のみ) で 263 default (30/30/15/25) 復元。
新規 test `t_budget_ratios_default_entities_minimum_10pct` も Phase 1 Red commit と atomic revert。

## §7. 次手

1. Phase 1 Red: 既存 5 test の expected 値書換 + 新 test 1 件追加、cargo test --lib で 2 件 FAIL 確認
2. Phase 2 Green: `Default for BudgetRatios` 3 行書換 (entities 0.15→0.10, kg 0.25→0.30, comment 更新)、cargo test --lib 全 PASS
3. Phase 3 Refactor: rustdoc 強化 (Phase 6 由来 + G-DB-R-2 -0.32% finding + H6-1/H6-2 明示)、clippy/fmt clean
4. `cargo build --release` 完走 (~5 min)
5. G-DB-R-2-v2 / G-DB-R-3-v2 smoke 実機 (~1.5h、要 MLX server 起動)
6. 2 gate 全 PASS → 項目 262 案 A T6 augment と direct paired 可能化、項目 264 Phase 2a と直交評価
7. Lab v22+ paired blocker 完全解消 → 項目 261/262/263/264 atomic combo paired 起動許可

## §8. metadata

- 起点 commits: 項目 263 Phase 1-3 (`9d2164f` Red / `761f3d8` Green / `da42c5d` Refactor)
- 起点 plan: `.claude/plan/dynamic-budget-ratio-tune.md`
- 関連 plan:
  - `.claude/plan/dynamic-token-budget-compaction.md` (項目 248 Phase 1-4)
  - `.claude/plan/dynamic-token-budget-phase5-axis-prune.md` (項目 261 Phase 5)
  - `.claude/plan/lab-v22-metric-redesign.md` (項目 247 Lab v22 paired metric)
- 想定推定項目番号: **項目 265** (項目 263 Phase 6 follow-up、項目 264 KG-augmented T6 と独立 track)
- 起票根拠: G-DB-R-2 borderline -0.32% 解消 + kg=30% で T6 +9.5% 維持期待 + Lab v22+ paired blocker 最終 polish
- production code touch: 3 行 (compaction.rs::impl Default、Phase 2 Green) + rustdoc (Phase 3)
- TDD strict: 既存 test 1 件 expected 値書換 + 新規 test 1 件 (entities 10% 下限 sanity)
