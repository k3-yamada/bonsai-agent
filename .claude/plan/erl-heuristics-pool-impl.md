# Plan: ERL Heuristics Pool — Reflexion 由来の heuristics layer を Vault に追加

> **由来**: arxiv 2603.24639 "Experiential Reflective Learning (ERL)" (2026-03)、Gaia2 で **+7.8% over ReAct**、pass@3 で +8.3% (Execution) / +10.6% (Search)。本 plan は `memory/research_arxiv_2026_05_07.md` 領域 4 の ★★★ 高優先 3 番「ERL heuristics pool — Vault に heuristics layer、SkillStore + ExperienceStore と統合 (中規模 plan)」の **完全版**。
>
> **目的**: Lab v8/v9/v10/v14/v15 の **天井 5 連続 (プロンプト系変異の改善余地枯渇)** を **構造変異** で打開する。Reflexion 由来の高水準ヒューリスティック (例: 「ファイル不存在エラーは create 系 tool で先回り回避」) を、既存 Skill (tool_chain) / Experience (record) / Vault (decisions/patterns md) のいずれにも該当しない第 4 層として独立保管し、Lab 開始時 system prompt に注入する。

## Task Type
- [ ] Frontend
- [x] Backend (新規モジュール `src/memory/heuristics.rs` + Vault 連携 + ExperimentLoop hook)
- [ ] Fullstack

## 1. 背景・動機
### 1.1 ERL 論文の核
ERL (Experiential Reflective Learning) は、ReAct 系 agent の **失敗 trajectory に対する reflection から自然言語の heuristics を生成し、共通 pool に蓄積、次回 task 開始時に注入** する枠組み。

- **heuristic** = 「特定状況で適用される短い助言」(例: "When file not found, use `mkdir -p` before file_write to ensure parent directory")
- **収集** = task 終了時に LLM 自身に reflection を行わせ、pass/fail いずれでも複数の heuristic を抽出
- **再利用** = 次の task 開始時 system prompt に top-K (relevance × recency) を注入
- **Gaia2 結果**: ReAct baseline +7.8%, pass@3 では +8.3%-10.6%

### 1.2 bonsai 既存の隣接構造
bonsai は ERL 中核要素を **3 層別々に** 持っているが、**heuristic 層が欠けている**:

| 層 | 既存実装 | 形式 | 単位 | 注入経路 |
|---|---|---|---|---|
| **Skill** (項目 161) | `skill.rs::SkillStore` | tool_chain (構造化) | `file_write -> shell` 等 | `find_matching` で検索注入 |
| **Experience** (項目 77) | `experience.rs::ExperienceStore` | success/failure/insight | task_context + action + outcome | `find_similar` で検索注入 |
| **Vault Rules** (項目 76) | `knowledge/vault.rs::Vault` + Decision/Pattern | 自然言語 (md) | 1 line / 1 entry | `read_rules` で常時注入 |
| **(欠落)** | — | 自然言語の助言 | 1 heuristic = 1 short sentence | — |

ERL の "heuristic" は **Vault::Pattern より具体的、Skill より自然言語的、Experience より抽象的** という中間層であり、既存構造の **拡張ではなく新層** として配置するのが正解。

### 1.3 天井 5 連続との接続
Lab v8 (0/10) → v9 (1/14) → v10 (1/9) → v14 (1/4 真新規 0/2) → v15 (項目 207 0/3) で **プロンプト系変異の天井** が確定。CLAUDE.md 末尾「派生デフォルト化変異」には項目 10/47/50/136 の 4 件のみ、いずれも 2026-04 以前の発見。

ERL の heuristics pool は **system prompt 自動拡張のメカニズム** を提供することで、Lab 変異が「人間が書く事前ルール」から「Reflexion で自動収穫した運用知」にシフトする。本 plan の効果は単発 ACCEPT delta よりも **「Lab v16+ で構造変異の余地を再生成する」** 点にある。

## 2. 目的
1. **heuristic データモデル新設**: trigger pattern / advice text / source trajectory / score 4 軸の構造体を定義
2. **Reflexion からの抽出ロジック**: SessionEnd 時 (run_agent_loop 末尾 hook) または Lab post-pass で events → heuristics を generate
3. **Vault 配置**: `~/.config/bonsai-agent/vault/heuristics.md` をカテゴリ追加 (StockCategory::Heuristic) で永続化
4. **system prompt 注入**: Lab cycle 開始時に top-K (score 順) を `<context type="heuristics">` で注入
5. **SkillStore との dedup 規約**: tool_chain で表現可能なものは Skill 側に流し、自然言語助言のみ heuristic に残す
6. **項目 206 で確証された deterministic dedup 設計** を踏襲: trigger_hash + advice 先頭 80 文字の content fingerprint で重複排除

## 3. 既存項目との関係
| 既存項目 | 関係 | 改修要否 |
|---|---|---|
| **76 Vault** | StockCategory に `Heuristic` を追加 (extractor.rs / vault.rs) | 拡張 |
| **77 Experience** | 抽出元として参照 (failure/insight 経験を heuristic 候補化) | 参照のみ |
| **161 Skill (軌跡昇格)** | dedup 規約で boundary 確定 | 参照のみ |
| **162 EventStore (Option B export hook)** | scoping snapshot を再利用、Lab cycle 単位で抽出 | 参照のみ |
| **80 contextual injection** | `<context type="heuristics">` タグ追加 | 拡張 |
| **179 MemoryBlock** | system prompt 注入経路の参考 | 参考のみ |
| **201 AgentHER (HSL)** | 失敗 trajectory 抽出と協調 (HSL は subgoal、ERL は advice) | 共存、competition なし |
| **205 Option A 移行** | `&MemoryStore` 必須化を踏襲 | 設計踏襲 |
| **206 deterministic dedup** | content fingerprint dedup を deterministic に | 設計踏襲 |

**新層 = 既存 4 層と独立**。skill_chain / experience_record / vault_rule のいずれにも match しない自然言語 heuristic のみを heuristic_pool に蓄積。

## 4. 設計
### 4.1 データモデル (新規 `src/memory/heuristics.rs`)
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heuristic {
    pub id: i64,
    /// 自然言語助言 (≤200 文字推奨)
    pub advice: String,
    /// trigger キーワード (会話・task_context マッチ用)、JSON Vec<String>
    pub trigger_patterns: String,
    pub source_session_id: Option<String>,
    pub source_task: String,  // 先頭 80 文字
    pub category: String,  // failure_recovery / efficiency / verification 等
    pub score: f64,  // recency × utility × diversity, 0.0〜1.0
    pub used_count: i64,
    pub success_after_use: i64,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

pub struct HeuristicStore<'a> {
    conn: &'a rusqlite::Connection,
}
```

### 4.2 Reflexion 抽出ロジック (`extract_heuristics_from_events`)
入力: Lab cycle scoping 範囲の `Vec<Event>` (項目 162 同等の Iterator API)

抽出フロー:
```
1. session_id ごとに events split
2. 各 session で:
   a. SessionEnd 必須 (項目 162 整合)
   b. min 2 ToolCallStart 必須 (項目 201 HSL と同基準)
   c. final_outcome = 最後の StepCompleted の success
   d. failure 系 events から (error_type, recovery_step) ペアを mining
3. backend に 1 reflection prompt:
   "Given trajectory T, output ≤3 heuristics in JSON list of {advice, trigger_patterns[], category}."
4. 返却 JSON parse、advice 先頭 80 文字 fingerprint で dedup
5. score 初期値:
   recency = 1.0、utility = 0.5 (実利用前)、diversity = 1.0/(既存 same-category + 1)
   score = (recency + utility + diversity) / 3
```

**重要**: reflection LLM call は **1 session 1 call** に集約 (Bonsai-8B context 短さに合わせる)。Lab cycle 全体 ~24 session ≈ 12min ≈ 12% Lab duration。

### 4.3 Vault 配置と StockCategory 拡張
```rust
pub enum StockCategory {
    Decision, Fact, Preference, Pattern, Insight, Todo,
    Heuristic,  // 新規
}

impl StockCategory {
    pub fn is_rule(&self) -> bool {
        matches!(self, Self::Decision | Self::Pattern | Self::Heuristic)
    }
    pub fn all() -> &'static [StockCategory] { &[..., Self::Heuristic] }
}
```

`vault.rs::Vault::new` の create 時 categories list に `"heuristics"` を追加 → `vault/heuristics.md` 自動生成。

### 4.4 SkillStore との dedup 規約
heuristic 生成時:
1. advice text に **tool 名が 2 個以上順序付きで明記** (例: `file_read.*shell`) → HeuristicStore に書かず、`SkillStore::promote_with_prefix(_, "erl_")` に流す (項目 206 deterministic dedup 経由)
2. **自然言語のみ**: HeuristicStore に persist

skill ↔ heuristic 境界は **「tool_chain 表現可能性」** で線引き、相互流入なし。

### 4.5 system prompt 注入 (`<context type="heuristics">`)
Lab cycle 開始時 `run_experiment_loop`:
```rust
let store = HeuristicStore::new(memory_store.conn());
let top_k = store.find_top_k_for_task(&task_description, 5)?;
// 既存 contextual injection (項目 80) と同形式
```

注入位置: `inject_memory_blocks` (項目 179) と `inject_contextual_memories` (項目 80) の間。順序: **block → heuristics → memory/skill/vault**。

### 4.6 score 更新 (発火後)
heuristic が注入された task が完了したら、`HeuristicStore::record_outcome(id, task_succeeded)`:
- `used_count += 1`
- `success_after_use += if task_succeeded { 1 } else { 0 }`
- `score = (recency × 0.3) + (utility × 0.5) + (diversity × 0.2)`
- `utility = success_after_use / used_count` (default 0.5 for used_count=0)
- `recency = exp(-Δdays / 14.0)` 14 日 half-life

### 4.7 prune (月次 = Lab 30 cycle ごと)
- `score < 0.2 && used_count >= 5` → 廃却 (低 utility 確証)
- `score < 0.2 && used_count == 0 && created_at > 30 日前` → 廃却 (発火なし腐敗)
- 上限 200 件、超過時は score 昇順削除

## 5. TDD strict 5 phase
### Phase 1 — Red
新規テスト ~10 件 (heuristics.rs / vault.rs / extractor.rs / experiment.rs):
- `t_heuristic_store_save_basic` / `t_heuristic_store_dedup_fingerprint`
- `t_extract_heuristics_requires_session_end` / `t_extract_heuristics_min_steps`
- `t_extract_heuristics_returns_skill_for_tool_chain` (HeuristicStore 0 件 + SkillStore erl_ prefix 1 件)
- `t_find_top_k_for_task_filters_by_trigger`
- `t_record_outcome_updates_score` / `t_score_recency_decay`
- `t_vault_creates_heuristics_md`
- `t_heuristic_is_rule`
- `t_lab_cycle_calls_heuristics_extract_pass`

期待: 全 RED (compile error 含む)。

### Phase 2 — Green
1. `src/memory/heuristics.rs` 新規実装 (~250 行): Heuristic struct + HeuristicStore (new/save/find_top_k_for_task/record_outcome/prune) + extract_heuristics_from_events + fingerprint helper
2. `db/schema.rs` SCHEMA_V10 追加 (heuristics テーブル 11 列)
3. `db/migrate.rs` migration 追記
4. `knowledge/extractor.rs::StockCategory::Heuristic` 追加
5. `knowledge/vault.rs::Vault::new` create category に `"heuristics"` 追加
6. `agent/experiment.rs::run_experiment_loop` 末尾 post-pass hook (項目 202 配線と同形式): `run_heuristics_pass(store, lab_start_event_id, backend)?;`
7. `agent/agent_loop.rs` system prompt builder に `inject_heuristics(store, &task_context)` 配線
8. `memory/mod.rs` に `pub mod heuristics;` 追加

期待: 1058 → 1068 passed (+10)、clippy 0 / fmt 0。

### Phase 3 — Refactor
1. reflection prompt を `prompts/heuristic_reflection.txt` に分離
2. score 計算式を `Heuristic::recompute_score(now)` helper に切り出し
3. `find_top_k_for_task` の trigger match を regex pre-compile cache 化 (`LazyLock` / `once_cell`)
4. Bonsai-8B 用 reflection prompt の温度・max_tokens を tune (推奨 temp=0.3, max_tokens=400)
5. `record_outcome` の I/O を `Result<()>` で統一
6. `MemoryStore::reset_session_data_for_lab` (項目 206) と協調する `HeuristicStore::reset_for_lab` 追加

### Phase 4 — Smoke (G-4)
```bash
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 2>&1 | tee /tmp/erl_smoke.log
# 期待 log:
# [INFO][lab.heuristics] ERL post-Lab: extracted=N saved=M skipped_to_skill=K pruned=P
```

判定:
- ✅ Build green (1068 passed 維持)
- ✅ HeuristicStore に少なくとも 1 件 persist
- ✅ duration 増加 ≤ +10% (reflection LLM call cost ≤ 12% Lab budget)
- ✅ events DB に新規 schema migration 成功

### Phase 5 — Effectiveness 検証 (Lab v16 別 plan)
本 plan scope 外。Lab v16 で `--enable-heuristics` flag on/off、core 22 × 5 cycle paired t-test で +Δscore 検証。**ACCEPT 基準: Δscore ≥ +0.015 かつ p < 0.1**。

## 6. API 影響 (新規 public API 一覧)
| modulo path | 関数 / 構造体 |
|---|---|
| `crate::memory::heuristics::Heuristic` | struct (4.1) |
| `HeuristicStore::new / save / find_top_k_for_task / record_outcome / prune / reset_for_lab` | 6 method |
| `extract_heuristics_from_events(events, backend) -> Vec<Heuristic>` | free fn |
| `crate::knowledge::extractor::StockCategory::Heuristic` | enum variant |
| `crate::agent::experiment::run_heuristics_pass(store, since_event_id, backend) -> Result<HeuristicSummary>` | post-Lab hook |
| `HeuristicSummary` | struct (extracted/saved/skipped_to_skill/pruned 4 fields) |

**API 名空間**: 全て新規 + StockCategory 拡張のみ。既存 API signature 変更ゼロ → 後方互換 100%。

## 7. risks / mitigations
| # | Risk | 影響 | Mitigation |
|---|---|---|---|
| **R1** | heuristic 重複爆発 (1bit reflection 同 advice 反復) | DB 肥大、context 圧迫 | content fingerprint dedup (advice 先頭 80 文字) save 強制、項目 206 deterministic dedup 踏襲、prune で score < 0.2 削除 |
| **R2** | Vault md 肥大 (200 件超で vault read コスト増) | Lab duration 退行 | 上限 200、score 昇順削除、`Vault::read_rules(max_per_category)` cap (項目 76) と整合 |
| **R3** | dream_deep との semantics 競合 | 注入冗長、context 混入 | dreams は insight category (`<context type="memory">`)、heuristics は別 type で排他、dreams.rs 側に「Heuristic 候補は HeuristicStore に流す」route 切替追加 |
| **R4** | pre-screen 汚染 (項目 205 と同パターン) | Lab v16 false positive | 項目 205 `scratch_store` 設計踏襲、`extract_heuristics_from_events` 呼び出しは `lab_start_event_id` scoping 必須 (`since_event_id: i64` 引数) |
| **R5** | 1bit variance で効果検証困難 (Δ がノイズに埋没) | ACCEPT/REJECT 不安定 | Phase 5 は 5 cycle paired t-test + 項目 200 RDC/VAF 併用、本 plan は Phase 4 で merge 可、effectiveness 別 plan |
| **R6** | reflection LLM cost (1 cycle +12 min) | Lab duration regression | max_tokens=400 / temp=0.3 制限、Phase 4 で 12 min 想定確認、超過時は失敗 session のみで cost 半減 |
| **R7** | trigger pattern マッチ過剰広範 | 注入過多で context 圧迫 | reflection prompt で「2-5 keywords specific phrase」明示、`find_top_k_for_task` k=5 上限固定、min_length=4 検証で reject |
| **R8** | schema V10 migration 失敗 | Lab 起動不能 | Phase 2 migration test 必須、`CREATE TABLE IF NOT EXISTS`、rollback 手順を SCHEMA_V10 docstring 記述 |

## 8. quality gates
| Gate | 内容 | 検証 | 必須 |
|---|---|---|---|
| **G-1 (Phase 1 Red)** | 新規 ~10 test が compile error or assertion fail で Red 確認 | `cargo test --lib heuristics` | 必須 |
| **G-2 (Phase 2 Green)** | 全 test PASS + 1058 → 1068 passed + clippy 0 + fmt 0 + migration test PASS | `cargo test/clippy/fmt` | 必須 |
| **G-3 (Phase 3 Refactor)** | prompt 切り出し、score helper 抽出、reset_for_lab 追加完了、code 重複ゼロ | self-review | 必須 |
| **G-4 (Phase 4 Smoke)** | core 22 1 cycle で extracted ≥ 1 / saved ≥ 0 / build green / duration ≤ +10% | `BONSAI_LAB_SMOKE=1 --lab` + log grep | 必須 |
| **G-5 (Final)** | net +400 行以下 (production +400、テスト除く) | `git diff --stat` | 任意 |
| **G-6 (Effectiveness, 別 plan)** | Lab v16 paired t-test で Δscore ≥ +0.015 かつ p < 0.1 | 5 cycle 別 handoff | 別 plan |

G-1〜G-4 PASS で merge 可能。

## 9. 見積もり
| Phase | 内容 | 所要 |
|---|---|---|
| **P1 (Red)** | test ~10 件、cargo test Red 確認 | 1.5h |
| **P2 (Green)** | heuristics.rs ~250 行 + schema V10 + extractor + vault + experiment hook + agent_loop inject | 3.5h |
| **P3 (Refactor)** | prompt 切り出し、score helper、reset_for_lab、clippy/fmt | 1h |
| **P4 (Smoke)** | release build + 1 cycle smoke + log 確認 | 1h (うち実機 ~15-20 min) |
| **P5 (commit + handoff)** | 5 commits + CLAUDE.md 項目 + MEMORY.md | 1h |
| **計** | | **~8h、1 day** |

## 10. 次の段階
### 着手判断
- ✅ AgentHER (項目 201-205) production-ready
- ✅ Beyond pass@1 (項目 200) で stability metric 併用可
- ✅ schema V9 → V10 余地あり
- ✅ Lab v15 で天井 5 連続確定、構造変異 evidence 揃った
- ⏳ 1 day まとまった作業時間がある

### 先送り条件
- ❌ Lab v16 設計と reflection 手法統合する場合は先に Lab v16 design plan
- ❌ vllm-mlx 切替実施中なら backend 安定後
- ❌ AgentHER HSL の relabel 件数極端少 (handoff 05-07i `relabels=1` 規模) なら先に HSL yield 改善 (項目 204 拡充)

## 11. Quick Start
```bash
# 1. caller 全網羅
rg -n "run_hindsight_pass|AgentHER post-Lab" src/
rg -n "StockCategory" src/
rg -n "SCHEMA_V" src/db/schema.rs

# 2. Phase 1 Red
$EDITOR src/memory/heuristics.rs        # test mod のみ
$EDITOR src/knowledge/vault.rs          # t_vault_creates_heuristics_md
$EDITOR src/knowledge/extractor.rs      # t_heuristic_is_rule
$EDITOR src/agent/experiment.rs         # t_lab_cycle_calls_heuristics_extract_pass
rtk cargo test --lib heuristics

# 3. Phase 2 Green
$EDITOR src/memory/heuristics.rs        # struct + impl
$EDITOR src/db/schema.rs                # SCHEMA_V10
$EDITOR src/db/migrate.rs               # migration entry
$EDITOR src/knowledge/extractor.rs      # variant + match arms
$EDITOR src/knowledge/vault.rs          # category list
$EDITOR src/agent/experiment.rs         # run_heuristics_pass
$EDITOR src/agent/agent_loop.rs         # inject_heuristics
$EDITOR src/memory/mod.rs               # pub mod heuristics
rtk cargo test --lib --release

# 4. Phase 3 Refactor
$EDITOR src/memory/heuristics.rs
$EDITOR prompts/heuristic_reflection.txt
rtk cargo clippy --release -- -D warnings && rtk cargo fmt --check

# 5. Phase 4 Smoke
rtk cargo build --release
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 | tee /tmp/erl_smoke.log
grep "lab.heuristics" /tmp/erl_smoke.log

# 6. Commit
git add -A && git commit -m "test(erl): Phase 1 Red"
```

## 12. 参考
- arxiv 2603.24639 ERL (2026-03)
- `memory/research_arxiv_2026_05_07.md` 領域 4 (★★★ #3)
- 既存隣接構造: 項目 76/77/161/162/201-206
- 関連 plan: `agenther-option-a-migration.md`, `arag-hierarchical-retrieval-docs.md`
- CLAUDE.md 項目候補: 208 (本 plan 完遂時)
