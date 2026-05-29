# Lab v22 paired metric 必須化 plan — σ_Δ noise floor 確立 + 全 scaffolding ACCEPT 判定基準統一

**起票日**: 2026-05-30
**起点**: 項目 264 G-T6-D-2 REJECT 確定 + DEFINITIVE root cause (H-A4 measurement noise leading)
**関連 memory**: `session_2026_05_30_handoff.md` §5 教訓 (2)(5)、`compaction_budget_static_finding_2026_05_30.md`
**優先度**: ★★★ (全 BUDGET 系 feature の判定信頼性を支配)
**production code touch**: なし (script + process plan only)

---

## 1. 背景

### 1.1 unpaired smoke の statistical error 実例

項目 263 ACCEPT (G-DB-R-3 +9.5% / G-DB-R-2 -0.32%) と項目 264 G-T6-D-1 finding (-12.0% vs G-T6-2)、G-T6-D-2 (-19.1%) は全て **異なる binary + 異なる時刻 + 5 task × k=3 = 15 run small sample** で paired 設計なし。

| 比較 | 期待効果 | 観測効果 | sample size | paired? | 真効果 / noise 識別可能? |
|------|---------|---------|--------------|---------|----------------------------|
| 項目 262 G-T6-1 vs G-T6-2 | AUGMENT +0.05+ | +0.1107 (+14.4%) | 15 vs 15 | × | × |
| 項目 263 G-DB-R-3 (T6 ratio tune) | BUDGET +0.05+ | +0.0725 (+9.5%) | 15 vs 15 (historical baseline) | × | × |
| 項目 264 G-T6-D-1 (BUDGET stack) | BUDGET 加算 | -0.1052 (-12.0%) | 15 vs 15 | × | × |
| 項目 264 G-T6-D-2 (stack 3) | MEMORY_AUG +0.05+ | -0.1475 (-19.1%) | 15 vs 15 (D-1 baseline) | × | × |

→ **σ_Δ noise floor 未確立 = 真効果 ≥ noise かの判定不能**。

### 1.2 既存 metric (Lab v22) の到達点と gap

`scripts/lab_v22_metric.py` (項目 247) は Wilcoxon signed-rank + Cohen's dz + factcheck 補助 + Pearson r 診断 + ACCEPT/REJECT 統合 + A/A mode 完備。
ただし **A/A test 未実行** (項目 247 起票時の Phase A `lab_v22_aa_test.sh` は script ready だが production smoke で run 未済) → σ_Δ 実測値が project 履歴上に存在しない。

### 1.3 過去の structural finding

Lab v20/v21 paired smoke (項目 238/245) で `(conf+matched+unknown)/total=1.0` deterministic 観測 (matched 軸 variance 不在で Pearson r=0)。本 plan の paired re-run でも同 finding 再現可能性あり → factcheck enable 必要 (項目 242 で baseline cover、score 軸は別途)。

---

## 2. ゴール

1. **σ_Δ noise floor 確立**: `lab_v22_aa_test.sh` 完走 + `lab_v22_metric.py --mode aa` で sd(Δ) 測定、project-level 共通 noise floor を docs/quality/ に shipping
2. **ACCEPT 閾値の機械化**: 以後の全 scaffolding (項目 263 ratio tune / 項目 264 案 D-2 / 項目 262 AUGMENT 等) の ACCEPT は **Δ ≥ max(0.010, σ × 2)** で統一判定
3. **過去 finding の paired re-evaluation 義務化**: 項目 262/263/264 の本効果を同 binary + paired 設計で再測定、unpaired finding は historical only として annotate
4. **session handoff の必須 metric**: BUDGET 系 feature ship 前に paired re-run evidence 添付を慣行化

---

## 3. process plan (4 phase)

### Phase 1: A/A test 完走 (前提 = `.claude/plan/max-context-tokens-reduction-force-prune.md` ACCEPT 後推奨)

**時系列**:
1. MLX server 起動 (`./scripts/start-mlx-server.sh`、port 8000)
2. `cargo build --release` (binary 鮮度確保、max_context_tokens reduction 適用後を使用すれば prune 実 path も評価可能)
3. `nohup ./scripts/lab_v22_aa_test.sh ./lab-v22-aa-logs > /tmp/lab_v22_aa_run.log 2>&1 &` で 10 cycle (5 paired) 起動
   - env: AUGMENT=1, BUDGET=1, MEMORY_AUG=0, TEMP=0, MLX_ONLY=1, WARMUP=1, TASK_LIMIT=5, SMOKE=1
   - wall: ~5h (T=0 で cycle 30 min 想定 × 10)
4. 完了後 `python3 scripts/lab_v22_metric.py ./lab-v22-aa-logs --mode aa`
5. sd(Δ) = σ_noise が出力される (期待 ~0.020-0.050)
6. MLX server kill (port 8000 free)
7. 結果を `docs/quality/noise-floor-aa-{date}.md` に記録 (sd / max diff / 5 paired cycle 一覧)

**outcome**:
- σ_noise (project-level noise floor) 確定値
- ACCEPT 閾値 = `max(0.010, σ_noise × 2)` の defendable 基準確立

### Phase 2: 過去 finding の paired re-evaluation

**target priority**:
1. ★★★ 項目 263 default 30/30/15/25 vs 40/30/20/10 = T6×k=3 paired re-run (G-DB-R-3 v2 / 案 A original A/B)
2. ★★★ 項目 264 案 D-2 (MEMORY_AUG=1 vs MEMORY_AUG=0) = G-T6-D-1 v2 / G-T6-D-2 v2 paired re-run
3. ★★ 項目 262 AUGMENT=1 vs unset = G-T6-2 v2 paired re-run
4. ★ Phase 6 plan kg 25→30% paired

**process per target**:
- 同 binary (本 plan 起票後 build & freeze)
- 同 task set (5 T6 lh_* × k=3)
- 同 T=0、同 MLX server session (途中 restart 禁止)
- A/B/A/B/A/B/A/B/A/B (10 cycle = 5 paired) 推奨、wall ~5h × N target
- `python3 scripts/lab_v22_metric.py ./lab-{target}-paired-logs --mode paired --noise-floor {σ_noise}`
- ACCEPT: Δ score ≥ max(0.010, σ_noise × 2) + Wilcoxon p < 0.05 + Cohen's dz ≥ 0.3

**outcome**:
- 過去 ACCEPT の真偽が機械的に確証 (項目 263 ratio tune 真効果 / 項目 264 案 D-2 完全 REJECT 確証)
- σ_noise を超える効果のみ scaffolding default 採用

### Phase 3: docs/quality 蓄積 + handoff template update

- `docs/quality/noise-floor-aa-{date}.md` で σ_noise 履歴蓄積 (binary version / MLX model / env 一覧)
- `docs/quality/scores.md` に「paired re-evaluation 必須」明記 + 過去 unpaired finding に annotation
- session_handoff template (`memory/session_*_handoff.md`) に「scaffolding ACCEPT 判定では paired evidence + σ_noise 比 必須」chk添加
- CLAUDE.md `~~~` 直近項目 entry でも「paired re-eval 済」のみ ACCEPT 表記、unpaired は INVESTIGATIONAL annotation 必須化

### Phase 4: drift monitor 統合 (option)

- `scripts/drift/run_lint.sh` に Phase 5 として `paired_evidence_check.py` 追加 case (検出: CLAUDE.md 項目で `ACCEPT` 言及があるが paired log 不在の場合 warn) — 別 plan、本 plan 対象外

---

## 4. ACCEPT 条件

### 4.1 Phase 1 ACCEPT
- (a) A/A test 完走 (10 cycle、wall ≤ 6h、SSE timeout 0、MLX server 中断なし)
- (b) sd(Δ) = σ_noise 値が出力され、`docs/quality/noise-floor-aa-{date}.md` に記録
- (c) σ_noise ≤ 0.10 (期待 ~0.02-0.05、Bonsai-8B 1bit greedy で構造的に低値)

### 4.2 Phase 2 ACCEPT (per target)
- (d) paired cycle ≥ 5 完走
- (e) Wilcoxon p / Cohen's dz / Δ score が `noise-floor` を超える scaffolding のみ採用

---

## 5. risk + rollback

### risk
- **MLX server 中断**: Phase 1/2 で MLX server stall → cycle drop 発生時、再起動後の cycle は別 group として記録、metric 側で除外可能 (lab_v22_metric.py の `--exclude {label}` で対応想定)
- **σ_noise 高値**: > 0.10 なら Bonsai-8B 1bit 評価基盤の信頼性問題、別 plan で T=0 以外の noise control 検討
- **wall budget 超過**: max_context_tokens reduction 未適用で paired re-run = 各 1 cycle ~80 min × 10 cycle × 4 target = ~50h、要逐次実施 (session 分割)

### rollback
- Phase 1 結果不採用: σ_noise = 0.05 fallback (項目 247 plan 推定値) で運用、要 update 注記
- 本 plan 不採用: 既存 unpaired finding 継続使用、project annotation のみ追加 (production code touch ゼロ)

---

## 6. cross-references

- 前提 plan: `.claude/plan/max-context-tokens-reduction-force-prune.md` (prune 強制発火経路確保で paired re-eval の measurement meaningful 化)
- 関連 plan: `.claude/plan/dynamic-budget-phase6-kg-microtune.md` (本 plan ACCEPT 後に kg 30% paired evaluate)
- 関連 memory: `session_2026_05_30_handoff.md` §5 教訓 / `compaction_budget_static_finding_2026_05_30.md` §3.5 DEFINITIVE conclusion
- 関連 script: `scripts/lab_v22_aa_test.sh` (Phase 1) / `scripts/lab_v22_metric.py` (Phase 1/2 共通 metric) / `scripts/lab_v22_paired.sh` (Phase 2)
- 関連項目: 247 (Lab v22 metric redesign) / 261/262/263/264 (paired re-eval target)

---

## 7. 期待効果 (session_2026_05_30_handoff §5 教訓を機械化)

| 教訓 | 本 plan による機械化 |
|------|------------------------|
| (1) Scaffolding > Model 原則の再検討 | paired re-eval 結果のみ "Model" を語る根拠とする |
| (2) paired smoke design 必須化 | Phase 1+2 で慣行化 |
| (3) emit ≠ 実 prune | max_context_tokens reduction plan 連動 |
| (4) Test PASS と Production effect 別問題 | paired evidence で production effect 確証 |
| (5) ACCEPT 前 paired re-run + σ_Δ 確立 | 本 plan 主旨 = root principle 機械化 |
