# Plan: G4 — Task-Complexity-Aware System Prompt 動的生成 (CapabilityTier 連動)

> **由来**: `.claude/plan/building-ai-coding-agents-gap-analysis.md` § 4 G4 (★★ 中優先、P10 Adaptive Prompting 深掘り)。論文 arxiv 2603.05344 "Building AI Coding Agents for the Terminal" の Cursor `.cursorrules` / Devin task-tier prompt 設計を bonsai に取り込む。
>
> **位置付け**: 静的 SOUL.md + DEFAULT_SYSTEM_PROMPT (人間ペルソナ + 16 ルール固定) を **拡張** し、推定 `CapabilityTier` (T1-T6、項目 209) ごとに **追加 directive ブロック** を tier 別 prompt として inject する。SOUL.md は base persona として残し、削除しない。
>
> **falsifiable hypothesis (Phase 5 Lab で検証)**: Adaptive directive ON で 6 tier 別 paired Δscore のうち **少なくとも 1 tier で +0.02 以上、かつ全 tier で -0.01 を下回らない** (副作用なし)。AgentFloor 30 task suite (`agentfloor_tasks()`) で tier 別計測。
>
> **設計の差別化点**: 項目 211 (Self-Verify Phase 5 `BONSAI_LAB_PHASE5_FOCUS`) は「単一 advisor threshold variant の rotate」だったが、本 plan は **CapabilityTier 軸で system prompt 自体を切替** する。`detect_task_complexity` (項目 210 由来、bool) は coarse すぎるため、AgentFloor の 6-tier capability_tier (実装済 commit `2b63441`) を直接活用する。

## Task Type
- [ ] Frontend
- [x] Backend (新規 module `src/agent/task_aware_prompt.rs` + 6 tier prompt template + `inject_memory_blocks` 直後の augment 注入経路、production code は env 未設定で完全互換)
- [ ] Fullstack
- [ ] Docs

## 1. 背景

### 1.1 現状: 静的 system prompt + 静的 SOUL.md
`AgentConfig.system_prompt` は `DEFAULT_SYSTEM_PROMPT` (`src/agent/agent_loop/config.rs:80-120`、16 ルール固定) を使う。`AgentSettings.soul_path` (`src/config.rs:324`) で SOUL.md を 3 段検索 (`load_soul`、`src/agent/context_inject.rs:113-131`)、`inject_memory_blocks` (`context_inject.rs:187-200`) で `<context type="block:persona">` として system 側に注入。両者とも **task complexity に対して静的** = T1 (1 文質問) も T6 (5+ step long-horizon plan) も完全に同じ prompt で実行される。

### 1.2 Cursor / Devin が実証した tier 別 prompt 設計
Cursor `.cursorrules` は階層構造 (project / dir / file 単位)、Devin は task-type 別 system prompt (debug / refactor / new feature) を持つ。論文 arxiv 2603.05344 (gap-analysis plan §3.2 P10) は **「task complexity に応じた system prompt 動的生成は安定 pattern」** と総括。bonsai は項目 8 (SOUL.md) で persona 注入はあるが、complexity 軸の切替がない (P10 部分実装)。

### 1.3 既存資源: 直接活用可能な 2 layer
1. **CapabilityTier (項目 209、Phase 2 Green commit `2b63441`)**: T1-T6 6 tier、`src/agent/benchmark.rs:47-101` 実装済、`BenchmarkTask.capability_tier` に tag 付与済 (1093+ tier-tagged tasks)、production-ready
2. **`detect_task_complexity` (項目 210 由来、`src/agent/agent_loop/outcome.rs:131-152`)**: 既存 bool 判定 (signal_count >= 2 or len > 200)、Lab で実機検証済、ただし **coarse すぎて T1-T6 6 tier には不足**

本 plan は (1) を inference 時の input 推定に流用する **`TaskComplexityClassifier`** を新規追加 (input message → CapabilityTier への 1 関数)、(2) を内部参照のみで再利用 (delete しない)。

### 1.4 Lab 天井 7 連続 (項目 215) との関係
Lab v8/v9/v10/v14/v15/v16/v17 で構造変異 (prompt + config + context level) すべて REJECT、HypothesisGenerator が prompt-level 変異を枯渇 (項目 207 副次知見 = 既デフォルト #47/#50 を再生成)。本 plan は **prompt-level だが「per-tier 局所最適」軸の構造変異** であり、HypothesisGenerator が探索しない領域 (= variant 表は global 1 prompt 想定) を対象にする。期待値は天井打破ではなく、AgentFloor T6 等の高難度 tier に局所改善を入れて weakest_tier (`benchmark.rs:605`) を底上げすること。

### 1.5 1bit Bonsai-8B variance との整合
1bit モデルは pass^k variance が大きい (項目 184 Zone 解析、Lab v17 ON pair 1-4 std≈0.010)。tier 別 prompt 効果が variance noise に埋もれるリスクがあるため、(a) Phase 5 Lab で k=3 minimum 維持、(b) 30 task / 6 tier = 5 task/tier 集計で安定化、(c) 単 tier で +0.02 を threshold とし、tier 集約後の paired t-test ではなく tier ごとの独立判定に倒す。

## 2. 目的

1. **tier 別 prompt 最適化**: T1-T6 ごとに「短文 / 計画強制 / フォールバック / self-revision」など tier-specific directive を追加で inject、weakest_tier 底上げを狙う
2. **Lab 天井 7 連続打破の補助線 (副次目標)**: per-tier 局所最適は global 1 prompt 探索の盲点 (項目 207-215) を埋める軸、ACCEPT であれば打破、REJECT でも tier 別効果差の知見が残る (項目 215 副次 finding pattern 踏襲)
3. **SOUL.md 拡張 (削除しない)**: SOUL.md は base persona として残置、tier prompt は **augment** として system prompt の最後に追加、`inject_memory_blocks` 直後・`inject_heuristics` 直前の位置に挟む

### 非目標
- SOUL.md 削除 / 既存 `load_soul` 経路改変 (両者完全互換維持)
- `DEFAULT_SYSTEM_PROMPT` 16 ルール改変 (既存 prompt は不変、新規 directive は append のみ)
- `detect_task_complexity` (bool) の削除や signature 変更 (項目 210 で advisor skip に活用中、不変維持)
- `CapabilityTier` enum 拡張 / 新 enum 追加 (項目 209 実装済を直接活用、新 tier 軸を作らない)
- production default の挙動変更 (env 未設定で観測動作完全互換、項目 214/216 toggle pattern 踏襲)
- Phase 5 Lab effectiveness の本 plan 内実装 (別 plan で Lab v18+ 候補)

## 3. 既存項目との関係

| 項目 | 関係 | 改修要否 |
|---|---|---|
| **8** SOUL.md 3 段検索 | base persona 経路、本 plan は augment のみ追加 | 不変 (read-only 流用) |
| **47** ツール使用前 `<think>` 強制 (Lab v6.2 ACCEPT、デフォルト化) | T2-T6 directive で「ツール使用前 think」を tier 別に強調 (重複ではなく強調レベル調整) | 不変 |
| **50** フォールバック戦略 (Lab v6.2 ACCEPT) | T5 ErrorRecovery directive の中核に組込 | 不変 |
| **136** 回答前ファイル内容確認 (Lab v9 ACCEPT) | T3-T6 directive で再強調 | 不変 |
| **80** Contextual memory injection (`context_inject.rs`) | 同 module に注入関数を追加、配置順序は memory_blocks 直後・heuristics 直前 | minor add |
| **172** TaskTier (Core/Extended) | 直交軸、本 plan は CapabilityTier 軸のみ参照 | 不変 |
| **179** MemoryBlock + load_blocks (Letta candidate 3) | 既存 block 注入経路の直後に tier directive 注入 | 不変 (順序のみ) |
| **209** CapabilityTier (Phase 2 Green commit `2b63441`) | 本 plan の中核依存、`benchmark.rs:47-101` の enum + `paper_baseline` を直接利用 | 不変 (read-only 依存) |
| **210** Self-Verify dynamic skip (`AdvisorConfig.dynamic_skip_threshold`) | `detect_task_complexity` を共有、本 plan は別経路で CapabilityTier 推定 | 不変 (read-only 共存) |
| **214** ERL toggle 機構 (`BONSAI_ERL_ENABLED`) | env opt-in pattern を踏襲: `BONSAI_TASK_AWARE_PROMPT_ENABLED=1` | 設計踏襲 |
| **216** ERL defaults OFF 切替 | 同パターン (production default = env unset で legacy) | 設計踏襲 |
| **211** Self-Verify Phase 5 `BONSAI_LAB_PHASE5_FOCUS` | Lab variant 機構、本 plan は Phase 5 Lab で同等の env focus を別 plan で起票候補 | 設計踏襲 (将来) |
| **207** Lab v15 baseline=0.7812 Zone A | base 比較値、Phase 5 Lab で control とする | 不変 (read-only) |
| **215** Lab v17 REJECT 副次 finding (stability) | 効果検証の手本、本 plan も score + stability 併記で評価 | 設計踏襲 |

## 4. 設計

### 4.1 新規 module `src/agent/task_aware_prompt.rs` (~180 行)

```rust
//! Task-Complexity-Aware System Prompt augment (G4、項目 222 候補)。
//!
//! `CapabilityTier` (項目 209 = arxiv 2605.00334 AgentFloor) を inference 時 input から
//! 推定し、tier 別 directive を system prompt の最後 (memory_blocks 直後 / heuristics 直前)
//! に append する。production default OFF (`BONSAI_TASK_AWARE_PROMPT_ENABLED` env unset で
//! 観測動作完全互換)、env=1 で opt-in。
//!
//! 設計上 SOUL.md / DEFAULT_SYSTEM_PROMPT は不変、tier directive は augment のみ。

use crate::agent::benchmark::CapabilityTier;
use crate::agent::conversation::{Message, Session};
use crate::config::TaskAwarePromptConfig;

/// `BONSAI_TASK_AWARE_PROMPT_ENABLED=1` (or `true`) で opt-in。
pub(crate) fn is_task_aware_prompt_enabled() -> bool {
    std::env::var("BONSAI_TASK_AWARE_PROMPT_ENABLED")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// 入力 (user message + 任意 hint) から CapabilityTier を推定する classifier。
///
/// 推定 input:
///   - user message 文字列 (必須)
///   - 任意の expected_tools hint (None でも動作、AgentFloor smoke は Some 注入)
///
/// 推定アルゴリズム (priority order):
///   1. 長さ + 複数動詞 (項目 210 detect_task_complexity 互換) で T6 候補
///   2. error / fix / debug / fallback keyword で T5
///   3. tool count >= 2 + ステップ語 ("then", "next") で T4
///   4. 複数ツール候補語 ("list", "search", "find") で T3
///   5. 単一ツール語 ("read", "write", "run") で T2
///   6. デフォルト T1
///
/// 推定不能 (低 confidence) なら None 返却 → SOUL.md のみで legacy 動作 (R1 軽減)。
pub struct TaskComplexityClassifier;

impl TaskComplexityClassifier {
    /// 推定実装。閾値 hit signals < 1 で None、>=1 で Some(tier)。
    pub fn classify(input: &str, hint_tools: Option<&[String]>) -> Option<CapabilityTier> {
        // ... priority 上書き優先 (T6 → T5 → T4 → T3 → T2 → T1)
        // 単純 keyword + length + hint_tools.len() の合議
        // signal_count = 0 で None
        todo!()
    }
}

/// tier 別 directive を fs から読込 (`AgentSettings.task_aware_prompts` Config 由来)。
/// path 不在 / 内容空白で None (graceful skip、R3 軽減)。
fn load_directive(
    config: &TaskAwarePromptConfig,
    tier: CapabilityTier,
) -> Option<String> {
    let path = config.tier_path(tier)?;
    std::fs::read_to_string(path).ok().filter(|s| !s.trim().is_empty())
}

/// session に tier directive を inject。
/// `inject_memory_blocks` (SOUL.md persona) の直後、`inject_heuristics` の直前に呼ぶ。
/// 注入位置は項目 80 タグ統一方針: `<context type="task_aware_directive:t{1-6}">`。
///
/// 注入順序保証 (重要):
///   1. system_prompt (DEFAULT_SYSTEM_PROMPT 16 ルール)
///   2. inject_memory_blocks (block:persona = SOUL.md、+ extras)
///   3. **inject_task_aware_directive (本関数)** ← ここ
///   4. inject_heuristics (項目 213)
///   5. inject_contextual_memories (memory/experience/skill/graph、項目 80)
///   6. inject_planning_step (advisor、項目 10/89)
pub(crate) fn inject_task_aware_directive(
    session: &mut Session,
    task_context: &str,
    config: &TaskAwarePromptConfig,
) -> Option<CapabilityTier> {
    if !is_task_aware_prompt_enabled() { return None; }
    let tier = TaskComplexityClassifier::classify(task_context, None)?;
    let directive = load_directive(config, tier)?;
    let tag = match tier {
        CapabilityTier::InstructionFollowing => "t1",
        CapabilityTier::SingleToolUse => "t2",
        CapabilityTier::ToolSelection => "t3",
        CapabilityTier::MultiStepToolChain => "t4",
        CapabilityTier::ErrorRecovery => "t5",
        CapabilityTier::LongHorizonPlanning => "t6",
    };
    session.add_message(Message::system(format!(
        "<context type=\"task_aware_directive:{tag}\">\n{}\n</context>",
        directive.trim()
    )));
    Some(tier)
}
```

**重要設計点**:
- `is_task_aware_prompt_enabled()` env 短絡 = production default OFF、項目 214/216/217 と同パターン
- `classify()` は **None 返却を許容** = 推定不能なら SOUL.md のみで legacy 動作 (R1 軽減)
- directive 読込失敗 (file 不在 / 内容空白) で graceful skip = R3 軽減
- 注入位置は memory_blocks 直後・heuristics 直前 = **persona 後・タスク特化指示前** の自然な順序、項目 80 タグ統一方針に準拠
- `CapabilityTier` enum 拡張なし = 項目 209 実装をそのまま利用

### 4.2 prompts/ ディレクトリ tier 別 directive (6 file)

`prompts/task_aware/` 新規 dir、6 file:

```
prompts/task_aware/t1.txt   InstructionFollowing  ~3-5 行: 短く直接答える、ツール不要時はツール呼ばない
prompts/task_aware/t2.txt   SingleToolUse         ~5-8 行: 1 ツール選択、結果簡潔要約
prompts/task_aware/t3.txt   ToolSelection         ~8-12 行: 複数候補から選択、判断 think で明示
prompts/task_aware/t4.txt   MultiStepToolChain    ~12-18 行: 計画強制 (項目 10 重ね掛け)、step 列挙、tool_chain example
prompts/task_aware/t5.txt   ErrorRecovery         ~15-20 行: フォールバック戦略 (項目 50 強調)、エラー観察→別 tool、loop 回避
prompts/task_aware/t6.txt   LongHorizonPlanning   ~20-25 行: 中間検証 step、self-revision 強制、subgoal 分解、stall 検出後の再計画指示
```

**ファイル数 = 6** (旧 plan 概念の "18 = 6 tier × 3 variant" は scope creep 回避のため tier 単位 1 variant に簡素化、Phase 5 で variant 化検討)。

各 directive 文面は **DEFAULT_SYSTEM_PROMPT 16 ルールと矛盾しない** ことを Phase 3 self-review で確認 (R6 軽減)。

### 4.3 `inject_memory_blocks` 直後・`inject_heuristics` 直前への配線

`src/agent/agent_loop/core.rs:136-141` の既存順序:
```rust
inject_memory_blocks(session, &config.soul_path, &config.memory_blocks);
let injected_heuristic_ids = inject_heuristics(session, &task_context, store);
inject_contextual_memories(session, &task_context, store);
inject_planning_step(session, &task_context);
```

**変更後**:
```rust
inject_memory_blocks(session, &config.soul_path, &config.memory_blocks);
// G4 項目 223 候補: tier 別 directive (env=BONSAI_TASK_AWARE_PROMPT_ENABLED=1 で opt-in)。
// production default unset で no-op (legacy 完全互換)。推定 None で skip。
let _injected_tier = crate::agent::task_aware_prompt::inject_task_aware_directive(
    session,
    &task_context,
    &config.task_aware_prompts,
);
let injected_heuristic_ids = inject_heuristics(session, &task_context, store);
inject_contextual_memories(session, &task_context, store);
inject_planning_step(session, &task_context);
```

`_injected_tier` は本 plan では使わない (将来 Phase 5 Lab metric で audit log に流す候補)。

### 4.4 env opt-in: `BONSAI_TASK_AWARE_PROMPT_ENABLED=1`

| state | 動作 | 互換性 |
|---|---|---|
| env unset (production default) | `is_task_aware_prompt_enabled() = false` で短絡、no-op | observable 動作 100% 互換 |
| env="1" or "true" | classifier 起動、推定成功で directive inject | augment 経路 |
| env="0" or その他 | 短絡 (parse error 扱いで OFF) | OFF 互換 |

env name 命名は項目 217 `BONSAI_DECAY_ENABLED` / 項目 218 `BONSAI_REVIEW_ENABLED` / 項目 219 `BONSAI_WORKING_CAP_ENABLED` の 3 機構と整合 (`BONSAI_<feature>_ENABLED` opt-in 形式)。

### 4.5 推定不能 / 読込失敗時は SOUL.md のみで legacy 動作

R1/R3 軽減のため、以下 3 case で **silent skip** (panic / log error しない):

1. `TaskComplexityClassifier::classify()` が None 返却 (signal_count = 0、低 confidence)
2. `config.task_aware_prompts.tier_path(tier)` が None 返却 (該当 tier の path 未設定)
3. `load_directive()` が None 返却 (file 不在 / 内容空白)

すべての case で `inject_task_aware_directive` は `None` 返却、session には何も追加しない = SOUL.md + DEFAULT_SYSTEM_PROMPT のみで legacy 動作と同一。

### 4.6 Lab metric: tier 別 score 改善計測

Phase 5 Lab effectiveness (別 plan、本 plan の delivery 範囲外、Lab v18+ 候補):

- `MultiRunTaskScore` に `inferred_capability_tier: Option<CapabilityTier>` 追加 (audit log 経由で classifier 推定結果を保存)、または既存 `BenchmarkTask.capability_tier` を ground truth として使い classifier 一致率 (= classifier accuracy) を別途計測
- AgentFloor `agentfloor_tasks()` (30 task = 5/tier × 6 tier、Phase 2 Green 実装済) で paired t-test ON/OFF
- ACCEPT 判定: 6 tier 中 1 tier 以上で **paired Δ ≥ +0.02 かつ全 tier で Δ ≥ -0.01** (副作用なし)
- score axis に加えて項目 200 RDC/VAF/GDS reliability_decay も併記 (Lab v17 副次 finding pattern 踏襲)

本 plan では Phase 5 Lab は **scope 外**、infra (env opt-in + classifier + directive) のみ提供。

## 5. TDD strict 5 phase

### Phase 1 — Red (新規 ≥ 7 test)

**`src/agent/task_aware_prompt.rs` 純関数 / classifier test (5 件)**:
- `t_is_task_aware_prompt_enabled_default_false` — env unset で false (項目 214 同パターン)
- `t_is_task_aware_prompt_enabled_explicit_true` — env="1" / "true" で true、"0" / "false" / その他で false
- `t_classify_short_question_returns_t1` — "今何時？" → InstructionFollowing
- `t_classify_complex_implementation_returns_t6` — 250 文字超 + "実装して" + "テスト書いて" → LongHorizonPlanning
- `t_classify_low_confidence_returns_none` — 空文字列 / 1 文字 → None (skip 経路)

**`inject_task_aware_directive` 統合 test (3 件、合計 8)**:
- `t_inject_short_circuits_when_env_unset` — env 未設定で session 不変 + None 返却 (legacy 互換確証)
- `t_inject_appends_directive_when_enabled` — env="1" + tmpdir prompt file + classify 成功で `<context type="task_aware_directive:t{n}">` 含む 1 メッセージ追加 + Some(tier) 返却
- `t_inject_skips_when_directive_file_missing` — env="1" + classify 成功 + path None / file 不在で session 不変 + None 返却 (graceful skip、R3 確証)

env mutation race avoidance: module-local `static MUTEX: Mutex<()>` で serialize (項目 218 `REVIEW_INJECT_TEST_LOCK` pattern 踏襲、`context_inject.rs:639` 参照)。

期待: compile error (新規 module / `TaskAwarePromptConfig` / `CapabilityTier` 流用 / `inject_task_aware_directive` 未定義) → Red 確認。

### Phase 2 — Green

1. **`src/config.rs`** に `TaskAwarePromptConfig` 追加 (additive、`AgentSettings` に opt-in field):
   ```rust
   #[derive(Debug, Clone, Default, Serialize, Deserialize)]
   pub struct TaskAwarePromptConfig {
       /// path map: tier 短コード → directive file path。
       /// 未設定 tier は load_directive で None。
       #[serde(default)]
       pub tiers: std::collections::HashMap<String, std::path::PathBuf>,
   }
   impl TaskAwarePromptConfig {
       pub fn tier_path(&self, tier: CapabilityTier) -> Option<&std::path::PathBuf> {
           self.tiers.get(tier.short_code())  // benchmark.rs::short_code() 流用
       }
   }
   ```
   `AgentSettings` に `pub task_aware_prompts: TaskAwarePromptConfig` 追加 (default = empty、TOML 互換維持)。
2. **`src/agent/agent_loop/config.rs::AgentConfig`** に `pub task_aware_prompts: TaskAwarePromptConfig` 追加 (Default::default で empty)、項目 179 `memory_blocks` の隣に配置
3. **`src/agent/task_aware_prompt.rs` 新規** (~180 行、§ 4.1 設計通り、todo!() を実装に置換)
4. **`src/agent/mod.rs`** に `pub mod task_aware_prompt;` 追加
5. **`src/agent/agent_loop/core.rs:136-141`** の `inject_memory_blocks` 直後に `inject_task_aware_directive` call 追加 (§ 4.3 設計通り)
6. **`prompts/task_aware/t{1-6}.txt`** 6 file 新規 (§ 4.2 内容、各 ≤ 25 行、UTF-8、Bonsai-8B 1bit 向け簡潔表現)
7. **`AgentSettings` config 経路** が builder で `AgentConfig.task_aware_prompts` に伝播することを確認 (`benchmark.rs:1638` / `1730` の `soul_path` 同様)

期待: **1150 → ≥ 1158 passed (+8 / clippy 0 / fmt 0)**、env 未設定で既存 1150 test 退行ゼロ。

### Phase 3 — Refactor

- `task_aware_prompt.rs` docstring (G4 plan 参照、項目 209 CapabilityTier 利用、env name 由来)
- `classify()` の keyword set を `const &[&str]` 配列に抽出 + 文書コメント (将来 PR で外部化容易化)
- `inject_task_aware_directive` の戻り値 `Option<CapabilityTier>` を audit log 経由で `LogLevel::Debug` 出力 (Lab metric 準備、将来 Phase 5 で活用)
- env mutation test を test-local Mutex で serialize (項目 218 pattern 踏襲)
- prompt file 6 件の冒頭に `# tN-{TierName} directive (G4 項目 223 候補)` ヘッダ統一
- DEFAULT_SYSTEM_PROMPT 16 ルールとの矛盾チェック (Phase 3 self-review、結果は plan section 13 の review log 追記)

### Phase 4 — Smoke (G-4)

`cargo test --release task_aware_prompt context_inject` で 8 新規 test green、既存 1150 test 退行ゼロ確認。

実機 smoke (release build、env=1 + tmp prompts/task_aware/{t1-t6}.txt 簡素化版):
1. `cargo build --release`
2. `BONSAI_TASK_AWARE_PROMPT_ENABLED=1 cargo run --release -- --benchmark --core` で 1 cycle 完走 (~15 min)
3. log で `task_aware_directive:t{n}` タグが session に挿入されていること確認 (debug level)
4. score regression 範囲確認 (handoff 05-08 baseline=0.7344 ± 0.05 = 1bit variance 範囲、ACCEPT/REJECT 判定は Phase 5 Lab で paired t-test 実施)

env 未設定で同 smoke を 1 回追加実行 → score が baseline ± 0.05 以内 = legacy 経路無影響確証。

### Phase 5 — Effectiveness (Lab v18+ 候補、別 plan)

本 plan では Phase 5 を **scope 外** とする (項目 217 同 pattern)。infra delivery 完了で本 plan 完結、effectiveness 検証は別 plan (`lab-v18-task-aware-prompt-effectiveness.md` 候補) で起票:

- `agentfloor_tasks()` 30 task / k=3 / paired ON/OFF
- ACCEPT 判定: 6 tier 中 1 tier 以上で paired Δ ≥ +0.02 かつ全 tier で Δ ≥ -0.01
- 副次評価: 項目 200 RDC/VAF/GDS reliability_decay tier 別 variance 観測
- REJECT 時 = `BONSAI_TASK_AWARE_PROMPT_ENABLED` production default unset 維持 (= 構造変更不要、項目 217 dead-code 候補化 pattern 踏襲)、prompts/task_aware/ は他 store / 他軸変異の base として残置

## 6. API 影響

| modulo path | 関数 / 構造体 | 種別 |
|---|---|---|
| `crate::agent::task_aware_prompt` | new pub mod | 新規 |
| `crate::agent::task_aware_prompt::is_task_aware_prompt_enabled` | pub(crate) fn | 新規 |
| `crate::agent::task_aware_prompt::TaskComplexityClassifier::classify` | pub fn | 新規 |
| `crate::agent::task_aware_prompt::inject_task_aware_directive` | pub(crate) fn | 新規 |
| `crate::config::TaskAwarePromptConfig` | pub struct + Default + Serialize/Deserialize | 新規 |
| `crate::config::TaskAwarePromptConfig::tier_path` | pub fn | 新規 |
| `crate::config::AgentSettings::task_aware_prompts` | pub field (default empty) | 新規 (Default 経由で TOML 後方互換) |
| `crate::agent::agent_loop::AgentConfig::task_aware_prompts` | pub field (default empty) | 新規 (Default 経由で 後方互換) |
| `crate::agent::agent_loop::core::run_agent_loop_with_session` | inject 1 行追加 | 拡張 (signature 不変) |
| `crate::agent::benchmark::CapabilityTier::short_code` | (既存) `tier_path` 内部で参照 | 不変 (read-only 利用) |

**API 完全 additive** (signature 変更ゼロ、新 field は Default 経由で TOML 後方互換、env unset で観測動作完全互換)。

`AgentSettings` への field 追加は項目 179 `memory.blocks` と同パターン (`#[serde(default)]` で旧 TOML 互換)。

## 7. Risks / Mitigations

| # | Risk | 影響 | Mitigation |
|---|---|---|---|
| **R1** | `TaskComplexityClassifier::classify()` の誤判定で間違った directive 注入 (T6 long-horizon を T1 と誤認 → directive なしで legacy / T1 を T6 と誤認 → 過剰 directive で逆効果) | tier 別効果の打ち消し / 単 tier 退行 | (i) classify が None 返却で skip 経路を設計 (4.1 / 4.5)、(ii) priority order を T6→T5→...→T1 で定義し過剰検出より過小検出に倒す、(iii) Phase 5 Lab で BenchmarkTask.capability_tier ground truth と classifier 推定の一致率を audit、< 0.6 なら classifier 改修 plan 起票 |
| **R2** | tier-specific prompt の cross-tier hallucination (T6 directive 内の "5+ step plan" 強調が T2 task で過剰計画 → ノイズ) | 単 tier 退行 | (i) 各 directive は 1 tier 範囲に閉じる文面 (§ 4.2 行数上限)、(ii) Phase 3 self-review で 6 file × 16 ルール の cross-check、(iii) Phase 5 Lab で全 tier Δ ≥ -0.01 の副作用なし条件を falsifiable hypothesis に明記 |
| **R3** | SOUL.md / DEFAULT_SYSTEM_PROMPT 16 ルールとの整合性破綻 (重複・矛盾) | session 全体の prompt 衝突で 1bit モデル混乱 | (i) directive は **augment のみ、override しない**、(ii) Phase 3 で 6 file × DEFAULT_SYSTEM_PROMPT 16 ルール と SOUL.md 既定文面の cross-check matrix を作成 (review log)、(iii) 重複は許容 (項目 47/50/136 の重ね掛け強調)、矛盾は文面修正 |
| **R4** | env opt-in の test 並列実行で env mutation race (`std::env::set_var`) | 偽陽性 / 偽陰性 test fail | module-local `static TASK_AWARE_TEST_LOCK: Mutex<()>` で serialize (項目 218 REVIEW_INJECT_TEST_LOCK pattern 踏襲、`context_inject.rs:639` 同形式) |
| **R5** | 1bit Bonsai-8B variance > tier 別 directive 効果 (Lab v17 ON pair 1-4 std≈0.010、項目 215 副次) | Phase 5 Lab で全 tier 差が noise に埋もれ REJECT | (i) k=3 minimum 維持、(ii) AgentFloor 30 task / 5 task per tier で集約安定化、(iii) ACCEPT 閾値を「全体 paired t-test」ではなく「単 tier Δ ≥ +0.02 + 全 tier Δ ≥ -0.01」に倒す (4.6 / 5 設計済) |
| **R6** | prompt template 6 file の保守負担 (将来 directive 改訂で同期漏れ) | 直接削除可能 (env unset で no-op) なので blast radius 限定 | (i) 6 file は最小 scope (variant なし、tier 1 file)、(ii) Phase 5 REJECT 時は prompts/task_aware/ ごと削除可 (env unset で legacy)、(iii) Phase 3 review log に「6 file 改訂時 cross-check matrix 必須」明記 |
| **R7** | `inject_task_aware_directive` 注入位置のバグで heuristics / contextual memory の前後関係崩壊 | session 構造汚染で他項目影響 | (i) § 4.3 で順序を明文化、(ii) `t_inject_appends_directive_when_enabled` で session.messages.len() == before + 1 + tag prefix 確証、(iii) heuristics inject test (`context_inject.rs:601-635`) は本 plan で改変なし、退行ゼロ確証 |

## 8. Quality Gates

| Gate | 内容 | 検証 |
|---|---|---|
| **G-1 (Phase 1 Red)** | 8 test compile error or assertion fail で Red 確認 | `cargo test --lib task_aware_prompt 2>&1 \| grep "test result"` |
| **G-2 (Phase 2 Green)** | 1150 → ≥ 1158 passed + clippy 0 + fmt 0、env unset で既存 1150 test 退行ゼロ | `cargo test --release && cargo clippy --tests -- -D warnings && cargo fmt --check` |
| **G-3 (Phase 3 Refactor)** | docstring + cross-check matrix (16 ルール × 6 directive) + test mutex | self-review + review log を本 plan section 13 に追記 |
| **G-4 (Phase 4 Smoke)** | release build で env=1 / env unset 各 1 cycle 完走、score baseline ± 0.05 (1bit variance 範囲)、`task_aware_directive:t{n}` tag が session に挿入確認 | `cargo run --release -- --benchmark --core` × 2 |
| **G-5 (production default OFF 確証)** | env 未設定で `is_task_aware_prompt_enabled() == false`、`inject_task_aware_directive` 短絡、session 不変 | unit test `t_inject_short_circuits_when_env_unset` |
| **G-6 (Effectiveness、Phase 5 Lab、別 plan)** | Lab v18 paired t-test で AgentFloor 30 task / k=3 / ON-OFF、6 tier 中 1 tier ≥ +0.02 + 全 tier ≥ -0.01 で ACCEPT | 別 plan |

G-1 〜 G-5 PASS で本 plan merge 可能。G-6 は別 plan の責務 (項目 217/218 と同設計)。

## 9. 完了条件

1. `src/agent/task_aware_prompt.rs` 新規モジュール ~180 行 + `is_task_aware_prompt_enabled` + `TaskComplexityClassifier::classify` + `inject_task_aware_directive` 実装
2. `prompts/task_aware/t{1-6}.txt` 6 file 新規 (§ 4.2、各 ≤ 25 行、Bonsai-8B 1bit 向け簡潔表現)
3. `src/config.rs` に `TaskAwarePromptConfig` 追加 + `AgentSettings.task_aware_prompts` field 追加 (additive、TOML 後方互換)
4. `src/agent/agent_loop/config.rs::AgentConfig.task_aware_prompts` field 追加 + Default::default で empty
5. `src/agent/agent_loop/core.rs:136-141` の inject 順序に `inject_task_aware_directive` を memory_blocks 直後・heuristics 直前に挿入
6. Phase 1 Red で ≥ 8 test 追加、Phase 2 Green で 1150 → ≥ 1158 passed
7. clippy 0 + fmt 0 + env unset で既存 1150 test 退行ゼロ
8. env=1 で release build smoke 成功、`task_aware_directive:t{n}` tag が session に挿入確認
9. Phase 3 review log を本 plan § 13 に追記 (6 directive × 16 ルール cross-check matrix)
10. CLAUDE.md 項目 222 candidates (= 223 候補) 追記 + MEMORY.md handoff 追加 + 2-3 commit ahead

## 10. 見積もり

| Phase | 内容 | 所要 |
|---|---|---|
| **P1 (Red)** | 8 test (5 純関数 + 3 統合)、cargo test Red 確認 | 0.7h |
| **P2 (Green)** | task_aware_prompt.rs 実装 + config 拡張 + core.rs 1 行追加 + prompts/ 6 file + cargo test Green | 2.5h |
| **P3 (Refactor)** | docstring + keyword 抽出 + audit log debug + test mutex + 16 ルール × 6 directive cross-check matrix | 1h |
| **P4 (Smoke)** | release build × 2 (env=1 / env unset) + log tag 確認 | 1h |
| **P5 (Effectiveness)** | scope 外 (別 plan、~6h、Lab v18+ 候補) | — |
| **P6 (commit + handoff)** | 2-3 commits + CLAUDE.md 項目 candidate + MEMORY.md handoff | 0.5h |
| **計 (本 plan delivery)** | | **~5.7h ≈ 0.7-1 day** |

Phase 5 Lab effectiveness は別 plan で +6h、合計 ~12h ≈ 1.5 day (但し本 plan の責務外)。

## 11. Quick Start

```bash
# 0. 着手前 verify
cargo test --release 2>&1 | tail -5     # 1150 passed baseline
cat .claude/plan/task-aware-system-prompt-impl.md   # 本 plan 確認
cat .claude/plan/building-ai-coding-agents-gap-analysis.md   # 由来 G4 確認

# 1. Phase 1 Red
$EDITOR src/agent/task_aware_prompt.rs        # 新規 module、todo!() 含む 8 test
$EDITOR src/agent/mod.rs                      # pub mod task_aware_prompt 追加
$EDITOR src/config.rs                         # TaskAwarePromptConfig + AgentSettings field
$EDITOR src/agent/agent_loop/config.rs        # AgentConfig.task_aware_prompts field
cargo test --lib task_aware_prompt --release 2>&1 | grep "test result"
# 期待: compile error or test fail (Red 確認)

# 2. Phase 2 Green
# task_aware_prompt.rs の todo!() を § 4.1 設計に従い実装
mkdir -p prompts/task_aware
$EDITOR prompts/task_aware/t1.txt
$EDITOR prompts/task_aware/t2.txt
$EDITOR prompts/task_aware/t3.txt
$EDITOR prompts/task_aware/t4.txt
$EDITOR prompts/task_aware/t5.txt
$EDITOR prompts/task_aware/t6.txt
$EDITOR src/agent/agent_loop/core.rs          # inject 1 行追加 (memory_blocks 直後)
cargo test --release && cargo clippy --tests -- -D warnings && cargo fmt --check
# 期待: 1150 → ≥ 1158 passed、clippy 0、fmt 0

# 3. Phase 3 Refactor
# docstring + audit log + 16 ルール × 6 directive cross-check matrix を本 plan § 13 に追記
$EDITOR src/agent/task_aware_prompt.rs
$EDITOR .claude/plan/task-aware-system-prompt-impl.md   # § 13 review log 追記

# 4. Phase 4 Smoke
cargo build --release
BONSAI_TASK_AWARE_PROMPT_ENABLED=1 cargo run --release -- --benchmark --core 2>&1 | tee smoke_on.log
cargo run --release -- --benchmark --core 2>&1 | tee smoke_off.log
grep "task_aware_directive" smoke_on.log    # tag 確認
# 期待: env=1 で tag 挿入、env unset で tag 不在、score baseline ± 0.05 範囲

# 5. Commit
git add src/agent/task_aware_prompt.rs src/agent/mod.rs src/config.rs \
        src/agent/agent_loop/config.rs src/agent/agent_loop/core.rs \
        prompts/task_aware/ .claude/plan/task-aware-system-prompt-impl.md
git commit -m "feat(prompt): G4 task-aware system prompt augment 実装 (項目 223 候補)"

# 6. (別 session) Phase 5 Lab effectiveness 別 plan 起票
$EDITOR .claude/plan/lab-v18-task-aware-prompt-effectiveness.md
```

## 12. 参考

### 由来
- `.claude/plan/building-ai-coding-agents-gap-analysis.md` § 4 G4 (★★ 中優先、本 plan 起票元)
- arxiv 2603.05344 "Building AI Coding Agents for the Terminal: Scaffolding, Harness, Context Engineering" (2026-03、P10 Adaptive Prompting)
- arxiv 2605.00334 "How Far Up the Tool Use Ladder Can Small Open-Weight Models Go?" (項目 209 AgentFloor 6-tier)

### bonsai 既存 plan (本 plan 参照)
- `.claude/plan/agentfloor-tier-eval-impl.md` (CapabilityTier 設計手本、Phase 2 Green commit `2b63441` 実装済、本 plan の中核依存)
- `.claude/plan/cerememory-decay-port-impl.md` (env opt-in pattern + production default OFF 設計、§ 4.4 / 4.5 / R4 の手本)
- `.claude/plan/self-verify-dilemma-impl.md` (動的 advisor 機構の前例、項目 210 / 211 経路)
- `.claude/plan/erl-defaults-off-switch-impl.md` (env name `BONSAI_<feature>_ENABLED` 統一、項目 216 同設計)
- `.claude/plan/lab-v17-erl-effectiveness.md` (Phase 5 Lab variant 機構の手本、項目 211/212 BONSAI_LAB_PHASE5_FOCUS pattern)
- `.claude/plan/cerememory-extension-roadmap-d-g.md` (Phase D-G env opt-in roadmap、本 plan は同系列の Phase H 候補)

### bonsai 既存項目 (本 plan で reference する CLAUDE.md 項目)
- 項目 8 (SOUL.md 3 段検索) — base persona 経路、本 plan は augment 拡張
- 項目 47 (ツール使用前 `<think>`、Lab v6.2 ACCEPT デフォルト化) — T2-T6 directive で重ね掛け
- 項目 50 (フォールバック戦略、Lab v6.2 ACCEPT) — T5 ErrorRecovery の中核
- 項目 80 (contextual memory injection) — タグ統一方針 `<context type="...">` 準拠
- 項目 136 (回答前ファイル内容確認、Lab v9 ACCEPT) — T3-T6 directive で重ね掛け
- 項目 172 (TaskTier Core/Extended) — 直交軸、本 plan は CapabilityTier 軸のみ
- 項目 179 (MemoryBlock + load_blocks) — `inject_memory_blocks` 直後に本 plan の inject 配線
- 項目 209 (CapabilityTier、Phase 2 Green commit `2b63441`) — 本 plan の中核依存
- 項目 210 (Self-Verify dynamic skip + `detect_task_complexity`) — 並存、本 plan は別経路
- 項目 211 (Self-Verify Phase 5 `BONSAI_LAB_PHASE5_FOCUS`) — Phase 5 Lab variant 機構手本
- 項目 213 (ERL Heuristics Pool) — `inject_heuristics` 直前に本 plan の inject 配線
- 項目 214 (ERL toggle 機構 `BONSAI_ERL_DISABLED` → 216 で `_ENABLED` 統一) — env opt-in pattern
- 項目 216 (ERL defaults OFF 切替) — production default OFF + env opt-in 設計手本
- 項目 217 (Cerememory decay `BONSAI_DECAY_ENABLED`) — env name 統一
- 項目 218 (Cerememory ReviewState `BONSAI_REVIEW_ENABLED` + REVIEW_INJECT_TEST_LOCK) — test mutex pattern
- 項目 219 (Working Memory Cap `BONSAI_WORKING_CAP_ENABLED`) — 三本柱完成と同 env opt-in
- 項目 207 (Lab v15 baseline=0.7812 Zone A) — score base 比較値
- 項目 215 (Lab v17 REJECT 副次 finding stability) — Phase 5 評価設計手本

### bonsai source files (本 plan で 参照 / 改修)
- `src/agent/agent_loop/core.rs` (inject 順序、136-141 行に 1 行追加)
- `src/agent/agent_loop/config.rs` (AgentConfig field 追加)
- `src/agent/context_inject.rs` (inject_memory_blocks / inject_heuristics 既存、本 plan は別 module で並列)
- `src/agent/task_aware_prompt.rs` (新規)
- `src/agent/benchmark.rs` (CapabilityTier::short_code 利用、47-101 行 read-only)
- `src/agent/agent_loop/outcome.rs` (detect_task_complexity 既存 bool、131-152 行 read-only)
- `src/agent/agent_loop/advisor_inject.rs` (classify_task_type 既存、本 plan は別経路で classifier を持つ)
- `src/config.rs` (AgentSettings + MemoryBlockConfig pattern 踏襲、321-366 行)
- `prompts/task_aware/t{1-6}.txt` (新規 6 file)

### CODEX_SESSION
- (新規取得 — meta 由来 plan は building-ai-coding-agents-gap-analysis、本 plan の Phase 1-4 着手時に取得)

## 13. Phase 3 Review Log (Phase 3 完了後追記)

### 13.1 6 directive × 16 ルール cross-check matrix
> Phase 3 着手後にここに追記する。各セルは「重複」(= 重ね掛け強調 OK) / 「矛盾」(= 文面修正必須) / 「独立」(= no-op) の 3 値。

| 16 ルール ↓ \ tier → | t1 | t2 | t3 | t4 | t5 | t6 |
|---|---|---|---|---|---|---|
| 1 簡潔 | (TBD) | (TBD) | (TBD) | (TBD) | (TBD) | (TBD) |
| 2 繰り返し回避 | (TBD) | (TBD) | (TBD) | (TBD) | (TBD) | (TBD) |
| ... | ... | ... | ... | ... | ... | ... |

### 13.2 SOUL.md base persona との整合
> Phase 3 着手後にここに追記する。SOUL.md base persona の文面 (3 段検索でのデフォルト) と各 tier directive の整合性を確認。

## 14. ★ Phase 5 Effectiveness REJECT 時 handling

Lab v18 paired t-test で全 tier Δscore が +0.02 未満:

1. **production default `BONSAI_TASK_AWARE_PROMPT_ENABLED` 未設定維持** (= legacy 既定化、構造変更不要)
2. **`prompts/task_aware/` 全削除候補** (env unset で無効、保守負担削減)、または将来の variant base として残置
3. **`task_aware_prompt.rs` dead-code 候補化** (項目 217 ERL pattern 踏襲、CLAUDE.md negative finding 記録)
4. **classifier 単独活用検討**: `TaskComplexityClassifier` は audit log の `inferred_capability_tier` として残置、Lab metric の tier 別変異効果可視化に流用 (`MultiRunTaskScore` 拡張候補)
5. **CLAUDE.md** に negative finding 記録 (Lab 天井打破失敗 evidence、項目 215 pattern)

## 13. session 05-11b gap analysis 補注 (★ minor、次 session 着手時に注意)

> **補注由来**: handoff `session_2026_05_11b_handoff.md` 完了後、本 plan を deep-read で gap 検査した結果。**design レベル gap なし** (`inject_memory_blocks` / `inject_heuristics` / `inject_contextual_memories` / `inject_planning_step` 4 関数全存在、`CapabilityTier::short_code` 存在、core.rs:136-141 順序が plan §4.3 と完全一致)。minor 4 件を以下に集中記録、次 session 着手 (Phase 2 Green) 時に本 §13 を必ず参照して renumber。

### G-1: CLAUDE.md 項目番号衝突 (★★★ blocking、5 箇所参照)
- plan §4.1 docstring「項目 222 候補」/ §4.2 prompt file ヘッダ「項目 223 候補」/ §9 「項目 222 candidates (= 223 候補)」/ §10 commit message「項目 223 候補」/ §11 P5 表 で項目 222/223 を G4 用に予約
- **訂正**: 本 session で項目 220-225 を予約済 (220=sqlite-vec / 221=G-4.2 REJECT / 222=wiring 削除 / **223=AgentFloor** / **224=pre-screen tier fix** / 225=experiment-from-results-deletion 予約)
- **G4 は項目 226 以降** (G1 Critic/ds4 後の実装順次第) に renumber 必要
- 影響: §4.1 docstring + §4.2 prompt file ヘッダ 6 件 + §9 / §10 / §11 内 commit message + handoff entry

### G-2: Lab v18 番号衝突 (★★★ blocking、§5 Phase 5 / §10 / §11)
- plan §5 Phase 5 / §10 / §11 で「Lab v18+ 候補」「`lab-v18-task-aware-prompt-effectiveness.md` 候補」と記載
- **訂正**: 既存 `lab-v18-critic-effectiveness.md` (41.2KB、G1 Critic 用、handoff 05-10d §4 起票済) と衝突
- **G4 effectiveness は Lab v19 (`lab-v19-task-aware-prompt-effectiveness.md`) または `lab-v18b-task-aware-prompt-effectiveness.md`** に renumber 必要
- 影響: 本 plan §5 / §10 / §11 + 派生 effectiveness plan 起票時

### G-3: 期待 test count outdated (★ low、§5 Phase 2 + §10)
- plan §5 Phase 2 「**1150 → ≥ 1158 passed**」と記載
- **訂正**: 本 session (commit `a52edc6` 項目 224 pre-screen tier fix) で test 1162→**1165 passed** に増、本 plan の正確な期待値は **1165 + 8 = ≥ 1173 passed**
- 影響: Quality Gate G-2 と完了条件 #6 の数値修正必要

### G-4: 1093+ task count outdated (★ informational、§1.3)
- plan §1.3 「`BenchmarkTask.capability_tier` に tag 付与済 (1093+ tier-tagged tasks)」と記載
- **訂正**: 本 session (commit `2b63441` AgentFloor Phase 2 Green) で **agentfloor_tasks() 30 task suite 追加**、現状 source 内の `capability_tier:` field 出現 = 94 task fixture matches (test fixture + production task)、informational only
- 影響: なし、軽微な数値表現のみ

### gap analysis サマリー
- **major blocking**: 0 件 (dependencies 全 OK、design 実装可能)
- **minor (実装影響あり)**: G-1 (項目番号 5 箇所 renumber)、G-2 (Lab v18 → v19/v18b renumber)
- **minor (数値 outdated)**: G-3 (test count 1158→1173)
- **informational**: G-4 (task count 1093→~94 fixture matches) — 対応任意

### Phase 0 追加 (本 plan §5 に追加、次 session 着手時)
Phase 0a: 本 §13 G-1 で項目番号を実際の使用状況に合わせて renumber (CLAUDE.md 末尾を grep して max 番号確認 + 1)、5 箇所 (§4.1 / §4.2 / §9 / §10 / §11) を一括置換
Phase 0b: G-2 で Lab version を確認 (既存 v18 = G1 Critic、本 plan effectiveness を v19 に確定 or v18b sub-version 採用判断)
Phase 0c: G-3 の test count を当時の実値 (1165 or G1/ds4 着手後の最新値) に更新
