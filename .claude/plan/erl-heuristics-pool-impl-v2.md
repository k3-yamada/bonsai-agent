# Plan v2: ERL Heuristics Pool — Reflexion 由来の heuristics layer (Codex audit 反映)

> **由来**: arxiv 2603.24639 "Experiential Reflective Learning (ERL)" (2026-03)、Gaia2 で **+7.8% over ReAct**。
>
> **v1 との差分**: `.claude/plan/erl-heuristics-pool-impl.md` (v1, 308 行) を Codex audit (10 findings: 1 CRITICAL / 5 HIGH / 4 MEDIUM) で再 review。HEAD 整合 (test 1058→1079 / Lab v16 既消化 / agent_loop split / EventRepository trait dividend) + V10 schema 競合解消 + injection path 訂正 + reflection prompt 契約前倒し + SQLite source-of-truth 原則 + falsifiable hypothesis を反映した実装直前版。
>
> **目的**: Lab v8/v9/v10/v14/v15/**v16 の天井 6 連続** (CLAUDE.md 項目 207/212) を **構造変異 (context-level)** で打開する。Reflexion 由来の自然言語助言を、SkillStore (tool_chain) / ExperienceStore (record) / Vault (rules) のいずれにも該当しない第 4 層として独立保管し、Lab 開始時 system prompt に注入する。

## Task Type
- [x] Backend (新規モジュール `src/memory/heuristics.rs` + `src/agent/context_inject.rs::inject_heuristics` + ExperimentLoop hook)

## 0. v1 → v2 主要変更 (Codex audit 反映)
| ID | Severity | v1 課題 | v2 解決 |
|---|---|---|---|
| F1 | CRITICAL | V10 schema を AgentFloor plan と二重予約 | **本 plan が V10 を確保**、AgentFloor plan は V11 へ ship 順序を明文化 (本 plan 側で merge order 宣言) |
| F2 | HIGH | 1058 → 1068 / 「天井 5」/ Lab v16 future 前提 | **1079 → 1089** / 「天井 6」/ Lab v16 完走済 (v17 effectiveness 別 plan) に全更新 |
| F3 | HIGH | 注入経路を `run_experiment_loop` と誤記 | `src/agent/agent_loop/core.rs:133-134` の `inject_memory_blocks` → **`inject_heuristics`** → `inject_contextual_memories` 順 (実装は新規 `context_inject::inject_heuristics`) |
| F4 | HIGH | run_hindsight_pass との順序未規定 | `run_hindsight_pass` 先 → `run_heuristics_pass` 後、両者同一 `lab_start_event_id`、両者 non-fatal (`experiment.rs:1293` の `match` パターンを継承) |
| F5 | HIGH | private `promote_with_prefix` 経由を仮定 | `SkillStore` に **新規 public method** `promote_from_erl_advice(&self, candidate, source_session_id) -> Result<Option<i64>>` を追加 (prefix `"erl_"`)、または ERL 側で TrajectoryCandidate を直接構築せず HeuristicStore のみに persist |
| F6 | MEDIUM | tool-chain 検出ルール曖昧 | `detect_tool_chain_in_advice(advice: &str, known_tools: &[&str]) -> Option<Vec<String>>` を新 helper、ALLOWLIST + word-boundary regex + max 8 token gap、table-driven test 6 件以上 |
| F7 | HIGH | reflection prompt 契約が Phase 3 | **Phase 1/2 で確定**、JSON-only 出力 + schema 強制 + parse 失敗時 0 抽出 (Lab 失敗扱いせず) + malformed test 必須 |
| F8 | MEDIUM | HeuristicStore の trait 化判断未確定 | **trait 化しない** (premature abstraction)。event 読取のみ `&dyn EventRepository` (項目 209 dividend) を採用、HeuristicStore は inherent `&Connection` 維持 |
| F9 | MEDIUM | SQLite ↔ vault.md の同期方向不明 | **SQLite = source of truth**、`vault/heuristics.md` は v1 では作らない (export hook は将来案件) |
| F10 | MEDIUM | falsifiable hypothesis 不在 | 項目 212 副次知見 (`relabels=4 skills=2 insights=4`) を base に **「ERL は tool_chain 表現不能の自然言語助言で advisor-threshold 不達カテゴリを補完する」** を仮説化、smoke G-4 は infrastructure のみ確認、effectiveness は Lab v17 paired t-test で別 plan |

## 1. 背景・動機
### 1.1 ERL 論文の核 (v1 から継承)
ERL = ReAct 系 agent の **失敗 trajectory に対する reflection から自然言語の heuristics を生成し、共通 pool に蓄積、次回 task 開始時に注入** する枠組み。Gaia2 で +7.8% (pass@3 で +8.3%-10.6%)。

### 1.2 bonsai 既存隣接構造 (v1 から継承)
| 層 | 既存実装 | 形式 | 単位 | 注入経路 |
|---|---|---|---|---|
| **Skill** (項目 161) | `skill.rs::SkillStore` | tool_chain (構造化) | `file_write -> shell` 等 | `find_matching` で検索注入 |
| **Experience** (項目 77) | `experience.rs::ExperienceStore` | success/failure/insight | task_context + action + outcome | `find_similar` で検索注入 |
| **Vault Rules** (項目 76) | `knowledge/vault.rs::Vault` + Decision/Pattern | 自然言語 (md) | 1 line / 1 entry | `read_rules` で常時注入 |
| **(新層 = ERL)** | `memory/heuristics.rs::HeuristicStore` | 自然言語の助言 | 1 heuristic = 1 short sentence | `inject_heuristics` で常時注入 |

### 1.3 天井 6 連続 (v8/v9/v10/v14/v15/**v16**) との接続
CLAUDE.md 項目 207 (v15 baseline=0.7812 / 0/3 ACCEPT) + 項目 212 (v16 baseline=0.7761 / 0/3 ACCEPT、advisor-threshold 全 REJECT) で天井確定。HypothesisGenerator が既デフォルト #47/#50 を再生成する (tried_details 54 件履歴枯渇) のが症状。

ERL は **system prompt 自動拡張のメカニズム** を提供することで、Lab 変異が「人間が書く事前ルール」から「Reflexion で自動収穫した運用知」にシフトする。本 plan の効果指標は単発 ACCEPT delta よりも **「Lab v17+ で構造変異の余地を再生成する」** 点。

### 1.4 ★ 仮説 (Codex F10 反映、falsifiable)
**H_ERL**: Lab v16 で AgentHER が `relabels=4 skills=2 insights=4` (項目 212 副次知見) と yield を確認している。これらはすべて tool_chain (`promote_from_hindsight_relabel`) 経由で SkillStore に流れた成果。一方、**「最初に file_read して状況確認してから shell を打て」「コード変更後は git diff で確認せよ」など、実行 step の自然言語助言は SkillStore に表現不能で現在捨てられている**。ERL はこの「tool_chain 表現不能助言」 path だけを HeuristicStore に保管し、system prompt 注入で Lab v17 baseline を **+0.015 以上 (= ACCEPT 基準)** 押し上げる。

**反証条件**: Lab v17 で `--enable-heuristics on` × `--enable-heuristics off` paired t-test (5 cycle、core 22) で **Δscore < +0.015 または p ≥ 0.1** ならば H_ERL は棄却、heuristics 機構は dead-code 候補。

## 2. 目的 (v1 維持 + 修正)
1. **heuristic データモデル新設** (4.1)
2. **Reflexion からの抽出ロジック** (4.2): EventRepository trait 経由で event 読取
3. **(変更)** ~~Vault 配置~~ → SQLite のみが source of truth、md export は v2 scope 外 (F9)
4. **system prompt 注入** (4.5): `src/agent/context_inject.rs::inject_heuristics`、core.rs から呼出
5. **SkillStore との dedup 規約** (4.4): 新 public method + table-driven detector
6. **(継承)** 項目 206 deterministic dedup: trigger_hash + advice 先頭 80 文字 fingerprint

## 3. 既存項目との関係 (v1 から修正)
| 既存項目 | 関係 | 改修要否 |
|---|---|---|
| **76 Vault** | (v2) **連携しない** (SQLite のみ) | 改修なし |
| **77 Experience** | 抽出元として参照 (failure/insight 経験を heuristic 候補化) | 参照のみ |
| **161 Skill (軌跡昇格)** | dedup 規約で boundary 確定 + 新 public method `promote_from_erl_advice` | 拡張 |
| **162 EventStore (Option B export hook)** | scoping snapshot を再利用、Lab cycle 単位で抽出 | 参照のみ |
| **80 contextual injection** | `<context type="heuristics">` タグ追加 (`context_inject.rs`) | 拡張 |
| **179 MemoryBlock** | system prompt 注入順序の 1 個前に挿入 | 参考 |
| **201 AgentHER (HSL)** | run_hindsight_pass 先 → run_heuristics_pass 後、両者 non-fatal | 順序明示 |
| **205 Option A 移行** | `&MemoryStore` 必須化を踏襲 | 設計踏襲 |
| **206 deterministic dedup** | content fingerprint dedup を deterministic に | 設計踏襲 |
| **209 EventRepository trait** | event 読取を `&dyn EventRepository` で抽象化 (Mock test 経由) | dividend 活用 |
| **210 Self-Verify dynamic skip** | 干渉なし (advisor 層、本 plan は context 層) | 共存 |
| **AgentFloor plan** | **本 plan が V10、AgentFloor は V11** | merge 順序明示 |

## 4. 設計
### 4.1 データモデル (`src/memory/heuristics.rs`)
```rust
use crate::agent::event_store::EventRepository;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heuristic {
    pub id: i64,
    pub advice: String,                  // ≤200 文字推奨
    pub trigger_patterns: String,        // JSON Vec<String>
    pub source_session_id: Option<String>,
    pub source_task: String,             // 先頭 80 文字
    pub category: String,                // failure_recovery / efficiency / verification
    pub score: f64,                      // 0.0〜1.0
    pub used_count: i64,
    pub success_after_use: i64,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

pub struct HeuristicStore<'a> {
    conn: &'a Connection,
}

#[derive(Debug, Clone, Default)]
pub struct HeuristicSummary {
    pub extracted: usize,
    pub saved: usize,
    pub skipped_to_skill: usize,
    pub pruned: usize,
    pub parse_failures: usize,           // F7: malformed JSON 件数
}
```

**API (inherent、trait 化なし — F8)**:
- `HeuristicStore::new(&Connection) -> Self`
- `save(&self, advice, triggers, source_session_id, source_task, category) -> Result<i64>`
- `find_top_k_for_task(&self, task_context: &str, k: usize) -> Result<Vec<Heuristic>>`
- `record_outcome(&self, id, task_succeeded: bool) -> Result<()>`
- `prune(&self) -> Result<usize>`
- `reset_for_lab(&self) -> Result<()>` (項目 206 reset_session_data_for_lab と協調)

### 4.2 Reflexion 抽出ロジック (`extract_heuristics_from_events`)
```rust
pub fn extract_heuristics_from_events(
    events: &dyn EventRepository,             // F8: trait 経由 (Mock test 可能)
    since_event_id: i64,                      // F4: lab_start_event_id (run_hindsight_pass と共有)
    backend: &dyn LlmBackend,
) -> Result<Vec<HeuristicCandidate>>;
```

**フロー** (v1 維持):
1. `events.extract_failed_trajectories_since_id(since, 0.8, 2)?` で failure trajectory 取得 (項目 209 既存 API)
2. 同様に `extract_successful_trajectories_since_id` で成功側
3. session ごとに集約 → 1 reflection LLM call (1 session につき 1 call、Bonsai-8B context 短さ対応)
4. (★ F7) **JSON-only 厳格 prompt** で `[{"advice": str, "trigger_patterns": [str], "category": str}]` を要求
5. parse 失敗 → `parse_failures += 1`, この session 分は捨てる (Lab 失敗扱いせず)
6. content fingerprint dedup (advice 先頭 80 文字、trigger_hash 併用)
7. score 初期値 = (recency 1.0 + utility 0.5 + diversity 1.0/(同 category +1)) / 3

### 4.3 ★ Reflection Prompt Template (F7、Phase 1/2 で確定)
新規ファイル `prompts/heuristic_reflection.txt` を Phase 2 着手と同時に作成:

```
You are a reflection engine. Read this trajectory and output up to 3 short
natural-language heuristics. Output ONLY a JSON array, no prose, no markdown.

Schema:
[{"advice": str ≤200 chars, "trigger_patterns": [str ≥4 chars, 2-5 items], "category": "failure_recovery"|"efficiency"|"verification"}]

Trajectory:
<task: {task_description}>
<final_outcome: {success|failure}>
<events:
{event_summary}
>

Constraints:
- advice MUST NOT name 2 or more known tools in order (those go to SkillStore)
- trigger_patterns MUST be specific phrases, not generic words
- If no heuristic applies, output []
```

`temp=0.3 max_tokens=400`、parse failure threshold=2 で session skip。

### 4.4 ★ SkillStore dedup boundary (F5/F6)
```rust
// src/memory/heuristics.rs
pub(crate) fn detect_tool_chain_in_advice(
    advice: &str,
    known_tools: &[&str],
) -> Option<Vec<String>>;
```

**ルール**:
- ALLOWLIST = `&["file_read", "file_write", "shell", "git", "web_fetch", "repomap", "multi_edit", "grep", "glob"]` (現行 ToolRegistry から)
- word-boundary regex: `\b(file_read|file_write|...)\b`
- **2 個以上、出現順 ≤ 8 token gap、両者間に逆接接続詞 (`but`/`しかし`) なし** → tool_chain 検出
- 検出時: `SkillStore::promote_from_erl_advice` (新 public method) 呼出、HeuristicStore には保存しない

**新 public method** (F5、`src/memory/skill.rs` に追加):
```rust
impl SkillStore<'_> {
    pub fn promote_from_erl_advice(
        &self,
        tool_chain: &[String],
        advice: &str,
        source_session_id: &str,
    ) -> Result<Option<i64>> {
        // 内部で promote_with_prefix("erl_") 委譲、private 維持
    }
}
```

**table-driven test (F6)** ≥ 6 件:
| input advice | expected tool_chain |
|---|---|
| "Use file_read then shell to verify" | Some(["file_read", "shell"]) |
| "shell でテスト後 file_write で記録" | Some(["shell", "file_write"]) |
| "file_read だけで OK" | None (tool 1 個) |
| "file_read but use file_write instead" | None (逆接) |
| "file_read ... [50 tokens] ... shell" | None (gap >8) |
| "Run shell, but check repomap and grep" | Some(["repomap", "grep"]) |

### 4.5 system prompt 注入 (`src/agent/context_inject.rs::inject_heuristics`、F3)
```rust
// src/agent/context_inject.rs (既存 file の inject_memory_blocks/inject_contextual_memories と同居)
pub fn inject_heuristics(
    session: &mut Session,
    task_context: &str,
    store: &MemoryStore,
) -> Vec<i64> {
    let h_store = HeuristicStore::new(store.conn());
    let top_k = h_store.find_top_k_for_task(task_context, 5).unwrap_or_default();
    if top_k.is_empty() { return vec![]; }
    let body = top_k.iter()
        .map(|h| format!("- {}", h.advice))
        .collect::<Vec<_>>().join("\n");
    session.add_system(&format!("<context type=\"heuristics\">\n{body}\n</context>"));
    top_k.iter().map(|h| h.id).collect()  // task 完了後 record_outcome 用 ID
}
```

**`agent_loop/core.rs:133-134` の修正**:
```rust
// 修正前 (現行)
inject_memory_blocks(session, &config.soul_path, &config.memory_blocks);
inject_contextual_memories(session, &task_context, store);

// 修正後 (本 plan)
inject_memory_blocks(session, &config.soul_path, &config.memory_blocks);
let injected_heuristic_ids = inject_heuristics(session, &task_context, store);
inject_contextual_memories(session, &task_context, store);
// LoopState に injected_heuristic_ids を保存 (task 完了 hook で record_outcome 呼出)
```

**id carry**: 注入された heuristic IDs は `LoopState` 経由で task 完了 hook に渡す → `record_outcome` で utility update。

### 4.6 score 更新 (発火後、v1 維持)
- `recency = exp(-Δdays / 14.0)` 14 日 half-life
- `utility = success_after_use / used_count` (default 0.5)
- `score = (recency × 0.3) + (utility × 0.5) + (diversity × 0.2)`

### 4.7 prune (月次、v1 維持)
- `score < 0.2 && used_count >= 5` → 廃却
- `score < 0.2 && used_count == 0 && created_at > 30 days ago` → 廃却
- 上限 200、超過時は score 昇順削除

## 5. TDD strict 5 phase
### Phase 1 — Red (新規 ~12 test)
**heuristics.rs (8 件)**:
- `t_heuristic_store_save_basic`
- `t_heuristic_store_dedup_fingerprint`
- `t_extract_heuristics_requires_session_end`
- `t_extract_heuristics_min_steps`
- `t_extract_heuristics_returns_skill_for_tool_chain` (HeuristicStore 0 件 + SkillStore erl_ prefix 1 件)
- `t_extract_heuristics_parse_failure_skips_session` (★ F7)
- `t_find_top_k_for_task_filters_by_trigger`
- `t_record_outcome_updates_score`

**detect_tool_chain_in_advice (table-driven 6 件、F6)**: 4.4 の表

**SkillStore (1 件、F5)**:
- `t_promote_from_erl_advice_basic`

**Mock event (★ F8 dividend)**:
- `t_extract_heuristics_with_mock_event_repository` (`MockEventRepository` 経由 SQLite なし)

期待: compile error 含む全 RED。

### Phase 2 — Green
1. `src/memory/heuristics.rs` 新規 (~280 行): struct + impl + extract_heuristics + detect_tool_chain helper + fingerprint
2. `prompts/heuristic_reflection.txt` 新規 (★ F7 前倒し)
3. `src/db/schema.rs` SCHEMA_V10 (heuristics テーブル 11 col、F1: AgentFloor は V11 を使う旨 docstring)
4. `src/db/migrate.rs` migration entry V10
5. `src/memory/skill.rs` `promote_from_erl_advice` 新 public method (F5)
6. `src/agent/context_inject.rs` `inject_heuristics` 新規
7. `src/agent/agent_loop/core.rs:133-134` 修正 (F3)
8. `src/agent/experiment.rs` `run_heuristics_pass` 新規 (1293 行 `match run_hindsight_pass` の **直後** に同パターン挿入、F4):
```rust
match run_hindsight_pass(store, lab_start_event_id) { ... }
match run_heuristics_pass(store, lab_start_event_id, backend) {
    Ok(s) => log_event(LogLevel::Info, "lab.heuristics",
        format!("ERL post-Lab: extracted={} saved={} skipped_to_skill={} pruned={} parse_failures={}",
                s.extracted, s.saved, s.skipped_to_skill, s.pruned, s.parse_failures)),
    Err(e) => log_event(LogLevel::Warn, "lab.heuristics", format!("ERL pass failed (non-fatal): {e}")),
}
```
9. `src/memory/mod.rs` `pub mod heuristics;` 追加
10. `src/agent/event_store.rs` の MockEventRepository 既存 (項目 209) を heuristics test で再利用

期待: **1079 → 1091 passed (+12 / clippy 0 / fmt 0)**。

### Phase 3 — Refactor
- score 計算式を `Heuristic::recompute_score(now)` helper に
- `find_top_k_for_task` の trigger match を `LazyLock<Regex>` cache 化
- `record_outcome` の I/O を `Result<()>` で統一
- `HeuristicStore::reset_for_lab` を項目 206 と協調

### Phase 4 — Smoke (G-4、infrastructure 確認のみ)
```bash
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 2>&1 | tee /tmp/erl_smoke.log
grep "lab.heuristics\|lab.agenther" /tmp/erl_smoke.log
```

判定 (F10: effectiveness は除外):
- ✅ Build green (1091 passed 維持)
- ✅ HeuristicStore に少なくとも 1 件 persist (failure session 由来)
- ✅ run_hindsight_pass log → run_heuristics_pass log の順序確認 (F4)
- ✅ duration 増加 ≤ +12% (reflection LLM call cost、24 session × 1 call ≈ ±2 min)
- ✅ schema V10 migration 成功 (新 DB と既存 V9 DB の両方で)
- ✅ parse_failures 件数 log

### Phase 5 — Effectiveness 検証 (Lab v17 別 plan、F10 falsifiable)
本 plan scope 外。次 plan `.claude/plan/lab-v17-erl-effectiveness.md` で:
- `--enable-heuristics on/off` paired t-test
- core 22 × 5 cycle
- ACCEPT 基準: **Δscore ≥ +0.015 かつ p < 0.1**
- 反証条件: Δscore < +0.015 または p ≥ 0.1 → H_ERL 棄却 (heuristics 機構 dead-code 候補化)

## 6. API 影響 (新規 public 一覧)
| modulo path | 関数 / 構造体 |
|---|---|
| `crate::memory::heuristics::Heuristic` | struct |
| `HeuristicStore::{new, save, find_top_k_for_task, record_outcome, prune, reset_for_lab}` | 6 method |
| `extract_heuristics_from_events(&dyn EventRepository, since_event_id, &dyn LlmBackend) -> Result<Vec<HeuristicCandidate>>` | free fn (F8) |
| `crate::memory::heuristics::HeuristicSummary` | struct (5 fields) |
| `crate::memory::skill::SkillStore::promote_from_erl_advice` | method (F5) |
| `crate::agent::context_inject::inject_heuristics` | fn (F3) |
| `crate::agent::experiment::run_heuristics_pass` | post-Lab hook (F4) |

**API 名空間**: 全て新規 + skill.rs 1 method 追加 + agent_loop/core.rs:133-134 修正のみ (シグネチャ変更ゼロ → 後方互換 100%)。

## 7. risks / mitigations (v1 R1-R8 + Codex F1/F4/F6/F7/F9/F10 反映)
| # | Risk | 影響 | Mitigation |
|---|---|---|---|
| **R1** | heuristic 重複爆発 (1bit reflection 同 advice 反復) | DB 肥大、context 圧迫 | content fingerprint dedup (advice 先頭 80 文字) save 強制、項目 206 deterministic dedup 踏襲、prune で score < 0.2 削除 |
| **R2** | reflection LLM cost (1 cycle +12 min) | Lab duration regression | max_tokens=400 / temp=0.3、Phase 4 で確認、超過時は failure session のみ抽出で cost 半減 |
| **R3** | trigger pattern マッチ過剰広範 | 注入過多で context 圧迫 | reflection prompt で「2-5 keywords specific phrase」明示、`find_top_k_for_task` k=5 上限固定、min_length=4 検証で reject |
| **R4** | pre-screen 汚染 (項目 205 と同パターン) | Lab v17 false positive | `extract_heuristics_from_events` 呼び出しは `lab_start_event_id` scoping 必須 (F4)、項目 205 `scratch_store` 設計踏襲 |
| **R5** | 1bit variance で効果検証困難 | ACCEPT/REJECT 不安定 | Phase 5 別 plan で 5 cycle paired t-test + 項目 200 RDC/VAF 併用 |
| **R6** | reflection JSON parse 失敗 (1bit malformed 出力) | extracted=0 が Lab 失敗扱い化 | (★ F7) parse_failures カウント、session skip、Lab continue (non-fatal) |
| **R7** | tool-chain 検出誤検知 (skill 流入過多 or 不足) | Skill / Heuristic 区分崩壊 | (★ F6) ALLOWLIST + table-driven test 6 件、誤検知 ≤ 5% を Phase 4 smoke で確認 |
| **R8** | schema V10 migration 失敗 | Lab 起動不能 | Phase 2 migration test 必須、CREATE TABLE IF NOT EXISTS、(★ F1) AgentFloor V11 reservation を docstring に明示 |
| **R9** | run_hindsight_pass + run_heuristics_pass 順序崩壊 | HSL skill が ERL に上書き or 二重昇格 | (★ F4) 順序 hindsight 先 → heuristics 後、両者同 lab_start_event_id、両者 non-fatal、ERL は detect_tool_chain で skill ルート判定し HSL 既昇格 tool_chain と重複しないか SkillStore 側 dedup で安全 |
| **R10** | (★ F9) Vault md とのデータ二重化 | source-of-truth ambiguity | v1 では Vault 連携を作らない、SQLite が source-of-truth、md export は次 plan |
| **R11** | (★ F10) effectiveness が Gaia2 → bonsai に translate しない | Lab v17 で REJECT | H_ERL を falsifiable に明文化、smoke G-4 は infrastructure のみ確認、effectiveness は Lab v17 別 plan で paired t-test |

## 8. quality gates
| Gate | 内容 | 検証 | 必須 |
|---|---|---|---|
| **G-1 (Phase 1 Red)** | 新規 ~12 test が compile error or assertion fail で Red 確認 | `cargo test --lib heuristics` | 必須 |
| **G-2 (Phase 2 Green)** | 全 test PASS + **1079 → 1091 passed** + clippy 0 + fmt 0 + migration test PASS (V9 DB → V10 マイグレーション both 確認) | `cargo test/clippy/fmt` | 必須 |
| **G-3 (Phase 3 Refactor)** | prompt 切り出し済 (Phase 2 で完了済)、score helper 抽出、reset_for_lab 追加、code 重複ゼロ | self-review | 必須 |
| **G-4 (Phase 4 Smoke、★ F10 反映: infrastructure のみ)** | core 22 × 1 cycle で extracted ≥ 1 / saved ≥ 0 / build green / duration ≤ +12% / hindsight log → heuristics log の順序確認 / parse_failures 記録 | `BONSAI_LAB_SMOKE=1 --lab` + log grep | 必須 |
| **G-5 (Final)** | net +500 行以下 (production +500、テスト除く) | `git diff --stat` | 任意 |
| **G-6 (Effectiveness、★ F10 別 plan)** | Lab v17 paired t-test で **Δscore ≥ +0.015 かつ p < 0.1** | 5 cycle 別 handoff | **本 plan の scope 外** |

G-1〜G-4 PASS で merge 可能。G-6 は別 session/plan。

## 9. 見積もり
| Phase | 内容 | 所要 |
|---|---|---|
| **P1 (Red)** | test ~12 件、cargo test Red 確認 | 1.5h |
| **P2 (Green)** | heuristics.rs ~280 行 + reflection prompt + schema V10 + skill 新 method + context_inject + experiment hook + agent_loop core 修正 | 4h |
| **P3 (Refactor)** | score helper、reset_for_lab、clippy/fmt | 1h |
| **P4 (Smoke)** | release build + 1 cycle smoke + log 確認 + 順序確認 | 1.5h (うち実機 ~15-20 min) |
| **P5 (commit + handoff)** | 5 commits + CLAUDE.md 項目 213 + MEMORY.md | 1h |
| **計** | | **~9h、1 day + 1h** (v1 比 +1h、F1/F4/F6/F7 対応で +1h) |

## 10. 次の段階
### 着手判断 (v2 で更新)
- ✅ AgentHER (項目 201-205) production-ready
- ✅ Beyond pass@1 (項目 200) で stability metric 併用可
- ✅ EventRepository trait (項目 209) で Mock test 経由可能 (F8 dividend)
- ✅ schema V9 → V10 余地あり (本 plan で V10 確保、AgentFloor は V11 要 update)
- ✅ Lab v16 で天井 6 連続確定 (項目 212)
- ⏳ AgentFloor plan が V11 へ更新可能か事前確認

### 先送り条件
- ❌ AgentFloor plan が V10 を譲らない場合 → 先に AgentFloor 着手 → 本 plan は V11 に書き換え
- ❌ vllm-mlx 切替実施中なら backend 安定後

## 11. ★ 着手前チェックリスト (F1 対応、必須)
1. [ ] `.claude/plan/agentfloor-tier-eval-impl.md` を grep して V10 references を全て V11 に書き換える plan が出来ているか確認
2. [ ] AgentFloor Phase 1 Red commit (`227771c`) は schema 変更を含んでいないか確認 (`git show 227771c -- src/db/`)
3. [ ] 本 plan の Phase 1 Red 着手前に上記 2 点 GO サイン

## 12. Quick Start (v2 修正)
```bash
# 0. (★ F1) AgentFloor V10→V11 確認
grep -n "V10\|SCHEMA_V10\|version: 10" .claude/plan/agentfloor-tier-eval-impl.md

# 1. caller 全網羅
rg -n "run_hindsight_pass|AgentHER post-Lab" src/
rg -n "StockCategory" src/
rg -n "SCHEMA_V" src/db/schema.rs
rg -n "inject_memory_blocks|inject_contextual_memories" src/agent/

# 2. Phase 1 Red
$EDITOR src/memory/heuristics.rs        # struct + test mod (12 test)
$EDITOR src/memory/skill.rs             # t_promote_from_erl_advice_basic
$EDITOR src/agent/context_inject.rs     # 既存 file 確認、test 追加
$EDITOR src/agent/experiment.rs         # t_lab_cycle_calls_heuristics_pass
rtk cargo test --lib heuristics

# 3. Phase 2 Green
$EDITOR src/memory/heuristics.rs        # 実装 ~280 行
$EDITOR prompts/heuristic_reflection.txt # ★ F7 前倒し
$EDITOR src/db/schema.rs                # SCHEMA_V10 (AgentFloor V11 docstring)
$EDITOR src/db/migrate.rs               # migration entry
$EDITOR src/memory/skill.rs             # promote_from_erl_advice
$EDITOR src/agent/context_inject.rs     # inject_heuristics
$EDITOR src/agent/agent_loop/core.rs    # line 133-134 修正
$EDITOR src/agent/experiment.rs         # run_heuristics_pass + post-Lab hook
$EDITOR src/memory/mod.rs               # pub mod heuristics
rtk cargo test --lib --release

# 4. Phase 3 Refactor
rtk cargo clippy --release -- -D warnings && rtk cargo fmt --check

# 5. Phase 4 Smoke
rtk cargo build --release
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 | tee /tmp/erl_smoke.log
grep "lab.heuristics\|lab.agenther" /tmp/erl_smoke.log

# 6. Commit
git add -A && git commit -m "test(erl): Phase 1 Red — Heuristics Pool 12 tests (項目 213)"
```

## 13. 参考
- arxiv 2603.24639 ERL (2026-03)
- `memory/research_arxiv_2026_05_07.md` 領域 4 (★★★ #3)
- 既存隣接構造: 項目 76/77/161/162/179/201-206/209/210
- Lab 天井 evidence: 項目 207 (v15)、項目 212 (v16)
- Codex audit (本 v2): F1-F10 (CRITICAL×1, HIGH×5, MEDIUM×4)
- 関連 plan: `agenther-option-a-migration.md`, `agentfloor-tier-eval-impl.md` (V11 更新要)
- v1 plan: `erl-heuristics-pool-impl.md` (308 行、本 v2 で supersede)
- CLAUDE.md 項目候補: 213 (本 plan 完遂時)

## 14. SESSION_ID (for /ccg:execute use)
- CODEX_SESSION: 019e064a-334c-7692-9735-c5d95231ebf1
- GEMINI_SESSION: (failed: exit 1 / empty output / model availability issue、未取得)
