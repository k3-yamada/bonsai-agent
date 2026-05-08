# Plan: Lab v17 — ERL Heuristics Pool Effectiveness 検証 (項目 213 Phase 5、F10 falsifiable hypothesis)

> **由来**: 項目 213 (ERL Heuristics Pool Phase 2 Green、commit `41b6ac3`、1099 passed) の **Phase 5 effectiveness 検証**。`erl-heuristics-pool-impl-v2.md` plan §10 G-6 で別 plan 化と明記済。
>
> **目的**: ERL の `<context type="heuristics">` 注入が Lab v8/v9/v10/v14/v15/v16 **天井 6 連続** (項目 207/212) を構造変異 (context-level) で打破するかを **paired t-test で data-driven に判定**。F10 falsifiable hypothesis = 「ERL は tool_chain 表現不能の自然言語助言で advisor-threshold 不達カテゴリを補完」。Δscore < +0.015 または p ≥ 0.1 ならば H_ERL 棄却 = heuristics 機構 dead-code 候補化。
>
> **前提**: 項目 213 で `HeuristicStore` / `inject_heuristics` / `run_heuristics_pass` / SCHEMA_V10 が実装済 (production default ON、空 pool で no-op = 後方互換 100%)。本 plan は **Lab harness 拡張のみ** (toggle 機構 + paired t-test 集計、production code は toggle hook 1 件のみ)。

## Task Type
- [ ] Frontend
- [x] Backend (toggle 機構 + Lab harness の paired ペア実行 + scipy 不使用 paired t-test 自前実装)
- [ ] Fullstack

## 1. 背景
### 1.1 項目 213 の到達点 (Phase 1〜4 partial)
- `HeuristicStore` (6 method) + `inject_heuristics` + `run_heuristics_pass` 完全配線
- `LoopState::injected_heuristic_ids` carry + `record_heuristic_outcomes` で task 完了時 utility update
- `extract_heuristics_from_events` は AgentHER 直後 (F4 順序) で session ごと 1 LLM call、temp=0.3 max=400 で cap
- Codex audit 2HIGH/3MEDIUM 反映済、production default ON、empty pool で no-op
- 1099 passed / clippy 0 / fmt 0、Phase 4 Smoke は **release build green のみ**、実機 lab cycle は本 plan で初実施

### 1.2 残課題 (項目 213 末尾「次=★★★」)
- effectiveness 未検証 → 構造変異 evidence 取得が次セッション
- F10 falsifiable hypothesis (H_ERL) を実機データで accept/reject 判定する根拠が無い
- production default ON のままで dead-code 化判定不可 (Lab v17 結果次第で項目 211 と同様 negative finding 取得 → defaults 残置適切判定)

### 1.3 Lab v15/v16 baseline 参照
| Lab | baseline (core 22, k=3) | 備考 |
|---|---|---|
| v15 | 0.7812 | Option A 移行後 (handoff 05-08b、項目 207) |
| v16 | 0.7761 | Self-Verify Phase 5 effectiveness (項目 212、advisor threshold 全 REJECT) |

本 plan は Lab v17 として **項目 213 後の baseline + ON/OFF paired** を計測。期待する数値 (情報のみ、判定基準は §4.4):
- baseline (OFF): 0.77 ± 0.02 (Lab v15/v16 と同等)
- variant (ON、warm-up 後): baseline + Δ ≥ +0.015 で ACCEPT

## 2. 目的
1. **Toggle 機構**: env var `BONSAI_ERL_DISABLED=1` で `inject_heuristics` + `run_heuristics_pass` を一括 short-circuit (production default ON、env unset で既存挙動 100% 維持)
2. **Warm-up 戦略**: HeuristicStore 空 → effective pool には 2 cycle 程度の ON 実行が必要、本 plan は warm-up 2 cycle + paired 5 cycle = 計 12 cycle
3. **Paired t-test 自前実装**: scipy 依存なし、`sample_size=5` で degrees of freedom=4、片側 p < 0.1 (大標本でなくても conservative)
4. **ACCEPT 判定**: mean Δscore ≥ +0.015 **かつ** paired t-test p < 0.1 (両条件 AND)
5. **副次計測**: 各 cycle の `extracted` / `saved` / `pruned` / `parse_failures` / `injected_count` を TSV に追記、Phase 4 Smoke 兼用
6. **F10 反証条件**: Δscore < +0.015 または p ≥ 0.1 → H_ERL **棄却** = 項目 213 の inject_heuristics + run_heuristics_pass を `BONSAI_ERL_DISABLED=1` を defaults 化する判定 (= production default OFF 移行 = 機能 dead-code 候補)

## 3. 既存項目との関係
| 項目 | 関係 | 改修要否 |
|---|---|---|
| **213** ERL Phase 2 Green | 本 plan の前提実装、production code 変更ゼロで本 plan は toggle 機構のみ追加 | toggle 1 hook 追加 |
| **211/212** Self-Verify Phase 5 | 同 Phase 5 effectiveness 検証パターンを踏襲、Lab variant pool は使わず flag toggle で paired 化 | 設計踏襲 |
| **209** EventRepository trait | `extract_heuristics_from_events` が trait 経由 = Lab toggle 影響なし | 参照のみ |
| **207** Lab v15 long run | 89 min 完走 + 0/3 ACCEPT 天井 5 連続 evidence、本 plan で v15 baseline 0.7812 と比較 | 参照のみ |
| **200** Beyond pass@1 RDC/VAF | informational metric で stability 観点を補強 (paired t-test とは独立) | 参照のみ |
| **205** Option A 移行 | `BenchmarkSuite::run_k(persistent_store: &MemoryStore)` 必須化済、heuristics persist 跨ぎ可能 | 設計踏襲 |
| **172** Tier::Core/Extended | core 22 タスクで実機 (BONSAI_BENCH_TIER=core) | 参照のみ |

## 4. 設計
### 4.1 Toggle 機構 (`src/memory/heuristics.rs` + `src/agent/context_inject.rs`)
**最小変更** (toggle 1 hook、env var 1 個、~10 行追加):

```rust
// src/memory/heuristics.rs (新規 helper)
/// `BONSAI_ERL_DISABLED=1` で ERL 機構全体を short-circuit。
/// 既存挙動 100% 維持 (env unset は false 返却 = 通常動作)。
pub(crate) fn is_erl_disabled() -> bool {
    std::env::var("BONSAI_ERL_DISABLED")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}
```

**呼出側 2 箇所**:
- `src/agent/context_inject.rs::inject_heuristics`: 関数冒頭で `if is_erl_disabled() { return vec![]; }` (heuristics 注入 skip、record_outcome 用 IDs も空)
- `src/agent/experiment.rs::run_heuristics_pass`: 関数冒頭で `if is_erl_disabled() { return Ok(HeuristicSummary::default()); }` (post-Lab pass 全体 skip)

**production default**: env unset → false → 既存挙動 (項目 213 ON のまま)。
**Lab variant**: `BONSAI_ERL_DISABLED=1 ./target/release/bonsai --lab ...` で OFF 化。

### 4.2 Warm-up 戦略
HeuristicStore 空時は `inject_heuristics` が即 `vec![]` 返却 = ON でも OFF と同等挙動。effective evaluation には pool に内容が必要。

- **Warm-up Phase**: 2 cycle ON を先行実行 (pool 蓄積、reflection LLM call 経由で最低 4-8 件の heuristic を期待)
- **Test Phase**: 5 paired cycle (`ON1, OFF1, ON2, OFF2, ON3, OFF3, ON4, OFF4, ON5, OFF5` の 10 連続実行)
  - cycle 内 task list は同一 (core 22 deterministic order)
  - HeuristicStore は cycle 跨ぎで persist (項目 205 Option A `&MemoryStore` 必須化により)

合計 cycle 数 = warm-up 2 + paired 10 = **12 cycle**、各 ~60-90 min で **12-18h 完走見込**。

### 4.3 Paired t-test 自前実装 (`src/agent/experiment.rs` test mod) または bash 解析
scipy 不使用、5 サンプル paired:

```rust
// src/agent/experiment.rs (test mod or pub(crate) helper)
/// paired t-test for paired samples (df = n-1)。
/// 本 plan は ON/OFF 1 対 1 paired のため n=5、df=4。
/// 戻り値 (mean_delta, t_stat, p_value_one_sided) は p<0.1 / >0.1 の判定用。
pub(crate) fn paired_t_test(deltas: &[f64]) -> (f64, f64, f64) {
    let n = deltas.len() as f64;
    let mean = deltas.iter().sum::<f64>() / n;
    let var = deltas.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / (n - 1.0);
    let std_err = (var / n).sqrt();
    let t_stat = if std_err > 0.0 { mean / std_err } else { 0.0 };
    // df=4 で one-sided p<0.1 ⇔ t > 1.533 (t-table)
    // 簡易: t > 1.533 → p < 0.1、t > 1.0 → 0.1<p<0.2、その他 0.2<=p<0.5
    let p = if t_stat > 1.533 { 0.05 }   // 慣習的に <0.1 として 0.05 表示
            else if t_stat > 1.0 { 0.15 }
            else if t_stat > 0.0 { 0.30 }
            else { 0.5 };
    (mean, t_stat, p)
}
```

**実装簡素化方針**: paired t-test を Rust 側で完備するより、**TSV を Python で集計** が運用容易 (scipy.stats.ttest_rel)。本 plan は Rust 自前実装を **mandatory ではなく optional** とし、TSV 出力 + python script (`scripts/lab_v17_paired_ttest.py`) を delivery する。

### 4.4 ACCEPT 判定 (mandatory)
**両条件 AND** (どちらも満たす場合のみ ACCEPT):
- (a) **mean Δscore ≥ +0.015** (Lab 標準 ACCEPT delta、項目 207-212 一貫)
- (b) **paired t-test p < 0.1** (片側、df=4 で t > 1.533)

| 結果 | 判定 | 帰結 |
|---|---|---|
| (a) AND (b) | **ACCEPT** | H_ERL 採用、production default ON 確定、defaults 化変異リストに項目 213 追加 |
| 否 | **REJECT** | H_ERL 棄却、項目 213 機構を `BONSAI_ERL_DISABLED=1` defaults 化 (= production OFF 移行) で dead-code 候補化、後続 plan で削除判定 |

### 4.5 Lab harness 拡張 (新規 script `scripts/lab_v17_paired.sh`)
```bash
#!/usr/bin/env bash
set -euo pipefail
LOG_DIR="${1:-./lab-v17-logs}"
mkdir -p "$LOG_DIR"

# Warm-up Phase: 2 cycle ON (空 pool → 蓄積)
for i in 1 2; do
    echo "=== warm-up cycle $i (ON) ==="
    BONSAI_BENCH_TIER=core ./target/release/bonsai --lab --lab-experiments 0 \
        2>&1 | tee "$LOG_DIR/warmup_${i}.log"
done

# Test Phase: 5 paired (alternating ON/OFF)
for i in 1 2 3 4 5; do
    echo "=== test cycle $i (ON) ==="
    BONSAI_BENCH_TIER=core ./target/release/bonsai --lab --lab-experiments 0 \
        2>&1 | tee "$LOG_DIR/test_on_${i}.log"

    echo "=== test cycle $i (OFF) ==="
    BONSAI_ERL_DISABLED=1 BONSAI_BENCH_TIER=core ./target/release/bonsai --lab --lab-experiments 0 \
        2>&1 | tee "$LOG_DIR/test_off_${i}.log"
done

# 集計 + paired t-test
python scripts/lab_v17_paired_ttest.py "$LOG_DIR"
```

### 4.6 Python 集計 script (新規 `scripts/lab_v17_paired_ttest.py`)
~50 行、scipy.stats.ttest_rel + summary print:
- log から `composite_score` 抽出 (各 cycle 末尾の "[lab] baseline=score=X.XXXX" or 既存 TSV 読込)
- 5 ペアの paired t-test (one-sided)
- ACCEPT 判定 (Δ ≥ 0.015 AND p < 0.1) を表示

### 4.7 副次計測 (Phase 4 Smoke 兼用、informational)
ON cycle ごとに `lab.heuristics` log から以下を抽出:
- `extracted=N` / `saved=N` / `skipped_to_skill=N` / `pruned=N` / `parse_failures=N`
- `inject_heuristics` 呼出回数 (run_agent_loop で cycle あたり 22 task × pre+post task = ~22 回呼出)
- HeuristicStore SQLite SELECT で row count、cycle 終了時に persist 確認

**Phase 4 Smoke 完遂判定** (項目 213 残):
- ✅ release build green (commit 41b6ac3 で確認済)
- ✅ HeuristicStore に少なくとも 1 件 persist (warm-up 1 cycle で確認)
- ✅ run_hindsight_pass log → run_heuristics_pass log の順序確認 (F4 audit)
- ✅ duration 増加 ≤ +12% (ON vs OFF cycle pair で確認)
- ✅ schema V10 migration 成功 (新 DB と既存 V9 DB 両方)
- ✅ parse_failures 件数 log

## 5. TDD strict 5 phase
### Phase 1 — Red (新規 ~5 test)
**toggle 機構**:
- `t_is_erl_disabled_default_unset`: env unset で `false`
- `t_is_erl_disabled_explicit_1`: env="1" で `true`
- `t_is_erl_disabled_case_insensitive`: env="TRUE"/"true" で `true`
- `t_inject_heuristics_short_circuits_when_disabled`: `BONSAI_ERL_DISABLED=1` で空 Vec 返却
- `t_run_heuristics_pass_short_circuits_when_disabled`: 同上、Default summary 返却

期待: compile error / assertion fail で Red 確認。

### Phase 2 — Green
1. `src/memory/heuristics.rs` `pub(crate) fn is_erl_disabled()` 追加 (~7 行)
2. `src/agent/context_inject.rs::inject_heuristics` 冒頭 short-circuit (~2 行)
3. `src/agent/experiment.rs::run_heuristics_pass` 冒頭 short-circuit (~2 行)

期待: **1099 → 1104 passed (+5 / clippy 0 / fmt 0)**

### Phase 3 — Refactor
- toggle helper の docstring に env name + 想定使い方明記
- `is_erl_disabled` を `pub(crate)` のまま (testing module から呼出)

### Phase 4 — Smoke (G-4) — 本 plan は実機 Lab v17 で兼用
別途 release build 確認は項目 213 で完了済。本 plan で実機検証。

### Phase 5 — 実機 Lab v17 paired t-test (G-6)
1. `scripts/lab_v17_paired.sh` 起動 (warm-up 2 + test 10 = 12 cycle、~12-18h)
2. `python scripts/lab_v17_paired_ttest.py` で集計
3. ACCEPT/REJECT 判定 → CLAUDE.md 項目 215 + handoff 起票

## 6. API 影響 (新規 public 一覧)
| modulo path | 関数 / 構造体 |
|---|---|
| `crate::memory::heuristics::is_erl_disabled` | pub(crate) fn (env var 読込) |

**API 名空間**: 1 個追加のみ (シグネチャ変更ゼロ → 後方互換 100%)。

## 7. risks / mitigations
| # | Risk | 影響 | Mitigation |
|---|---|---|---|
| **R1** | warm-up 2 cycle で pool が空のまま (1bit reflection が JSON malformed で全 parse_failures) | Test phase で ON/OFF 差分が 0 = false negative | warm-up 後に SQLite SELECT COUNT で件数 ≥ 1 確認、不足なら追加 warm-up cycle、parse_failures 連続 4 cycle 以上ならテスト中止 + reflection prompt 改善別 plan |
| **R2** | 5 paired sample で statistical power 不足 (df=4) | t-test p 値 conservative すぎて type II error | n=5 は最小限 (LFAI 慣習)、Δ≥0.015 ACCEPT 基準で実用判定可 (Lab v15/v16 と同基準)、必要なら sample 増 (n=10 で 24-30h、別 plan) |
| **R3** | ON/OFF cycle 内で Lab 自体の randomness で大 variance | t-test 判定不安定 | RDC/VAF (項目 200) で stability 別観点併用、Δ平均が +0.015 を中央値中心に分布する場合は ACCEPT |
| **R4** | env var が threading 越境で副作用 | test 隔離性損なう | unit test では `std::env::set_var` を mutex で囲む、または `#[serial]` (serial_test crate) — ただし試験的なのでまず ad-hoc unset で動作確認 |
| **R5** | Lab cycle が 90 min 超過 (項目 188 -c 12288 制約) | 12-18h が 24h+ に膨張 | OFF cycle で reflection LLM call skip = duration 短縮効果あり、warm-up は serial で実行、test は 5 paired のみ |
| **R6** | Phase 5 実機中の crash / panic で再開困難 | 部分データのみ残る → 統計検定不可 | warm-up + test を別 script + log 分離、各 cycle 完了で TSV append、cycle 単位再開可能 |
| **R7** | HeuristicStore pool が cycle 跨ぎで肥大 (R1 重複爆発、項目 213 R1 系) | DB 肥大 / context 圧迫 | `prune` が cycle ごとに実行 (run_heuristics_pass 末尾)、上限 200 で score 昇順削除、warm-up 中にも実行 |
| **R8** | F10 反証 (REJECT) → 項目 213 機構を defaults OFF 化する判断 | dead-code 化 + 後段 plan で削除 | REJECT 時は本 plan で `BONSAI_ERL_DISABLED=1` defaults 化を別 commit、production code は incremental に削除 (1 plan で部分削除) |

## 8. quality gates
| Gate | 内容 | 検証 | 必須 |
|---|---|---|---|
| **G-1 (Phase 1 Red)** | 新規 5 test が compile error or assertion fail で Red 確認 | `cargo test --lib heuristics` | 必須 |
| **G-2 (Phase 2 Green)** | 全 test PASS + **1099 → 1104 passed** + clippy 0 + fmt 0 + production code 変更最小 | `cargo test/clippy/fmt` | 必須 |
| **G-3 (Phase 3 Refactor)** | docstring 整備、production code 動作 binary equivalent | self-review | 必須 |
| **G-4 (Phase 4 Smoke、項目 213 残 6 項目)** | warm-up 1 cycle で persist 件数 ≥ 1 / lab.heuristics log 順序確認 / duration 増加 ≤ +12% / schema V10 migration / parse_failures log | warm-up cycle log + SQLite SELECT | 必須 |
| **G-5 (Final, net 行)** | net +50 行以下 (production +10、test +40) | `git diff --stat` | 任意 |
| **G-6 (Effectiveness、Phase 5 別 session 起動)** | Lab v17 paired t-test で **Δscore ≥ +0.015 かつ p < 0.1** | warmup 2 + test 10 cycle log + python 集計 | 必須 (本 plan の核心) |

G-1〜G-4 PASS で merge 可能 (toggle 機構 commit)。G-6 は **Phase 5 実機 = 12-18h 別 session で実施** (toggle commit 後に user 起動の llama-server で長時間実行)。

## 9. 見積もり
| Phase | 内容 | 所要 |
|---|---|---|
| **P1 (Red)** | 5 test、cargo test Red 確認 | 0.5h |
| **P2 (Green)** | is_erl_disabled helper + 2 短絡 hook | 0.5h |
| **P3 (Refactor)** | docstring + clippy/fmt | 0.25h |
| **P4 (Smoke)** | release build + warm-up 1 cycle (~80 min) で persist + log 確認 | 1.5h (うち実機 ~80 min) |
| **P5 (Effectiveness、別 session)** | warm-up 2 + test 10 cycle (paired) + python 集計 + 判定 | 12-18h (実機) + 0.5h (集計 + 判定) |
| **P6 (commit + handoff)** | toggle commit + Phase 5 結果 commit + CLAUDE.md 項目 214/215 + MEMORY.md | 1h |
| **計** | | **~3-4h plan 実装 + 12-18h Lab 実機** |

P1-P4 + P6 = ~3.75h で本 plan のコード変更と Phase 4 Smoke 完了 (= 項目 213 plan v2 の P4 残を消化)。
P5 は user 起動の llama-server 長時間運用が必要のため別 session、本 plan の delivery は P1-P4 + P6 の commit まで。

## 10. 次の段階
### 着手判断
- ✅ 項目 213 (commit 41b6ac3) production-ready (1099 passed、clippy 0、空 pool で no-op)
- ✅ Lab v15 baseline 0.7812 / v16 baseline 0.7761 取得済 (handoff 05-08b/05-08f)
- ✅ Option A 移行で `&MemoryStore` 必須 = HeuristicStore persist 跨ぎ可能 (項目 205)
- ✅ run_heuristics_pass が AgentHER 直後で scoping 済 (項目 213 F4 audit)

### 先送り条件
- ❌ ベンチマーク tier (core 22) が変更されている → 比較性損失、別 baseline 計測必要
- ❌ llama-server 長時間運用が unstable (項目 188 hang 系) → backend 安定後

## 11. ★ 着手前チェックリスト
1. [ ] `git log -1 --stat 41b6ac3` で項目 213 commit 内容確認
2. [ ] `cargo test --lib heuristics` で 16 ERL test green 確認
3. [ ] `BONSAI_BENCH_TIER=core ./target/release/bonsai --manifest` で core tier 22 task 確認
4. [ ] llama-server 起動可能 (user action、~12-18h 占有)

## 12. Quick Start
```bash
# 1. 着手前 verify
git log -1 --stat 41b6ac3  # 項目 213 確認
cargo test --lib heuristics --release 2>&1 | tail -5  # 16 ERL test PASS

# 2. Phase 1 Red
$EDITOR src/memory/heuristics.rs        # 5 test 追加 (toggle test)
$EDITOR src/agent/context_inject.rs     # short-circuit test (separate test mod)
cargo test --lib --release 2>&1 | grep "is_erl_disabled\|short_circuits"

# 3. Phase 2 Green
$EDITOR src/memory/heuristics.rs        # is_erl_disabled() helper
$EDITOR src/agent/context_inject.rs     # inject_heuristics 冒頭 short-circuit
$EDITOR src/agent/experiment.rs         # run_heuristics_pass 冒頭 short-circuit
cargo test --lib --release && cargo clippy --lib --tests -- -D warnings && cargo fmt --check

# 4. Phase 4 Smoke (warm-up 1 cycle、~80 min)
cargo build --release
BONSAI_BENCH_TIER=core ./target/release/bonsai --lab --lab-experiments 0 \
    2>&1 | tee /tmp/erl_smoke_warmup.log
sqlite3 ~/Library/Application\ Support/bonsai-agent/db.sqlite \
    "SELECT COUNT(*) FROM heuristics" # ≥ 1 確認
grep "lab.agenther\|lab.heuristics" /tmp/erl_smoke_warmup.log  # 順序確認

# 5. Phase 5 Effectiveness (別 session、12-18h)
$EDITOR scripts/lab_v17_paired.sh        # warm-up 2 + test 10
$EDITOR scripts/lab_v17_paired_ttest.py  # paired t-test 集計
chmod +x scripts/lab_v17_paired.sh
nohup ./scripts/lab_v17_paired.sh ./lab-v17-logs &
# ~12-18h 後
python scripts/lab_v17_paired_ttest.py ./lab-v17-logs

# 6. Commit (P1-P4 only、P5 は別 session)
git add -A && git commit -m "feat(erl): Lab v17 toggle 機構 (BONSAI_ERL_DISABLED) + Phase 4 Smoke 完遂 (項目 214)"
```

## 13. 参考
- arxiv 2603.24639 ERL (2026-03)
- 項目 213 commit `41b6ac3` (本 plan の前提実装)
- 項目 207 Lab v15 long run / 項目 212 Lab v16 effectiveness (paired Phase 5 検証パターン)
- 項目 211 Self-Verify Phase 5 Lab variant (本 plan の構造類似先行例)
- `erl-heuristics-pool-impl-v2.md` plan v2 §10 G-6 (Effectiveness Phase 5 別 plan 化と明記)
- CLAUDE.md 項目候補: 214 (toggle 機構実装) / 215 (Phase 5 結果)

## 14. SESSION_ID (for /ccg:execute use)
- CODEX_SESSION: (新規取得が必要、項目 213 plan v2 の `019e064a-334c-7692-9735-c5d95231ebf1` は ERL 実装 context のため Lab v17 検証では再利用可だが文脈差し替え必要)
- GEMINI_SESSION: (前回 failed、本 plan は backend 設計のみで Codex 単独で十分)

## 15. ★ 失敗 (REJECT) 時の handling
F10 反証条件 (Δscore < +0.015 または p ≥ 0.1) を引いた場合:
1. **CLAUDE.md 項目 215** に REJECT 結果 + 数値 (各 cycle 5 pair の score table)
2. **production code 修正** = `BONSAI_ERL_DISABLED=1` を defaults 化 (= `is_erl_disabled` を `is_erl_enabled` 反転 + `inject_heuristics` / `run_heuristics_pass` で early return が default) → 別 commit
3. **後続 plan**: 項目 213 機構の段階削除を判定 (削除メリット = ~870 行 + SCHEMA_V10 dead-table、削除デメリット = 別 LLM での migration ROI、段階削除 plan 起票)
4. **handoff**: 「ERL は Bonsai-8B 1bit には translate しない」negative finding を継承

ACCEPT 時:
1. **CLAUDE.md 項目 215** に ACCEPT 結果 + Δscore + p 値 + 派生デフォルト化変異リストへ追加 (項目 10/47/50/136 と並ぶ第 5 default)
2. **handoff**: 天井 6 連続打破の構造変異 evidence として CLAUDE.md と handoff に明記
3. **後続**: AgentFloor Phase 2 Green (項目 208 plan #2) の baseline として活用、項目 213 の learning loop が AgentFloor 30 task で更に効果上回るか追加検証
