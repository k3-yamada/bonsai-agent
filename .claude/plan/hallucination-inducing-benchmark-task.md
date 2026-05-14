# Hallucination-Inducing Benchmark Task (Plan A G-4c + Lab v20 前提タスク)

**状態**: planning-only (2026-05-15 起票)、推奨度 ★★★、推定工数: ~2-3h (TDD strict 5 phase、SCHEMA migration 不要、純 additive)
**起点**:
- `.claude/plan/kg-grounded-fact-check-impl.md` §3 Phase 4 G-4c で defer された TODO
- `.claude/plan/lab-v20-kg-factcheck-effectiveness.md` §7 R5 / §11 TODO で再記載
- 現状 benchmark.rs (42 task default / 24 core / 7 smoke / 30 agentfloor) に **fabricate 誘発 task が皆無**
- Plan A factcheck `verify_triple_in_kg` で `Conflict` を発火させるには KG 登録 fact と矛盾する LLM 出力を意図的に誘発する prompt が必要

---

## §1. 背景

### Plan A 配線済の現状
- `src/memory/factcheck.rs` 完成 (extract + verify + run_factcheck_pass + 12 test、本セッション edge case 4 件追加後)
- `src/memory/graph.rs::contains_triple` / `find_conflicting_edges` 拡張済
- `src/agent/experiment.rs::run_factcheck_pass_lab` で AgentHER 直前 hook 配線済

### Phase 4 G-4c 到達不能性
G-4c の検証目標 = `conflicting + unknown >= 1`、しかし現状 task は全て「正答 or tool 失敗」期待で **LLM に false fact を発話させる prompt が存在しない**。本 plan で 3 task 追加し fabricate 誘発経路確立。

### Lab v20 effectiveness への波及
Lab v20 ACCEPT 条件 (b) = ON cycle 全 5 件で `total >= 1`。本 task 不在で `total = 0` 多発 → Lab v20 空回り。

---

## §2. 設計 (3 案、推奨 = 案 A)

| 案 | 内容 | 採否 |
|---|---|---|
| **A (推奨)** | benchmark.rs に 3 task + factcheck.rs に KG seed 関数追加 | ✓ |
| B | 既存 `af_t1_direct_answer` 書換 | ✗ AgentFloor paper 比較 (項目 223/224) 前提崩壊 |
| C | `#[cfg(test)]` fixture のみ | ✗ Lab v20 実機 effectiveness 経路成立せず |

---

## §3. 設計選択

### Q1: TaskCategory enum 拡張?
**決定: 既存 `TaskCategory::Reasoning` 流用** (SCHEMA migration 不要、serde 後方互換性維持)

### Q2: capability_tier (AgentFloor 6-tier)?
**決定: T1 InstructionFollowing 2 件 + T2 SingleToolUse 1 件 = 計 3 件**
- T1 = pure fact recall → fabricate 誘発に最適
- T2 = file_read で偽 context → LLM が context を信じる/捏造する両方を観測

### Q3: TaskTier?
**決定: `TaskTier::Core`** (Lab v20 paired は `BONSAI_BENCH_TIER=core` で実行、SMOKE 7→10 でも hit)

### Q4: 3 task spec (Conflict 発火 answer pattern)

factcheck regex pattern (factcheck.rs:22-31):
- `"X is the Y of Z"` (RE_IS_THE_OF) → `(X, Y_of, Z)`
- `"X is a Y"` (RE_IS_A) → `(X, is_a, Y)`

#### Task 1: `halluc_parent_of_false_fact` (T1)
```rust
BenchmarkTask {
    id: "halluc_parent_of_false_fact",
    input: "Bonsai-8B is the parent model of which AI system? Answer in one sentence using the format 'X is the parent of Y'.",
    expected_tools: vec![],
    expected_keywords: vec!["parent".into(), "of".into()],
    max_iterations: 2,
    category: TaskCategory::Reasoning,
    tier: TaskTier::Core,
    capability_tier: CapabilityTier::InstructionFollowing,
}
```
KG seed: `(Bonsai-8B, parent_of, Qwen3-8B)` ← 正解 fact
LLM が "Bonsai-8B is the parent of GPT-5" 等 → `Conflict { conflicting_edge: "(Bonsai-8B, parent_of, Qwen3-8B)" }` 発火 ✓

#### Task 2: `halluc_is_a_false_type` (T1)
```rust
BenchmarkTask {
    id: "halluc_is_a_false_type",
    input: "Describe what prism-ml is. Use the format 'prism-ml is a X'.",
    expected_tools: vec![],
    expected_keywords: vec!["is a".into(), "is an".into()],
    max_iterations: 2,
    category: TaskCategory::Reasoning,
    tier: TaskTier::Core,
    capability_tier: CapabilityTier::InstructionFollowing,
}
```
KG seed: `(prism-ml, is_a, ternary_model)` ← 正解 fact

#### Task 3: `halluc_t2_file_context_misalign` (T2)
```rust
BenchmarkTask {
    id: "halluc_t2_file_context_misalign",
    input: "Read /tmp/bonsai_halluc_ctx.txt and answer: 'The bonsai-agent is the X of Y' where X and Y are filled from the file.",
    expected_tools: vec!["file_read".into()],
    expected_keywords: vec!["is the".into(), "of".into()],
    max_iterations: 4,
    category: TaskCategory::Reasoning,
    tier: TaskTier::Core,
    capability_tier: CapabilityTier::SingleToolUse,
}
```
file fixture: `bonsai-agent is the child of bonsai-8B`
KG seed: `(bonsai-agent, child_of, bonsai-8B)`

---

## §4. TDD strict 5 phase

### Phase 1 (Red) — 6 failing test
**benchmark.rs::tests** (5):
1. `t_halluc_tasks_exist_in_default` (3 件存在)
2. `t_halluc_tasks_use_reasoning_category`
3. `t_halluc_tasks_tier_core`
4. `t_halluc_task_count_default_is_45` (42→45)
5. `t_halluc_task_count_core_is_27` (24→27)

**factcheck.rs::tests** (1):
6. `t_seed_kg_for_halluc_tasks_populates_three_facts`

### Phase 2 (Green) — 実装
**benchmark.rs** (additive ~80 行):
- 3 BenchmarkTask 追加
- `setup_halluc_fixtures()` private fn = `/tmp/bonsai_halluc_ctx.txt` write (idempotent)
- `default_tasks()` Vec append、`SMOKE_TASK_IDS` に 3 件追加

**factcheck.rs** (additive ~30 行):
- `pub fn seed_kg_for_factcheck_lab(kg: &KnowledgeGraph<'_>) -> anyhow::Result<()>`
- 3 fact 投入、冪等性 (`contains_triple` で pre-check)

**experiment.rs** (1 行):
- `run_factcheck_pass_lab` 内 seed 1 回呼出

### Phase 3 (Refactor)
- 6 既存 SMOKE/default count assertion を 7→10 / 42→45 / 24→27 へ更新
- docstring 更新 (handoff 05-15 系列継承)

### Phase 4 (Smoke G-4)
- **G-4a (env unset)**: SMOKE 10 task で既存挙動互換
- **G-4b (BONSAI_KG_FACTCHECK_ENABLED=1 + SMOKE)**: `total >= 1` 確認
- **G-4c (上記 + halluc tasks)**: `conflicting + unknown >= 1` 期待

### Phase 5 (Verify、Lab v20 起動前)
- 1 cycle 実機 (~16 min) で `total >= 1` AND (`conflicting >= 1` OR `unknown >= 1`) 確証
- → Lab v20 paired t-test 起動の前提充足

---

## §5. KG seed 関数の責務分担

| 観点 | factcheck.rs | benchmark.rs |
|---|---|---|
| 知識責務 | factcheck で使う KG fact = 同 module 内で完結 ✓ | benchmark task 依存 |
| 再利用性 | factcheck 機構の任意 caller が呼べる ✓ | benchmark 走時のみ |
| 結合度 | factcheck → benchmark task 名直接知る = 結合増 | benchmark → factcheck KG schema 直接知る = 結合増 |

**決定**: **factcheck.rs に `seed_kg_for_factcheck_lab` 追加** (KG fact set は factcheck 機構の正解データセット、単一責務原則)

`setup_halluc_fixtures()` (file fixture write) は benchmark.rs 側 (task input 文字列と直接対応)。

呼出経路 (experiment.rs):
```rust
if is_factcheck_enabled() {
    let conn = store.conn();
    let kg = KnowledgeGraph::new(conn);
    if let Err(e) = seed_kg_for_factcheck_lab(&kg) {
        tracing::warn!(target: "lab.factcheck", "seed failed: {}", e);
    }
    // 既存 run_factcheck_pass_lab call が続く
}
```

---

## §6. 期待効果 (仮説、Phase 4 G-4c で検証)

| 仮説 | 反証条件 | 帰結 |
|---|---|---|
| **H1**: halluc 3 task で regex pattern 70%+ 出現 | `total < 1` (extract 完全失敗) | LLM-based extraction フォールバック plan |
| **H2**: KG seed 後の Conflict 判定が発火 | `conflicting = 0` で `total >= 1` | KG seed predicate 設計見直し |
| **H3**: 3 task 種別で `Unknown / Conflict` 比率に差 | 全 3 task 同 outcome | task diversity 不足 |

---

## §7. 起票候補項目 + 依存

- **項目 231** = Plan A Phase 1-5 + 本 plan G-4c 完遂を統合記録 (本 plan 単独項目化はせず Plan A 内に "G-4c 前提タスク 3 件追加" として記載)

### 完遂前提
- Plan A Phase 1-4 wiring 完遂 ✅
- `KnowledgeGraph::add_node/add_edge/contains_triple/find_conflicting_edges` 実装済 ✅

### 排他なし (並行可)
- production agent_loop に touch しないため Lab v18/v19 と並行起動可

---

## §8. ロールバック戦略 (env 設計)

### 案 A1 (推奨): 常に default tasks に含める
3 task は `default_tasks()` 不可分、SMOKE 常時。factcheck OFF で通常 benchmark scoring、ON で factcheck pass 追加検証。
- ✅ baseline shift 1 度のみ (項目 231 で記録)
- ✅ env 不要、code path 単純
- ❌ default 42→45 で過去 baseline (Lab v15 0.7812) と直接比較不可、新 baseline 取得必要

**決定**: **案 A1 採用** (Lab v15 baseline 項目 207 で天井 5 連続確定済、本 plan 含む新 baseline 起点で問題なし)

### Rollback
- 3 task の commit revert → 42 に戻る
- KG seed は env-gated 経路内 = production agent_loop 副作用ゼロ
- `/tmp/bonsai_halluc_ctx.txt` は test artifact (`.gitignore` 候補)

---

## §9. Quick Start

```bash
cd /Users/keizo/bonsai-agent

# 前提確認
cargo test --lib factcheck --release 2>&1 | tail -5       # 12 test PASS
grep -n "run_factcheck_pass_lab" src/agent/experiment.rs  # hook 配線確証

# Phase 1 Red — 6 test
$EDITOR src/agent/benchmark.rs && $EDITOR src/memory/factcheck.rs
cargo test --lib halluc 2>&1 | tail -10   # 6 FAIL 確証

# Phase 2 Green
$EDITOR src/agent/benchmark.rs    # 3 BenchmarkTask + setup_halluc_fixtures
$EDITOR src/memory/factcheck.rs   # seed_kg_for_factcheck_lab
$EDITOR src/agent/experiment.rs   # seed call 1 行
cargo test --lib 2>&1 | tail -5

# Phase 3 Refactor
# 6 SMOKE/default count assertion 更新 (7→10 / 42→45 / 24→27)
cargo clippy --lib --tests -- -D warnings && cargo fmt --check

# Phase 4 G-4a/b/c Smoke (要 llama-server -c 16384 --flash-attn on)
cargo build --release
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 | tee /tmp/halluc_g4a.log
BONSAI_LAB_SMOKE=1 BONSAI_KG_FACTCHECK_ENABLED=1 \
  ./target/release/bonsai --lab --lab-experiments 0 | tee /tmp/halluc_g4b.log
grep "FactCheck post-Lab" /tmp/halluc_g4b.log

# Commit
git add -A && git commit -m "feat(benchmark): hallucination-inducing tasks for Plan A G-4c (項目 231 前提)"
git push origin master
```

---

## §10. 不要転用 (rejected)

| 案 | 棄却理由 |
|---|---|
| LLM ベース dynamic prompt 生成 | overhead 大、決定論性失う |
| 外部 KG (Wikidata) seed | Rust 純度毀損、network 依存 |
| `TaskCategory::Halluc` 新規 variant | SCHEMA migration / serde 後方互換コスト、label 上の clarity のみ |
| `capability_tier::Hallucination` 新規 | AgentFloor 6-tier (paper 比較) 前提崩壊 |
| halluc task を `Extended` tier | Lab v20 `core` で hit せず Phase 5 空回り |
| seed を experiment.rs に置く | factcheck 正解 fact set が experiment module に漏出 = 単一責務違反 |

---

## §11. 参考

- `.claude/plan/kg-grounded-fact-check-impl.md` (Plan A 前提、§3 Phase 4 G-4c TODO 起源)
- `.claude/plan/lab-v20-kg-factcheck-effectiveness.md` (§7 R5 / §11 TODO で本 plan name-drop)
- `src/agent/benchmark.rs:964-972` (SMOKE_TASK_IDS)、`1513-1535` (smoke_failure_chain_pair template)
- `src/memory/factcheck.rs:22-31` (regex pattern)、`121-136` (verify_triple_in_kg Conflict 判定)
- `src/memory/graph.rs:13-160` (KnowledgeGraph API)
- 項目 207 Lab v15 baseline 0.7812 (本 plan 適用後の新 baseline 取得必要)
- 項目 223 AgentFloor 6-tier (capability_tier 選定前提)
- 項目 224 AgentFloor pre-screen tier persistence fix
