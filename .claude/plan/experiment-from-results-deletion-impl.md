# Plan: Legacy `Experiment::from_results` + `BenchmarkResult` Dead-Code Deletion

> **由来**: `session_2026_05_11b_handoff.md` §11 で本 session の `tier_t1..t6: None` 全網羅検査の副次発見として確証された dead-code chain。`BenchmarkSuite::run` (single-run、`run_k` の前身) + `BenchmarkResult` struct + `BenchmarkResult::composite_score()` impl + `Experiment::from_results` (legacy ctor) + `test_experiment_from_results` (test 専用 caller) の **全 chain が production caller 0**、`run_k` 系統への完全移行 (commit `572a9a4` で run_k tier wiring 完遂、本 plan で legacy 系統 cleanup) を反映した cleanup plan。
>
> **位置付け**: 項目 222 `sqlite-vec wiring 削除` と同 pattern (caller 0 → deletion-only、API additive な後方非互換)。優先度 ★ (機能影響なし、cleanup のみ、Lab 動作不変)。

## Task Type
- [ ] Frontend
- [x] Backend (`src/agent/benchmark.rs` BenchmarkResult struct + impl + BenchmarkSuite::run + 2 test fixtures 削除、`src/agent/experiment_log.rs` Experiment::from_results + test_experiment_from_results + 関連 import 削除)
- [ ] Fullstack
- [x] Docs (CLAUDE.md 項目 225 = legacy ctor deletion + dead-code chain 掃除完遂)

## 1. 背景

### 1.1 dead-code chain (session 05-11b §11 で確証済)
| Site | 種別 | production caller |
|---|---|---|
| `benchmark.rs:1739-1801` `BenchmarkSuite::run(...) -> Result<BenchmarkResult>` | production fn | **0** (`run_k` への移行完了済) |
| `benchmark.rs:230-246` `pub struct BenchmarkResult` + `impl composite_score()` | production struct | 自身の `BenchmarkSuite::run` のみ |
| `benchmark.rs:1955+` test_run 等の test fixtures | test | self-contained |
| `experiment_log.rs:180-217` `Experiment::from_results(...)` (legacy ctor) | production fn | **0** (`from_multi_results` 完全移行) |
| `experiment_log.rs:490` `use BenchmarkResult, TaskScore;` | test import | self-contained |
| `experiment_log.rs:546-585` `test_experiment_from_results` | test | self-contained (削除対象 fn の test) |

**結論**: 全 chain が caller 0、5-tier deletion 安全。

### 1.2 `MultiRunBenchmarkResult` への完全移行履歴
- 項目 4-7 (pass^k 評価導入)、`MultiRunBenchmarkResult` 採用
- 項目 200 (Beyond pass@1)、stability_delta/RDC/VAF/GDS 拡張
- 項目 209 (CapabilityTier、AgentFloor 6-tier)、tier_avg_scores 追加
- 項目 223 (AgentFloor 統合)、tier_t1..t6 SCHEMA_V14 永続化
- **項目 224** (本 plan の前 commit `a52edc6`)、pre-screen REJECT 経路 baseline tier carry-over

`BenchmarkResult` (single-run) は項目 4 以前の legacy 系、本 plan で完全削除。

### 1.3 削除しても残る機能
- `MultiRunBenchmarkResult` (`run_k` 経路) は完全保持
- pass^k 指標 (`pass_at_k` / `pass_consec_k`) 計算ロジック保持
- tier_avg_scores 集計ロジック保持
- `Experiment::from_multi_results` (full-cycle 経路) 保持
- `build_prescreen_reject_experiment` (項目 224、pre-screen 経路) 保持
- AgentHER / ERL hook 系統保持

## 2. 目的
1. **dead-code chain 完全削除** — 5-tier deletion で codebase の認知負荷低減
2. **項目 224 wiring fix の cleanup 完遂** — 本 session で発見した dead-code を同 session 系列で deletion して 3 段配線 + 5-tier dead-code 掃除のセットを完成
3. **`MultiRunBenchmarkResult` への一本化を docs / code で明示** — 将来 wiring fix 時に「BenchmarkResult ctor 経路は存在しない」を保証

### 非目標
- `MultiRunBenchmarkResult` の API 変更 — 完全保持
- pass^k 指標の計算ロジック改修 — 完全保持
- production code の動作変更 — Lab / experiment / benchmark 動作完全互換
- `BenchmarkSuite::run_k` の signature 変更 — 完全保持
- 他 file の touch (削除対象外の signature 変更や引数変更なし)

## 3. 既存項目との関係
| 項目 | 関係 | 改修要否 |
|---|---|---|
| **222 (sqlite-vec wiring 削除)** | 同 pattern (caller 0 → deletion-only) | 設計踏襲 |
| **224 (pre-screen tier fix、本 session)** | 直前 plan、§11 で本 plan の dead-code chain 確証 | 不変 (実装済) |
| **209 (CapabilityTier)** | 不変 | 不変 |
| **216 (ERL defaults OFF)** | 同 cleanup pattern (機能 OFF + dead-code 化候補化) | 参照のみ |

## 4. 設計

### 4.1 削除対象 (5 site)
| # | File | Lines | 内容 | 依存削除 |
|---|---|---|---|---|
| 1 | `benchmark.rs:230-246` | ~17 行 | `pub struct BenchmarkResult` + `impl composite_score()` | なし |
| 2 | `benchmark.rs:1739-1801` | ~63 行 | `BenchmarkSuite::run(&self, ...) -> Result<BenchmarkResult>` | site #1 削除のため必須同時 |
| 3 | `benchmark.rs:1955+` (test_run / test_composite_score 等) | ~50 行 | site #2 を verify する test | site #2 削除のため必須同時 |
| 4 | `experiment_log.rs:180-217` | ~38 行 | `Experiment::from_results(...)` legacy ctor | site #1 削除のため signature 不可 |
| 5 | `experiment_log.rs:490 + 546-585` | ~42 行 (import 1 + test 41) | `use BenchmarkResult, TaskScore;` + `test_experiment_from_results` | site #4 削除のため必須同時 |

**合計削除**: ~210 行 (5 site)、production code 動作変更ゼロ。

### 4.2 削除順序 (依存逆順、TDD strict 5 phase)
1. site #5 (test 削除) → site #4 (legacy ctor 削除) → site #3 (run test 削除) → site #2 (run fn 削除) → site #1 (struct + impl 削除) の順で削除すれば compile error 段階的解消
2. 各 site 削除後 cargo build / test で verify、site 跨ぎの compile error が出たら依存方向を再調査

### 4.3 SCHEMA / TSV / config への影響
- **SQLite**: 変更なし (Experiment 構造変わらず、tier 列もそのまま)
- **TSV**: 変更なし (21 列フォーマット維持)
- **config**: 変更なし
- **env**: 変更なし

## 5. TDD strict 5 phase

### Phase 1 — Red
新規 test なし (deletion plan のため)。Phase 1 を省略、Phase 2 で test + 本体を順次削除 (deletion plan で TDD Red の意義は薄い)。代わりに **削除前後の cargo test count 差分**を Phase 2 完遂判定とする (期待: 1165 → ~1160、test 5 件減)。

### Phase 2 — Green (deletion 主体)
依存逆順で 5 commits (small atomic diffs):
1. `refactor(benchmark): remove test_experiment_from_results test (site #5)` — experiment_log.rs:546-585 削除 + line 490 import 削除
2. `refactor(benchmark): remove Experiment::from_results legacy ctor (site #4)` — experiment_log.rs:180-217 削除
3. `refactor(benchmark): remove BenchmarkSuite::run single-run tests (site #3)` — benchmark.rs:1955+ 削除
4. `refactor(benchmark): remove BenchmarkSuite::run single-run fn (site #2)` — benchmark.rs:1739-1801 削除
5. `refactor(benchmark): remove BenchmarkResult struct + composite_score impl (site #1)` — benchmark.rs:230-246 削除

各 commit 後 `cargo build --lib + cargo test --lib` で verify、test 件数を確認。

### Phase 3 — Refactor
- 削除後の clippy / fmt clean 確認
- import block の不要 use 削除 (e.g., `BenchmarkResult` を import している file があれば修正)
- docstring update 不要 (削除のみ)

### Phase 4 — Smoke 検証 (1 段、軽量)
```bash
# G-4a: Lab 動作完全互換確認
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0
# 期待: ~1160 pass、smoke 完走、`run_k` 系統で動作不変
```

判定基準:
- ✅ G-4a: smoke 完走、log 出力 baseline 系統正常 (既存 `[INFO][lab.agentfloor]` emit 不変)
- ✅ unit test: 1165 → ~1160 (test 5 件減、production test 保持)

### Phase 5 — Commit + handoff + CLAUDE.md 項目 225
追加 commit (Phase 2 で 5 commits 既あり):
6. `docs(claude.md): 項目 225 — legacy BenchmarkResult + Experiment::from_results 削除完遂 (項目 222 同 pattern)`

## 6. API 影響
| API | 変更 | 後方互換 |
|---|---|---|
| `BenchmarkSuite::run` | **削除** | ✗ deprecated public API 削除、production caller 0 のため実質互換 |
| `BenchmarkSuite::run_k` | 不変 | ✓ |
| `BenchmarkResult` struct | **削除** | ✗ deprecated public struct、production caller 0 のため実質互換 |
| `BenchmarkResult::composite_score()` | **削除** | ✗ |
| `MultiRunBenchmarkResult` 系統 | 不変 | ✓ |
| `Experiment::from_results` | **削除** | ✗ deprecated public ctor、production caller 0 |
| `Experiment::from_multi_results` | 不変 | ✓ |
| `Experiment` struct | 不変 | ✓ |
| `build_prescreen_reject_experiment` (項目 224) | 不変 | ✓ |
| SQLite / TSV / config / env | 変更なし | ✓ |

**項目 222 と同 pattern**: 削除対象は `pub` だが production caller 0、外部依存なし、Lab production code 動作完全互換。

## 7. Risks / Mitigations
| # | Risk | 影響 | Mitigation |
|---|---|---|---|
| R1 | `BenchmarkResult` を import する file が他 module に存在する可能性 | compile error | (i) Phase 0 で `rtk grep -rn "BenchmarkResult\b" src/` で全 caller 確認 (本 plan §1.1 で 5 site のみ確証済、他 module なし) (ii) Phase 2 各 commit 後に cargo build で 早期発見 |
| R2 | 削除順序を間違えて compile error 連鎖 | Phase 2 中断 | (i) §4.2 の依存逆順 (test → ctor → run-test → run → struct) を厳守 (ii) 各 commit 後 cargo build で都度 verify |
| R3 | `BenchmarkResult::composite_score` を `MultiRunBenchmarkResult::composite_score` と取り違える | 機能削除リスク | (i) `MultiRunBenchmarkResult` (benchmark.rs:553-559) は本 plan 削除対象外、明示的に site #1 のみ削除 (ii) cargo test で `composite_score` 関連 test PASS 維持確認 |
| R4 | `Experiment::from_results` の test (`test_experiment_from_results`) を削除すると test count 減で他 plan の test count 期待値とズレる | 並列実装時の混乱 | (i) handoff で明示 (ii) 本 plan を G1 Critic 等の前に消化 |
| R5 | dead-code 削除で external user / 他 fork が壊れる可能性 | 互換性 | (i) `bonsai-agent` は private project、external fork なし (ii) 項目 222 sqlite-vec 削除も同 risk profile で実行済、前例あり |
| R6 | rebuild 後の binary size 変化 (微減) | なし | informational only、~210 行削減で binary size 微減見込み (±10KB) |

## 8. Quality Gates
- **G-1 Phase 0**: 全 caller 検査完了 (`rtk grep -rn "BenchmarkResult\b" src/` で 5 site のみ確証)
- **G-2 Phase 2 各 site 削除後**: `cargo build --lib` PASS + `cargo test --lib` PASS で test 件数モニタリング (1165 → 1164 → 1163 → 1161 → 1160 → ~1160 段階的減)
- **G-3 Phase 3 Refactor**: clippy 0 / fmt 0 / 退行ゼロ
- **G-4 Phase 4 Smoke**: smoke G-4a 完走、`[INFO][lab.agentfloor]` emit 不変
- **G-5 Final**: 6 commits + CLAUDE.md 項目 225 + handoff 起票

## 9. 完了条件
1. ✅ `BenchmarkResult` struct + impl 削除 (benchmark.rs)
2. ✅ `BenchmarkSuite::run` (single-run) 削除 (benchmark.rs)
3. ✅ `Experiment::from_results` 削除 (experiment_log.rs)
4. ✅ 関連 test (test_experiment_from_results、test_run 等) 削除
5. ✅ import block cleanup (`use BenchmarkResult, TaskScore;` 削除)
6. ✅ cargo test --lib: ~1160 passed (test 5 件減、production test 保持)
7. ✅ clippy 0 / fmt 0 / 退行ゼロ
8. ✅ smoke G-4a PASS (Lab 動作完全互換)
9. ✅ CLAUDE.md 項目 225
10. ✅ 6 commits push (5 deletion + 1 docs)

## 10. 見積もり
| Phase | 内容 | 時間 |
|-------|------|------|
| Phase 0 | 全 caller 検査 (本 plan §1.1 で完了済、再確認のみ) | 0.2h |
| Phase 2 | 5-site 削除 (依存逆順、各 commit 後 build+test verify) | 1.5h |
| Phase 3 | Refactor (clippy + fmt + import cleanup) | 0.3h |
| Phase 4 | Smoke G-4a (smoke 1 cycle、~25 min wall) | 0.5h (実機 wall 含) |
| Phase 5 | Commit + CLAUDE.md 項目 225 + handoff | 0.5h |
| Buffer | site 跨ぎの compile error 解消 | 0.5h |
| **合計** | | **~3.5h ≈ 0.5 day** (項目 222 と同 scale) |

## 11. Quick Start
```bash
# 0. 全 caller 検査 (本 plan §1.1 で完了済、再確認のみ)
rtk grep -rn "BenchmarkResult\b" src/ | grep -v MultiRun
rtk grep -rn "from_results" src/ | grep -v from_multi_results

# 1. Phase 2 — site #5 削除 (test 先行)
$EDITOR src/agent/experiment_log.rs  # line 546-585 (test_experiment_from_results) + line 490 import 削除
rtk cargo test --lib  # ~1164 passed
git commit -am "refactor(benchmark): remove test_experiment_from_results test (site #5)"

# 2. site #4 削除 (legacy ctor)
$EDITOR src/agent/experiment_log.rs  # line 180-217 (from_results 全体) 削除
rtk cargo test --lib  # ~1164 passed (test 件数変化なし、ctor 削除のみ)
git commit -am "refactor(benchmark): remove Experiment::from_results legacy ctor (site #4)"

# 3. site #3 削除 (run tests)
$EDITOR src/agent/benchmark.rs  # 1955+ test_run 等削除
rtk cargo test --lib  # ~1162 passed
git commit -am "refactor(benchmark): remove BenchmarkSuite::run single-run tests (site #3)"

# 4. site #2 削除 (run fn)
$EDITOR src/agent/benchmark.rs  # line 1739-1801 削除
rtk cargo test --lib  # ~1162 passed (test 件数変化なし、fn 削除のみ)
git commit -am "refactor(benchmark): remove BenchmarkSuite::run single-run fn (site #2)"

# 5. site #1 削除 (struct + impl)
$EDITOR src/agent/benchmark.rs  # line 230-246 削除
rtk cargo test --lib  # ~1162 passed (test 件数変化なし、struct 削除のみ、composite_score 別 impl 残存確認)
git commit -am "refactor(benchmark): remove BenchmarkResult struct + composite_score impl (site #1)"

# 6. Phase 3-4 verify
rtk cargo clippy --lib --tests -- -D warnings
rtk cargo fmt --check
rtk cargo build --release
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0  # G-4a smoke

# 7. CLAUDE.md 項目 225 + handoff + 最終 commit
$EDITOR /Users/keizo/bonsai-agent/CLAUDE.md  # 項目 225 追加
git commit -am "docs(claude.md): 項目 225 — legacy BenchmarkResult + Experiment::from_results 削除完遂 (項目 222 同 pattern)"
```

## 12. 参考
- 由来 handoff: `~/.claude/projects/-Users-keizo-bonsai-agent/memory/session_2026_05_11b_handoff.md` §11 (本 session の wiring gap analysis、本 plan の前提)
- 直前 plan: `.claude/plan/agentfloor-prescreen-tier-fix.md` (項目 224、本 plan の前 commit `a52edc6`)
- 同 pattern 前例: 項目 222 (sqlite-vec wiring 削除)、`.claude/plan/sqlite-vec-wiring-removal-impl.md` (削除 plan template)
- CLAUDE.md 関連項目: 222 (sqlite-vec wiring 削除)、223 (AgentFloor 統合)、224 (pre-screen tier fix、本 plan の前)、**225 (本 plan)**
- 削除対象 source:
  - `src/agent/benchmark.rs:230-246` (BenchmarkResult struct + impl)
  - `src/agent/benchmark.rs:1739-1801` (BenchmarkSuite::run)
  - `src/agent/benchmark.rs:1955+` (test_run 等の test fixtures、削除前に line 範囲確認)
  - `src/agent/experiment_log.rs:180-217` (Experiment::from_results)
  - `src/agent/experiment_log.rs:490` (import use BenchmarkResult, TaskScore)
  - `src/agent/experiment_log.rs:546-585` (test_experiment_from_results)
- 保持確認 source:
  - `src/agent/benchmark.rs:461-559` (`MultiRunBenchmarkResult` + `composite_score` impl、本 plan 対象外)
  - `src/agent/experiment_log.rs:218-271` (`from_multi_results`、本 plan 対象外)
  - `src/agent/experiment.rs:900` (`build_prescreen_reject_experiment`、項目 224)
- 優先度: ★ (低、機能影響なし、cleanup のみ)
- 推奨実行 timing: 項目 224 PASS 確認後、G1 Critic / gbrain Stage 1 着手前 (本 session で plan 起票済 = 即実行可、または別 session)
