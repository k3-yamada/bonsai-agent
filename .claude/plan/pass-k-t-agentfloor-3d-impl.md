# PASS@(k,T) × AgentFloor 6-tier 3D 統合 (tier × k × T capability/efficiency 立体評価)

**状態**: planning-only (2026-05-15 起票)、推奨度 ★★、推定工数: ~6h (0.5-1 day scope)
**起点**: 項目 225 (PASS@(k,T) 2D metric、SCHEMA_V15) + 項目 223/224 (AgentFloor 6-tier、`tier_avg_scores` populate)。T6-LongHorizon = weakest_tier (項目 224 G-4c v4 で score=0.47 paper +0.17) 攻略の前提として、tier × k × T の 3D 解像度で score 観測。

---

## §1. 背景

### 既存 2 軸の独立成立 (項目 223/224/225 の現状)

| 軸 | metric | env | persist |
|----|--------|-----|---------|
| **Capability (tier)** | `tier_avg_scores: [Option<f64>; 6]` (T1..T6 平均) | `BONSAI_BENCH_LADDER=1` | SCHEMA_V14、TSV 21 列 |
| **Capability×Efficiency (k×T)** | `pass_at_k_t_steps: Vec<(usize, f64)>` / `pass_at_k_t_seconds: Vec<(f64, f64)>` (全 task 平均) | `BONSAI_PASS_K_T_STEPS=3,5,7` / `BONSAI_PASS_K_T_SECONDS=60,180,600` | SCHEMA_V15、TSV 23 列 |

### 解像度ギャップ
全 task 平均 PASS@(k,T) は tier 軸を平滑化。T6-LongHorizon は max_iterations=8-10 で T_steps=3 では本来全失敗するはずが、T1/T2 (max_iter=2-4) の高 pass 率と混ざって `composite_pass_at_k_t_steps[T=3] ≈ 0.5` 程度に見える。**T6 単独で T_steps=3 を見れば 0.0 近い**はず。

### Bonsai-8B 能力プロファイル (項目 224 G-4c v3/v4 baseline)
```
T1-Instruct=0.68 (paper 0.85, -0.17)
T2-SingleTool=0.52 (paper 0.75, -0.23)
T3-ToolSelect=0.77 (paper 0.65, +0.12)
T4-MultiStep=0.64 (paper 0.50, +0.14)
T5-ErrorRecov=0.70 (paper 0.45, +0.25)
T6-LongHorizon=0.47 (paper 0.30, +0.17、weakest 確定)
```

T6-LongHorizon (paper +0.17 / 絶対値最弱 0.47) = overall composite_score を底上げできる **最大 leverage 点**。

### 3D 化の動機: tier × k × T 立体評価

| シナリオ | 2D で見える | 3D で初めて見える |
|---------|------------|------------------|
| T6 efficiency 改善変異 | composite_PASS@(k,T_steps=5) +0.01 (noise) | tier_pass_at_k_t_steps[T6][T=5] +0.04 (信号顕在化) |
| T6 T_seconds=600 で初 pass | composite は T1-5 で頭打ち | tier_pass_at_k_t_seconds[T6][T_seconds=600] = 0.6 vs [180]=0.1 |
| HypothesisGenerator T6 偏向改修 | 全 task 平均で +0.01 程度に薄まる | T6 metric で +0.04 を検出 |

---

## §2. 設計

### 案 A + 案 C 融合 (推奨)

- `MultiRunBenchmarkResult` に method 追加 (構造体不変):
  - `composite_tier_pass_at_k_t_steps() -> [Vec<(usize, f64)>; 6]`
  - `composite_tier_pass_at_k_t_seconds() -> [Vec<(f64, f64)>; 6]`
  - `weakest_tier_at_t_steps(t_steps: usize) -> Option<(CapabilityTier, f64)>` (T6 攻略前提データ取得)
- `MultiRunTaskScore` に `capability_tier: CapabilityTier` field 追加 (`#[serde(default)]`、tier lookup 用、`run_k` で 1 行 wiring)
- `Experiment` に 2 Vec field 追加:
  ```rust
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub tier_pass_at_k_t_steps: Option<[Vec<(usize, f64)>; 6]>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub tier_pass_at_k_t_seconds: Option<[Vec<(f64, f64)>; 6]>,
  ```
- `from_multi_results` で composite method 経由 populate
- SQLite V17 migration (`ALTER TABLE experiments ADD COLUMN tier_pass_at_k_t_{steps,seconds} TEXT` × 2、JSON encode = `[[T1_entries], ..., [T6_entries]]`)
- TSV 23→25 列、None → `-`

### SCHEMA 番号交渉 (frontier plan §229 と)
| Plan | SCHEMA | 提案順序 |
|------|--------|--------|
| `frontier-benchmark-impl.md` (frontier 5 fields、項目 229) | **V16** | 先行 merge 前提 |
| **本 plan** (3D 統合、2 fields、項目 231 候補) | **V17** | 後続 merge |

merge 順反転時は migrate.rs の version 数字差替えのみで対応。

### env 制御 (新規 env 追加なし、AND 条件)
- `BONSAI_BENCH_LADDER=1` + `BONSAI_PASS_K_T_STEPS=3,5,7` + `BONSAI_PASS_K_T_SECONDS=60,180,600` 三者 AND で 3D 軸 populate
- 三者 AND 不成立 → 3D 軸 None / log 0 件 / DB NULL = 既存挙動 100% 互換 = production default OFF

### Lab summary 出力例
```
[INFO][lab.tier_pass_k_t] baseline: composite=0.7812 LADDER=on PASS_K_T=on(steps=[3,5,7] secs=[60,180,600])
[INFO][lab.tier_pass_k_t]   tier × T_steps:
[INFO][lab.tier_pass_k_t]     T6-LongHorizon    T=3:0.05 T=5:0.21 T=7:0.42  ← weakest
[INFO][lab.tier_pass_k_t]   weakest_tier_at_T_steps=5 = T6 (rate=0.21)
```

---

## §3. TDD strict 5 phase

### Phase 1 (Red) — 9 failing tests
1. `t_composite_tier_pass_at_k_t_steps_basic` (6 tier partition + 平均計算)
2. `t_composite_tier_pass_at_k_t_seconds_epsilon_bucket` (1e-6 epsilon、項目 225 既存 pattern)
3. `t_composite_tier_pass_at_k_t_empty_tier_returns_empty_vec` (該当 task ゼロ tier)
4. `t_composite_tier_pass_at_k_t_when_env_unset_returns_empty_arrays`
5. `t_multirun_task_score_capability_tier_default_t1` (旧 JSON 後方互換)
6. `t_weakest_tier_at_t_steps_returns_min` (T6 最弱 synthetic data)
7. `t_experiment_serde_with_tier_pass_k_t_3d` (3D 軸 round-trip JSON)
8. `t_experiment_db_roundtrip_with_tier_pass_k_t_v17` (V17 schema 経由 DB insert/select)
9. `t_build_prescreen_reject_experiment_carries_over_tier_pass_k_t` (項目 224 helper 拡張、wiring gap 先制回避)

### Phase 2 (Green) — 12 step
1. `MultiRunTaskScore::capability_tier` field 追加 + `#[serde(default)]`
2. `BenchmarkSuite::run_k` で `task_score.capability_tier = task.capability_tier;` 1 行 wiring
3. `composite_tier_pass_at_k_t_steps()` 実装 (group_by tier + 既存 epsilon bucket 流用)
4. `composite_tier_pass_at_k_t_seconds()` 実装 (f64 epsilon 1e-6)
5. `weakest_tier_at_t_steps()` 実装
6. `Experiment` 2 Vec field 追加 (`Option<[Vec<...>; 6]>` + `#[serde(default, skip_serializing_if)]`)
7. `Experiment::from_multi_results` で composite method 経由 populate
8. **`build_prescreen_reject_experiment` (項目 224 helper) 拡張**: baseline.tier_pass_at_k_t_{steps,seconds} を carry-over (項目 229 副次 finding (a) を先制回避)
9. SQLite migration V16 → V17 (ALTER TABLE × 2、JSON encode)
10. `save_to_db` / `recent_experiments` SQL に 2 列追加 (`serde_json::to_string` / `from_str`)
11. TSV 23 → 25 列 (末尾追加、None → `-`)
12. Lab summary log macro 拡張 (三者 AND 時のみ emit)

### Phase 3 (Refactor) — dedup + 後方互換
1. dedup helper: `pub(crate) fn bucket_pass_rates<T: Copy + PartialEq>(...) -> Vec<(T, f64)>` で項目 225 epsilon bucket ロジック共有
2. SQLite migration test (V16 既存 row が V17 ALTER 後も NULL 保持)
3. TSV 後方互換 (header 駆動 reader 推奨)
4. JSON serde 後方互換 (旧 Experiment JSON → load 成功で None)
5. docstring に `項目 231、arxiv 2604.14877 PASS@(k,T) × arxiv 2605.00334 AgentFloor 3D 統合` 由来明記

### Phase 4 (Smoke) — G-4a/b/c
- **G-4a** (env 未指定): 3D 軸 NULL + log 0 件 + 既存挙動 100% 互換、TSV 25 列ヘッダで末尾 `-\t-`
- **G-4b** (LADDER のみ): tier_avg_scores populate / 3D 軸 NULL
- **G-4c** (LADDER + PASS_K_T 両指定): 3D 軸 SQLite JSON 保存 + log emit + `weakest_tier_at_t_steps` 返却、手計算 1 件で ±1e-6 一致確証

### Phase 5 (Docs + Commit)
CLAUDE.md 項目 231 (1 行 summary) + 5 commits 構成:
1. `test(benchmark+experiment): Phase 1 Red — tier × PASS@(k,T) 3D 9 件 failing tests`
2. `feat(benchmark): Phase 2 Green — composite_tier_pass_at_k_t_{steps,seconds} + capability_tier on MultiRunTaskScore + weakest_tier_at_t_steps`
3. `feat(experiment_log): Phase 2 — Experiment 2 Vec field + V17 migration + TSV 25 列 + build_prescreen_reject helper 拡張`
4. `refactor(benchmark+experiment): Phase 3 — bucket_pass_rates helper dedup + Lab summary log macro`
5. `docs(claude.md): 項目 231 — PASS@(k,T) × AgentFloor 3D 統合完遂 + smoke G-4a/b/c PASS`

---

## §4. 期待効果 (仮説、Phase 5 で検証)

| 仮説 | 反証条件 |
|------|--------|
| **H1**: T6-LongHorizon × T_steps=3 で pass_rate < 0.1 | T6 max_iterations >= 8 設計から論理予測、smoke G-4c で確証 |
| **H2**: T6 × T_seconds 軸で monotonic 増加 (T=60→600 で 0.05→0.5+) | 逆ならむしろ stability/parallelism 不足が真の制約 |
| **H3**: tier-targeted 変異の effect size +0.04 以上見える | Lab v22+ で HypothesisGenerator T6 偏向改修後の検証基準 |
| **H4**: T1 × T_steps=3 で天井 (pass=0.95+) で攻略余地なし | T1 攻略は leverage 低、T6 集中 design rationale 強化 |

H1/H2 成立 → **T6 攻略前提データ取得完遂** = Lab 天井 7 連続打破の構造軸 2 軸目 (frontier = context-length、本 plan = tier-targeted efficiency)。

---

## §5. 起票候補項目

- **項目 231** = 本 plan Phase 1-3 完遂 + Phase 4 G-4a/b/c smoke 3/3 PASS
- **項目 232** (将来) = `BONSAI_TIER_PASS_K_T_ENABLED` 単独 flag (LADDER off + PASS_K_T off でも 3D 取得要件発生時)
- **項目 233** (将来、Lab v22+) = HypothesisGenerator T6 偏向改修 + 本 3D metric での effect 検証 (paired t-test ACCEPT 判定)

---

## §6. 依存

| 項目 | 状態 | 依存性 |
|------|------|--------|
| **項目 223** AgentFloor 6-tier wiring | 完遂 (commit `572a9a4`) | `BenchmarkTask::capability_tier` 利用 |
| **項目 224** AgentFloor pre-screen helper | 完遂 (commit `a52edc6`) | helper 拡張 (3D 軸 carry-over) |
| **項目 225** PASS@(k,T) 2 軸 metric (V15) | 完遂 | 同 type を tier 単位で集約 |
| **項目 229** frontier benchmark | 進行中 (Phase 1-3 済、Phase 4/5 Lab v19 起動) | SCHEMA migration 番号競合 (frontier=V16 / 本=V17) |

---

## §7. 不要転用 (rejected) / YAGNI

- **全組合せ 3D matrix (tier 6 × k 5 × T 3 = 90 cells)** — k 軸増加 cost 線形増、本 plan は k=固定 3
- **task category × tier × T 4D** — cell 過多 + sample noise 増、極めて稀な要件
- **active gate 化** (`tier_pass_at_k_t_steps[T6][5] > threshold`) — smoke 10+ サイクル蓄積後の別 plan (項目 232 候補)
- **HypothesisGenerator T6 偏向改修** — 本 plan は前提データ取得のみ、変異戦略は Lab v22+ 別 plan (項目 233 候補)
- **`BONSAI_TIER_PASS_K_T_ENABLED` 単独 flag** — 既存 3 env AND で十分、cognitive load 回避

---

## §8. ロールバック戦略

### env opt-in default OFF
三者 AND 不成立 → 3D 軸 None、log 0 件、DB NULL = 既存挙動 100% 互換

### 部分ロールバック
| 粒度 | 手順 | 影響 |
|---|---|---|
| runtime | env unset (LADDER or PASS_K_T どちらか) | 即時無効、code 変更なし |
| commit | `git revert HEAD~5..HEAD` | V17 ALTER は残存だが NULL のままで実害なし |
| SCHEMA | `ALTER TABLE ... DROP COLUMN` × 2 | SQLite 3.35+ サポート、Phase 2 migration test で確証 |

---

## §9. risks / mitigations

| # | Risk | Mitigation |
|---|------|-----------|
| **R1** | 6 tier × T 3 段 × 2 軸 = 36 数値 / cycle で log readability 劣化 | env 三者 AND 不成立で log 全 skip、tier 内 task ゼロ row skip |
| **R2** | tier 内 task 1 件のみで PASS@(k,T) noise 大 | AgentFloor 30 task suite (5/tier) で最低 5 サンプル/tier |
| **R3** | SCHEMA V17 が frontier plan (V16) と順序競合 | §2 SCHEMA 番号交渉済、merge 順反転時は migrate.rs 1 関数差替え |
| **R4** | `build_prescreen_reject_experiment` の 3D 軸 carry-over が baseline 未取得時に panic | helper 内 `unwrap_or(None)` 化、Phase 1 test 9 で明示検証 |
| **R5** | f64 epsilon bucket で異なる sample が同 bucket 化 | 既存 1e-6 (項目 225 一貫)、env で T 値 sparse 指定推奨 |
| **R6** | TSV 25 列化で外部解析 script (21/23 列 reader) 破損 | 末尾 2 列追加で前 23 列 semantic 不変、header 駆動 reader robust |
| **R7** | LADDER 未使用時 (`capability_tier == T1 default`) で tier 0 集約 | Lab summary header に LADDER=on/off flag 明記、env 三者 AND で skip |

---

## §10. API 影響 (additive only)

| API | 変更 | 後方互換 |
|---|---|---|
| `MultiRunTaskScore` | `capability_tier: CapabilityTier` field 追加 | OK `#[serde(default)]` (T1 = tier 0 集約 = 既存 LADDER 未使用挙動) |
| `BenchmarkSuite::run_k` | 内部 1 行 wiring | OK signature 不変 |
| `MultiRunBenchmarkResult::composite_tier_pass_at_k_t_{steps,seconds}` | 新規 method | — |
| `MultiRunBenchmarkResult::weakest_tier_at_t_steps` | 新規 method | — |
| `Experiment` | 2 Option<[Vec<_>; 6]> field 追加 | OK `#[serde(default, skip_serializing_if)]` |
| `build_prescreen_reject_experiment` | baseline.tier_pass_at_k_t_* clone 拡張 | OK signature 不変 |
| SQLite V16 → V17 | 2 列 ALTER TABLE | OK additive |
| TSV 23 → 25 列 | 末尾追加 | header 駆動 reader OK |
| env | 既存 3 env AND、新規 env なし | OK 三者 AND 不成立で既存挙動 |

**signature 変更ゼロ** — 既存 4 caller (experiment.rs:582/595/868/1017) は無変更動作。

---

## §11. 完了条件 + Quick Start

```bash
# 0. 既存 caller 確認
grep -rn "composite_pass_at_k_t\|tier_avg_scores\|build_prescreen_reject_experiment" src/

# 1-3. Phase 1-3
cargo test --release --lib tier_pass_at_k_t   # Red 9 件
# 実装後: 1176 → 1185 passed
cargo clippy --release --lib --tests -- -D warnings
cargo fmt --check

# 4. Phase 4 Smoke 3 段 (release build 必須)
cargo build --release
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 | tee /tmp/g4a.log
BONSAI_LAB_SMOKE=1 BONSAI_BENCH_LADDER=1 ./target/release/bonsai --lab --lab-experiments 0 | tee /tmp/g4b.log
BONSAI_LAB_SMOKE=1 BONSAI_BENCH_LADDER=1 \
BONSAI_PASS_K_T_STEPS=3,5,7 BONSAI_PASS_K_T_SECONDS=60,180,600 \
  ./target/release/bonsai --lab --lab-experiments 0 | tee /tmp/g4c.log
grep "lab.tier_pass_k_t" /tmp/g4c.log

# 5. 5 commits + CLAUDE.md 項目 231
```

完了条件:
1. cargo test 1176 → **1185 passed** (+9、退行ゼロ)
2. clippy / fmt 0 件
3. V16 → V17 migration 適用成功、既存 row NULL 保持
4. TSV 25 列ヘッダ、env 三者 AND 不成立で末尾 2 列 `-`
5. smoke baseline composite_score variance 範囲内 (±0.03)
6. env 三者指定 smoke で `[INFO][lab.tier_pass_k_t]` 出力
7. 手計算 1 件で ±1e-6 一致
8. CLAUDE.md 項目 231 追記

---

## §12. 参考

### 一次資料
- arxiv 2604.14877 — Does RL Expand the Capability Boundary of LLM Agents? PASS@(k,T) Analysis (項目 225 由来)
- arxiv 2605.00334 — AgentFloor: How Far Up the Tool Use Ladder Can Small Open-Weight Models Go? (項目 223 由来)

### bonsai 内部参照
- `.claude/plan/pass-k-t-metric-impl.md` (項目 225、T 軸 metric 起源)
- `.claude/plan/agentfloor-tier-eval-impl.md` (項目 223、CapabilityTier enum 起源)
- `.claude/plan/agentfloor-prescreen-tier-fix.md` (項目 224、helper 起源)
- `.claude/plan/frontier-benchmark-impl.md` (項目 229、SCHEMA 番号交渉)
- CLAUDE.md 項目 200 / 207 / 215 / 223 / 224 / 225 / 229
- `src/agent/benchmark.rs:330-358` (`MultiRunTaskScore`)
- `src/agent/benchmark.rs:507-578` (`MultiRunBenchmarkResult` composite method 群)
- `src/agent/benchmark.rs:1184-1288` (`BenchmarkSuite::run_k`)
- `src/agent/experiment_log.rs:900` (`build_prescreen_reject_experiment`)
- `src/agent/experiment_log.rs:285-323` (`append_tsv`)
