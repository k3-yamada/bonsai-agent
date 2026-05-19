# Lab v22b Task Composition — Hallucination Task 50% target (項目 247b 候補)

**状態**: planning-only (2026-05-19 起票、CCG Gemini 提案より)
**推奨度**: ★ (項目 247 metric redesign 完遂後の effect-sensitivity 改善案)
**推定工数**: ~1h plan + ~1-2h impl (benchmark.rs 再配分) + Phase 4 smoke
**起点**:
- Gemini 提案: 「Antagonistic Hallucination Tasks の増量、50% まで」 (CCG synthesis)
- Lab v21 smoke で halluc 3/15=20% → matched 12/15=80% deterministic に近い
- factcheck の真価は「正解維持」より「地雷を踏まないこと」、現状 task 構成では差分出にくい

---

## §1. 問題定義

### 1.1 現状の task 構成 (SMOKE_TASK_IDS = 15)
- success_fact: 5 (matched 軸を稼ぐ、項目 242 で追加)
- hallucination: 3 (factcheck conflicting 軸を稼ぐ)
- 既存 instruct/reasoning/etc: 7

→ halluc 比率 = 3/15 = **20%**

### 1.2 何が問題か
- Lab v21 smoke で conflict=3 deterministic (毎 cycle 同じ 3 件 detect)
- success_fact は matched 12/15 で安定、conflict 軸の variance 不足
- = factcheck 効力を「地雷検出能力」で測る軸が小さすぎて effect size が出ない

### 1.3 Gemini 提案
> 現在の success_fact 中心から、「意図的に嘘を混ぜたプロンプト (Trick questions)」を 50% まで増やす。
> factcheck の真価は「正解を維持すること」ではなく「地雷を踏まないこと」にある。

---

## §2. 設計 (案 A 推奨)

### 案 A (推奨): success_fact 5 → 3 / halluc 3 → 7 / 既存 7 → 5
- halluc 比率 7/15 = **47%** (50% target に近い)
- success_fact は項目 243 input 書換で十分動作、3 件で matched 軸 keep
- 既存 7→5 = factcheck 非対象 task を減らし、factcheck 発火率を上げる

### 案 B (棄却): 15 → 30 に倍増 + halluc 比率維持
- wall time 倍増、Phase A noise floor 採取コストも倍
- 効果サイズより総量で稼ぐ案、検出力 power table とは別軸

### 案 C (棄却): halluc 専用 smoke tier 追加
- BONSAI_LAB_SMOKE=halluc 等の env 拡張
- task selector 設計拡大で scope creep

---

## §3. 実装 (Phase 1-3、TDD strict)

### Phase 1 (Red) — 3 failing test

1. `t_smoke_task_ids_halluc_ratio_at_least_45_percent`: halluc 比率 ≥ 0.45 を assert
2. `t_smoke_task_ids_includes_existing_halluc_pool`: 既存 halluc 3 task + 追加 4 task が含まれる
3. `t_smoke_task_ids_total_15`: 総数 15 task 維持

### Phase 2 (Green)

- `src/agent/benchmark.rs::SMOKE_TASK_IDS` の構成変更:
  - 削除: success_fact 2 件 + 既存 2 件
  - 追加: 新規 halluc task 4 件 (KG seed に対応する fact 拡張も必要、項目 244 lint 対象)

### Phase 3 (Refactor)
- 既存 halluc task pool 拡大 (現状 3 → 7、`hallucination_tasks()` 関数 expand)
- KG seed `seed_kg_for_factcheck_lab` を 8 → 12 fact に拡張 (新 halluc 4 件分の counter-fact)

---

## §4. risks / mitigations

| # | Risk | Mitigation |
|---|------|-----------|
| R1 | halluc task 増で score base が下がり、Δscore 評価が歪む | 比率変更前の baseline (Phase A A/A test) を v22b 適用後再採取で比較 |
| R2 | 新 halluc task の KG seed 整合性破壊 | 項目 244 KG lint で seed_kg_for_factcheck_lab clean check を必須化 (G-9 同等) |
| R3 | task selection 公平性 (halluc 偏重で他軸 (reasoning/instruct) のカバー減) | 既存 7 → 5 削減時に各軸 1 件以上残す配分原則 |

---

## §5. 期待効果
- factcheck conflicting 軸の variance up (3 deterministic → 5-7 variable)
- Lab v22 metric (Wilcoxon + dz) で effect size 大きくなる可能性
- 「地雷検出力」軸で factcheck の真価を計測

---

## §6. 依存 / 並行性

### 完遂前提
- 項目 247 Lab v22 Phase A-D 完遂 (本 plan は task 軸、Phase A-E と直交)

### 並行可
- 項目 246 Vault lint 実装 (vault_lint.rs 独立)
- 項目 248 Dynamic Budget Compaction (compaction.rs 独立)

### 排他
- benchmark.rs 同時編集不可 (本 plan + 項目 244 KG seed 拡張は同じ section)

---

## §7. metadata
- 起点: CCG Gemini 「Antagonistic Hallucination Tasks 増量、50% まで」
- 関連 plan: `lab-v22-metric-redesign.md` (項目 247、metric 軸)
- 想定 commit 範囲: 2-3 commit (benchmark.rs 構成変更 + KG seed 拡張 + smoke G-VV)
- 想定 line 範囲: +60 行 / -40 行 (benchmark.rs SMOKE_TASK_IDS list + halluc task pool)
