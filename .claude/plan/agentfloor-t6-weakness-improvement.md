# Plan: AgentFloor T6-LongHorizon Weakness Improvement — research + 3 候補比較

> **状態**: planning-only / read-only research (2026-05-21 起票)、production code 変更ゼロ、推奨度 ★★★
>
> **起点**: 項目 223/224 で確立した Bonsai-8B 能力プロファイル (T1=0.68 / T2=0.52 / T3=0.77 / T4=0.64 / T5=0.70 / **T6=0.47 weakest**) のうち、T6-LongHorizon は paper_baseline 0.30 を +0.17 上回るが絶対値最弱、composite_score を底上げできる **最大 leverage 点**。
>
> **falsifiable hypothesis**: 推奨案実装後、AgentFloor T6 5-task suite paired Δscore ≥ **+0.08** (0.47→0.55、目標 17% relative improvement)、かつ T1-T5 への regression Δ ≤ -0.01。

## Task Type

- [x] Docs / Research

---

## §1. T6 弱点の根本原因仮説

### §1.1 T6 task 内容 (確定 evidence)

`src/agent/benchmark.rs:1349-1413` から T6 5 task fixture を grep + 直読:

| Task ID | 操作内容 | max_iter | expected_tools | expected_keywords |
|---------|---------|----------|---------------|-------------------|
| `lh_plan_refactor_5files` | RepoMap → file_read×3+ → リファクタ計画提示 | 10 | repo_map, file_read | リファクタ, 計画 |
| `lh_test_red_green` | benchmark.rs 読取 → test 2件提案 | 8 | file_read | test, assert |
| `lh_dependency_chain` | mod.rs 読取 → 各 module file_read → 依存グラフ | 10 | file_read | 依存, モジュール |
| `lh_plan_then_revise` | Cargo.toml 読取 → 計画 → リスク改訂版 | 8 | file_read | 計画, 改訂, リスク |
| `lh_multi_modal_audit` | shell(git log) + ファイル数調査 + Cargo.toml | 10 | shell, file_read, git | コミット, 依存, レポート |

加えて default 40 task のうち T6-tag 付与済 2 件 (`benchmark.rs:1758,1769`): `tool_chain_10steps`, `implement_50steps`。

**T6 共通構造** = (a) `max_iterations ≥ 8` (b) 複数 tool sequential (c) 中間状態の蓄積 (d) 最終 keyword 2-3 個 の総合評価。

### §1.2 T6 失敗 patterns

| Pattern | observed | evidence |
|---------|----------|---------|
| **P1: 中間 file_read 失敗で計画 abort** | 高頻度 | `lh_plan_refactor_5files` で 5 file 中 2-3 件目で iteration 切れ |
| **P2: keyword 不一致 (計画≠実装)** | 中頻度 | "リファクタ"/"計画" 出力するが具体性なし |
| **P3: iteration_used 上限張付** | 高頻度 | `efficiency=0` で score 内訳 10% を全失 |
| **P4: tool_chain misorder** | 低頻度 | `expected_tools` 不一致 |
| **P5: 計画と検証の分離不可** | 中頻度 | `lh_plan_then_revise` の "改訂" 検出不能 |

### §1.3 仮説 4 件

**H1: Middle-step drift** — 5+ step 中盤で context dilution、ERL+Cerememory が部分 cover、残 gap = T6 success path prior 注入不足
**H2: Long context attention dilution** — file_read 5+ で 15-25k tokens 蓄積、項目 248 Phase 5 完遂後も T6 specific tune 残
**H3: Tool chaining p^n catastrophe** — p^5 = 0.44 ≈ 観測値 0.47 と整合、Core ハーネスは個別 step rate 補強済だが chain 全体未対処
**H4: 1bit quantization precision floor** — Critic Phase 1 (項目 226) で外部補強候補、Scaffolding > Model 原則の境界

### §1.4 仮説優先度

| 仮説 | 既存対処度 | 残 gap | cost | 改善期待 |
|------|-----------|--------|------|---------|
| H1 (drift) | 中 | 中 | 中 | +0.03-0.05 |
| H2 (dilution) | 高 (項目 248 完遂) | 中 | 中 | +0.04-0.06 |
| **H3 (p^n cliff)** | **低** | **大** | **中** | **+0.05-0.08** |
| H4 (precision) | 低 | 大 | 高 | +0.03-0.07 |

**最重要 = H3**: score function (`benchmark.rs:136-143` 40% completed + 30% tools + 20% keyword + 10% efficiency) の `completed=true/false` binary cliff が p^n 失敗の主因。

---

## §2. 3 案 (+ 新規 1 案) 比較

### 案 A: T6 特化 prompt template (低コスト)

T6 検出時に directive inject:
- "Before any tool call, write step-by-step plan as numbered list."
- "After every 3rd tool call, restate plan progress in 1 sentence."
- "If 2+ consecutive tool failures, stop and revise plan."

env: `BONSAI_T6_PROMPT_AUGMENT=1`

| 軸 | 評価 |
|----|------|
| 実装工数 | **低** (~3-4h) |
| T6 改善期待 | 中 (+0.03-0.05) |
| 他 tier 影響 | **無** (T6 only scope) |
| リスク | 低 |
| Rollback | **最高** (env flip) |

### 案 B: T6 用 advisor injection 強化 (中コスト)

`capability_tier == T6 AND step_count >= 3` で advisor hint inject。

| 軸 | 評価 |
|----|------|
| 実装工数 | 中 (~5-7h) |
| T6 改善期待 | 中 (+0.03-0.05) |
| リスク | 中 (context bloat 懸念) |

### 案 C: T6 task subdivision (高コスト)

T6 input を sub-task 列に decompose、aggregator step で keyword 統合。

| 軸 | 評価 |
|----|------|
| 実装工数 | **高** (~12-15h) |
| T6 改善期待 | **高** (+0.06-0.10、H3 直接対処) |
| リスク | **高** (Lab baseline 互換破壊) |

### 案 D (新規): Memory-augmented T6

項目 228 KG fusion 活用、T6 検出時に「過去 T6 success trajectory」を KG 検索 top-3 inject。env: `BONSAI_T6_MEMORY_AUG=1`。

| 軸 | 評価 |
|----|------|
| 実装工数 | 中-高 (~8-10h、項目 228 流用) |
| T6 改善期待 | **高** (+0.05-0.08、H1+H4 部分対処) |
| 他 tier 影響 | **無** |
| リスク | 中 (cold start 問題) |

### CCG synthesis & 推奨

- **Codex**: 案 C (subdivision) 最大改善期待だが実装リスク高
- **Gemini**: 案 A 即適用可、案 D で長期改善期待
- **Claude final = 案 A (Phase 1) → 案 D (Phase 2) sequential**:
  1. Phase 1 (案 A): 3-4h、+0.03-0.05 早期 ROI
  2. Phase 2 (案 D): 案 A 確証後、+0.05-0.08 加算 (T6 0.47 → 0.55-0.60)

**案 C は保留**: Lab baseline 破壊リスクが Scaffolding > Model 原則と非整合。

---

## §3. TDD strict 3-phase outline (推奨案 = 案 A)

### Phase 1 Red (~1h)

4 failing test:
- `t_t6_prompt_augment_env_default_off`
- `t_t6_prompt_augment_env_on_t6_task_injected`
- `t_t6_prompt_augment_env_on_non_t6_task_not_injected`
- `t_t6_prompt_augment_includes_three_directives`

新規 module: `src/agent/t6_prompt_augment.rs` (~80 LOC)
- `pub fn is_t6_prompt_augment_enabled() -> bool`
- `pub fn t6_augment_directive() -> &'static str`
- `pub fn augment_system_prompt(system: &str, task_tier: CapabilityTier) -> String`

### Phase 2 Green (~2h)

- env getter parse (`BONSAI_T6_PROMPT_AUGMENT`、default false)
- directive 固定 text 定義 (3 件)
- `augment_system_prompt()` 本実装
- `src/agent/context_inject.rs` の `inject_memory_blocks` 直後に env-gated hook 1 行

### Phase 3 Refactor (~30min)

- cargo fmt + clippy clean
- module rustdoc (env value + directive 設計 + 案 D Phase 2 hook 点)
- `cargo test --lib` 1356 → 1360 (+4)

### Phase 4 wiring test (~30min、optional)

- integration test `t_inject_memory_blocks_with_t6_augment_env_on_includes_directive`

---

## §4. Phase 4 Smoke acceptance

### G-T6-1: T6 baseline 再計測 (~15 min)
```bash
BONSAI_BENCH_LADDER=1 BONSAI_T6_PROMPT_AUGMENT=0 BONSAI_LAB_TEMP=0 \
  ./scripts/agentfloor_smoke.sh --t6-only --k 3
```
**ACCEPT**: T6 = 0.44-0.50、weakest_tier == T6 維持

### G-T6-2: 案 A 適用後 (~15 min)
**ACCEPT**: T6 ≥ 0.52 (最低 +0.05)、stretch 0.55、cohen's d ≥ 0.3

### G-T6-3: 他 tier regression 確認 (~30 min)
**ACCEPT**: T1-T5 全 tier で Δ ≥ -0.01

### G-T6-Lab (オプション、~5h)
Lab v22 metric paired 検証。**ACCEPT**: p < 0.05 AND dz ≥ 0.5

---

## §5. Rollback strategy

### 5.1 完全 revert
- env unset → augment 経路完全 skip
- module 残置可 (env-gated で不活性)

### 5.2 段階 rollback
- 改善 < +0.03 → env default off で merge
- regression → `cfg!(test)` gate で隔離

### 5.3 AgentFloor baseline 維持
- env unset 時の T6 score が項目 224 baseline (0.47) ± 0.03 内に維持を test で永続確証

---

## §6. 既存資産との整合性

- **項目 217-219 Cerememory**: 補完関係、案 D で T6-scoped retrieval 統合可
- **項目 213 ERL**: orthogonal axis、Lab paired で並列計測可
- **項目 228 KG fusion**: 案 D Phase 2 で直接活用
- **項目 248 Dynamic Budget Phase 5 (完遂)**: H2 直接対処、本 plan は H4+H1 部分対処、2x2 design 可
- **項目 226 G1 Critic**: stack 可、案 A ACCEPT 後に併用 smoke 検討
- **項目 256 Z-4 Layer Linter**: agent layer 配置、SIZE 80 LOC で whitelist 追加不要

---

## §7. 次手

1. **plan review** (~30min): 案 A 推奨採用判断
2. **案 A Phase 1-3** (~3-4h): TDD strict
3. **G-T6-1/2/3 smoke** (~1h)
4. **案 A ACCEPT → 案 D Phase 2 plan 起票**
5. **案 A REJECT → 案 B or C re-evaluate**

---

## 付録 A: production code touch 範囲 (案 A 採用時)

- **新規 file** (1 件): `src/agent/t6_prompt_augment.rs` (~80 LOC)
- **編集 file** (2 件):
  - `src/agent/context_inject.rs` (+1 hook 行)
  - `src/agent/mod.rs` (+1 行)
- **test**: +4 (Phase 1) + 1 (Phase 4)
- **env var**: `BONSAI_T6_PROMPT_AUGMENT=1`、default false

---

## 付録 B: research メタデータ

- 起票日: 2026-05-21
- 起票 trigger: ecc:planner agent (G-RT2 smoke 並列実行中)
- 参照 source: `benchmark.rs:1349-1413` / `experiment.rs` grep 19件 / `experiment_log.rs` grep 3件
- 参照 plan: 5 件 (重複なし、glob 0 件確証)
- production code touch: **ゼロ**
