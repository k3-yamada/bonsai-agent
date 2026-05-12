# Plan: Lab v18 — G1 Critic 別 LLM Effectiveness 検証 paired t-test (項目 226 Phase 5、F11 falsifiable hypothesis)

> **項目番号訂正 (2026-05-12)**: 起票時 "項目候補 224" だったが、項目 224 (AgentFloor pre-screen tier fix) と項目 225 (PASS@(k,T)) が先行確定したため、G1 Critic Phase 1 完遂は **項目 226** に確定 (commit `b95e809` + LOW fix commit、1190 passed)。本 plan は項目 226 の Phase 5 effectiveness 検証として位置付けを更新。

> **由来 plan**: `.claude/plan/critic-separate-llm-impl.md` (G1、640 行起票済) の **Phase 5 effectiveness 検証** として明記。本 plan は G1 critic 機構が **天井 8 候補** か **dead-code 候補** かを paired t-test で data-driven に判定する Lab v18 設計。
>
> **位置付け**: G1 critic 機構 (`BONSAI_CRITIC_ENABLED=1`、`CriticConfig::DifferentSystemPrompt`) は実装後 production default OFF で観測動作完全互換。本 plan は ON/OFF paired cycle で **mean Δscore ≥ +0.015 AND p < 0.1** (Lab v17 と同基準、項目 215 教訓踏襲) を AND 判定し、ACCEPT で defaults 昇格 / REJECT で dead-code 化判定。
>
> **F11 falsifiable hypothesis**: 「別 system prompt + 別 temperature による critic 役分離 = 構造変異の第 4 軸 (multi-role variation) で Lab 天井 7 連続 (v8/v9/v10/v14/v15/v16/v17) を打破する」。Δscore < +0.015 または p ≥ 0.1 なら H_CRITIC 棄却 = critic infrastructure dead-code 候補化 (項目 222 sqlite-vec wiring 削除と同経路)。
>
> **production code 変更ゼロ前提** — 本 plan は Lab 設計のみ。G1 critic 実装は別 plan (`critic-separate-llm-impl.md`) の delivery 後に本 plan を起動する execution plan。

## Task Type
- [ ] Frontend
- [x] Backend (Lab paired t-test 設計のみ。bash script + python 集計 script delivery、production code 変更ゼロ。G1 critic 実装は前提)
- [ ] Fullstack

**由来**: `critic-separate-llm-impl.md` Phase 5 (G1 critic effectiveness)
**関連項目**: 項目 1 (Reflexion) / 項目 210-212 (Self-Verify dynamic skip + Lab v16 REJECT) / 項目 213-216 (ERL Phase 5 + Lab v17 + defaults OFF) / 項目 215 (Lab v17 REJECT、天井 7 連続) / 項目 222 (sqlite-vec wiring 削除、REJECT 後 dead-code pattern)

## 1. 背景

### 1.1 G1 critic 機構の到達点 (`critic-separate-llm-impl.md` delivery 後の前提)

`critic-separate-llm-impl.md` で **production-ready** 実装を完遂後、以下が available:

| 構成要素 | 配置 | 役割 |
|---|---|---|
| `CriticConfig` (9 field) | `runtime/model_router.rs` | env から構築、`enabled=false` default OFF |
| `CriticMode` enum (3 variant) | 同上 | `SamePromptDifferentTemperature` / `DifferentSystemPrompt` (Phase 1 中核) / `SeparateBackend` (Phase 2 派生) |
| `inject_critic_review` | `agent_loop/advisor_inject.rs` | Reflexion (`inject_verification_step`) 直後 hook |
| `prompts/critic.txt` | `prompts/` | 25 行、AGREE/DISAGREE/UNCERTAIN 接頭辞 deterministic 出力 |
| `AuditAction::CriticCall` | `observability/audit.rs` | mode / outcome / prompt_len / response_len / duration_ms 記録 |
| `CriticStats` informational | `agent/benchmark.rs` | `agreement_rate` / `disagreement_rate` 副次指標 (項目 200 RDC/VAF と同 pattern) |
| `BONSAI_CRITIC_ENABLED` env | (default OFF) | 項目 214 / 217-219 と pattern 統一 |

G1 critic 実装の Phase 4 Smoke G-4a/b/c PASS 前提 (本 plan 起動の必須条件):
- ✅ G-4a (env unset): 既存挙動完全互換、`critic_calls=0`、score / duration baseline ± variance
- ✅ G-4b (`different_prompt + log_only`): critic call ≥ 1、AuditAction::CriticCall emit、production 影響ゼロ、Uncertain 比率 ≤ 50%
- ✅ G-4c (`different_prompt + inject`): critic inject 経路 wiring、score Δ ≥ -0.05 (lenient)、duration +30% 以下

### 1.2 Lab v17 REJECT 教訓 (項目 215、本 plan の設計直接踏襲)

| 教訓 | 本 plan 反映 |
|---|---|
| **5 paired sample で n=4 df は statistical power 不足** → Δ mean +/- 0.05 を識別困難 | sample size n=5 維持 (Lab v17 と整合)、ただし R3 で n=10 拡張 plan を mitigation に明記 |
| **ACCEPT 基準は Δ≥+0.015 AND p<0.1 の AND** が Lab v15-v17 全 REJECT で一貫 | 同 AND 基準を §4.3 で明示 |
| **副次 finding (stability 軸 std 縮小) は ACCEPT 基準外でも報告価値あり** (Lab v17 ON pair 1-4 std≈0.010 vs OFF std≈0.034) | §4.4 secondary metric (RDC/VAF stability) を informational だが mandatory 報告化 |
| **smoke pre-screen は config-level 変異で unreliable** (Lab v16 経験、項目 184 ×0.42 補正係数は prompt 変異向け) | §5 で smoke pre-screen を本 Lab では bypass、direct paired のみで判定 |
| **REJECT 時の dead-code 化パスを事前明記** (項目 222 sqlite-vec pattern) | §6 ACCEPT/REJECT フローで明示、dead-code 削除別 plan 起票指示 |

### 1.3 Lab 天井 7 連続経緯 (CLAUDE.md 項目 215 確定)

| Lab | 軸 | 結果 |
|---|---|---|
| v8/v9/v10 | prompt-level (system prompt 変異) | 全 REJECT |
| v14 | benchmark tier 変更 (core 22) | REJECT |
| v15 | Option A 移行後長時間安定性 | 全 pre-screen REJECT |
| v16 | config-level (advisor threshold variant) | 全 REJECT (天井 6 連続) |
| **v17** | context-level (ERL heuristics inject) | REJECT (Δ=−0.0014, p=0.5072、**天井 7 連続**) |
| **v18 (本 plan)** | **role-level (multi-role variation = critic 分離)** | 検証 |

prompt + config + context の 3 軸構造変異が全失敗 → **第 4 軸 = multi-role variation** が本 plan の falsifiable hypothesis (G1 critic = 別 system prompt + 別 temperature で同 backend 内仮想ロール分離)。

### 1.4 1bit Bonsai-8B 文脈での Critic 仮説

- Reflexion 単独は **同思考** で self-critique → 1bit モデルの hallucination は同じ盲点を再生産
- critic.txt + temperature 0.7 で executor (0.3) と差別化 → **異視点 critique** が Reflexion miss の捕捉率を上げる仮説
- Phase 1 の `DifferentSystemPrompt` (同 backend、別 prompt、別 temperature) は token cost +25% で実装最小、Phase 5 ACCEPT なら Phase 2 (真の別 model = `SeparateBackend` = gpt-4-class) への投資根拠になる

### 1.5 既存 Lab paired plan との関係

| Lab plan | n | criteria | 結果 (天井 連続) |
|---|---|---|---|
| `lab-v17-erl-effectiveness.md` | 5 paired (12 cycle 含 warm-up 2) | Δ≥+0.015 AND p<0.1 | REJECT (項目 215、Δ=−0.0014, p=0.5072) |
| **本 plan (v18)** | 5 paired (12 cycle 含 warm-up 2) | 同上 (Lab v17 同形) | **検証** |

本 plan は **Lab v17 と完全同形** (n、criteria、warm-up 戦略、bash script + python 集計) で進める。差分は env (`BONSAI_CRITIC_ENABLED=1` vs `BONSAI_ERL_DISABLED=1`) と secondary metric (CriticStats agreement_rate、項目 200 RDC/VAF stability 併用) のみ。

## 2. 目的

1. **G1 critic effectiveness の data-driven 判定**
   - paired t-test (n=5, df=4, one-sided p<0.1) で critic ON vs OFF の `composite_score` Δ を検定
   - mean Δ ≥ +0.015 AND p < 0.1 の AND 判定 (Lab v15-v17 と一貫した ACCEPT 基準)
2. **Lab 天井 8 候補の評価**
   - 天井 7 連続 (v8-v17) を打破する第 4 軸 (multi-role variation) として critic 機構が effective かを検証
   - ACCEPT なら派生デフォルト化変異リストに項目候補 224 (G1 critic) を追加 (項目 10/47/50/136 と並ぶ)
3. **dead-code 化判定基準明確化**
   - REJECT 時の判定 = `BONSAI_CRITIC_ENABLED=1` defaults 化 (= production OFF 移行) → 後続 plan で `CriticConfig` / `inject_critic_review` / `prompts/critic.txt` / `AuditAction::CriticCall` の段階削除 (項目 222 sqlite-vec wiring 削除と同経路)
   - dead-code net 行 = G1 plan delivery 行数 (~80 inject_critic_review + ~25 critic.txt + ~150 CriticConfig+enums + ~30 audit + ~40 CriticStats + ~10 core.rs hook ≈ ~335 行 net delete 候補)

### 非目標

- **新規実装ゼロ**: 本 plan は Lab 設計のみ。G1 critic 実装は前提 (`critic-separate-llm-impl.md` 完遂)
- **Lab variant pool 拡張**: 本 plan は `BONSAI_CRITIC_ENABLED` env toggle のみ、`HypothesisGenerator::param_mutations` 等への追加 variant 不要 (Lab v17 と同 env opt-in 経路)
- **`SeparateBackend` Phase 2 評価**: 本 plan は Phase 1 (`DifferentSystemPrompt`) のみ評価、`SeparateBackend` (gpt-4-class) は別 Lab plan
- **`BeforeToolCall` hook 評価**: 本 plan は `AfterStepOutcome` (Phase 1 default) のみ、`BeforeToolCall` (Phase 2 候補) は別 plan
- **factorial 4 cell (Reflexion ON/OFF × Critic ON/OFF) 設計**: G1 plan R7 で挙げられているが、本 plan は単純 paired (Reflexion 既定 ON 維持、Critic ON/OFF) で n=5、4 cell × n=5 = 20 cycle は別 plan (Lab v19 候補)

## 3. 既存項目との関係

| 項目 | 関係 | 本 plan での扱い |
|---|---|---|
| **1 (Reflexion)** | 同一 LLM self-critique | 共存 (Reflexion 既定 ON 維持で paired)、本 plan は critic 単独効果を測定 |
| **89 (verification_prompt 統一)** | AdvisorConfig prompt フィールド先例 | 参照のみ。critic は別 struct (`CriticConfig::critic_system_prompt`) |
| **163 (HttpAdvisorJudge)** | Lab 評価専用別 LLM call | judge.rs read-only、本 plan は critic_call 経路と独立 |
| **210 (Self-Verify dynamic skip)** | AdvisorConfig 拡張先例 | 共存設計参考、Phase 5 critic 動的 skip は将来別 plan |
| **211 (Self-Verify Phase 5 Lab variant)** | focus filter による threshold 変異 | 構造類似先行例だが本 plan は env opt-in 経路 (Lab v17 同形) |
| **212 (Lab v16 Self-Verify REJECT)** | 同一 LLM 内 skip 機構の効果限界 evidence | 本 plan の動機補強 (同一 LLM 完結の限界打破) |
| **213 (ERL Heuristics)** | `prompts/heuristic_reflection.txt` 同居 | `prompts/critic.txt` 同 dir 配置、include_str! pattern 流用 |
| **214 (Lab v17 toggle 機構)** | `BONSAI_ERL_DISABLED` env opt-in | 本 plan は同 pattern (`BONSAI_CRITIC_ENABLED`)、ただし enable 方向 (default OFF → ON) |
| **215 (Lab v17 REJECT、天井 7 連続)** | 構造変異枯渇 evidence、ACCEPT 基準 | 本 plan の核心動機。同 ACCEPT 基準 (Δ≥+0.015 AND p<0.1) を §4.3 で踏襲 |
| **216 (ERL defaults OFF 切替)** | Lab v17 REJECT 後の env name 反転 | REJECT 時の処理 pattern。本 plan REJECT 時は default OFF 維持で legacy 既定継続 (反転不要) |
| **217-219 (Cerememory 三本柱)** | env opt-in default OFF pattern | 同 pattern 統一、本 plan の `BONSAI_CRITIC_ENABLED` も default OFF |
| **220 (sqlite-vec Activation)** | infrastructure 配線 + REJECT 経路 | 参照のみ |
| **222 (sqlite-vec wiring 削除)** | REJECT 後 dead-code 化 pattern (~290 行 net delete) | 本 plan REJECT 時の dead-code 削除別 plan 設計の手本 (~335 行 net delete 候補) |

## 4. 設計

### 4.1 paired cycle 設計 (15 cycle 構成、Lab v17 と同形 + buffer +3)

| Phase | cycle 数 | env | 目的 |
|---|---|---|---|
| **Warmup** | 2 cycle | (1) `BONSAI_CRITIC_ENABLED=1` (2) unset | (1) critic OFF→ON で wiring 動作確証 + duration / cost 校正 (2) llama-server warm cache (n_ctx burst 排除、項目 188 教訓) |
| **Buffer** | 1 cycle | unset | warmup 2 終了後の system stabilization (LRU cache、Bonsai-8B kv-cache settling) |
| **Paired Test** | 10 cycle (5 paired × 2) | interleave | Test phase 本体、§4.2 配置 |
| **Buffer** | 2 cycle | unset | Lab v17 経験で実機長時間運用後の variance 安定化 |

合計 = 2 (warmup) + 1 (buf) + 10 (paired) + 2 (buf) = **15 cycle**。Lab v17 (12 cycle) より +3 buffer = 1bit variance 吸収のため拡張。実機 wall time = §4.6 で試算。

### 4.2 ON/OFF 配置 (interleave、決定論性確保)

Test Phase 5 paired = 10 cycle、**奇数 cycle = OFF、偶数 cycle = ON** で interleave:

| cycle # | env | 役割 |
|---|---|---|
| warm 1 | `BONSAI_CRITIC_ENABLED=1` | wiring 確証 (G-4b 同形 smoke) |
| warm 2 | unset | warm cache stabilize |
| buf 1 | unset | buffer |
| **test 1** | unset (OFF) | paired pair 1 OFF |
| **test 2** | `BONSAI_CRITIC_ENABLED=1` (ON) | paired pair 1 ON |
| **test 3** | unset (OFF) | paired pair 2 OFF |
| **test 4** | `BONSAI_CRITIC_ENABLED=1` (ON) | paired pair 2 ON |
| ... | ... | ... |
| **test 9** | unset (OFF) | paired pair 5 OFF |
| **test 10** | `BONSAI_CRITIC_ENABLED=1` (ON) | paired pair 5 ON |
| buf 2 | unset | buffer |
| buf 3 | unset | buffer |

**配置の理由**:
- 各 paired pair で OFF → ON 順を維持 (delta = ON − OFF を直接計算)
- interleave で system state drift を平均化 (連続 OFF → 連続 ON は不可)
- buffer + warm の env unset (= OFF) majority で legacy 既定の安定性確保

### 4.3 ACCEPT criteria (Δscore ≥ +0.015 AND p < 0.1、Lab v15-v17 と一貫)

paired t-test (one-sided、 H1: Δ > 0):

| 条件 | 内容 | 出典 |
|---|---|---|
| (a) | mean Δscore ≥ +0.015 | 項目 215 ERL Lab v17 と同基準 (Δ=−0.0014 で REJECT した先例) |
| (b) | one-sided p < 0.1 (df=4 で t > 1.533) | 項目 215 同基準 (Lab v17 で p=0.5072) |
| **判定** | (a) AND (b) → ACCEPT、否 → REJECT | Lab v15/v16/v17 全 REJECT 一貫基準 |

paired t-test 自前実装 (Lab v17 同形、scipy 不使用、`scripts/lab_v18_paired_ttest.py` で運用):

```python
# 5 paired delta = critic_on_score - critic_off_score
deltas = [pair[1] - pair[0] for pair in pairs]  # ON − OFF
n = len(deltas)  # 5
mean = sum(deltas) / n
var = sum((d - mean) ** 2 for d in deltas) / (n - 1)
std_err = (var / n) ** 0.5
t_stat = mean / std_err if std_err > 0 else 0.0

# df=4 one-sided p<0.1 ⇔ t>1.533
import scipy.stats as st
p_one_sided = 1 - st.t.cdf(t_stat, df=n-1)

accept_a = mean >= 0.015
accept_b = p_one_sided < 0.1
verdict = "ACCEPT" if (accept_a and accept_b) else "REJECT"
```

| 結果 | 判定 | 帰結 |
|---|---|---|
| (a) AND (b) | **ACCEPT** | H_CRITIC 採用、production default ON 検討 (`BONSAI_CRITIC_ENABLED=1` defaults)、派生デフォルト化変異リストに項目候補 224 追加 (項目 10/47/50/136 並列、第 5 default) |
| 否 | **REJECT** | H_CRITIC 棄却、項目 215 ERL pattern 踏襲: `BONSAI_CRITIC_ENABLED` default OFF 維持 (legacy 既定で反転不要)、dead-code 削除別 plan 起票 (項目 222 sqlite-vec wiring 削除と同経路、~335 行 net delete 候補) |

### 4.4 secondary metric (項目 200 RDC/VAF stability、CriticStats、副次指標 = ACCEPT 判定外で informational)

§4.3 ACCEPT 判定とは独立に **必ず報告**:

| metric | 出典 | 値域 | 期待 (ACCEPT 時) | Lab v17 副次 finding 相当 |
|---|---|---|---|---|
| **stability_delta (RDC/VAF)** | 項目 200 (`MultiRunTaskScore`、SQLite V9 / TSV 15 列) | [-1, 1] | ON で std 縮小 → +0.0X | Lab v17 で ON pair 1-4 std≈0.010 vs OFF std≈0.034 で stability 軸顕著優位 (informational ACCEPT 候補) |
| **agreement_rate (CriticStats)** | G1 plan §4.6 | [0, 1] | 0.5-0.9 想定 (1bit critic、過剰 AGREE 警戒、過剰 DISAGREE は R5 hallucination 警戒) | 新規 |
| **disagreement_rate (CriticStats)** | 同上 | [0, 1] | 0.1-0.3 (effective) / >0.5 (hallucinate / R5) / <0.05 (no-op / R12) | 新規 |
| **uncertain_rate (CriticStats)** | parse_critic_response 副産物 | [0, 1] | < 0.5 (R5 gate、G1 G-4b 基準) | 新規 |
| **critic_call duration_ms** | AuditAction::CriticCall payload | ms | mean +20-30% / cycle (R2 expected) | 新規 |
| **token cost (informational)** | llama-server response token | tokens | mean +25% / cycle (G1 plan §4.2) | 新規 |

**副次 ACCEPT 候補** (G-5 Final Quality Gate で記録、§4.3 主 ACCEPT 不達でも報告必須):
- ON で stability_delta ≥ +0.05 (std 縮小顕著) → 項目 200 RDC/VAF re-eval 候補化
- agreement_rate ∈ [0.6, 0.85] かつ disagreement_rate ∈ [0.1, 0.3] → critic 健全動作確証 (主 ACCEPT 不達でも `SeparateBackend` Phase 2 投資根拠)

### 4.5 critic config (Phase 1 = `DifferentSystemPrompt`、temperature 0.0 vs 0.7)

| env var | Lab v18 設定 | 備考 |
|---|---|---|
| `BONSAI_CRITIC_ENABLED` | ON cycle: `1`、OFF cycle: unset | 本 plan の paired toggle 主軸 |
| `BONSAI_CRITIC_MODE` | ON cycle: `different_prompt` | Phase 1 中核 (G1 plan §4.2)、`same_temp` は too-weak、`separate_backend` は Phase 2 |
| `BONSAI_CRITIC_TEMPERATURE` | ON cycle: `0.7` | executor (0.3、設定不変) との差別化 |
| `BONSAI_CRITIC_MAX_USES` | ON cycle: `3` | advisor max_uses=3 と独立、合計 LLM call/step 上限 6 (R2 mitigation) |
| `BONSAI_CRITIC_HOOK` | ON cycle: `after_step` (default) | Reflexion 直後 hook、Phase 1 中核 (G1 plan §4.4) |
| `BONSAI_CRITIC_DISAGREEMENT` | ON cycle: `inject` (default) | DisagreementAction = InjectAsSystemMessage、production-like 設定 |

**executor temperature 設定不変**: bonsai 既定 0.3 (config.toml 既定値)、本 plan で変更なし。critic temperature 0.7 のみ env override。

**G1 plan §4.2 引用**: 「同 backend、別 system prompt (critic.txt)、別 temperature (Phase 1 中核)、token cost +25%」 → Phase 5 効果検証で +25% コスト分の score Δ を回収できるかが論点。

### 4.6 試験 wall time 試算

| Phase | cycle 数 | per-cycle | 計 |
|---|---|---|---|
| Warmup | 2 | 60-90 min (smoke 同形) | 2-3 h |
| Buffer (3 件、warm + buf 2) | 3 | 60-75 min (OFF baseline) | 3-3.75 h |
| Paired Test ON (5 件) | 5 | 75-110 min (critic +25-30%) | 6.25-9.2 h |
| Paired Test OFF (5 件) | 5 | 60-75 min (baseline) | 5-6.25 h |
| **計 (15 cycle)** | 15 | 平均 67-83 min | **~14.5-19 h** |

Lab v17 (12 cycle / 15h 37min) との対比:
- +3 cycle = +3-4 h
- ON cycle で critic +25% duration overhead = +1.5-2 h
- 実測想定 = **~16-20 h wall** (一晩 + 半日想定、Lab v17 より +1.5-2.5h)

**user 起動条件**:
- llama-server `-c 16384` 安定運用 (項目 188 F1)
- 16-20h 占有可能なタイムスロット (一晩 nohup 推奨、`scripts/lab_v18_paired.sh` で daemon 化)

### 4.7 統計検出力 (statistical power) 限界の透明化

n=5 paired (df=4) の検出力試算 (Lab v17 同形):
- 1bit Bonsai-8B core 22 score の cycle-to-cycle σ ≈ 0.025-0.04 (Lab v15/v17 経験)
- Δ=+0.015 を p<0.1 で検出するには paired σ < 0.020 が必要
- **R3 (1bit variance >> Δ で statistical power 不足) が最大リスク**、§7 で n=10 拡張代替案明記

## 5. 実行手順 (3 step、Lab v17 と同形)

### Step 1 — Warmup (2 cycle、~2-3 h、wiring 確証)

```bash
mkdir -p /Users/keizo/bonsai-agent/lab-v18-logs
cd /Users/keizo/bonsai-agent

# Warmup 1: critic ON で wiring 動作確証 (G-4b 同形、smoke 兼用)
BONSAI_CRITIC_ENABLED=1 BONSAI_CRITIC_MODE=different_prompt \
  BONSAI_CRITIC_TEMPERATURE=0.7 BONSAI_CRITIC_DISAGREEMENT=inject \
  BONSAI_BENCH_TIER=core ./target/release/bonsai --lab --lab-experiments 0 \
  2>&1 | tee lab-v18-logs/warmup_1_on.log

# 確認:
grep -c "critic_call\|CriticCall" lab-v18-logs/warmup_1_on.log  # 期待 ≥ 1
grep "Uncertain\|UNCERTAIN" lab-v18-logs/warmup_1_on.log | wc -l  # 比率 ≤ 50% 確認
sqlite3 ~/Library/Application\ Support/bonsai-agent/db.sqlite \
  "SELECT COUNT(*) FROM audit_log WHERE action_type='critic_call'"  # ≥ 1

# Warmup 2: critic OFF で baseline 校正 + warm cache stabilize
BONSAI_BENCH_TIER=core ./target/release/bonsai --lab --lab-experiments 0 \
  2>&1 | tee lab-v18-logs/warmup_2_off.log
```

**Warmup 完了基準**:
- ✅ Warmup 1: critic_call ≥ 1 / Uncertain 比率 ≤ 50% / score Δ (vs Warmup 2) ≥ -0.05 (lenient gate、G-4c 同形)
- ✅ Warmup 2: score = Lab v15/v16/v17 baseline (0.76-0.78) ± variance、cycle duration < 90 min
- ✗ wiring 失敗 (critic_call=0 or Uncertain >50%): R5 fire、Test phase 中止 + critic.txt prompt 改善別 plan

### Step 2 — Buffer + Paired Test (11 cycle、~12-15 h、本 plan 中核)

```bash
# Buffer 1
BONSAI_BENCH_TIER=core ./target/release/bonsai --lab --lab-experiments 0 \
  2>&1 | tee lab-v18-logs/buf_1.log

# Paired Test 5 cycle × OFF/ON interleave
for i in 1 2 3 4 5; do
  echo "=== test pair $i OFF (cycle $((i*2-1))) ==="
  BONSAI_BENCH_TIER=core ./target/release/bonsai --lab --lab-experiments 0 \
    2>&1 | tee lab-v18-logs/test_${i}_off.log

  echo "=== test pair $i ON (cycle $((i*2))) ==="
  BONSAI_CRITIC_ENABLED=1 BONSAI_CRITIC_MODE=different_prompt \
    BONSAI_CRITIC_TEMPERATURE=0.7 BONSAI_CRITIC_DISAGREEMENT=inject \
    BONSAI_BENCH_TIER=core ./target/release/bonsai --lab --lab-experiments 0 \
    2>&1 | tee lab-v18-logs/test_${i}_on.log
done

# Buffer 2, 3
BONSAI_BENCH_TIER=core ./target/release/bonsai --lab --lab-experiments 0 \
  2>&1 | tee lab-v18-logs/buf_2.log
BONSAI_BENCH_TIER=core ./target/release/bonsai --lab --lab-experiments 0 \
  2>&1 | tee lab-v18-logs/buf_3.log
```

**daemon 化 (一晩 nohup 運用)**:
```bash
$EDITOR scripts/lab_v18_paired.sh   # 上記 Step 1+2 を一括 bash script 化
chmod +x scripts/lab_v18_paired.sh
nohup ./scripts/lab_v18_paired.sh ./lab-v18-logs &
echo $! > /tmp/lab_v18.pid
# ~16-20h 後、`ps -p $(cat /tmp/lab_v18.pid)` で完走確認
```

### Step 3 — 集計 + paired t-test (~30 min、Lab v17 同形)

```bash
$EDITOR scripts/lab_v18_paired_ttest.py  # ~80 行、scipy.stats + CriticStats 集計

python scripts/lab_v18_paired_ttest.py ./lab-v18-logs
# 出力例 (synthetic、production data ではない):
# === Lab v18 — G1 Critic Effectiveness (n=5 paired, df=4) ===
# Pair 1 OFF=0.7567 ON=0.7423 Δ=−0.0144
# Pair 2 OFF=0.7340 ON=0.7588 Δ=+0.0248
# Pair 3 OFF=0.7456 ON=0.7621 Δ=+0.0165
# Pair 4 OFF=0.7234 ON=0.7398 Δ=+0.0164
# Pair 5 OFF=0.7689 ON=0.7456 Δ=−0.0233
#
# === Statistics ===
# OFF mean=0.7457 std=0.0184
# ON  mean=0.7497 std=0.0107  (stability_delta=+0.0077, std 縮小、informational)
# Δ   mean=+0.0040 std=0.0210
# t-stat=0.4259 df=4 one-sided p=0.3461
#
# === ACCEPT/REJECT ===
# (a) Δ ≥ +0.015: NG (0.0040 < 0.015)
# (b) p < 0.1:    NG (0.3461 ≥ 0.1)
# Verdict: REJECT
#
# === Secondary (informational) ===
# agreement_rate=0.65 disagreement_rate=0.21 uncertain_rate=0.14
# critic_call duration_ms mean=2843 (executor +28% overhead)
# stability_delta=+0.0077 (副次 ACCEPT 候補閾値 +0.05 未達)

# Verdict に応じて §6 ACCEPT/REJECT フローへ
```

集計 script の主な責務:
1. 5 paired log から `composite_score` 抽出 (TSV 経由 = `experiment_log.rs` の TSV 12-15 列、項目 200 で 12→15 拡張)
2. paired delta 計算 + scipy.stats.ttest_rel (one-sided)
3. CriticStats 集計 (audit_log SQLite SELECT で `action_type='critic_call'`、payload JSON parse: `mode`, `outcome`, `prompt_len`, `response_len`, `duration_ms`)
4. Verdict + secondary metric 統一フォーマット出力

## 6. ACCEPT/REJECT 判定フロー (Lab v17 と同形)

### 6.1 ACCEPT 時 (Δ≥+0.015 AND p<0.1)

| step | 内容 | 担当 plan |
|---|---|---|
| 1 | CLAUDE.md 項目 224 (or 次番) に ACCEPT 結果 + Δ + p 値 + 5 paired score table | 本 plan handoff |
| 2 | handoff session file (`session_2026_05_XX_handoff.md`) で **天井 8 候補打破 evidence** として明記 | 同上 |
| 3 | 派生デフォルト化変異リストに項目 224 追加 (項目 10/47/50/136 と並ぶ第 5 default) | 同上 |
| 4 | `BONSAI_CRITIC_ENABLED` defaults 昇格 plan 起票 (= `CriticConfig::default().enabled = true`) | 別 plan (`critic-defaults-on-impl.md`、~0.3 day) |
| 5 | Phase 2 派生 plan 起動推奨 (`critic-separate-backend-phase2-impl.md`、SeparateBackend = gpt-4-class、~1.5 day) で更なる Δ 取得検証 | 別 plan |
| 6 | factorial 4 cell (Reflexion ON/OFF × Critic ON/OFF) Lab v19 起票検討 (R7 mitigation、~24h wall) | 別 plan |

### 6.2 REJECT 時 (Δ<+0.015 OR p≥0.1、項目 222 sqlite-vec wiring 削除と同経路)

| step | 内容 | 担当 plan |
|---|---|---|
| 1 | CLAUDE.md 項目 224 (or 次番) に REJECT 結果 + 数値 + **天井 8 連続確定** evidence | 本 plan handoff |
| 2 | handoff で副次 finding 報告 (項目 215 同 pattern): stability_delta、agreement_rate、disagreement_rate、uncertain_rate、duration overhead | 同上 |
| 3 | **副次 ACCEPT 候補確認**: stability_delta ≥ +0.05 ならば項目 200 RDC/VAF re-eval 候補、agreement_rate ∈ [0.6, 0.85] かつ disagreement_rate ∈ [0.1, 0.3] ならば critic 健全動作確証 (Phase 2 SeparateBackend 投資根拠は残置) | 同上 |
| 4 | **dead-code 化判定**: production default OFF 維持 (legacy 既定で反転不要、項目 216 ERL defaults OFF とは異なり default 反転すら不要) | (production code 変更ゼロ) |
| 5 | dead-code 削除別 plan 起票 (`critic-wiring-removal-impl.md`、~0.5 day) で以下を段階削除:<br>- `CriticConfig` / `CriticMode` / `CriticHook` / `CriticDisagreementAction` / `CriticOutcome` (~150 行、`runtime/model_router.rs`)<br>- `inject_critic_review` + `parse_critic_response` (~80 行、`agent_loop/advisor_inject.rs`)<br>- `core.rs` Reflexion 直後 hook (~10 行)<br>- `AuditAction::CriticCall` variant (~30 行、`observability/audit.rs`)<br>- `CriticStats` struct + `MultiRunBenchmarkResult` field (~40 行、`benchmark.rs`)<br>- `prompts/critic.txt` (~25 行)<br>- 計 ~335 行 net delete 候補 (項目 222 sqlite-vec wiring 削除 ~290 行 net delete と同オーダー) | 別 plan (`critic-wiring-removal-impl.md`) |
| 6 | dead-code 削除 plan の方針 = 項目 222 と同 pattern: TDD strict 5 phase (Red = caller 不在の参照 fail / Green = 削除 / Refactor = 残置 production code 整合 / Smoke = 1150 → 1145 期待 / G-5 net 行 ≤ −300) | (項目 222 reference) |
| 7 | 「critic は Bonsai-8B 1bit には translate しない」negative finding を継承 (天井 8 連続経緯に追加) | CLAUDE.md |

### 6.3 部分的 ACCEPT (副次 ACCEPT のみ)

主 ACCEPT 不達 (Δ<+0.015 OR p≥0.1) かつ副次 finding が顕著 (例: stability_delta ≥ +0.05) の場合:
- production default OFF 維持 (主 ACCEPT 不達のため defaults 昇格しない)
- dead-code 化は **保留** (削除しない)、§6.2 step 5 の wiring removal plan は起票しない
- 後続検討: Phase 2 (`SeparateBackend`) で再測定 → 真の別 model でΔ 改善する仮説検証
- 項目 200 RDC/VAF stability re-eval candidate に追加

## 7. Risks / Mitigations

| # | Risk | severity | Mitigation |
|---|---|---|---|
| **R1** | critic disagreement 多発で executor を誤誘導、score 低下 (1bit critic hallucination) | **HIGH** | (a) §4.5 で `BONSAI_CRITIC_DISAGREEMENT=inject` (production-like) を採用、`log_only` (shadow) は smoke 専用<br>(b) Warmup 1 で critic_call ≥ 1 + Uncertain 比率 ≤ 50% gate (G1 G-4b 同基準)<br>(c) Test phase 中で score Δ ≤ −0.10 が paired 連続 3 件で発生したら早期中止 + critic.txt prompt 改善別 plan<br>(d) REJECT 時 dead-code 化経路 §6.2 で確保 |
| **R2** | token cost +25-30% で wall time 想定 (~16-20 h) を超過、24h+ に膨張 | MEDIUM | (a) §4.6 で wall time 試算明示 (16-20 h)<br>(b) `BONSAI_CRITIC_MAX_USES=3` で advisor max_uses=3 と独立、合計 LLM call/step 上限 6<br>(c) `BONSAI_CRITIC_TOKENS=400` (G1 plan §4.5、advisor 700 より小)<br>(d) llama-server `-c 16384` 安定運用前提 (項目 188 F1)、必要なら `-c 12288` に縮小して duration -22% 効果再活用 |
| **R3** | 1bit variance σ ≈ 0.025-0.04 >> Δ=0.015 で statistical power 不足、Δ=+0.020 でも p > 0.1 で REJECT (type II error) | **HIGH** | (a) Lab v17 経験で Δ=−0.0014 / p=0.5072 で REJECT、本 plan も同 risk 内在<br>(b) §4.4 secondary metric (stability_delta) で std 縮小を informational ACCEPT 候補化<br>(c) REJECT 時 副次 finding 報告必須 (§6.2 step 3)<br>(d) 主 REJECT + 副次 stability_delta ≥ +0.05 なら Phase 2 SeparateBackend で n=10 拡張 (24-30h wall) を別 plan で検討<br>(e) n=10 拡張時の paired t-test df=9 で t>1.383 で p<0.1 (n=5 の t>1.533 より緩、検出力向上) |
| **R4** | env mutation race で paired test の決定論性損失 (cycle 跨ぎ env unset 漏れ等) | MEDIUM | (a) bash script で各 cycle 直前に明示的 env (un)set + `printenv \| grep BONSAI_CRITIC` で confirm<br>(b) 各 cycle log 末尾に `env diagnostic` セクション追記 (script 内で `env \| grep BONSAI` を tee)<br>(c) 集計 script で env unset cycle に critic_call 検出されたら ERROR fail-fast<br>(d) Lab v17 で類似 R 確認済 (test mutex 不要 = bash level で env scope 隔離) |
| **R5** | parse_critic_response が AGREE/DISAGREE 接頭辞を 1bit モデルが守らず always Uncertain 化 (G1 plan R5 同) | HIGH | (a) Warmup 1 で Uncertain 比率 ≤ 50% gate (G1 G-4b 基準と同形、Step 1 完了基準)<br>(b) Uncertain 多発時は `Skipped { reason: "parse_failed" }` 扱いで production 影響ゼロ (G1 plan §4.4)<br>(c) gate 不達なら Test phase 中止 + critic.txt prompt 改善別 plan、本 Lab plan は再起動 |
| **R6** | llama-server crash / panic で paired pair 部分損失 (12-20h 中の 1 回) | MEDIUM | (a) 各 cycle 完了で TSV append + log tee 分離 (Lab v17 と同 pattern)<br>(b) crash 時は影響 cycle のみ再走 (script 内で resume capability、cycle ID per log file)<br>(c) 1 cycle 損失時は subsequent 1 cycle 追加で n=5 維持<br>(d) llama-server `--retries 3` + `--keep-alive 600` 推奨 (`config.toml`) |
| **R7** | Reflexion (既定 ON) と Critic (本 plan paired toggle) の効果が分離不能 — REJECT 時 critic 単独効果ゼロか Reflexion との重複かが不明 | MEDIUM | (a) §2 非目標で factorial 4 cell (Reflexion ON/OFF × Critic ON/OFF) を別 plan (Lab v19 候補) で扱うと明記<br>(b) 本 plan は Reflexion 既定 ON 維持で Critic 単独効果のみ評価 (= 「Reflexion 上に Critic 追加で Δ あるか」の検証)<br>(c) ACCEPT 時 Lab v19 起票で完全 factorial 検証推奨 (§6.1 step 6) |

(以下 R8-R12 は省略せず継続)

| # | Risk | severity | Mitigation |
|---|---|---|---|
| **R8** | critic.txt 改変で Lab v18 結果が再現不可能 | LOW | (a) `include_str!` で binary 内に埋込、git 履歴で改変追跡可<br>(b) 集計 script で `git rev-parse HEAD` を Experiment metadata に記録 (TSV 末尾列)<br>(c) Phase 5 完走後に critic.txt 改変は Lab v19 の scope (本 plan は immutable) |
| **R9** | n=5 paired で stability_delta secondary metric の検出力が更に低 (項目 200 RDC は n≥10 推奨) | MEDIUM | (a) §4.4 で secondary は informational のみで主 ACCEPT 判定外と明記<br>(b) stability_delta ≥ +0.05 を副次 ACCEPT 候補閾値に設定 (Lab v17 経験で std 0.034→0.010 = Δ +0.024 観測、+0.05 は保守的閾値)<br>(c) n=10 拡張時 (Lab v18.5 別 plan) で再測定推奨 |
| **R10** | G1 critic 実装の Phase 4 Smoke G-4c (`inject` mode) が未 PASS の状態で本 Lab plan 起動 | **HIGH (blocking)** | (a) §1.1 で G1 G-4a/b/c PASS を必須前提と明記<br>(b) Step 1 Warmup 1 (= G-4b 同形) で wiring 確証を再実行、失敗時 Test phase 中止<br>(c) 本 Lab plan の起動条件 = `critic-separate-llm-impl.md` の G-1〜G-4 全 PASS commit pushed |
| **R11** | `BONSAI_CRITIC_MODE=separate_backend` を誤って Test phase で設定 → Phase 1 で `unimplemented!()` panic | LOW | (a) Step 1-2 の bash script で `BONSAI_CRITIC_MODE=different_prompt` を明示 set<br>(b) G1 plan §4.5 で `from_env` が `separate_backend` 受け取り時に warn log + default に置換 (R11 mitigation)<br>(c) Phase 2 派生 plan delivery 後は同 Lab pattern で `mode=separate_backend` も評価可能 |
| **R12** | critic disagreement_rate < 0.05 (= Critic が常に AGREE = no-op) で Δ=0 stationary、REJECT 確定で dead-code 化判定 (削除メリット明確) | LOW | (a) §4.4 secondary metric で disagreement_rate を必須報告化<br>(b) <0.05 検出時は §6.2 step 5 の dead-code 削除別 plan を即起票推奨 (Phase 2 SeparateBackend でも改善見込み低と判定)<br>(c) >0.5 (R5 hallucinate) との両極端を識別、健全 zone [0.1, 0.3] を §4.4 期待値に明示 |

## 8. Quality Gates

| Gate | 内容 | 検証 | 必須 |
|---|---|---|---|
| **G-1 (前提確認)** | `critic-separate-llm-impl.md` の G-1〜G-4 PASS commit pushed (`CriticConfig` / `inject_critic_review` / `prompts/critic.txt` / `AuditAction::CriticCall` 全 production-ready)、1158-1160 passed | `git log --oneline \| grep critic` + `cargo test --lib critic` | 必須 (blocking、R10) |
| **G-2 (Step 1 Warmup)** | Warmup 1 で critic_call ≥ 1、Uncertain 比率 ≤ 50%、score Δ vs Warmup 2 ≥ −0.05 (lenient gate)、cycle duration < 90 min | log grep + sqlite SELECT | 必須 |
| **G-3 (Step 2 Paired Test)** | 5 paired 全完走 (10 cycle、各 60-110 min)、crash/panic ゼロ、各 cycle TSV emit、env scope 漏れなし (`env \| grep BONSAI_CRITIC` 各 cycle 確認) | log + TSV + audit_log SELECT | 必須 |
| **G-4 (Step 3 集計)** | paired t-test 完走、5 pair score table 出力、ACCEPT/REJECT verdict 確定、secondary metric (stability_delta / agreement_rate / disagreement_rate / uncertain_rate / duration overhead) 全報告 | python script 出力 | 必須 |
| **G-5 (Final、handoff + CLAUDE.md)** | CLAUDE.md 項目 224 (or 次番) 追記、handoff session file (`session_2026_05_XX_handoff.md`) 起票、INDEX.md 「Lab paired t-test」section 更新、Lab 天井連続数更新 (ACCEPT 7→打破 / REJECT 7→8) | git diff | 必須 |
| **G-6 (REJECT 時のみ、別 plan 起票)** | REJECT 確定時 `critic-wiring-removal-impl.md` 起票 (~0.5 day、~335 行 net delete、項目 222 pattern) | plan ファイル新規 | REJECT 時必須 |
| **G-7 (ACCEPT 時のみ、別 plan 起票)** | ACCEPT 時 `critic-defaults-on-impl.md` 起票 (~0.3 day、`CriticConfig::default().enabled = true`) + Lab v19 factorial 4 cell 起票検討 | plan ファイル新規 | ACCEPT 時必須 |

G-1 〜 G-5 全必須、G-6/G-7 は verdict 依存。

## 9. 完了条件

1. ✅ `critic-separate-llm-impl.md` G-1〜G-4 PASS 確認 (G-1 前提)
2. ✅ Step 1 Warmup 2 cycle 完走 + G-2 PASS
3. ✅ Step 2 Buffer + Paired Test 11 cycle 完走 + G-3 PASS
4. ✅ Step 3 集計 + paired t-test verdict 確定 (G-4)
5. ✅ ACCEPT/REJECT に応じた dead-code 削除 plan / defaults 昇格 plan 起票 (G-6 or G-7)
6. ✅ CLAUDE.md 項目 224 (or 次番) 追記 + handoff 起票 + INDEX.md 更新 (G-5)
7. ✅ secondary metric (stability_delta / agreement_rate / disagreement_rate / uncertain_rate / duration overhead) 全報告 (G-4 副条件)
8. ✅ 副次 ACCEPT 候補 (stability_delta ≥ +0.05 / agreement zone [0.6, 0.85]) 報告 (REJECT 時)
9. ✅ production code 変更ゼロ (本 plan は Lab 設計のみ、bash script + python script delivery のみ)
10. ✅ `scripts/lab_v18_paired.sh` + `scripts/lab_v18_paired_ttest.py` 新規 commit (~150 行 + ~80 行)
11. ✅ Lab 天井連続数更新 (ACCEPT で 7 → 打破 / REJECT で 7 → 8)

## 10. 見積もり (実 Lab 含めて 2-3 day)

| Phase | 内容 | 時間 |
|---|---|---|
| **P0 (前提確認)** | `critic-separate-llm-impl.md` G-1〜G-4 PASS 確認、Lab v17 plan / 項目 215 / 項目 222 再読、本 plan 追記事項調整 | 0.3 h |
| **P1 (script delivery)** | `scripts/lab_v18_paired.sh` (~150 行、warmup 2 + buf 1 + paired 10 + buf 2) + `scripts/lab_v18_paired_ttest.py` (~80 行、scipy + audit_log SELECT + secondary metric 集計) 新規実装 | 1.5 h |
| **P2 (Step 1 Warmup 実機)** | warmup 2 cycle、~2-3 h wall (G-2) | 2-3 h |
| **P3 (Step 2 Paired Test 実機)** | buffer + paired 10 + buffer = 11 cycle、~12-15 h wall (G-3) | 12-15 h (主に nohup wait) |
| **P4 (Step 3 集計)** | paired t-test verdict + secondary metric (G-4) | 0.5 h |
| **P5 (commit + handoff + CLAUDE.md)** | scripts commit + handoff + CLAUDE.md 項目 224 + INDEX.md (G-5) | 1.0 h |
| **P6 (REJECT 時 dead-code 削除別 plan 起票 / ACCEPT 時 defaults 昇格 plan 起票)** | G-6 or G-7、別 plan 起票のみ (実装は別 session) | 1.0 h |
| **計** | | **~18-22 h ≈ 2-3 day (うち実機 wall ~14-18 h、攻めた稼働で 1.5 day で着地可)** |

実機 wall 期 (P2-P3、~14-18 h) は user 起動の llama-server 占有が必要 = 一晩 + 半日想定。

派生 plan 候補 (本 plan delivery 後):
- `critic-wiring-removal-impl.md` (REJECT 時、~0.5 day、~335 行 net delete、項目 222 pattern)
- `critic-defaults-on-impl.md` (ACCEPT 時、~0.3 day、`CriticConfig::default().enabled = true` + 関連 test 修正)
- `lab-v19-critic-factorial-impl.md` (ACCEPT 時の factorial 4 cell、~24h wall、別 plan)
- `critic-separate-backend-phase2-impl.md` (ACCEPT 時 or 副次 ACCEPT 時、Phase 2 真の別 model、G1 plan §10 既起票候補)

## 11. Quick Start

```bash
# 0. 前提確認 (G-1 blocking、~5 min)
cd /Users/keizo/bonsai-agent
git log --oneline | head -10 | grep -i critic    # critic-separate-llm-impl.md G-1〜G-4 commits 確証
cargo test --lib critic --release 2>&1 | tail -5  # 8-10 critic test PASS 確認
ls prompts/critic.txt && cat prompts/critic.txt | head -10  # critic.txt 配置確認
grep -c "AuditAction::CriticCall" src/observability/audit.rs  # ≥ 1 確認

# 1. P1 — bash + python script delivery (~1.5 h)
$EDITOR scripts/lab_v18_paired.sh         # ~150 行、§5 Step 1+2 一括化
$EDITOR scripts/lab_v18_paired_ttest.py   # ~80 行、§5 Step 3 集計
chmod +x scripts/lab_v18_paired.sh
git add scripts/lab_v18_paired.sh scripts/lab_v18_paired_ttest.py
git commit -m "feat(lab-v18): G1 critic effectiveness paired t-test scripts (項目候補 224)"

# 2. P2-P3 — Lab 実機 (~14-18 h、user 起動 llama-server 必須)
mkdir -p lab-v18-logs
nohup ./scripts/lab_v18_paired.sh ./lab-v18-logs > lab-v18-logs/master.log 2>&1 &
echo $! > /tmp/lab_v18.pid
date  # 開始時刻記録
# ... ~16-20 h 後 ...
ps -p $(cat /tmp/lab_v18.pid) || echo "完走"
date  # 完走時刻記録

# 3. P4 — 集計 (~30 min)
python scripts/lab_v18_paired_ttest.py ./lab-v18-logs
# verdict 確定 (ACCEPT or REJECT)

# 4. P5 — commit + handoff + CLAUDE.md
$EDITOR CLAUDE.md  # 項目 224 (or 次番) 追記、Lab セクション v18 ブロック追加
$EDITOR .claude/projects/-Users-keizo-bonsai-agent/memory/session_2026_05_XX_handoff.md
$EDITOR .claude/plan/INDEX.md  # 「Lab paired t-test」section 更新
git add CLAUDE.md .claude/projects/-Users-keizo-bonsai-agent/memory/session_2026_05_XX_handoff.md .claude/plan/INDEX.md
git commit -m "docs(lab-v18): G1 critic effectiveness 結果 + 項目 224 + handoff"

# 5. P6 — verdict 別 plan 起票
if [ "$VERDICT" = "REJECT" ]; then
  $EDITOR .claude/plan/critic-wiring-removal-impl.md  # ~335 行 net delete plan、項目 222 pattern
  git add .claude/plan/critic-wiring-removal-impl.md
  git commit -m "docs(lab-v18): REJECT 後 dead-code 削除別 plan 起票 (項目候補 225)"
else
  $EDITOR .claude/plan/critic-defaults-on-impl.md     # CriticConfig::default().enabled = true
  $EDITOR .claude/plan/lab-v19-critic-factorial-impl.md  # 4 cell factorial、Lab v19
  git add .claude/plan/critic-defaults-on-impl.md .claude/plan/lab-v19-critic-factorial-impl.md
  git commit -m "docs(lab-v18): ACCEPT 後 defaults 昇格 + Lab v19 factorial 別 plan 起票"
fi
```

## 12. 参考

### 由来 plan (本 plan の起票元)
- **`.claude/plan/critic-separate-llm-impl.md`** — G1 critic 実装本体 (640 行、Phase 5 effectiveness を本 plan として明記)、§ 4.6 / §6 / §7 R3 / §10 派生 plan 候補で本 plan を参照

### Lab paired t-test 先例 (本 plan の構造手本)
- **`.claude/plan/lab-v17-erl-effectiveness.md`** — Lab v17 (5 paired / 12 cycle / Δ=−0.0014 p=0.5072 REJECT、項目 215)、本 plan の構造を完全踏襲 (n=5 / criteria / warmup 戦略 / bash + python script / paired t-test 自前実装)
- `.claude/plan/erl-heuristics-pool-impl-v2.md` — Lab v17 の前段、env opt-in pattern 確立元

### dead-code 削除 pattern 先例 (REJECT 時 §6.2 step 5 手本)
- **`.claude/plan/sqlite-vec-wiring-removal-impl.md`** — 項目 222、~290 行 net delete、TDD strict 5 phase (Red = caller 不在 fail / Green = 削除 / Refactor = 整合 / Smoke = 1158 → 1150 / G-5 net 行)、本 plan REJECT 時の `critic-wiring-removal-impl.md` 起票の直接 template

### bonsai 既存 CLAUDE.md 項目 (本 plan で reference)
- 項目 1: Reflexion (同一 LLM self-critique) — 共存対象、Reflexion 既定 ON 維持で Critic 単独効果のみ評価
- 項目 200: Beyond pass@1 RDC/VAF/GDS — secondary metric stability_delta の出典
- 項目 207: Lab v15 baseline 0.7812 (core 22 / k=3) — Warmup 2 OFF cycle の期待値
- 項目 210-212: Self-Verify Phase 1-5 (項目 211 = Lab variant 機構、項目 212 = Lab v16 REJECT)
- **項目 213**: ERL Heuristics Pool — `prompts/heuristic_reflection.txt` 同居先例、本 plan の `prompts/critic.txt` 先輩
- **項目 214**: Lab v17 toggle 機構 — env opt-in pattern (`BONSAI_ERL_DISABLED`)、本 plan は同 pattern で enable 方向 (`BONSAI_CRITIC_ENABLED`)
- **項目 215**: Lab v17 REJECT (天井 7 連続) — 本 plan の核心動機、ACCEPT 基準 (Δ≥+0.015 AND p<0.1) の出典
- 項目 216: ERL defaults OFF 切替 — REJECT 後 env name 反転 pattern (本 plan REJECT 時は反転不要 = legacy 既定で default OFF 維持)
- 項目 217-219: Cerememory 三本柱 — env opt-in default OFF pattern 統一
- **項目 222**: sqlite-vec wiring 削除 — REJECT 後 dead-code 化 pattern (~290 行 net delete)、本 plan REJECT 時の `critic-wiring-removal-impl.md` 起票の直接手本

### 論文・survey
- **arxiv 2603.05344** — Building AI Coding Agents for the Terminal (G1 critic 由来論文、本 plan の主軸)
- arxiv 2602.03485 — Self-Verification Dilemma (項目 210 由来、Reflexion 過剰発動の課題、本 plan 動機補強)
- arxiv 2603.21357 — AgentHER ECHO + HSL (項目 201、hindsight relabel との相性検証候補 Lab v19 4 cell)

### 失敗時 handling (項目 215 ERL pattern + 項目 222 sqlite-vec wiring 削除 pattern 統合)
本 plan §6.2 / §10 P6 で完全明記済み。要点:
1. CLAUDE.md negative finding 記録 (天井 8 連続経緯追加)
2. production default OFF 維持 (反転不要)
3. dead-code 削除別 plan (`critic-wiring-removal-impl.md`、~335 行 net delete) 起票、項目 222 pattern で TDD strict 5 phase
4. 副次 finding (stability_delta / agreement_rate) で Phase 2 SeparateBackend 投資根拠を残置検討
