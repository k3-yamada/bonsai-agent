# bonsai-agent Lab 実機テスト結果

> Z-1 Phase 3 で CLAUDE.md から分離 (項目 255)。元の CLAUDE.md「Lab 実機テスト結果」セクション verbatim 移行 + 関連知見。

## Lab 天井連続記録 (Bonsai-8B 1bit, k=3, 10 cycle paired)

| Lab | 軸 | 結果 | 数値 | 教訓 |
|---|---|---|---|---|
| v17 | ERL Heuristics Pool | REJECT | Δ=−0.0014 / p=0.5072 | 天井 7 連続 |
| v19 | Frontier (score 軸) | REJECT | Δ=+0.0072 / p=0.4262 | 天井 8 連続 |
| v19 (案 A) | Frontier (context-length 軸) | **ACCEPT** | bucket 0→1 gradient = -0.1944 (基準 1.94x) | 第 6 軸 baseline 確立 |
| v20 | KG-FactCheck (Pearson r) | **REJECT** | r=0.0 / Δ=-0.0038 / p=0.5316 / wall 19h 9m | **天井 9 連続** + structural finding (matched=0 で variance ゼロ) |
| v21 smoke | KG-FactCheck (matched 軸 + Pearson r) | **REJECT** | r=0.0 / Δ=+0.0227 / p=0.2429 / wall 9h 33m | **天井 10 連続** + structural metric 再現 (`(conf+matched+unknown)/total=1.0` deterministic、matched 軸単独なら variance 0.77-0.81 で新 metric 軸候補) |
| v22 Phase A | A/A test (項目 247 noise floor σ_Δ 採取試行) | KILL | cycle 1=78 min / cycle 2=81 min (target 30 min の 2.6x slowdown 観測 → kill) | Phase A 完走前に項目 249 で SSE timeout + fallback retry overhead を根本対処する判断、kill 12:37 |
| 249 Phase 4 Smoke G-RT | F1+F2 (long_sse + mlx_only primary 切替) Lab Runtime Stabilization 検証 | **REJECT** | wall **103m42s** (target ≤35 min を 3x 超過) / SSE timeout 5 回発火 / 非ストリーミング fallback / 5 task × k=3 | F1 (180s) でも MLX 2-bit 初トークン latency catch しきれず、F1+F2 単独では Lab v22 paired 完走基準未達 = 次 phase F4 設計 (MLX pre-warm or non-streaming default) 必要、MLX 2-bit は llama-server 1-bit gguf より ~2x latency finding |
| 262 G-T6-1/2 | T6 PROMPT_AUGMENT 単独 (unpaired) | **ACCEPT** (historical) | G-T6-1=0.7671 / G-T6-2=0.8778 = **+14.4%** + perfect pass@k=1.000 / wall 56-60 min | 3 directive (step-by-step + restate + revise) で T6 LongHorizonPlanning に大幅効果、unpaired のため真効果は paired re-eval (項目 265 follow-up) で再評価必要 |
| 263 G-DB-R-2/3 | Dynamic Budget ratio tune (unpaired) | **ACCEPT** (historical) | G-DB-R-3 T6: 0.7671→0.8396=+9.5% strong / G-DB-R-2 mixed: 0.8298→0.8266=-0.32% borderline | default 30/30/15/25 (kg-heavy 救済) を確証、ただし unpaired = paired re-eval (項目 265 follow-up) 必要 |
| 264 G-T6-D-1/2 | T6 案 D-2 in-session MEMORY_AUG triple stack (unpaired) | **REJECT** | G-T6-D-1=0.7726 (AUGMENT+BUDGET stack baseline) / G-T6-D-2=0.6251 (+MEMORY_AUG)=**-19.1%** | triple augmentation stack で destructive 観測、ただし全 smoke で compaction prune 実発火ゼロ確証 → H-A4 (measurement noise / hidden binary diff) leading hypothesis、項目 265 (max_context reduction) で真因確定 |
| 265 G-MCT2 | max_context_tokens reduction (smoke=1 で 14000→6000 自動) | **infrastructure ACCEPT + ACCEPT 条件 (a) failed structural** | wall **62.7 min** / baseline score=0.8209 / pass@k=1.0000 / pass_consec=1.0000 / **AgentFloor T6-LongHorizon=0.82** (paper 0.30 を +0.52 上回る) / compaction.budget emit 49 件 total=6000 確証 / `[prev:`/`[summarized]`/`[saved:` 全て 0 | 構造的 finding 決定的解明 = production smoke k=3 baseline 各 iteration は独立 session で context reset、各 session が 4500 tokens 未満で完了 → level1 never fires = max_context=14000 でも 6000 でも prune accumulate しない構造設計に依拠。項目 264 `prune marker 全て 0` は H-A4 noise でも H-A2 misclass でもなく、**smoke task structure の構造的特性**で確証。項目 263 ratio tune ACCEPT (+9.5%) は H-A4 noise で確定、次手 = Phase 2 paired re-evaluation (g_paired_*_v2.sh で σ_noise 比較) |
| 266 G-PAIRED-265 (4/5 paired complete) | 案 D-2 MEMORY_AUG paired re-eval (ABAB...AB 9/10 cycle、cycle_b_5 MLX 接続失敗 abort) | **REJECT (statistically overwhelming)** | mean Δ=**-0.1384** (全 4 paired で B<A by 0.12-0.15 consistent) / Cohen's dz=**-10.5979** / paired t=-21.20 / Wilcoxon p=1.0 (B>A 方向、destructive 方向は p≈0.0) / A side stdev ~0.12 (cycle_a_5=0.5345 突発低) / B side stdev ~0.03 | 項目 264 D-2 (in-session MEMORY_AUG) **真の destructive effect 確定 (paired evidence)**、項目 264 G-T6-D-2 -19.1% (unpaired) を -14% に補正 + 真効果確証、H-A3 (1bit precision floor で dual augmentation attention dispersion) 仮説支持、BONSAI_T6_MEMORY_AUG **env default OFF 確定 (paired)** + infrastructure 削除 (item_264 case B) ROI 高い deletion candidate。bug fix 副次 = lab_v22_metric.py sanity_detail KeyError 解消、paired re-eval (g_paired_*_v2.sh series) 全 target で metric 動作可能化 |
| 267 case B cleanup (実機実施) | 案 D-2 MEMORY_AUG infrastructure 完全削除 | **完遂** (commit `df135df`) | 4 files changed / **+3 / -469 = net -466 LOC** / cargo test 1378→1372 passed (-6、退行ゼロ) / clippy clean / fmt clean / structural 4 passed / drift All PASS / backward compat 完全維持 (env unset で同等挙動) | paired evidence (項目 266) → case B 実施 (項目 267) を単一 session 内完遂、bonsai 「Scaffolding > Model」原則確証 (REJECT 機構速やか撤去)。並列実行: Track A (g_paired_263_v2 ~10h smoke 進行中) と本削除を干渉なく並列 (release binary 不変 / debug test profile separate / source 変更が走行中 smoke 無影響の設計確立) |

## Bonsai-8B 能力プロファイル (LADDER + AgentFloor、項目 224)
- T1 Instruct=**0.68** / T2 SingleTool=0.52 / T3 ToolSelect=0.77 / T4 MultiStep=0.64 / T5 ErrorRecov=0.70 / **T6 LongHorizon=0.47** (weakest)
- tier-targeted 変異の優先攻略 = T6 偏向 (Lab v22+ HypothesisGenerator 改修の前提)

## Plan A 系列完結 (項目 230 → 234 → 235 → 236 → 237 → 238 → 239 → 240 → 241 → 242 → 243)
- 3 段配線: (a) 230 wiring (b) 235 trajectory scope (c) 237 event emit → G-6b で **factcheck total=5 / conflicting=3 = Bonsai-8B fabricate 検出初成功**
- 項目 242 Phase 4 G-7b 実機: **total=11 matched=8 unknown=0 conflicting=3 mean_path_len=1.00** (Lab v20 structural finding 解消、matched 軸 variance 復活確証)
- 項目 243 G-7c 実機 (input 書換後): **total=15 matched=12 unknown=0 conflicting=3 mean_path_len=1.00 / score=0.7613 / pass@k=0.8889** = matched +50% / score +40.6% 大幅改善 + 副作用解消、Lab v21 paired 起動 ready
- **Lab v20 完走 (項目 241)**: ACCEPT 基準 (a) Pearson r=0.0 REJECT (天井 9 連続) / (b) ON 5/5 total≥1 PASS
- structural finding: `(conf+unk)/total=1.0` deterministic = matched=0 で variance ゼロ → Pearson r 計算不可能
- conf=3 deterministic 5/5 = **Plan A 機構自体は production-ready**、ただし「効果計測 metric 設計」が次の課題
- 案 A 採用 = KG seed 拡張で matched>0 シナリオ生成 + Lab v21 再 paired (別 plan、~2-3h plan + ~15h wall)

## 過去 Lab アーカイブ
- v1〜v14 / v15 / v8/v9/v10 = `memory/lab_history_v1_v6.md` (v1/v3/v5/v6.2、デフォルト化済変異の系譜)
- v15-v19 詳細 = `memory/lab_history_v9_period.md` + 各 `session_2026_*_handoff.md`

## Z-3 drift monitor との連動 (項目 257 候補)

本 file は Lab 完走後 hook で自動更新される候補 (`docs/quality/scores.md` と統合)。
詳細: `.claude/plan/drift-monitor-weekly-gc.md` Phase 4 (coverage section)。
