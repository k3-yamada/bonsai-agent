# Plan: AgentFloor Pre-Screen Tier Persistence Fix — pre-screen REJECT 経路 tier carry-over

> **由来**: handoff session 05-10d §7 task #1 G-4c v3 完全再走 (~3h 21min wall) で発覚した PARTIAL PASS。commit `572a9a4` (run_k tier populate) は意図通り動作するが、pre-screen REJECT 経路の Experiment literal が `tier_t1..t6: None` ハードコードのまま残置され、SQLite/TSV tier 永続化が失敗する。
>
> **由来 evidence (G-4c v3 実機)**:
> - `[INFO][lab.agentfloor]` 8 件 emit、baseline tier map fully captured (T1=0.68 / T2=0.52 / T3=0.77 / T4=0.64 / T5=0.70 / T6=0.47、weakest=T6)
> - SQLite id=223 (新規 row) tier_t1..t6 **全 NULL**
> - TSV 21 列 format ✓ だが tier 6 fields **全 `-` placeholder**
> - 真因 = pre-screen `delta=-0.1583 (4 tasks, k=1)` で REJECT、`Experiment` inline literal (experiment.rs:1116-1140) が tier_t1..t6 を全 None で構築
>
> **位置付け**: 項目 223 (AgentFloor 6-tier ladder) の最終 wiring 完遂。本 fix で tier-targeted 変異 (handoff §7 #9 / Lab v22+) の前提データが完全に揃う。

## Task Type
- [ ] Frontend
- [x] Backend (`src/agent/experiment.rs:1116-1140` pre-screen REJECT inline literal を baseline tier carry-over に変更)
- [ ] Fullstack
- [x] Docs (CLAUDE.md 項目 224 = pre-screen tier fix + T6 weakest 発見)

## 1. 背景

### 1.1 G-4c v3 で発覚した二重バグ構造
| Wiring layer | commit | 状態 | evidence |
|---|---|---|---|
| `BenchmarkSuite::run_k` で `MultiRunBenchmarkResult.tier_avg_scores` populate | `572a9a4` | ✓ 修正済 | log emit 8 件 ✓ |
| `Experiment::from_multi_results` で `result.tier_avg_scores` → `Experiment.tier_t1..t6` 転写 | `ec4bd73` | ✓ 動作中 (full-cycle 経路) | experiment_log.rs:264-269 で `tiers.and_then(\|t\| t[N])` |
| **pre-screen REJECT 経路の `Experiment` inline literal** | (未修正) | ❌ **本 plan 対象** | experiment.rs:1116-1140 で `tier_t1..t6: None` ハードコード |

### 1.2 真因コード位置 (experiment.rs:1110-1148)
```rust
if estimated_delta < loop_config.prescreening_threshold {
    eprintln!("[lab] pre-screen REJECT: ...");
    let snapshot = config_snapshot(&modified_config);
    let exp = Experiment {
        // 既存 fields...
        tier_t1: None,  // ← 本 plan 対象
        tier_t2: None,  // ← 本 plan 対象
        tier_t3: None,  // ← 本 plan 対象
        tier_t4: None,  // ← 本 plan 対象
        tier_t5: None,  // ← 本 plan 対象
        tier_t6: None,  // ← 本 plan 対象
    };
    ExperimentLog::save_to_db(store.conn(), &exp)?;
    if let Some(tsv) = &loop_config.tsv_path {
        ExperimentLog::append_tsv(tsv, &exp)?;
    }
    experiments.push(exp);
    experiment_count += 1;
    continue;
}
```

### 1.3 baseline 変数の型 (carry-over 元)
- `experiment.rs:999` `let mut baseline = suite.run_k(...)` → 型 `MultiRunBenchmarkResult`
- `MultiRunBenchmarkResult.tier_avg_scores: Option<[Option<f64>; 6]>` (benchmark.rs:476)
- baseline は `--lab-experiments 1` でも k=3 全 task 実行済 = tier_avg_scores **必ず populate されている** (LADDER mode 時)

### 1.4 副次発見: T6-LongHorizon が真の weakest_tier
G-4c v3 baseline (LADDER mode、core 22 + AgentFloor 30 task suite、k=3):
| Tier | bonsai (full) | bonsai (handoff smoke 7-task) | paper baseline | Δ vs paper |
|---|---|---|---|---|
| T1-Instruct | **0.68** | 0.00 | 0.85 | -0.17 |
| T2-SingleTool | 0.52 | 0.93 | 0.75 | -0.23 |
| T3-ToolSelect | 0.77 | 0.71 | 0.65 | +0.12 |
| T4-MultiStep | 0.64 | 0.96 | 0.50 | +0.14 |
| T5-ErrorRecov | 0.70 | 0.47 | 0.45 | +0.25 |
| **T6-LongHorizon** | **0.47** | (None) | 0.30 | +0.17 |

handoff §3 の T1=0.00 は smoke 7-task サンプルバイアスと判明、**真の weakest_tier = T6-LongHorizon** = tier-targeted 変異の優先攻略を T1 → T6 に修正必要 (本 plan 副次成果として CLAUDE.md 項目 224 に記録)。

## 2. 目的

1. **pre-screen REJECT 経路の tier 永続化を確立** — Option A baseline carry-over で `Experiment.tier_t1..t6` を埋める
2. **項目 223 wiring の最終完遂** — run_k populate (572a9a4) + from_multi_results transfer (ec4bd73) + pre-screen carry-over (本 plan) の 3 段配線を完成
3. **T6-LongHorizon weakest 確証を CLAUDE.md 項目 224 として記録** — handoff §3 smoke artifact 訂正、tier-targeted 変異の優先攻略修正

### 非目標
- pre-screen の精度向上 (4 task × k=1 → 6 task × k=1 等の sample size 拡張) — 別 plan
- pre-screen 経路で **新規 tier 計算実行** (LLM call 追加でコスト ↑) — 設計判断 = baseline carry-over が論理的に正しい
- `from_multi_results` の signature 変更 — full-cycle 経路は既動作、本 plan で触らない
- ACCEPT 判定基準の変更 — `accepted: false` のまま (pre-screen REJECT)
- production default 変更 — 既存 LADDER mode env opt-in pattern 維持

## 3. 設計

### 3.1 Option 比較表
| Option | 実装 | trade-off | 採否 |
|---|---|---|---|
| **A. baseline carry-over (推奨)** | pre-screen REJECT inline literal で `baseline.tier_avg_scores` を `and_then` 経由で展開 | ★ baseline は完全な tier 値持つ + experiment は "no improvement" なら baseline と同等が論理的に正しい + 既存 from_multi_results pattern と統一 | **採用** |
| B. partial compute | pre-screen の 4 task × k=1 から計算可能な tier だけ populate | NULL 混在で解釈困難、tier カバレッジ不完全 (4 task では 6 tier 全カバー保証なし) | 却下 |
| C. NULL 維持 + ドキュメント化 | pre-screen REJECT 時 NULL 仕様化 | tier-targeted 変異で REJECT 行を分析不能、Lab v22+ 設計上の制約として残る | 却下 |
| D. from_multi_results を流用 | pre-screen 用に MultiRunBenchmarkResult mock を作って同 constructor 経由 | mock 構築コスト + Experiment.experiment_score = baseline + estimated_delta の特殊計算が壊れる | 却下 |

### 3.2 Option A 実装案 — Inline expansion
```rust
// experiment.rs:1116-1140 を以下に変更
let snapshot = config_snapshot(&modified_config);
// baseline tier carry-over (項目 224): pre-screen REJECT は full run 未実行のため
// baseline tier 値を experiment row に転写 (= "no improvement" なら baseline と同等が論理的に正しい)
let baseline_tiers = baseline.tier_avg_scores;
let exp = Experiment {
    experiment_id: experiment_id.clone(),
    mutation_type: mutation.mutation_type,
    mutation_detail: mutation.detail,
    baseline_score: baseline.composite_score(),
    experiment_score: baseline.composite_score() + estimated_delta,
    delta: estimated_delta,
    accepted: false,
    duration_secs: 0.0,
    config_snapshot: snapshot,
    pass_at_k: None,
    pass_consecutive_k: None,
    score_variance: None,
    prescreened: true,
    reliability_decay: None,
    variance_amplification: None,
    graceful_degradation: None,
    stability_delta: None,
    tier_t1: baseline_tiers.and_then(|t| t[0]),
    tier_t2: baseline_tiers.and_then(|t| t[1]),
    tier_t3: baseline_tiers.and_then(|t| t[2]),
    tier_t4: baseline_tiers.and_then(|t| t[3]),
    tier_t5: baseline_tiers.and_then(|t| t[4]),
    tier_t6: baseline_tiers.and_then(|t| t[5]),
};
```

設計選択 = **inline expansion** (Option A 内で更に絞った下位選択):
- **採用**: inline で `and_then(|t| t[N])` を 6 行展開 (from_multi_results と同 pattern、可読性 ◎、私有 helper 抽出は YAGNI)
- 却下: `Experiment::from_prescreen_reject(...)` private helper 抽出 → 1 caller のため過剰抽象化

### 3.3 既存 test fixture (実装影響なし)
experiment.rs:2161-2253 の test fixtures (`t_extract_worst_reasoning_filters_rejects` / `t_extract_worst_reasoning_truncates`) も `tier_t1..t6: None` だが、これは **test 用の意図的 NULL** (extract_worst_reasoning が tier 値を見ない検証)、production bug ではない → **本 plan で touch しない**。

## 4. 既存項目との関係
| 項目 | 関係 | 改修要否 |
|---|---|---|
| **223 (AgentFloor 6-tier ladder 統合)** | 本 plan で 3 段配線最終完遂 (run_k populate + from_multi_results transfer + pre-screen carry-over) | 本 plan で完遂 |
| **172 (Core/Extended Tier)** | 直交軸、本 plan は CapabilityTier (T1-T6) のみ参照 | 不変 |
| **209 (CapabilityTier enum)** | 本 plan の前提依存 | 不変 (read-only) |
| **200 (Beyond pass@1 RDC/VAF/GDS)** | 同 inline literal の信頼性メトリクス 4 列も None ハードコード = pre-screen REJECT の解釈上正しい (full-cycle 不実行) | 不変 (本 plan 対象外) |

## 5. TDD strict 5 phase

### Phase 1 — Red
新規 test 3 件 (`src/agent/experiment.rs` mod tests):
1. `t_prescreen_reject_carries_baseline_tier_when_populated` — baseline.tier_avg_scores=Some([Some(0.68), Some(0.52), Some(0.77), Some(0.64), Some(0.70), Some(0.47)]) で pre-screen REJECT、Experiment.tier_t1..t6 が baseline と同値
2. `t_prescreen_reject_tier_none_when_baseline_none` — baseline.tier_avg_scores=None で pre-screen REJECT、Experiment.tier_t1..t6 が全 None (LADDER mode 未使用時の後方互換)
3. `t_prescreen_reject_partial_tier_carries_correctly` — baseline.tier_avg_scores=Some([Some(0.68), None, Some(0.77), None, Some(0.70), None]) で pre-screen REJECT、Experiment.tier_t{1,3,5} に値、t{2,4,6} が None (部分 NULL の伝搬)

期待: 全 3 test fail (現行コードは hardcoded None) で Red 確認。

### Phase 2 — Green
1. experiment.rs:1116-1140 の Experiment inline literal を §3.2 の Option A 実装に置換
2. 既存 `prescreened: true` を維持 (本 plan で変更なし)
3. cargo test --lib で 3 新規 test PASS、既存 1162 維持 = **1165 passed**

### Phase 3 — Refactor
- `let baseline_tiers = baseline.tier_avg_scores;` の局所束縛で 6 行の repetition を読みやすく
- 1 行 comment で `// baseline tier carry-over (項目 224): pre-screen REJECT は full run 未実行のため baseline 値転写` を残す (Why コメント、What ではない)
- clippy 0 / fmt 0 / 退行ゼロ

### Phase 4 — Smoke 検証 (3 段)
```bash
# G-4a: 既存経路 (env 未設定、後方互換)
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0
# 期待: 1165 pass 維持、tier 機構 OFF で従来挙動互換

# G-4b: LADDER + smoke で baseline tier emit 動作確証
BONSAI_BENCH_LADDER=1 BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0
# 期待: smoke 完走、[INFO][lab.agentfloor] 8 行 emit、SQLite tier 永続化なし (baseline は SQLite Experiment row として保存されない)

# G-4c v4: LADDER + experiment 1 cycle で pre-screen REJECT path 完全 verify
BONSAI_BENCH_LADDER=1 ./target/release/bonsai --lab --lab-experiments 1
# 期待: 完走 ~3h、SQLite id=224 (新規 row) tier_t1..t6 が baseline 値で populate
# (G-4c v3 で id=223 NULL だったが、本 fix 後の新規 row は non-NULL)
```

判定基準:
- ✅ G-4a: 既存経路 1165 passed 維持
- ✅ G-4b: log emit 8 行確認 (前 G-4b v2 で動作確証済)
- ✅ G-4c v4: SQLite verification `SELECT id, tier_t1..t6 FROM experiments WHERE id > 223` で全 tier non-NULL、TSV tail で 21 列 + tier fields に数値 (= `-` placeholder ではない)

### Phase 5 — Commit + handoff + CLAUDE.md 項目 224
4 commits:
1. `test(agentfloor): pre-screen REJECT tier carry-over Phase 1 Red — 3 test 追加`
2. `fix(agentfloor): pre-screen REJECT 経路で baseline tier 値 carry-over (項目 224)`
3. `refactor(agentfloor): tier carry-over の局所束縛 + Why コメント`
4. `docs(claude.md): 項目 224 — pre-screen tier persistence fix + T6 weakest 確証`

## 6. API 影響
| API | 変更 | 後方互換 |
|---|---|---|
| `Experiment` struct | 既存 fields のみ、変更なし | ✓ |
| `Experiment::from_multi_results` | 不変 (full-cycle 経路、変更なし) | ✓ |
| `ExperimentLog::save_to_db` / `append_tsv` | 不変 (受ける Experiment.tier_t1..t6 が non-NULL になるだけ) | ✓ |
| pre-screen REJECT path inline literal | tier_t1..t6 = baseline carry-over | ✓ baseline.tier_avg_scores=None 時は従来通り全 None で後方互換 |
| env / config / SQLite schema | 変更なし | ✓ |

**signature 変更ゼロ** — 全 inline 修正、Cerememory 三本柱 (項目 217-219) と同 additive pattern。

## 7. Risks / Mitigations
| # | Risk | 影響 | Mitigation |
|---|---|---|---|
| R1 | baseline.tier_avg_scores=None (LADDER mode 未使用時) で carry-over が全 None | tier 機構 OFF で意図通り後方互換、影響なし | `and_then` で None 伝搬、test 2 で確証 |
| R2 | "no improvement" 仮定の論理的妥当性 = pre-screen REJECT は実際に improvement がないとは限らない (4 task × k=1 推定) | 論理的に baseline と同等記録は近似値 | (i) `prescreened: true` flag で区別可能、tier-targeted 解析時に `WHERE prescreened=0 OR tier_t1 IS NOT NULL` 等で filter 可 (ii) pre-screen の sample size 拡張は別 plan |
| R3 | 既存 test fixture (experiment.rs:2161-2253) の `tier_t1..t6: None` 維持で混乱 | test 意図 = extract_worst_reasoning が tier を見ない検証、production bug ではない | (i) Phase 3 で touch しない (ii) 必要なら docstring 追加で意図明示 |
| R4 | test 3 件追加で既存 1162 → 1165 になるが、count 期待値が他 plan (G1 Critic 等) と衝突 | 並列実装時の test count 期待値ズレ | (i) 本 plan を G1 等の前に消化 (ii) handoff で明示 |
| R5 | pre-screen 内部実装変更 (`apply_smoke_correction_to_delta` 等) で baseline 参照が壊れる | 本 plan 影響範囲外、baseline は inline literal 直前で参照済 | (i) 既存 baseline.composite_score() 2 箇所と同 pattern (ii) Phase 4 G-4c v4 で実機検証 |
| R6 | G-4c v4 (~3h smoke) が tier 値持つ pre-screen REJECT を生まない (= mutation が pre-screen PASS してしまう) | 検証不能 | (i) 既存 mutation pool で pre-screen REJECT が頻出するため低リスク (G-4c v3 でも pre-screen REJECT 発生) (ii) 万一 PASS なら full-cycle で from_multi_results 経由 = 既動作確証あり、別経路で tier 永続化確認可 |

## 8. Quality Gates
- **G-1 Phase 1 Red**: 3 新規 test fail (compile OK)
- **G-2 Phase 2 Green**: 3 新規 test PASS + 既存 1162 維持 = **1165 passed** + clippy 0 + fmt 0
- **G-3 Phase 3 Refactor**: 局所束縛 + Why コメント + 既存 test 退行ゼロ
- **G-4 Phase 4 Smoke 3 段**:
  - G-4a: 既存経路 1165 pass 維持
  - G-4b: LADDER smoke で `[INFO][lab.agentfloor]` 8 行 emit
  - G-4c v4: LADDER + experiment 1 cycle、SQLite id>223 で tier_t1..t6 全 non-NULL + TSV 21 列 + tier fields に数値
- **G-5 Final**: 4 commits + CLAUDE.md 項目 224 + handoff 起票

## 9. 完了条件
1. ✅ experiment.rs:1116-1140 の Experiment inline literal で baseline tier carry-over 実装
2. ✅ 3 新規 test PASS、1165 passed 維持
3. ✅ smoke G-4a/b/c v4 全 PASS
4. ✅ SQLite verification: `WHERE id > 223 AND prescreened=1` で tier_t1..t6 が baseline 値と一致
5. ✅ TSV verification: tail 行で cols=21 + tier 6 fields に数値 (= `-` placeholder ではない)
6. ✅ CLAUDE.md 項目 224 (pre-screen tier fix + T6 weakest 発見)
7. ✅ 4 commits push

## 10. 見積もり
| Phase | 内容 | 時間 |
|-------|------|------|
| Phase 1 | Red — 3 test 追加 | 0.5h |
| Phase 2 | Green — inline literal 6 行置換 | 0.3h |
| Phase 3 | Refactor — 局所束縛 + コメント | 0.2h |
| Phase 4 | Smoke G-4a/b: 0.5h、G-4c v4: 3h (background) | 3.5h (実機 wall) |
| Phase 5 | Commit + CLAUDE.md 項目 224 + handoff | 0.5h |
| Buffer | G-4c v4 異常時の log 解析 | 1.0h |
| **合計** | | **~6h** (うち Phase 4 G-4c v4 3h は background 並列、active work ~3h) |

## 11. Quick Start
```bash
# 0. 既存 caller 全網羅
rtk grep -n "from_multi_results" src/agent/  # full-cycle 経路の確認

# 1. Phase 1 Red
$EDITOR src/agent/experiment.rs  # mod tests に 3 test 追加
rtk cargo test --lib t_prescreen_reject  # 3 test fail (compile OK)

# 2. Phase 2 Green
$EDITOR src/agent/experiment.rs  # 1116-1140 の inline literal 置換
rtk cargo test --lib  # 1165 passed

# 3. Phase 3 Refactor
$EDITOR src/agent/experiment.rs  # 局所束縛 + Why コメント
rtk cargo clippy --lib -- -D warnings
rtk cargo fmt --check

# 4. Phase 4 Smoke (G-4c v4 は llama-server 必須)
rtk cargo build --release
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0  # G-4a
BONSAI_BENCH_LADDER=1 BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0  # G-4b
BONSAI_BENCH_LADDER=1 ./target/release/bonsai --lab --lab-experiments 1 > /tmp/agentfloor_g4c_v4.log 2>&1 &  # G-4c v4 (3h background)

# 5. Verification (G-4c v4 完走後)
sqlite3 "$HOME/Library/Application Support/bonsai-agent/bonsai.db" \
  "SELECT id, experiment_id, ROUND(experiment_score,3), tier_t1, tier_t2, tier_t3, tier_t4, tier_t5, tier_t6 FROM experiments WHERE id > 223 ORDER BY id"
# 期待: id=224 で tier_t1..t6 全 non-NULL (baseline 値と一致)

tail -3 "$HOME/Library/Application Support/bonsai-agent/experiments.tsv" | awk -F'\t' '{print "cols:", NF, "id:", $1, "tier:", $16, $17, $18, $19, $20, $21}'
# 期待: cols=21、tier 6 fields に数値

# 6. Commit + CLAUDE.md 項目 224 + handoff
$EDITOR /Users/keizo/bonsai-agent/CLAUDE.md  # 項目 224 追加
```

## 12. 参考
- 由来 handoff: `~/.claude/projects/-Users-keizo-bonsai-agent/memory/session_2026_05_10d_handoff.md` §7 task #1
- 直前 plan: `.claude/plan/agentfloor-tier-eval-impl.md` (項目 223 本体、5 commits 完遂、本 plan は最終 wiring fix)
- 既存項目: 209 (CapabilityTier enum)、223 (AgentFloor 統合)
- 修正対象 source: `src/agent/experiment.rs:1116-1140` (pre-screen REJECT inline literal)
- 動作確証 source: `src/agent/experiment_log.rs:218-271` (`from_multi_results`、full-cycle 経路の参照実装)
- baseline 型定義: `src/agent/benchmark.rs:461-476` (`MultiRunBenchmarkResult` + `tier_avg_scores: Option<[Option<f64>; 6]>`)
- env opt-in pattern 手本: 項目 217-219 (Cerememory 三本柱)、項目 214/216 (ERL toggle)
- Lab 効果検証は不要 (本 plan は wiring fix のみ、production 観測動作変更は tier 値が NULL → non-NULL になるだけで Lab 評価関数に影響なし)
