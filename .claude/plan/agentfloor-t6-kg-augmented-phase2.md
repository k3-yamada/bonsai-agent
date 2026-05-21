# Plan: AgentFloor T6 KG-Augmented Retrieval — 案 D Phase 2 (Memory-Augmented T6)

**状態**: planning-only (2026-05-22 起票)、production code touch ゼロ、推奨度 ★★★
**起点**:
- 項目 261 案 D Phase 2 候補 (`.claude/plan/agentfloor-t6-weakness-improvement.md` §2.4): 項目 228 KG fusion (R@10=98.6) 活用、T6 task 検出時に過去 T6 success trajectory を KG 検索 top-3 を context 注入
- 項目 262 案 A ACCEPT (2026-05-22、T6 score 0.7671 → 0.8778 +14.4%)、案 D は案 A への加算
- 期待: T6 0.78 → 0.85+ (案 A+D combined +0.13pt)

**falsifiable hypothesis**: 推奨案実装後、T6 paired Δscore ≥ **+0.05** (G-T6-2 0.8778 vs 案 D ON)、stretch 0.85+。T1-T5 regression Δ ≤ -0.01。

## §1. Motivation + acceptance criteria

### 1.1 案 A 後の natural progression

案 A は T6 LongHorizon task 専用に 3 件の **固定 directive** (step-by-step plan / restate progress / revise plan) を system prompt 末尾 append、p^n cliff 構造的対処、+14.4% で ACCEPT。directive は static で、**過去 success trajectory の具体的 tool-chain pattern** を活用できていない。

案 D Phase 2 は項目 228 KG fusion を活用し、T6 success trajectory を KG 検索で発見し context 注入する **dynamic augmentation**。

### 1.2 仮説マッピング

| 仮説 | 案 A (項目 262) | 案 D Phase 2 (本 plan) |
|------|---------|--------|
| H1 (middle-step drift) | 部分対処 | **直接対処** (success path prior) |
| H2 (long context dilution) | 部分対処 (項目 248 完遂) | 軽微 (+500-1500 tokens) |
| H3 (p^n catastrophe) | **直接対処** | 部分強化 (具体 tool sequence) |
| H4 (1bit precision floor) | 軽微 | **部分対処** |

案 A 単独 +0.11pt 観測。案 D 単独想定 +0.05-0.08pt、stack 効果は diminishing return 込み **+0.04-0.07pt** 予測。

### 1.3 ACCEPT 条件

**Phase 4 Smoke**:
- G-T6-D-2 T6 score ≥ **0.83** (baseline 0.8778 +0.05)、stretch 0.85+
- G-T6-D-3 T1-T5 全 tier Δ ≥ -0.01
- cohen's d ≥ 0.3

**Phase 5 Lab v22 paired (~5-8h、任意)**:
- p < 0.05 AND dz ≥ 0.5

## §2. 3 案比較

### 案 D-1: cross-session top-K

過去全 session の T6 success trajectory (score ≥ 0.7 の AssistantMessage event 列) を KG 化、検出時に `KnowledgeGraph::bfs_bidirectional` で top-3 inject。

**実装**: `trajectory:t6:{task_id}:{step}` 形式 node 追加、`memory/search.rs::graph_search` pattern 継承。

| 軸 | 評価 |
|----|------|
| 実装工数 | 中-高 (~10-13h) |
| 改善期待 | **高** (+0.05-0.08) |
| Cold start | **大** (N=20+ 蓄積前 no-op) |
| Rollback | 高 |

### 案 D-2: in-session previous T6

`Arc<Mutex<Vec<T6SuccessRecord>>>` で session scope 保持、後続 T6 task に top-3 inject、session 終了で破棄。

| 軸 | 評価 |
|----|------|
| 実装工数 | **低** (~4-6h) |
| 改善期待 | 低-中 (+0.02-0.04) |
| Cold start | 中 (2 件目以降のみ) |
| Rollback | **最高** |

### 案 D-3: hybrid

案 D-1 + 案 D-2 + merge logic。env `BONSAI_T6_MEMORY_AUG_MODE=cross|in_session|hybrid`、default hybrid。

| 軸 | 評価 |
|----|------|
| 実装工数 | 高 (~12-15h) |
| 改善期待 | **最高** (+0.06-0.09) |
| Cold start | 低 |
| Rollback | 高 |

### CCG synthesis & 推奨

- **Codex**: 案 D-3 hybrid (項目 228 投資回収最大化)
- **Gemini**: 案 D-2 in-session (早期 ROI、cold start 回避)
- **Claude final = 案 D-3 を最終目標、Phase 2a (案 D-2) から段階 delivery**:
  - **Phase 2a (案 D-2、~4-6h)**: in-session top-2、smoke +0.02-0.04 確証
  - **Phase 2b (案 D-1 → D-3、~6-9h)**: Phase 2a ACCEPT 後、cross-session 追加で hybrid 化

## §3. TDD strict 3-phase outline (Phase 2a)

### Phase 1 Red (~1.5h)

新規 module: `src/agent/t6_memory_aug.rs` (~120 LOC)

```rust
pub struct T6SuccessRecord {
    pub task_id: String,
    pub input_keywords: Vec<String>,
    pub tool_chain: Vec<String>,
    pub final_keywords: Vec<String>,
    pub score: f64,
}

pub fn is_t6_memory_aug_enabled() -> bool;
pub fn t6_memory_aug_mode() -> T6AugMode;
pub fn jaccard_overlap(a: &[String], b: &[String]) -> f32;
pub fn pick_top_k_in_session(history: &[T6SuccessRecord], current_input: &str, k: usize) -> Vec<T6SuccessRecord>;
pub fn format_aug_block(records: &[T6SuccessRecord]) -> String;
pub fn augment_system_prompt_with_memory(system: &str, task_tier: CapabilityTier, history: &[T6SuccessRecord], current_input: &str) -> String;
```

5 failing test:
1. `t_t6_memory_aug_env_default_off`
2. `t_t6_memory_aug_mode_default_in_session_in_phase_2a`
3. `t_pick_top_k_returns_highest_jaccard_first`
4. `t_format_aug_block_contains_three_required_markers`
5. `t_augment_system_prompt_with_memory_no_op_when_non_t6`

cross-test env 競合: `ENV_LOCK` (項目 262 同形式)。

### Phase 2 Green (~2h)

- env getter 2 件本実装
- `jaccard_overlap` 素 Jaccard
- `pick_top_k_in_session` sort_by + truncate
- `format_aug_block` 固定 prefix `[T6 Past Success Examples]` + tool_chain + final_keywords、上限 1500 tokens
- `augment_system_prompt_with_memory` env+tier ガード後 append
- 5 test PASS

### Phase 3 Refactor (~1h)

- helper extraction: `tokenize_task_input` / const `T6_AUG_BLOCK_HEADER` / `T6_AUG_MAX_TOKEN_BUDGET = 1500`
- rustdoc 強化 (起点 + env + Phase 2b hook 点)
- clippy / fmt clean

### Phase 4 wiring (~1h)

`src/agent/benchmark.rs::BenchmarkSuite::run_with_multi_config()` の `augment_system_prompt` 直後に env-gated で `augment_system_prompt_with_memory` 1 行追加。

`t6_history: Vec<T6SuccessRecord>` を BenchmarkSuite field 追加、T6 task 完了時 score ≥ 0.7 で append。

integration test 1 件: `t_benchmark_suite_t6_memory_aug_appends_history_in_session`

## §4. Phase 4 Smoke acceptance

### G-T6-D-1: case A only baseline 再計測 (~15 min)

```bash
BONSAI_BENCH_LADDER=1 BONSAI_T6_PROMPT_AUGMENT=1 BONSAI_T6_MEMORY_AUG=0 \
  BONSAI_LAB_TEMP=0 ./scripts/agentfloor_smoke.sh --t6-only --k 3
```

**ACCEPT**: T6 = 0.85-0.91 (項目 262 baseline 再現)。

### G-T6-D-2: 案 D-2 in-session ON (~15-20 min)

```bash
BONSAI_T6_PROMPT_AUGMENT=1 BONSAI_T6_MEMORY_AUG=1 \
  BONSAI_T6_MEMORY_AUG_MODE=in_session BONSAI_LAB_TEMP=0 \
  ./scripts/agentfloor_smoke.sh --t6-only --k 3
```

**ACCEPT**: T6 ≥ **0.83**、stretch 0.85+、cohen's d ≥ 0.3。

### G-T6-D-3: T1-T5 regression check (~30 min)

```bash
BONSAI_T6_MEMORY_AUG=1 BONSAI_T6_MEMORY_AUG_MODE=in_session \
  ./scripts/agentfloor_smoke.sh --all-tiers --k 3
```

**ACCEPT**: T1-T5 全 tier Δ ≥ -0.01。

### G-T6-D-Lab (~5-8h、任意)

Lab v22 paired。**ACCEPT**: p < 0.05 AND dz ≥ 0.5。

## §5. 既存資産との整合

### 5.1 項目 228 KG fusion (R@10=98.6) 流用

Phase 2b で `KnowledgeGraph::neighbors` 継承、`trajectory:t6:*` node retrieval。RRF 3-stream には混ぜない (T6-scoped separate pass)。

### 5.2 項目 261/262 案 A 統合

両 env 並列 ON で stack:
```
BONSAI_T6_PROMPT_AUGMENT=1   ← 案 A (項目 262)
BONSAI_T6_MEMORY_AUG=1       ← 案 D Phase 2a (本 plan)
BONSAI_T6_MEMORY_AUG_MODE=in_session
```

inject 順序: base → directive (案 A) → examples (案 D)。

### 5.3 項目 220-222 sqlite-vec wiring removal との関係

本 plan は **KG-only、sqlite-vec 非依存**。Phase 2a `Vec` in-memory、Phase 2b 既存 KG table additive INSERT のみ。

### 5.4 項目 217-219 Cerememory との関係

Phase 2b で活用 (decay + freshness + 7±2 cap)。Phase 2a は session scope で freshness 自明。

### 5.5 layer rules (Z-4 linter)

`src/agent/t6_memory_aug.rs` は agent layer、800 LOC 未満、eprintln 不使用で 4 軸全合格。

## §6. Rollback strategy

### 6.1 完全 revert

env unset で即 pass-through、module 残置可。

### 6.2 段階 rollback

- G-T6-D-2 改善 < +0.02 → env default off で merge、案 A 単独運用
- T1-T5 regression > 0.01 → T6 hard gate 追加
- Lab v22 REJECT → Phase 2b 投資保留

### 6.3 Forward compatibility

Phase 2b 移行は `_MODE=in_session` default 維持しつつ `cross_session|hybrid` opt-in 追加。

## §7. 次手 + action items

### 7.1 順序

1. plan review (~30 min)
2. Phase 2a Phase 1 Red (~1.5h)
3. Phase 2a Phase 2 Green (~2h)
4. Phase 2a Phase 3 Refactor (~1h)
5. Phase 2a Phase 4 wiring + integration test (~1h)
6. `cargo build --release` (~5 min)
7. G-T6-D-1/2/3 Smoke (~1.5h)
8. (任意) Lab v22 paired (~5-8h)

**計 (Phase 2a)**: **~7-8h impl + ~1.5h smoke**

### 7.2 risk register

| Risk | 確率 | 影響 | mitigation |
|------|------|------|----|
| in-session 1 件で diminishing return | 中 | 中 | §6.2 段階 rollback |
| context budget 超過で項目 248 衝突 | 低 | 中 | `T6_AUG_MAX_TOKEN_BUDGET = 1500` 厳守 |
| Jaccard token 衝突で無関連 record | 中 | 低 | length penalty / score threshold 0.3+ filter |
| Phase 4 wiring `&mut self` 連鎖 | 低 | 中 | 既存 mut 流用、専用 helper で隔離 |
| Lab 稼働中 `cargo build --release` 違反 | 低 | **高** | Smoke 前に Lab 稼働状況確認 |

### 7.3 follow-up plan

1. `agentfloor-t6-kg-augmented-phase2b-cross-session.md`
2. `agentfloor-t6-kg-augmented-phase2c-hybrid.md`
3. `agentfloor-t6-aug-cerememory-integration.md`

## 付録 A: production code touch 範囲 (Phase 2a)

- **新規 file** (1): `src/agent/t6_memory_aug.rs` (~120 LOC)
- **編集 file** (2):
  - `src/agent/benchmark.rs` (+1 hook + `t6_history` field + score >= 0.7 append ~5 行)
  - `src/agent/mod.rs` (+1 行)
- **test**: +5 (Phase 1) + 1 (Phase 4) = **+6**
- **env var**: `BONSAI_T6_MEMORY_AUG=1` + `_MODE=in_session|cross_session|hybrid`、default false

## 付録 B: research メタデータ

- 起票日: 2026-05-22
- 起票 trigger: ecc:planner agent (T6 案 A ACCEPT 後の案 D Phase 2 設計)
- 重複 plan 確証: glob 0 件
- 想定推定項目番号: 項目 264 候補

## 完了サマリー

- **plan path**: `/Users/keizo/bonsai-agent/.claude/plan/agentfloor-t6-kg-augmented-phase2.md` (~280 行)
- **推奨案**: 案 D-3 hybrid 最終目標、Phase 2a (案 D-2、~7-8h + ~1.5h smoke) から段階 delivery
- **期待改善幅**: T6 0.8778 → **0.83-0.85+** (案 A baseline からさらに +0.05 stretch、cumulative +0.07-0.08pt)
