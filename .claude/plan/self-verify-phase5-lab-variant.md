# Plan: Self-Verification Dilemma Phase 5 — Lab variant pool で動的 skip threshold ACCEPT 判定

> **由来**: 項目 210 (Self-Verify Dilemma 動的 skip Phase 1-4 完遂、production code default OFF で既存挙動 100% 維持) の **Phase 5 effectiveness 検証**。Lab v15 variant pool に `dynamic_skip_threshold ∈ {0.3, 0.4, 0.5}` を追加し、core 22 baseline (threshold=0.0) vs variant の delta で ACCEPT 判定する。
>
> **目的**: 項目 210 で実装した動的 skip 機構の **実機効果を data-driven に検証**。Lab v8/v9/v10/v14/v15 天井 5 連続打開の構造変異 evidence を取得 (項目 207 副次知見)。
>
> **前提**: 項目 210 で `AdvisorConfig::dynamic_skip_threshold` field + `should_skip_verification` skip hook + `verification_success_rate` query が実装済 (default 0.0、既存 1075 passed 維持)。本 plan は **Lab harness 拡張のみ** (`HypothesisGenerator` の config mutation 拡張、`ExperimentLoop` の threshold override 適用)。

## Task Type
- [ ] Frontend
- [x] Backend (`HypothesisGenerator` 拡張、`Hypothesis` enum 化、`ExperimentLoop` config override hook)
- [ ] Fullstack

## 1. 背景
### 1.1 項目 210 の到達点
- `AdvisorConfig::dynamic_skip_threshold: f64 = 0.0` (default OFF)
- `AdvisorConfig::min_samples_for_skip: usize = 5`
- `should_skip_verification(advisor, store, task_context) -> bool` で `verification_success_rate < threshold` 時 skip
- `AuditAction::AdvisorSkip { reason, rate, threshold }` で audit log 記録
- `EventRepository::verification_success_rate(task_type, min_samples) -> Result<Option<f64>>` (項目 209 trait dividend)
- `classify_task_type` 4 カテゴリ (`code_edit`/`code_read`/`shell_exec`/`other`)
- 11 新規 test、production code default OFF で既存挙動 100% 維持

### 1.2 残課題 (項目 210 末尾「次=★★」)
- threshold default 0.0 のため **実機効果未検証**
- 構造的変異 evidence 不足 (項目 207 副次知見「天井 5 連続打開 = 構造的変異」)
- HypothesisGenerator は string-only prompt mutation のみ (項目 207 で 54 件履歴 dedup 困難確認)

### 1.3 Lab v15 ハーネスの限界
現行 `HypothesisGenerator::generate` は `Vec<String>` (system prompt suffix) のみ生成。**config mutation (numeric 値変動)** をサポートしない。本 plan で `Hypothesis` を enum 化し、`ConfigOverride` variant を導入する。

## 2. 目的
1. **Hypothesis enum 化**: `String` → `enum { PromptSuffix(String), ConfigOverride(ConfigDelta) }` 拡張
2. **AdvisorThreshold variant**: 3 値 (0.3, 0.4, 0.5) を Lab variant pool に投入
3. **ExperimentLoop config override**: variant 実行前に `AdvisorConfig` を一時 override、cycle 終了時 restore
4. **ACCEPT 判定**: core 22 baseline (0.0) vs variant の `composite_score` delta (項目 200 拡張対応)
5. **副次効果計測**: TSV に `verify_skip_count` / `verify_skip_rate` 列追加 (informational のみ、ACCEPT 判定不変)
6. **production default 維持**: Phase 5 後も threshold default 0.0 のまま、ACCEPT 確証後に項目 211+ で defaults 化判定

## 3. 既存項目との関係
| 項目 | 関係 | 改修要否 |
|---|---|---|
| **210** Self-Verify 動的 skip | Phase 5 の前提実装、production code 変更ゼロで本 plan が足場利用 | 参照のみ |
| **209** EventRepository trait | `verification_success_rate` 経由で Mock test 可、Phase 1 Red 容易化 | 参照のみ |
| **207** Lab v15 long run | 天井 5 連続 evidence + 89 min 完走パターンを再利用 | 参照のみ |
| **205** Option A 移行 | `&MemoryStore` 必須化済、`run_k` signature 既に `persistent_store: &MemoryStore` | 設計踏襲 |
| **200** Beyond pass@1 RDC/VAF | 13→14 列 + ACCEPT informational metric として併用 | 拡張 (TSV +1 列) |
| **172** Tier::Core/Extended | `core 22` 評価軸 (既存 BONSAI_BENCH_TIER=core で起動) | 参照のみ |
| **47** ツール思考強制 | 既存 default 化済変異、本 plan の variant とは独立 | — |

## 4. 設計
### 4.1 Hypothesis enum 化 (新規)
`src/agent/experiment.rs::Hypothesis`:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Hypothesis {
    /// 既存: system prompt suffix 文字列 (項目 1-207 の prompt 系変異)
    PromptSuffix {
        text: String,
        category: String,  // "tool_thinking"/"error_analysis"/etc.
    },
    /// 新規: AdvisorConfig 数値 override
    ConfigOverride(ConfigDelta),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConfigDelta {
    AdvisorThreshold(f64),  // dynamic_skip_threshold
    // 後続 plan で AdvisorMaxUses(usize) 等を追加可能
}

impl Hypothesis {
    pub fn label(&self) -> String {
        match self {
            Self::PromptSuffix { category, .. } => format!("prompt_{}", category),
            Self::ConfigOverride(ConfigDelta::AdvisorThreshold(t)) => format!("config_advisor_threshold_{:.2}", t),
        }
    }
    pub fn fingerprint(&self) -> String {
        // 既存 dedup (54 件履歴) と同形式、enum variant 別 hash
    }
}
```

**後方互換性**: `Hypothesis::PromptSuffix` への自動変換 helper を提供 (`From<String>`)、既存 caller 24 箇所無変更。

### 4.2 HypothesisGenerator 拡張
`src/agent/experiment.rs::HypothesisGenerator`:
```rust
impl HypothesisGenerator {
    pub fn generate(&mut self, count: usize) -> Vec<Hypothesis> {
        // 既存 prompt 系生成 (LLM call で N 件)
        let mut prompts = self.generate_prompt_variants(count.saturating_sub(3))?;
        // 新規 config 系 seeding (固定 3 件、Phase 5 専用)
        let configs = vec![
            Hypothesis::ConfigOverride(ConfigDelta::AdvisorThreshold(0.3)),
            Hypothesis::ConfigOverride(ConfigDelta::AdvisorThreshold(0.4)),
            Hypothesis::ConfigOverride(ConfigDelta::AdvisorThreshold(0.5)),
        ];
        prompts.extend(configs);
        prompts
    }
}
```

**env opt-in**: `BONSAI_LAB_CONFIG_VARIANTS=1` (default 0、後方互換)。0 のとき既存挙動 (prompt-only)。Phase 5 effectiveness 検証時のみ env=1 で起動。

### 4.3 ExperimentLoop config override hook
`src/agent/experiment.rs::run_experiment_loop`:
```rust
for hyp in hypotheses {
    // baseline 共通 (variant 適用前)
    let baseline_advisor = config.advisor.clone();

    // variant 適用
    let active_advisor = match &hyp {
        Hypothesis::PromptSuffix { .. } => baseline_advisor.clone(),
        Hypothesis::ConfigOverride(ConfigDelta::AdvisorThreshold(t)) => {
            let mut a = baseline_advisor.clone();
            a.dynamic_skip_threshold = *t;
            a
        }
    };
    config.advisor = active_advisor;

    // run_k 実行 (既存)
    let result = suite.run_k(&backend, &config, persistent_store, ...).await?;

    // restore (defensive、scope-end でも可だが explicit に記述)
    config.advisor = baseline_advisor;

    // experiment.rs::Experiment::from_multi_results(...) で SQLite/TSV 永続化 (既存)
}
```

**注**: `EventStore::verification_success_rate` は variant 内部で透過的に発動 (skip 判定は項目 210 ロジック)。本 hook は単に AdvisorConfig 切替のみ。

### 4.4 verify_skip 統計収集 (TSV +1 列)
`src/agent/benchmark.rs::run_k` の per-cycle 集計に追加:
```rust
pub struct MultiRunBenchmarkResult {
    // 既存 14 fields (項目 200 + 209 ベース)
    pub verify_skip_count: i64,       // 新規: cycle 内で AdvisorSkip 発火回数
    pub verify_skip_rate: Option<f64>, // 新規: skip / (skip + fire) 比 (None if fire=0)
}
```

集計ロジック:
- `EventStore::count_by_type("audit.advisor_skip", session_ids)` で skip 数取得 (項目 209 trait method)
- `EventStore::count_by_type("audit.advisor_fire", session_ids)` で fire 数取得 (項目 210 で既に emit 済)
- variant cycle 単位で集計、SQLite `experiments` table に列追加 (V9 → V10、`ALTER TABLE ADD COLUMN`)

### 4.5 ACCEPT 判定基準 (Phase 5 専用)
既存 ACCEPT 判定 (`composite_score >= baseline + 0.005` AND `score_variance <= baseline * 1.5`) に加え:

**ConfigOverride variant 専用 secondary criterion**:
- (A) **Score-equivalent skip gain**: `composite_score >= baseline - 0.005` AND `verify_skip_rate >= 0.30`
- (B) **Pure score gain**: `composite_score >= baseline + 0.015` (prompt 系変異の閾値より高めに設定、+0.005 だと 1bit variance に埋没するリスク高)

(A) または (B) のいずれか満たせば **ACCEPT 候補**、両方 fail で REJECT。

**理由**: ConfigOverride は token 削減/latency 削減という副次効果も価値、prompt 変異と同基準では効果が見えづらい。(A) は Self-Verify Dilemma 論文の「過剰検証 skip による cost 削減 + score 維持」シナリオに対応。

### 4.6 Defaults 化判定 (Phase 5 後の項目 211 候補)
Phase 5 で 3 variant のうち最良 1 件が ACCEPT なら:
1. 本 session で defaults 化はせず (項目 210 と同じ慎重さ)
2. handoff に「★★ defaults 化検討: threshold=0.X (Lab v16 変異 ID Y) で再検証 ~3h」を起票
3. 別 session で **Lab v16 (core 22 × 5 cycle paired t-test)** を実施、+0.015 維持確認後 default 化
4. CLAUDE.md 派生デフォルト化変異リストに追加 (項目 10/47/50/136 系譜)

## 5. TDD strict 5 phase
### Phase 1 — Red
新規 test 6 件 (`src/agent/experiment.rs::tests`):
1. `t_hypothesis_label_prompt` — `PromptSuffix { category: "tool_thinking", .. }.label() == "prompt_tool_thinking"`
2. `t_hypothesis_label_config_override` — `ConfigOverride(AdvisorThreshold(0.4)).label() == "config_advisor_threshold_0.40"`
3. `t_hypothesis_fingerprint_distinct` — 同一 threshold は同 fingerprint、異なる threshold は別 fingerprint
4. `t_generate_includes_config_when_env_set` — `BONSAI_LAB_CONFIG_VARIANTS=1` で 3 件 ConfigOverride 含有
5. `t_generate_excludes_config_when_env_unset` — env unset で 0 件 ConfigOverride
6. `t_run_experiment_loop_applies_threshold_override` — Mock backend + ConfigOverride(0.3) で `AdvisorConfig::dynamic_skip_threshold` が 0.3 に設定された状態で run_k 呼出 (signature mock で fnpointer 検証)

期待: 全 6 fail (compile error 含む)、commit `test(self-verify): Phase 5 Red — Hypothesis enum 化 6 test`

### Phase 2 — Green
1. `experiment.rs::Hypothesis` enum 化 (PromptSuffix + ConfigOverride 変換、`From<String>` impl)
2. `ConfigDelta::AdvisorThreshold(f64)` variant
3. `Hypothesis::label()` / `Hypothesis::fingerprint()` 実装
4. `HypothesisGenerator::generate` 拡張 (env-gated config seeding)
5. `run_experiment_loop` config override hook (clone + apply + restore)
6. 既存 caller 24 箇所 — `From<String>` 経由で transparent 移行確認

期待: 1075 + 6 = **1081 passed** / clippy 0 / fmt 0、commit `feat(self-verify): Phase 5 Green — Hypothesis enum + config override`

### Phase 3 — Refactor
1. `Hypothesis::label` 内の `format!` を `Display` impl に統一
2. `HypothesisGenerator::generate` の env 読込を `lazy_static`/`OnceLock` cache 化
3. `ConfigDelta` への docstring (Phase 5 用、後続 plan で variant 拡張可能性記述)
4. `MultiRunBenchmarkResult::verify_skip_count` field 追加 + `compute_skip_stats` helper

期待: 1081 passed 維持、commit `refactor(self-verify): Phase 5 Refactor`

### Phase 4 — Smoke (G-4 部分)
```bash
# G-4a: env unset = 既存挙動 (後方互換)
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 2>&1 | tee /tmp/sv5_smoke_a.log
# 期待: smoke 7 task 完走、ConfigOverride 0 件、既存挙動

# G-4b: env=1 + 1 experiment で seeding 確認
BONSAI_LAB_CONFIG_VARIANTS=1 BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 1
# 期待:
#   - generate() で 3 件 ConfigOverride 含有 (log: "Hypothesis variants: 1 prompt + 3 config")
#   - 1 experiment で AdvisorThreshold variant が選択されたら、AuditAction::AdvisorSkip log emit
#   - TSV に verify_skip_count 列が追加済 (column 14)
```

判定:
- ✅ G-4a: 後方互換 1075 passed 維持、ConfigOverride 0 件
- ✅ G-4b: ConfigOverride 3 件 seeding、TSV 列追加、AdvisorSkip log >= 1

### Phase 5 — Effectiveness 検証 (実機 Lab v16、~3h)
```bash
# 完全 Lab v16 cycle (core 22 × k=3 × 4 hypotheses [baseline + 3 threshold] ≈ 75 min)
BONSAI_LAB_CONFIG_VARIANTS=1 BONSAI_BENCH_TIER=core ./target/release/bonsai \
  --lab --lab-experiments 3 --lab-k 3 2>&1 | tee /tmp/sv5_lab_v16.log
```

判定 (3 variant のいずれか 1 件以上 ACCEPT で Phase 5 SUCCESS):
- (A) Skip-equivalent: `score >= 0.7560 - 0.005 = 0.7510` AND `verify_skip_rate >= 0.30`
- (B) Pure gain: `score >= 0.7560 + 0.015 = 0.7710`
- 両方 fail なら全 3 variant REJECT (項目 207 系譜)、handoff に「production default 0.0 維持確定」記録

### Phase 6 — Commit + handoff + CLAUDE.md
4-5 commits:
1. `test(self-verify): Phase 5 Red — Hypothesis enum 6 test`
2. `feat(self-verify): Phase 5 Green — Hypothesis enum + config override`
3. `refactor(self-verify): Phase 5 Refactor`
4. `feat(experiment): Phase 5 Lab v16 — verify_skip TSV + V10 migration`
5. `docs(claude.md): 項目 211 — Self-Verify Phase 5 effectiveness 結果`

## 6. API 影響
| API | 変更 | 後方互換 |
|---|---|---|
| `Hypothesis` | `String` → `enum` | ✅ `From<String>` で transparent |
| `ConfigDelta` enum | 新規 | — |
| `HypothesisGenerator::generate` | 戻り値 `Vec<Hypothesis>` (signature 不変) | ✅ |
| `MultiRunBenchmarkResult` | `verify_skip_count`/`verify_skip_rate` field 追加 | ✅ serde default + skip_if_none |
| `AdvisorConfig` | 既存 (項目 210) | ✅ 変更なし |
| SQLite | V9 → V10 (`verify_skip_count INTEGER`/`verify_skip_rate REAL`) | ✅ ALTER TABLE additive |
| TSV | 13 → 14 列 (`verify_skip_count`) | ⚠️ 末尾追加で前 13 列 semantic 不変 |
| env | `BONSAI_LAB_CONFIG_VARIANTS=1` 新規 | ✅ default unset で既存挙動 |

**signature 変更ゼロ** — 既存 caller 24 箇所無変更 (項目 205/209 同様 additive)。

## 7. Risks
| # | risk | severity | mitigation |
|---|------|----------|------------|
| R1 | 1bit variance で 3 variant 全 REJECT (項目 207 と同パターン) | HIGH | (A) skip-equivalent secondary criterion で救済余地、Phase 5 後の handoff で「天井 6 連続」evidence として CLAUDE.md 整理可、本機構自体は production code に既存 (項目 210) ため負債ゼロ |
| R2 | 3 variant 同時投入で Lab cycle 時間 +50% (75 min) | MEDIUM | core 22 で k=3 固定、`--lab-experiments 3` で必要最小、project-time のみ |
| R3 | env-gated config seeding が prompt-only Lab と挙動分岐 | MEDIUM | Phase 4 G-4a/b 両方検証、既存 default 経路 1075 passed 維持必須 |
| R4 | `AdvisorConfig::clone` cost (cycle 毎) | LOW | AdvisorConfig は ~5 field の小型 struct、negligible |
| R5 | restore-after-variant の defensive code が複雑化 | LOW | scope-end 自動で済むが explicit に記述、test 6 で fnpointer 検証 |
| R6 | TSV 14 列化で外部解析スクリプト破損 | LOW | 末尾追加で robust、column header 駆動 reader OK |
| R7 | SQLite V10 migration 失敗 (項目 209 で V9 既存) | LOW | `ALTER TABLE IF NOT EXISTS` パターン、Phase 2 migration test |
| R8 | Hypothesis enum 化が既存 prompt-only Lab v15 retrospective 解析を破る | LOW | 過去 Experiment SQLite row は `hypothesis_text TEXT` 列、enum 化後も `From<String>` で読込互換 |

## 8. Quality Gates
- **G-1 Phase 1 Red**: 6 新規 test compile error or 全 fail
- **G-2 Phase 2 Green**: 6 新規 test PASS + 1075 → 1081 passed + clippy 0 + fmt 0
- **G-3 Phase 3 Refactor**: docstring 完備、Display impl 統一、退行ゼロ
- **G-4 Phase 4 Smoke**:
  - G-4a: 既存経路 (env unset) 1075 維持
  - G-4b: env=1 で 3 件 ConfigOverride seeding + TSV 14 列 + AdvisorSkip log emit
- **G-5 Phase 5 Effectiveness (実機 Lab v16)**: 3 variant のいずれか (A)skip-equivalent or (B)pure gain で ACCEPT
- **G-6 Final**: handoff 起票 + CLAUDE.md 項目 211 + 5 commits

G-1〜G-4 で merge 可、G-5 結果で項目 211 内容が分岐 (success/all-reject)。

## 9. 見積もり
| Phase | 内容 | 時間 |
|-------|------|------|
| Phase 0 | review + plan 詳細読込 | 0.2h |
| Phase 1 | Red — 6 test 追加 | 0.4h |
| Phase 2 | Green — Hypothesis enum + override hook | 1.0h |
| Phase 3 | Refactor — Display impl + helper | 0.3h |
| Phase 4 | Smoke G-4a/b 2 段 | 0.5h |
| Phase 5 | 実機 Lab v16 (core 22 × k=3 × 4 hyp) | 1.5h (実機 wall ~75 min) |
| Phase 6 | handoff + CLAUDE.md + 5 commits | 0.6h |
| **合計** | | **~4.5h ≈ 0.5 day** |

注: 項目 210 plan の「Phase 5 別 session ~3h」より +1.5h は Hypothesis enum 化と TSV 列追加の infrastructure 部分。

## 10. 次の段階 (Phase 5 後)
### Phase 5 SUCCESS の場合 (最良 variant ACCEPT)
1. handoff に「★★ defaults 化検討: threshold=0.X paired t-test ~3h」起票
2. 別 session で Lab v16 (core 22 × 5 cycle paired) で +0.015 維持確認
3. confirm 後 `AdvisorConfig::default()` で `dynamic_skip_threshold = 0.X` に変更 (CLAUDE.md 派生デフォルト化変異リストに追加)

### Phase 5 ALL-REJECT の場合 (項目 207 系譜継続)
1. 「天井 6 連続確定」を CLAUDE.md 項目 211 に明記
2. **次の構造変異候補**: ERL Heuristics Pool (`erl-heuristics-pool-impl.md` 着手)、AgentFloor 6-tier 評価軸 (`agentfloor-tier-eval-impl-v2.md` 着手)
3. 項目 210 production code は **default 0.0 のまま負債ゼロ**、knowledge として CLAUDE.md 残置

### 拡張候補 (本 plan scope 外)
- ConfigDelta variant 拡充: `AdvisorMaxUses(usize)`, `MaxIterations(u32)`, `CompactionLevel(u8)`
- `BONSAI_LAB_CONFIG_VARIANTS` を sub-field 別 toggle 化 (`=advisor` / `=compaction` / `=all`)
- HypothesisGenerator LLM call で config 系も自動生成 (現行は固定 3 件)

## 11. Quick Start
```bash
# 0. caller 全網羅 (Hypothesis 既存 String 用法)
rg -n "Hypothesis|hypothesis_text|generate_hypothesis" src/

# 1. Phase 1 Red
$EDITOR src/agent/experiment.rs  # tests/phase5_red_tests.rs 追加
rtk cargo test --lib experiment_phase5  # compile error or 6 fail
git add -A && git commit -m "test(self-verify): Phase 5 Red — Hypothesis enum 6 test"

# 2. Phase 2 Green
$EDITOR src/agent/experiment.rs  # Hypothesis enum + ConfigDelta + From<String>
$EDITOR src/agent/experiment.rs  # HypothesisGenerator::generate env-gated
$EDITOR src/agent/experiment.rs  # run_experiment_loop config override hook
rtk cargo test --lib  # 1081 passed
git add -A && git commit -m "feat(self-verify): Phase 5 Green — Hypothesis enum + config override"

# 3. Phase 3 Refactor
$EDITOR src/agent/experiment.rs  # Display impl + helpers
rtk cargo clippy --lib -- -D warnings && rtk cargo fmt --check
git add -A && git commit -m "refactor(self-verify): Phase 5 Refactor"

# 4. Phase 4 Smoke
rtk cargo build --release
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 2>&1 | tee /tmp/sv5_a.log  # G-4a
BONSAI_LAB_CONFIG_VARIANTS=1 BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 1 2>&1 | tee /tmp/sv5_b.log  # G-4b

# 5. Phase 5 実機 Lab v16
BONSAI_LAB_CONFIG_VARIANTS=1 BONSAI_BENCH_TIER=core ./target/release/bonsai \
  --lab --lab-experiments 3 --lab-k 3 2>&1 | tee /tmp/sv5_lab_v16.log

# 6. Commit + handoff + CLAUDE.md 項目 211
```

## 12. 参考
- arxiv 2602.03485 Self-Verification Dilemma (2026-02)
- 項目 210 self-verify-dilemma-impl.md (Phase 1-4 完了)
- 項目 209 event-repository-trait-impl.md (`verification_success_rate` trait method)
- 項目 207 lab-v15 long run (天井 5 連続 evidence)
- 項目 200 beyond-pass1-rdc-vaf-impl.md (TSV/SQLite 拡張パターン)
- 項目 205 agenther-option-a-migration.md (signature 必須化と本 plan additive 比較)
- CLAUDE.md 項目候補: 211 (本 plan 完遂時)
- 関連後続 plan: `erl-heuristics-pool-impl-v2.md` / `agentfloor-tier-eval-impl-v2.md` (構造変異 evidence 共有)
