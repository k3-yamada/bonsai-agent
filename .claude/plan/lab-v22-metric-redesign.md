# Lab v22 Metric Redesign — Paired Δscore 主軸への移行 + Noise Floor 計測 (項目 247 候補)

**状態**: planning-only (2026-05-19 起票、CCG synthesis 経由)
**推奨度**: ★★★ (Lab v17〜v21 で「天井 10 連続 REJECT」、metric 設計が構造的 blocker)
**推定工数**: Phase A ~2h impl + 2h wall / Phase B ~1-2h impl / Phase C ~2h impl / Phase D ~5h wall / Phase E 10-13h wall
**起点**:
- Lab v21 smoke (15-task × k=3 × 10 cycle、wall 9h 33m) で REJECT 確定 (Pearson r=+0.0000)
- Lab v20 (19h 9m wall) でも同じ structural finding が出ていた → metric 設計が根本問題
- CCG synthesis (Codex 統計厳密性 + Gemini LLM eval literature) で合議形成

---

## §1. 起点と現状診断

### 1.1 Lab v21 smoke の実測
5 paired cycle (10 cycle total):
| pair | total | matched | conflict | unknown | mat/tot | Δscore | fail_rate |
|---|---|---|---|---|---|---|---|
| 1 | 15 | 12 | 3 | 0 | 0.80 | -0.0412 | 0.8889 |
| 2 | 13 | 10 | 3 | 0 | 0.77 | +0.0455 | 0.6667 |
| 3 | 19 | 15 | 3 | 1 | 0.79 | -0.0238 | 0.6000 |
| 4 | 14 | 11 | 3 | 0 | 0.79 | +0.0666 | 0.6667 |
| 5 | 16 | 13 | 3 | 0 | 0.81 | +0.0663 | 0.4444 |

- mean Δscore=+0.0227、t=+0.9855、df=4、one-sided p=0.2429、dz=0.441
- matched/total range 0.77〜0.81 (variance あり、conflict は常に 3 で deterministic、unknown ほぼ 0)

### 1.2 root cause: 現行 Pearson r の structural deterministic 問題
- 現行 ACCEPT 基準: `Pearson r((conflict+matched+unknown)/total, failure_rate) >= 0.3`
- 5 cycle 全て `(conflict+matched+unknown)/total = 1.0` → 片方の変量が deterministic → variance ゼロ → Pearson r は機械的に 0
- 「天井 10 連続 REJECT」(Lab v10〜v21) は **metric の数学的崩壊** で、Plan A 効力の問題ではない可能性が高い

### 1.3 CCG advisor 合議
- **Codex (統計)**: Pearson r を主判定から外し paired Δscore + Wilcoxon + Cohen's dz、smoke=10/decision=20/strict=27 cycle 推奨
- **Gemini (LLM eval)**: T=0 greedy 断行、halluc task 増、A/A test で noise floor 必須、FActScore / Brier Score 別軸検討
- **両者一致**: Pearson r を主判定から外し、Δscore 系を主軸、factcheck は補助ゲート

---

## §2. 問題定義

### 2.1 現行 metric が解決していないこと

| 課題 | 影響 |
|---|---|
| (conflict+matched+unknown)/total = 1.0 deterministic | Pearson r=0 が機械的に出る |
| n=5 paired で Pearson r threshold 0.3 を要求 | 統計的に意味なし (r=0.3 で one-sided p≤0.10 は不成立、r=0.69+ 必要) |
| paired Δscore は副次扱いで ACCEPT 判定に効かない | 真の効果信号を捨てている (Lab v21 で Δ=+0.0227 positive direction だが REJECT 確定) |
| 1bit Bonsai-8B sampling noise (T=0.5) | cycle 内 noise が信号を埋もれさせる |
| Lab v17〜v21 同 task set (success_fact + halluc + 既存) | hallucination 検出能力の効果が出にくい構造 |

### 2.2 何を解決するか

1. **metric 数学的崩壊の解消** (Pearson r 廃止 or 診断ログ降格)
2. **n=5-10 で意味ある効果検出力** (Wilcoxon + dz + 適切な α)
3. **noise floor の事前計測** (A/A test で σ_noise 確定)
4. **factcheck の補助ゲート化** (false positive 抑止だが ACCEPT 主判定から外す)
5. **task 構成の effect-sensitive 化** (halluc 増で差分検出しやすく)

---

## §3. 設計

### 3.1 統合 ACCEPT 基準 (Claude 案、両 advisor 折衷)

```
ACCEPT if:
  (a) mean(Δscore) >= max(+0.010, noise_floor × 2)
  (b) Wilcoxon one-sided p <= 0.10 (smoke) / 0.05 (full lab)
  (c) paired Cohen's dz >= 0.30 (smoke) / 0.40 (full)
  (d) factcheck sanity:
      mean(matched/total) >= 0.78
      AND mean(unknown/total) <= 0.05
      AND total >= 8 per cycle

(a) AND (b) AND (c) で主判定、(d) は補助ゲート (false-alarm 抑止)
```

### 3.2 検定手法の選定 (Codex × Gemini 折衷)

- **Wilcoxon Signed-Rank Test (主)**: paired Δscore の非正規性に robust、n=5-20 で使える
- **paired t-test (副次)**: 出力に併記、CLT 担保 (n が増えたとき)
- **Cohen's dz**: 効果サイズ、`mean(Δ) / sd(Δ)` で計算
- **Pearson r(matched/total, failure_rate)**: 診断ログのみ、n>=20 で意味あり判定

### 3.3 検出力 (Codex 実測 simulation より)

| cycles | one-sided α=0.10 | one-sided α=0.05 |
|---:|---:|---:|
| 10 | ~0.59 | ~0.43 |
| 15 | ~0.73 | ~0.58 |
| 20 | ~0.82 | ~0.70 |
| 27 | ~0.90 | ~0.81 |

→ **smoke=10 (方向確認)、decision=20 (α=0.10 で実用域)、strict=27 (α=0.05 で 80% power)**

### 3.4 task 構成改修 (Gemini 提案)

現行: 15 task = success_fact 5 + halluc 3 + 既存 7
新案: 15 task = success_fact 3 + halluc 7 + 既存 5
- halluc 比率 3/15=20% → 7/15=47% (Gemini 「50% まで増」目標に近づける)
- success_fact は input prompt 統一済 (項目 243 で修正)、3 件で十分
- 既存 7 → 5 にして factcheck 発火率を上げる

ただし task 構成変更は別 plan で詳細決め (本 plan は metric 軸 focus)。Lab v22 pilot は **現 task 構成のまま**で metric だけ差し替えて baseline 取り直し可能。

### 3.5 temperature 制御 (Gemini 提案)

- 現行: bonsai 全体 default T=0.5
- 提案: Lab 専用 env `BONSAI_LAB_TEMP=0` で greedy/deterministic 化
- 効果: cycle 内 sampling noise 排除、metric 信号上昇
- 影響範囲: Lab 内のみ、production code は T=0.5 維持 (後方互換)

---

## §4. 実装 Phase A-E

### Phase A: Noise Floor 計測 (A/A Test) — 必須 pre-experiment

**目的**: OFF × OFF paired 5 cycle で σ_noise 測定、Phase D-E の effect threshold の根拠を確定。

**実装**:
- `scripts/lab_v22_aa_test.sh` 新規 (現行 `lab_v21_smoke_paired.sh` の env を `BONSAI_KG_FACTCHECK_ENABLED` unset 両側に変更)
- 5 cycle (10 calls of OFF) で Δscore の sd を計算
- 出力: `noise_floor σ_Δ` = OFF×OFF の paired Δ sd 値

**Wall**: ~2h (T=0 で cycle 30 min × 10、最低 5h、ただし `BONSAI_LAB_TEMP=0` を Phase C 後にしないと正確値出ないので Phase C 後実施)

**ACCEPT (Phase A) 単体**: σ_Δ < 0.05 (1bit 噪音許容範囲) であれば Phase D 進行可。

### Phase B: 新 metric 実装

**実装**:
- `scripts/lab_v22_metric.py` 新規 (`lab_v21_paired_ttest.py` を base に extend)
- 出力指標:
  - `mean(Δscore)`, `sd(Δscore)`, `dz`
  - Wilcoxon W, one-sided p (small-n exact、または approximation)
  - paired t-test (副次)
  - factcheck sanity: matched/total mean, unknown/total mean, total per cycle
  - Pearson r 診断ログ (n>=20 のときのみ ACCEPT 適用、他は informational)
- Verdict: ACCEPT/REJECT を §3.1 基準で機械判定

**コード規模**: ~150 行 (`lab_v21_paired_ttest.py` 比 +50 行)

### Phase C: 実験設計改修

**実装**:
- `BONSAI_LAB_TEMP` env 追加: `src/agent/experiment.rs` で `BONSAI_LAB_TEMP=0` の時 inference temperature override (~10 行)
- (optional) task 構成変更は別 plan 候補 (項目 247 候補とは別、本 plan §6 で言及)
- backward compat: env unset 時は既定 T=0.5 維持

**Cost**: ~2h coding + ~30 min smoke G-VV (env on/off 確証)

### Phase D: Lab v22 smoke pilot (10-cycle decision)

**実装**:
- `scripts/lab_v22_paired.sh` 新規 (現行 v21 smoke を base に、env 変更):
  - `BONSAI_LAB_TEMP=0` for cycle (Phase C 後)
  - 5 paired (ON, OFF) × 2 = 10 cycle (smoke baseline)
- 出力解析: `python3 scripts/lab_v22_metric.py ./lab-v22-logs`
- Wall: ~5h (T=0 で cycle 30 min 想定、現行 47 min より速い)

**Phase D 判定**:
- ACCEPT → Phase E (full lab) 進行
- REJECT direction (Δ<0) → bonsai 1bit の改善余地構造的に薄い証拠
- REJECT but Δ>0 + n>5 で有意化 → Phase E 進行余地あり

### Phase E: 結果次第で decision (20-cycle) or strict (27-cycle)

- Phase D で ACCEPT direction なら 20-cycle で confirm (α=0.10 で 82% power)
- Phase D で Δ direction ambiguous なら 27-cycle で strict (α=0.05 で 81% power)
- Wall: 10h (20 cycle) or 13h (27 cycle) with T=0

---

## §5. risks / mitigations

| # | Risk | Mitigation |
|---|------|-----------|
| R1 | T=0 greedy で task variation が落ちて全 cycle 同じ score になる | A/A test で `sd(Δ)` 確認、`sd(Δ)>0` で variance あり継続、`sd(Δ)=0` なら T=0.1 等の微調整 |
| R2 | base rate problem で Phase D で REJECT 出る = 1bit Bonsai-8B の改善余地が構造的に薄い | Bayesian prior 0.2-0.3 で評価、REJECT も「機構自体は動くが効果が薄い」という finding として記録 |
| R3 | Wilcoxon が n=5 で小サンプル exact test 必要 | scipy.stats.wilcoxon の `mode='exact'` 使用、または手計算 (W statistic table) |
| R4 | task 構成変更 (halluc 増) は別 plan が筋、本 plan で混ぜると scope creep | 本 plan は **metric 軸のみ**、task 構成変更は別 plan 候補 (項目 247b 等) で対処 |
| R5 | Phase C の `BONSAI_LAB_TEMP` env 追加で既存 test 失敗 | env unset で T=0.5 default 維持、test fixture は env unset 前提なので影響なし |
| R6 | Phase B の Wilcoxon 実装で scipy 依存追加 | scipy は Phase 4 G-* で `scripts/` 内のみ使用、Cargo に依存追加なし |
| R7 | 「Phase D で ACCEPT 出ても Phase E で REJECT」のリスク | smoke 10 cycle で direction 強い signal 出てから full lab に進む保守的 protocol |

---

## §6. 期待効果

### 構造的 metric 修正
- Pearson r 数学的崩壊を解消、ACCEPT 判定が「天井問題」を構造的に生まず実 effect 検出可能
- 「天井 10 連続 REJECT」が metric 問題 or 真効力なし問題かを切り分け可能

### Lab v21 smoke の positive direction 信号 (+0.0227) を真評価
- 新 metric (Wilcoxon + dz) で再評価 → 直接効果 (post-hoc) or 間接効果 (context denoising) 切り分け
- factcheck = post-hoc設計の理論と実測の整合性 audit

### 1bit Bonsai-8B 評価 framework の体系化
- noise floor (σ_Δ) を Lab 起動前 mandatory check、以後の Lab v23+ で同 framework 再利用
- Wilcoxon + dz + 補助ゲートの combination は他の実験 (Vault lint, Dynamic Budget 等) でも使える

---

## §7. 起票候補項目

- **項目 247** = 本 plan の Phase A-D 完遂 (Lab v22 pilot 10 cycle で direction 確証)
- 項目 247b (別 plan、将来) = task 構成変更 (halluc 50% target)
- 項目 247c (別 plan、将来) = FActScore / Brier Score 追加 (Gemini 提案)
- 項目 247d (別 plan、将来) = Attention Entropy 計測で間接効果検証 (Gemini 提案)

---

## §8. 依存 / 並行性

### 完遂前提
- 項目 244 KG lint 完遂 ✅ (factcheck の sanity gate が安定動作の前提)
- Ternary Bonsai MLX 起動 ✅ (本 session 整備、Lab v22+ は本物の Ternary primary 経路で稼働)

### 並行可
- 項目 248 Dynamic Budget Compaction 実装と並行可 (本 plan は metric 軸、Dynamic Budget は compaction 軸で独立)
- 項目 246 Vault lint 実装と並行可 (vault_lint.rs 独立)

### 排他
- experiment.rs の Phase C 修正 (`BONSAI_LAB_TEMP`) と他の experiment.rs 同時編集回避
- Phase A の A/A test 実施中は他の Lab 起動禁止 (cycle 内 noise 計測中)
- Phase D pilot 中は他の Lab 起動禁止 (paired データの隔離)

---

## §9. ロールバック戦略

- 全変更は **新規 script (scripts/lab_v22_*.sh, lab_v22_metric.py) + env-gated 既存 code 改修 (`BONSAI_LAB_TEMP`)** のみ
- env unset で完全な従来動作 (T=0.5、現行 Pearson r metric も `lab_v21_paired_ttest.py` 経由で残置)
- 完全 rollback = 新規 scripts 削除 + `BONSAI_LAB_TEMP` env 分岐削除 (~1 commit)

---

## §10. Quick Start

```bash
cd /Users/keizo/bonsai-agent
git log -3 --oneline

# Phase A: A/A Test (Noise Floor 計測)
$EDITOR scripts/lab_v22_aa_test.sh  # OFF×OFF paired 5 cycle
nohup ./scripts/lab_v22_aa_test.sh ./lab-v22-aa-logs > /tmp/lab_v22_aa.log 2>&1 &
# ~2h 後
python3 scripts/lab_v22_metric.py ./lab-v22-aa-logs --mode aa
# 出力: σ_Δ = noise floor

# Phase B: Metric 実装
$EDITOR scripts/lab_v22_metric.py  # Wilcoxon + dz + 補助ゲート
# unit test 経由で出力確証 (mock paired data で ACCEPT/REJECT verdict)

# Phase C: BONSAI_LAB_TEMP env 追加
$EDITOR src/agent/experiment.rs  # BONSAI_LAB_TEMP=0 で T=0 override
cargo test --lib  # 1294 passed 維持
cargo build --release

# Phase D: Lab v22 smoke pilot (10 cycle、T=0)
$EDITOR scripts/lab_v22_paired.sh  # smoke 15 task × k=3 × 10 cycle、T=0
nohup ./scripts/lab_v22_paired.sh ./lab-v22-logs > /tmp/lab_v22_run.log 2>&1 &
# ~5h 後
python3 scripts/lab_v22_metric.py ./lab-v22-logs

# Phase E: 結果次第で decision (20) or strict (27)
nohup ./scripts/lab_v22_paired.sh ./lab-v22-full-logs --cycles 20 > /tmp/lab_v22_full.log 2>&1 &
```

---

## §11. metadata

- 起点 commit: `22805e1` (項目 245 完遂)
- 起点 advisors (CCG): Codex (statistical rigor) + Gemini (LLM eval literature)
  - artifact: `.omc/artifacts/ask/codex-bonsai-agent-rust-1bit-bonsai-8b-mac-m2-16gb-lab-paired-acce-2026-05-18T23-17-03-125Z.md`
  - artifact: `.omc/artifacts/ask/gemini-bonsai-agent-1bit-bonsai-8b-rust-lab-paired-metric-llm-evalu-2026-05-18T23-16-22-532Z.md`
- 関連 plan: `kg-lint-coverage-check.md` (項目 244)、`vault-lint-coverage-check.md` (項目 246 候補)、`dynamic-token-budget-compaction.md` (項目 248 候補)
- 関連 memory: `context_failure_modes_audit_2026_05_19.md`、`ternary_bonsai_paths_2026_05_19.md`、`session_2026_05_19_handoff.md`
- 想定 commit 範囲: 4-5 commits (Phase A script + Phase B metric + Phase C env + Phase D pilot harness + handoff)
- 想定 line 範囲: +400 行 / -10 行 (scripts/lab_v22_*.sh + lab_v22_metric.py + experiment.rs env-gated diff)
