# Plan: AgentFloor 6-tier Capability Ladder 統合 — Lab v16 評価設計

> **由来**: arxiv 2605.00334 AgentFloor (2026-05) "How Far Up the Tool Use Ladder Can Small Open-Weight Models Go?" の **6 tier capability ladder** を bonsai-agent Lab v16 評価軸として統合。Bonsai-8B (1bit ternary, 1.28GB, small open-weight) は AgentFloor 評価対象そのもの、Lab v9-v15 で観測された「天井 5 連続」(v8/v9/v10/v14/v15) 打開のため「**どの tier を攻めるか**」を可視化することが目的。
>
> **由来 plan / handoff**: `research_arxiv_2026_05_07.md` 領域 6 (★★★ #8) / CLAUDE.md 項目 172 / 項目 188-198 / 過去 plan `agenther-option-a-migration.md` `arag-hierarchical-retrieval-docs.md`

## Task Type
- [ ] Frontend
- [x] Backend (`benchmark.rs` の Tier enum 拡張、TSV/SQLite 列追加)
- [ ] Fullstack
- [x] Docs (CLAUDE.md 項目 209 + `memory/agentfloor_tier_design.md`)

## 1. 背景
### 1.1 AgentFloor 要点
- **6 tier capability ladder** で **30 task** (5 task/tier)、small open-weight 専用 benchmark
- tier 構造 (bonsai 用語マップ):
  1. **T1 Instruction Following**: 単一指示、ツール不要
  2. **T2 Single Tool Use**: 1 ツール 1 step
  3. **T3 Tool Selection**: 複数候補から選択
  4. **T4 Multi-Step Tool Chain**: 連続 2-3 ツール、出力橋渡し
  5. **T5 Error Recovery / Adaptive**: ツール失敗から代替手段
  6. **T6 Long-Horizon Planning**: 5+ step、計画→実行→検証
- 評価指標: 各 tier の pass rate を独立計測、ceiling tier 特定
- 論文 finding: 1B-8B class は T1-3 で 0.8+、T4-5 で 0.4-0.6、T6 で 0.2-0.4 (指数的減衰)

### 1.2 bonsai 既存 2-tier との関係
項目 172 (`Tier::Core` / `Tier::Extended`) は実装済 (40 task = Core 22 + Extended 18)。本 plan は:

| 観点 | 既存 (項目 172) | 本 plan (項目 209) |
|------|---|---|
| 軸 | 「実装年代」 | 「能力梯子」 |
| tier 数 | 2 | 6 |
| task 数 | 22 + 18 = 40 | 5 × 6 = **30** (新セット) |
| 切替 | `BONSAI_BENCH_TIER` env | `BONSAI_BENCH_LADDER` env |
| 用途 | MLX 環境劣化分離 | tier 別変異効果可視化 |
| 共存 | ✅ 直交軸 — 既存非削除、別 enum `CapabilityTier` 追加し各 task に 両 tag |

### 1.3 「Scaffolding > Model」原則と整合
- 設計原則 (CLAUDE.md 巻頭): 1bit モデル改善余地は限定的、ハーネスで底上げ
- AgentFloor は small open-weight の真の上限を計測する枠組み、bonsai が「ハーネスでどの tier を実質昇格させたか」を論文比較可能な形で示す指標
- 副次効果: 別 backend (gpt-4-class) の論文値との external validation 可能 (MCP-Bench plan #9 並ぶ)

### 1.4 動機: Lab 天井 5 連続打開仮説
仮説: 既存 40 task は **T2-4 に偏在**、T1 / T5-6 への変異効果が「平均 score」で打ち消されている。tier 別計測で「変異 X は T5 で +0.04、T1 で -0.02」のような方向性が見え、変異設計が **tier-targeted** になる。

## 2. 目的
1. **Bonsai-8B strength/weakness map 取得** — 6 tier 別 pass rate baseline 計測
2. **Lab 天井 5 連続打開** — 変異効果を tier 別 delta に分解、tier-targeted 変異の方向性 (Lab v17+ HypothesisGenerator 改修への準備)
3. **既存 2-tier との直交化** — `CapabilityTier` 別軸追加、bivariate 解析可能化
4. **論文比較可能な指標** — AgentFloor 5 task/tier 規格に従う

### 非目標
- AgentFloor 30 task 完全コピー (license 不明、概念のみ参照し bonsai 文脈で書き起こす)
- 既存 40 task 削除 (両軸併存、`agentfloor_tasks()` 別 method で提供)
- HypothesisGenerator tier-targeted 変異 (Lab v17+ 別 plan)
- Tier 別 ACCEPT 判定 (informational のみ、composite_score 維持)

## 3. 既存項目との関係
| 項目 | 関係 |
|---|---|
| 172 (Core/Extended) | 直交軸として共存。既存非削除、`CapabilityTier` 別軸 |
| 184 (MLX core 22) | tier 別評価で MLX 退行がどの tier に集中していたか後付け再解析可能 |
| 188 (Lab v15 baseline 0.7560) | baseline の tier 別分解、v15→v16 比較で改善 tier 特定 |
| 200 (Beyond pass@1) | tier × stability の 2D 解析、各 tier に RDC/VAF 独立計測 |
| 201/203 (AgentHER) | 失敗 trajectory 由来 relabel が T5 (Error Recovery) で集中するか観測 |
| 204 (smoke_partial_success_chain) | T5 代表 task として再利用候補 |
| 205 (Option A) | tier 別 events 蓄積で hindsight 効果の tier 別計測 |
| 206 (current_max_id helper) | scoping 機構により tier 別 cycle 切り出し可、追加変更不要 |

## 4. 設計
### 4.1 `CapabilityTier` enum (新規)
```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum CapabilityTier {
    #[default]
    InstructionFollowing,    // T1
    SingleToolUse,           // T2
    ToolSelection,           // T3
    MultiStepToolChain,      // T4
    ErrorRecovery,           // T5
    LongHorizonPlanning,     // T6
}

impl CapabilityTier {
    pub fn label(self) -> &'static str { /* "T1-Instruct" .. "T6-LongHorizon" */ }
    pub fn short_code(self) -> &'static str { /* "t1i" "t2s" "t3x" "t4m" "t5e" "t6l" */ }
    pub fn all() -> [CapabilityTier; 6] { /* T1..T6 */ }
    pub fn paper_baseline(self) -> f64 {
        match self {
            Self::InstructionFollowing => 0.85,
            Self::SingleToolUse => 0.75,
            Self::ToolSelection => 0.65,
            Self::MultiStepToolChain => 0.50,
            Self::ErrorRecovery => 0.45,
            Self::LongHorizonPlanning => 0.30,
        }
    }
}
```

### 4.2 `BenchmarkTask` への tag 追加
```rust
pub struct BenchmarkTask {
    pub id: String,
    // 既存 fields
    #[serde(default)]
    pub tier: TaskTier,                  // 既存 (Core/Extended)
    #[serde(default)]
    pub capability_tier: CapabilityTier,  // 新規
}
```

**移行戦略**: 既存 40 task に `capability_tier` を一括追加:
| TaskCategory | 既定 capability_tier | 例外 |
|---|---|---|
| Reasoning (ツール無) | T1 | — |
| ToolUse (1 ツール) | T2 | repo_structure → T3 |
| ToolSelection | T3 | — |
| MultiStep (2 ツール) | T4 | tool_chain_10steps/implement_50steps → T6 |
| ErrorRecovery | T5 | — |
| CodeGeneration | T1 | code_gen_sort (file_write) → T2 |
| Summarization | T2 | multi_file_summary → T4 |

### 4.3 AgentFloor 専用セット — `agentfloor_tasks()`
```rust
impl BenchmarkSuite {
    pub fn agentfloor_tasks() -> Self { /* 30 task */ }
}
```

既存 40 task のうち各 tier から 5 task 厳選 + 不足 tier (特に T6) は新規追加:

**T6 LongHorizonPlanning 5 task 新規**:
1. `lh_plan_refactor_5files`: 5 ファイル横断リファクタ計画 (RepoMap → file_read × 3 → multi_edit → 検算)
2. `lh_test_red_green`: 未実装関数に test 追加 (Red) → 実装 (Green) → 再実行 (Verify)
3. `lh_dependency_chain`: 関数 A の bug fix が B, C, D に波及する影響範囲報告
4. `lh_plan_then_revise`: 初期計画立案 → 失敗想定 → 改訂計画 (self-revision)
5. `lh_multi_modal_audit`: shell + file_read + git の 3 種混在で repo health audit

### 4.4 tier 別集計 — `MultiRunBenchmarkResult` 拡張
```rust
pub struct MultiRunBenchmarkResult {
    pub task_scores: Vec<MultiRunTaskScore>,
    pub duration_secs: f64,
    pub core_avg_score: Option<f64>,         // 既存 (項目 172)
    pub extended_avg_score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_avg_scores: Option<[Option<f64>; 6]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_pass_at_k: Option<[Option<f64>; 6]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_reliability_decay: Option<[Option<f64>; 6]>,
}

impl MultiRunBenchmarkResult {
    pub fn weakest_tier(&self) -> Option<(CapabilityTier, f64)>;
    pub fn paper_delta_map(&self) -> [Option<f64>; 6];  // bonsai - paper、負値 = 攻めるべき
}
```

### 4.5 strength_weakness_map (Lab summary)
```
[INFO][lab.agentfloor] AgentFloor capability map (cycle 12, baseline):
  T1-Instruct       : 0.92  (paper 0.85, +0.07)  ← 強い
  T2-SingleTool     : 0.81  (paper 0.75, +0.06)
  T3-ToolSelect     : 0.68  (paper 0.65, +0.03)
  T4-MultiStep      : 0.54  (paper 0.50, +0.04)
  T5-ErrorRecovery  : 0.39  (paper 0.45, -0.06)  ← 攻めるべき
  T6-LongHorizon    : 0.22  (paper 0.30, -0.08)  ← ceiling
  weakest_tier      = T6
  ceiling_breakthrough_candidate = T5 (delta -0.06)
```

### 4.6 Lab v16 への組込
`run_experiment_loop`:
- baseline + 各実験計測直後に tier 別 log 出力
- ACCEPT 判定は composite_score 維持 (informational のみ)
- Experiment 構造体に `tier_avg_scores` を SQLite + TSV 保存

`BONSAI_BENCH_LADDER=1` env:
- baseline と experiment で `agentfloor_tasks()` (30 task) 使用
- env 未設定 (default) は既存 `default_tasks()` (40 task) 維持 (後方互換)

### 4.7 SQLite + TSV 永続化
> ★ V10 確保元: ERL plan v2 (`erl-heuristics-pool-impl-v2.md`) が heuristics テーブルで V10 を使用するため、本 plan は ERL merge 後に V11 で上乗せする (依存順序: ERL → AgentFloor)。

**SQLite V10 → V11**:
```sql
ALTER TABLE experiments ADD COLUMN tier_t1 REAL;
ALTER TABLE experiments ADD COLUMN tier_t2 REAL;
ALTER TABLE experiments ADD COLUMN tier_t3 REAL;
ALTER TABLE experiments ADD COLUMN tier_t4 REAL;
ALTER TABLE experiments ADD COLUMN tier_t5 REAL;
ALTER TABLE experiments ADD COLUMN tier_t6 REAL;
```
**TSV 列 15 → 21 列**: 末尾 `tier_t1..tier_t6` 追加 (NaN は `-` で空表現)

## 5. TDD strict 5 phase
### Phase 1 — Red
新規 test 5 件 (benchmark.rs / experiment.rs):
1. `test_capability_tier_all_returns_six` — `CapabilityTier::all().len() == 6`
2. `test_capability_tier_label_short_code_unique` — 6 tier label/short_code unique
3. `test_default_tasks_capability_tier_coverage` — 全 40 task に tag、各 tier 最低 1 件 (T1-T5)、T6 は 2 件のみ存在 (`tool_chain_10steps`/`implement_50steps`)
4. `test_agentfloor_tasks_30_count` — `agentfloor_tasks().tasks.len() == 30` && 各 tier 正確に 5 件
5. `test_compute_capability_tier_avg_basic` — 既存 `compute_tier_avg` パターンで T3 平均、空 tier は None

期待: compile error or 全 fail で Red 確認。

### Phase 2 — Green
1. `CapabilityTier` enum + label/short_code/all/paper_baseline → test 1, 2 pass
2. `BenchmarkTask::capability_tier` field + 既存 40 task に tag 追加 → test 3 pass
3. `compute_capability_tier_avg()` → test 5 pass
4. `agentfloor_tasks()` (既存 25 + T6 新規 5) → test 4 pass
5. `MultiRunBenchmarkResult::tier_avg_scores` field + run_k 集計 + serde default 互換確認

期待: 既存 1058 + 新規 5 = **1063 passed** / clippy 0 / fmt 0

### Phase 3 — Refactor
- `compute_tier_avg` (既存) と `compute_capability_tier_avg` (新規) の重複 generic 化検討 → enum 異種のため YAGNI、関数別維持 OK
- `weakest_tier()` / `paper_delta_map()` helper 追加
- docstring 整備 (項目 209 参照、AgentFloor 由来明記)
- `BONSAI_BENCH_LADDER` env 読込 helper 追加 (既存 `BONSAI_BENCH_TIER` と並列)

### Phase 4 — Smoke 検証 (3 段)
```bash
# G-4a: 既存 default_tasks 経路 (後方互換)
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0
# 期待: smoke 7 task 実行、1058 pass 維持、tier_avg_scores=None で TSV 列空表現

# G-4b: AgentFloor 30 task sanity
BONSAI_BENCH_LADDER=1 BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0
# 期待: 30 task / k=3 = 90 run で 25-35 min、各 tier 最低 1 task 完走

# G-4c: 完全 baseline (1 cycle のみ)
BONSAI_BENCH_LADDER=1 ./target/release/bonsai --lab --lab-experiments 1
# 期待: log [INFO][lab.agentfloor] tier map 出力、各 tier ≥ 0.20 (1bit 下限)
```

判定:
- ✅ G-4a: 既存経路 1058 passed 維持
- ✅ G-4b: 30 task smoke で全 tier ≥ 1 valid score、`weakest_tier()` 値返却
- ✅ G-4c: T1 ≥ 0.50 / T6 ≥ 0.10 (1bit Bonsai-8B 現実的下限)

### Phase 5 — Commit + handoff + CLAUDE.md 項目 209
5 commits:
1. `test(agentfloor): Phase 1 Red — CapabilityTier + 30 task suite test`
2. `feat(agentfloor): Phase 2 Green — CapabilityTier + agentfloor_tasks + tier_avg`
3. `refactor(agentfloor): Phase 3 — weakest_tier helper + LADDER env`
4. `feat(experiment): Phase 4 — Lab summary tier map + V10→V11 + TSV 21 列`
5. `docs(claude.md): 項目 209 — AgentFloor 6-tier 統合完遂 + smoke G-4 PASS`

## 6. API 影響
| API | 変更 | 後方互換 |
|---|---|---|
| `BenchmarkTask` | `capability_tier` field | ✅ serde default |
| `CapabilityTier` enum | 新規 6 値 | — |
| `BenchmarkSuite::agentfloor_tasks()` | 新規 method | — |
| `MultiRunBenchmarkResult` | 3 field 追加 | ✅ serde default + skip_if_none |
| `weakest_tier()` / `paper_delta_map()` | 新規 method | — |
| `Experiment` (experiment_log) | tier_t1..t6 6 Option<f64> | ✅ default + skip |
| SQLite | V11 (6 列追加 ALTER TABLE)、★ ERL plan v2 (`erl-heuristics-pool-impl-v2.md`) で V10 を確保。本 plan は ERL merge 後に V11 で乗る | ✅ additive |
| TSV | 15 → 21 列 (末尾追加) | ⚠️ header 駆動 reader OK |
| env | `BONSAI_BENCH_LADDER=1` 新規 | ✅ default 未設定で既存挙動 |

**signature 変更ゼロ** — 全 additive、項目 205 のような必須化はなし。

## 7. Risks / Mitigations
| # | Risk | 影響 | Mitigation |
|---|---|---|---|
| R1 | 30 task × k=3 = 90 run で Lab cycle 時間 2x 膨張 (T6 long-horizon max_iterations=10+ で 35h risk) | 運用負担 | (i) `BONSAI_BENCH_LADDER` env で opt-in (ii) 初回 baseline は k=1 で 30 run 圧縮可 (iii) max_iterations ≤ 10 を T6 強制 |
| R2 | 項目 172 (Core/Extended) との概念重複で混乱 | maintainer onboarding 困難 | docstring + CLAUDE.md 項目 209 で「直交軸」明記、`memory/agentfloor_tier_design.md` に用語表 |
| R3 | T6 5 task の定義主観性 (「long-horizon とは何 step か」論文未明示) | bonsai 内部基準のみ比較可 | (i) max_iterations ≥ 8 を T6 必須条件と明文化 (ii) AgentFloor 論文 fetch 後 (Phase 4 までに) 定義校正 |
| R4 | 1bit variance で tier 区別困難 (T4 と T5 overlap) | tier map noisy | k=3 → k=5 増 + 項目 200 RDC/VAF と 2D で見る |
| R5 | SQLite V10 → V11 本番 DB 互換性 | 既存 .bonsai/db で ALTER TABLE 失敗 | migrate.rs 既存パターン (V8→V9) と同形、`PRAGMA user_version` チェック必須、Phase 2 migration test 必須 |
| R6 | TSV 21 列化で外部解析スクリプト破損 | grafana/jupyter notebook | 末尾追加で前 15 列 semantic 不変、column header で robust、CLAUDE.md 明記 |
| R7 | 25 件再利用 task の `capability_tier` 判定誤り | tier 別計測信頼性低下 | Phase 1 test 3 で全 40 task 全 tag assert、PR レビューで 1 名 (code-reviewer agent) 再判定 |

## 8. Quality Gates
- **G-1 Phase 1 Red**: 5 新規 test compile error or 全 fail
- **G-2 Phase 2 Green**: 5 新規 test PASS + 1058 維持 = 1063 passed + clippy 0 + fmt 0
- **G-3 Phase 3 Refactor**: docstring 完備 + helper 追加 + 既存 test 退行ゼロ
- **G-4 Phase 4 Smoke 3 段**:
  - G-4a: 既存経路 1058 pass 維持
  - G-4b: 30 task smoke で全 tier ≥ 1 valid score
  - G-4c: 30 task k=3 baseline で T1 ≥ 0.50 かつ T6 ≥ 0.10
- **G-5 Final**: AgentFloor tier map が Lab log 出力 + V11 + TSV 21 列 + handoff 起票 + CLAUDE.md 項目 209

## 9. 完了条件
1. ✅ `CapabilityTier` enum + `agentfloor_tasks()` 追加
2. ✅ 既存 40 task に capability_tier tag (T1-T5 各 ≥ 1、T6 ≥ 2)
3. ✅ T6 新規 5 task (合計 30 task)
4. ✅ `tier_avg_scores` 集計実装
5. ✅ `BONSAI_BENCH_LADDER=1` env で Lab 起動可
6. ✅ SQLite V11 + TSV 21 列 (★ ERL plan v2 が V10 を確保するため V11 に変更)
7. ✅ Lab summary に tier map (4.5 形式)
8. ✅ smoke G-4a/b/c 全 PASS
9. ✅ 1063+ passed 維持
10. ✅ CLAUDE.md 項目 209 + memory/agentfloor_tier_design.md

## 10. 見積もり
| Phase | 内容 | 時間 |
|-------|------|------|
| Phase 1 | Red — 5 test 追加 | 0.5h |
| Phase 2 | Green — enum + tag + agentfloor_tasks + tier_avg | 3.0h |
| Phase 3 | Refactor — helper + env + docstring | 1.0h |
| Phase 4 | Smoke 3 段 (うち c は 30 min × 1 cycle 実機) | 4.0h (実機 wall 2.5h) |
| Phase 5 | Commit + handoff + CLAUDE.md 項目 | 1.0h |
| Buffer | T6 task 定義校正、SQLite migration debug | 1.5h |
| **合計** | | **~11h ≈ 1.5 day** |

## 11. Quick Start
```bash
# 0. 既存 caller 全網羅
rtk grep -rn "TaskTier::" src/
rtk grep -rn "agentfloor|capability_tier|CapabilityTier" src/  # 期待 0 件
rtk grep -rn "BONSAI_BENCH_TIER" src/

# 1. Phase 1 Red
$EDITOR src/agent/benchmark.rs
rtk cargo test --lib capability_tier  # compile error or fail

# 2. Phase 2 Green
$EDITOR src/agent/benchmark.rs   # CapabilityTier + tag + agentfloor_tasks + tier_avg_scores
$EDITOR src/db/migrate.rs        # V10 → V11
$EDITOR src/agent/experiment_log.rs
rtk cargo test --lib  # 1063 passed

# 3. Phase 3 Refactor
$EDITOR src/agent/benchmark.rs   # weakest_tier / paper_delta_map / docstring
$EDITOR src/agent/experiment.rs  # LADDER env + tier map log

# 4. Phase 4 Smoke 3 段
rtk cargo build --release
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0  # G-4a
BONSAI_BENCH_LADDER=1 BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0  # G-4b
BONSAI_BENCH_LADDER=1 ./target/release/bonsai --lab --lab-experiments 1  # G-4c (30-40 min)
grep "lab.agentfloor" /tmp/bonsai_*.log

# 5. Commit + handoff + CLAUDE.md 項目 209
```

## 12. 参考
- arxiv 2605.00334 AgentFloor (https://arxiv.org/html/2605.00334)
- `agenther-option-a-migration.md` (品質基準・TDD strict 構成)
- `arag-hierarchical-retrieval-docs.md` (docs PR 品質基準)
- 既存 implementation: `benchmark.rs::TaskTier` / `compute_tier_avg`
- CLAUDE.md 項目 172 / 200 / 205
- 派生候補: Lab v17 tier-targeted 変異 / PASS@(k,T) 統合 (3D 評価)
