# Plan: G3 並列 Sub-Agent 役割分業 — Role Specialization with Parallel Dispatch

> **由来 meta-plan**: `.claude/plan/building-ai-coding-agents-gap-analysis.md` § 4 G3 (★ 低-中優先、P7 Sub-Agent Delegation 拡張)
> **由来項目**: 項目 120 (`SubAgentExecutor` 順次委任、`agent/subagent.rs`)、項目 160 (深度制限 2 + エラー境界)、項目 55 (読取ツール 2+ 連続自動並列、tool-level 並列)、項目 1 (Reflexion、parent agent merge 経路)、AgentFloor 6-tier (項目 209、T4 MultiStepToolChain 評価対象候補)
>
> **位置付け**: bonsai 既存 `SubAgentExecutor` (`src/agent/subagent.rs`) は (a) 順次委任 (`execute_sequential`) と (b) 独立性検出ベースの並列 (`execute_parallel`、`std::thread::scope`) を持つが、**「役割分業」軸が不在**。Cursor / Devin / Claude Code が実証する Planner / Coder / Reviewer / Tester の **Role Specialization + 並列 dispatch** を、既存 `SubAgentExecutor` の 3 つ目のディスパッチ経路として additive に追加する Phase 1。
>
> **Phase 1 scope (本 plan)**: Role enum + role 専用 system prompt + 並列 role dispatch + parent merge prompt の **infrastructure** を実装。production default OFF (`BONSAI_PARALLEL_SUBAGENT_ENABLED` 未設定で legacy 経路完全互換)。Lab effectiveness paired t-test は **別 plan `lab-v20-parallel-subagent.md`** に分離。
>
> **production code 変更ゼロ前提** — 本 plan merge 時点では plan ファイル 1 件追加のみ。実装は別 session で TDD strict 5 phase 経由で起動。

## Task Type
- [ ] Frontend
- [x] Backend (`src/agent/subagent.rs::SubAgentRole` 新規 + `execute_roles_parallel` 経路 + `prompts/subagent_roles/{planner,coder,reviewer,tester}.txt` 4 file 新規 + `BONSAI_PARALLEL_SUBAGENT_ENABLED` env opt-in)
- [ ] Fullstack
- [ ] Docs

## 1. 背景

### 1.1 現状: 順次委任 + タスク独立性ベースの並列 (役割軸なし)

`src/agent/subagent.rs` (943 行、項目 120/160 で確立) は 3 つの dispatch 経路を提供:

| 経路 | 関数 | 並列性 | 軸 | 既存項目 |
|---|---|---|---|---|
| **順次** | `execute_sequential` (subagent.rs:211-248) | sync | (なし、依存ありで強制) | 項目 120 |
| **並列 (タスク独立性)** | `execute_parallel` (subagent.rs:251-322) | `std::thread::scope` | task 文字列の独立性 (`check_independence`、20 marker 検出) | 項目 160 |
| **dispatch** | `execute` (subagent.rs:141-208) | — | (independent && file-backed-store && len>=2) で自動分岐 | 項目 120 |

**観察**:
- 並列軸は「タスク文字列の独立性」のみ → **同じロールの異なるタスク** を並列化する設計
- **役割分業 (Planner / Coder / Reviewer / Tester)** = 「同一タスクを異なる視点で同時に複数 LLM が処理」軸が不在
- `execute_single_subtask` (subagent.rs:411-502) は **AgentConfig.system_prompt を引数 goal でテンプレート化** するだけ (subagent.rs:325-338) → role 別 prompt を持たない

### 1.2 G3 概念 — Role Specialization + Parallel Dispatch

論文 arxiv 2603.05344 (Building AI Coding Agents) と arxiv 2602.07359 (W&D: Scaling Parallel Tool Calling for Efficient Deep Research) は **役割分業 + 並列 dispatch** を coding agent 安定 pattern として総括:

| Agent | Role 設計 | Dispatch | bonsai 既存対応物 |
|---|---|---|---|
| Cursor | Architect / Editor / Reviewer | 並列 | (G3 で対応) |
| Devin | Planner / Coder / Verifier | 並列 | (G3 で対応) |
| Claude Code | self-review pass + sub-agent | 部分並列 | 項目 55 (tool-level 並列のみ) |
| Aider | Architect mode + Editor mode | 順次 | 項目 120/160 |
| MASS-RAG (arxiv 2604.18509) | Evidence summarizer / Extractor / Reasoner | 並列 | (G3 で対応) |

**4/5 が役割分業 + 並列**、bonsai の項目 55 は **tool-level (file_read 2 連続) のみ** で agent-level 役割分業ではない = 真の gap。

### 1.3 既存資源: 直接活用可能な 2 layer

1. **`SubAgentExecutor::execute_parallel`** (subagent.rs:251-322): `std::thread::scope` + file-backed store + per-thread `MemoryStore::open()` 独立 Connection が完成済 → **本 plan は同 thread::scope パターンを再利用**、新規 runtime 導入なしで OK (R8 で詳細)
2. **`execute_single_subtask`** (subagent.rs:411-502): AgentConfig 受取って 1 サブタスクを完走させる pure helper → role 別 AgentConfig (system_prompt 差替) を渡せばそのまま流用可

本 plan は **既存 2 layer の上に薄い role layer を載せる** だけで、core ロジックは触らない (= 退行リスク最小化)。

### 1.4 1bit Bonsai-8B + M2 16GB 文脈の制約

- M2 16GB で **同 backend (llama-server) に 4 並列 LLM call** は **メモリ圧迫 / 競合ロック** リスク
- 1bit モデル context window (n_ctx=16384) で **role ごとに完全タスク context** を渡すと token budget 競合
- llama-server は **single-instance + sequential request** が前提 (HTTP API、項目 35-36)、真の並列は backend 側で内部キューイング
- → **「並列」は spawn 時点の論理並列、backend 側で sequential 化される実機制約** を明文化、wall-time 短縮効果は限定的の前提で設計

### 1.5 動機: AgentFloor T4 MultiStepToolChain 局所改善

Lab 天井 7 連続 (項目 215) で global 構造変異枯渇。本 plan は **Lab 天井打破ではなく** AgentFloor (項目 209) T4 MultiStepToolChain (multi-tool 連鎖) で **役割分業による局所改善** を狙う。期待値 = T4 で paired Δ ≥ +0.015 + duration -10% 以下 (cost neutral)、他 tier では副作用 -0.01 以下。

### 1.6 既存項目との親和性

- **項目 120/160**: SubAgentExecutor + 深度制限 = **本 plan で深度制限維持** (再帰禁止、parallel 数 default 3 上限で爆発防止)
- **項目 55**: tool-level 並列 = role-level 並列と直交、両立可能
- **項目 209**: AgentFloor 6-tier = T4 で Lab 効果計測の対象 tier (Phase 5 Lab 別 plan)
- **項目 214/216-219**: env opt-in default OFF pattern = `BONSAI_PARALLEL_SUBAGENT_ENABLED=1` で踏襲

## 2. 目的

1. **Role specialization**: Planner / Coder / Reviewer / Tester 4 role enum + role 専用 system prompt で 1 タスクを 4 視点から同時処理
2. **並列 dispatch で wall-time 短縮**: `std::thread::scope` + per-role AgentConfig 経路で多視点を logical 並列実行 (backend 側 sequential 化前提でも 1.5-2x 短縮期待)
3. **G1 critic と協調**: G1 (Critic 別 LLM 分離) と Reviewer role の **重複/補完関係を明文化**、共存設計

### 非目標
- **深度制限緩和しない**: 既存 `MAX_DEPTH=2` 維持 (項目 160)、parallel 数 default 3 上限で再帰爆発防止
- **既存 `execute_sequential` / `execute_parallel` の置換**: 第 3 経路として additive に追加、既存 2 経路は read-only
- **新 async runtime (tokio) 導入**: bonsai 既存は `std::thread::scope` で確立済、新 runtime 導入は scope creep (R8 で詳細、Phase 2 派生 plan で別途検討)
- **Lab paired t-test での効果検証**: 別 plan `lab-v20-parallel-subagent.md` の責務 (本 plan delivery 範囲外)
- **production default ON 化**: env opt-in default OFF (項目 214/216-219 と同 pattern)
- **role 数の動的拡張**: Phase 1 は固定 4 role、5 role 以上は別 plan
- **role 間通信**: Phase 1 は独立並列 (各 role は他 role の出力を見ない)、parent agent が merge prompt で集約

## 3. 既存項目との関係

| 項目 | 関係 | 本 plan での扱い |
|---|---|---|
| **1 (Reflexion)** | 同一 LLM self-critique | parent agent の merge 段で結果集約後に Reflexion 発火 (既存経路無改変) |
| **47 (`<think>` 強制)** | デフォルト化済 directive | 各 role 専用 system prompt 内で重ね掛け強調 (Coder / Reviewer 特に強調) |
| **50 (フォールバック戦略)** | デフォルト化済 directive | Coder / Tester role の system prompt で強調 |
| **55 (tool-level 並列)** | tool レベル並列 | 直交。本 plan = agent レベル並列、tool レベルと両立 |
| **120 (SubAgentExecutor 順次)** | 既存 dispatch 経路 | 不変。本 plan は第 3 経路として並列 role dispatch を追加 |
| **136 (回答前ファイル内容確認)** | デフォルト化済 directive | Reviewer role system prompt の中核 |
| **160 (深度制限 2 + エラー境界)** | 既存 invariant | 不変維持。並列 role dispatch でも `MAX_DEPTH` 継承、`SubAgentConfig::can_delegate` 経由で gate |
| **187 (ContextOverflowGuard / n_ctx_budget)** | 親→子 context budget 伝播 | 既存 `SubAgentConfig::n_ctx_budget` を role 並列でも継承 (subagent.rs:71 invariant 維持) |
| **209 (AgentFloor 6-tier)** | tier 評価基盤 | Phase 5 Lab で T4 MultiStepToolChain を中心に効果計測 |
| **214/216-219 (env opt-in pattern)** | `BONSAI_<feature>_ENABLED` 統一 | `BONSAI_PARALLEL_SUBAGENT_ENABLED=1` で踏襲、default OFF |
| **G1 (Critic 別 LLM、`.claude/plan/critic-separate-llm-impl.md`)** | role 分離設計の親戚 | 共存。G1 = step 中 critic、本 plan = sub-agent レベル Reviewer role。Phase 5 Lab で「G1 ON × G3 ON / OFF / ...」factorial 検討候補 (本 plan scope 外) |
| **G2 (Agent-Side TDD、`.claude/plan/agent-side-tdd-enforcement-impl.md`)** | TDD directive | Tester role system prompt で TDD directive を重ね掛け (G2 merge 後の依存関係) |
| **G4 (Task-Aware System Prompt、`.claude/plan/task-aware-system-prompt-impl.md`)** | tier 別 augment | 直交。G4 = parent agent prompt augment、本 plan = sub-agent role 切替。両 ON で重複しない (sub-agent の system_prompt は role 専用) |

## 4. 設計

### 4.1 `SubAgentRole` enum (新規、`src/agent/subagent.rs` 追記)

既存 `SubAgentConfig` と同 module、parallel 並走で役割識別。

```rust
/// G3 並列 Sub-Agent 役割分業 — role specialization with parallel dispatch
///
/// Phase 1: 4 role 固定 (Planner / Coder / Reviewer / Tester)。Phase 2 派生で 5+ role 検討。
///
/// 由来: building-ai-coding-agents-gap-analysis.md G3
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubAgentRole {
    /// Planner: タスク分解 + step 列挙 + 依存関係明示
    Planner,
    /// Coder: 実装案生成 + ツール選択 + パッチ提案
    Coder,
    /// Reviewer: 既存コード/出力の review + 矛盾検出 + 改善提案
    Reviewer,
    /// Tester: test 起草 + 検証 step + パス条件明示 (G2 TDD directive と協調)
    Tester,
}

impl SubAgentRole {
    /// role の小文字短コード (audit log / TSV / prompt path key)
    pub fn short_code(&self) -> &'static str {
        match self {
            Self::Planner => "planner",
            Self::Coder => "coder",
            Self::Reviewer => "reviewer",
            Self::Tester => "tester",
        }
    }

    /// role 専用 system prompt の embedded 文字列を返す。
    /// `prompts/subagent_roles/<role>.txt` を `include_str!` で binary 埋込。
    pub fn embedded_prompt(&self) -> &'static str {
        match self {
            Self::Planner => include_str!("../../prompts/subagent_roles/planner.txt"),
            Self::Coder => include_str!("../../prompts/subagent_roles/coder.txt"),
            Self::Reviewer => include_str!("../../prompts/subagent_roles/reviewer.txt"),
            Self::Tester => include_str!("../../prompts/subagent_roles/tester.txt"),
        }
    }

    /// Phase 1 default 4 role (固定順序: Planner → Coder → Reviewer → Tester)
    pub fn default_set() -> Vec<Self> {
        vec![Self::Planner, Self::Coder, Self::Reviewer, Self::Tester]
    }
}
```

### 4.2 `subagent` tool 拡張 — `roles: Vec<SubAgentRole>` 引数追加

bonsai の `subagent` tool 実装場所は Phase 2 Green 着手時に `rg "subagent" src/tools/` で特定する (既存 `Tool` trait 実装、`tools/mod.rs` の registry 経由)。本 plan は `SubAgentExecutor::execute_roles_parallel` を新規追加し、tool 側からは **既存 `execute` の追加引数として roles** を受ける形に拡張する。

#### 4.2.1 新規 method: `SubAgentExecutor::execute_roles_parallel`

```rust
impl<'a> SubAgentExecutor<'a> {
    /// Phase 1: 同一 task を複数 role で並列実行し、結果を集約。
    ///
    /// 既存 `execute` (順次/独立並列の自動分岐) と直交する第 3 経路。
    /// `BONSAI_PARALLEL_SUBAGENT_ENABLED=1` で opt-in、env unset で
    /// この method は呼ばれない (= dispatch site で短絡、subagent tool 側 gate)。
    ///
    /// # Invariants (項目 160 維持)
    /// - `MAX_DEPTH=2` 継承 (`sub_config.can_delegate` で gate)
    /// - parallel role 数上限 = 3 (`MAX_PARALLEL_ROLES`、4 役指定時は最初 3 のみ実行 + warn log)
    /// - 各 role は独立 `MemoryStore::open()` connection (file-backed 必須、in-memory は順次経路)
    /// - エラー境界: 1 role 失敗で他 role は続行、merge 段で部分集約
    ///
    /// # Returns
    /// `RoleDelegationResult { role_results: Vec<RoleResult>, merge_prompt: String }`
    pub fn execute_roles_parallel(
        &self,
        parent_task_id: &str,
        task_goal: &str,
        roles: &[SubAgentRole],
    ) -> anyhow::Result<RoleDelegationResult>;
}
```

#### 4.2.2 並列数上限 `MAX_PARALLEL_ROLES = 3` (default)

```rust
/// G3 並列 role dispatch の上限。M2 16GB + llama-server single-instance 制約で
/// 4 並列 LLM call は競合ロックリスク高 (R1)。default 3 で安全側に倒す。
/// `BONSAI_PARALLEL_SUBAGENT_MAX_ROLES` env で override 可、上限 4。
pub(crate) const MAX_PARALLEL_ROLES: usize = 3;
```

4 role 指定時は **最初 3 role のみ実行 + warn log** (Tester は最初の 3 から落ちやすいが、TDD 重視なら env で order 制御する将来拡張余地)。

#### 4.2.3 既存 `subagent` tool 引数拡張 (additive)

`subagent` tool の `parameters_schema` に optional field 追加:

```json
{
  "type": "object",
  "properties": {
    "task_goal": { "type": "string", "description": "..." },
    "subtasks": {
      "type": "array",
      "items": { "type": "string" },
      "description": "(既存) 順次/独立並列の sub-task 列挙"
    },
    "roles": {
      "type": "array",
      "items": { "type": "string", "enum": ["planner", "coder", "reviewer", "tester"] },
      "description": "(G3 新規 optional) 役割分業並列 dispatch、env=BONSAI_PARALLEL_SUBAGENT_ENABLED=1 で opt-in"
    }
  }
}
```

`roles` 未指定 (= 既存 caller / env OFF) で既存 `execute` 経路、`roles` 指定 + env ON で `execute_roles_parallel` 経路。

### 4.3 各 role 専用 system prompt (`prompts/subagent_roles/{4 file}.txt`)

`prompts/subagent_roles/` 新規 dir、4 file (G4 plan の `prompts/task_aware/` 同パターン)。各 file ≤ 25 行、UTF-8、Bonsai-8B 1bit 向け簡潔表現。

#### 4.3.1 `prompts/subagent_roles/planner.txt` (~15 行)

```
あなたは1bit ローカル LLM の **Planner** sub-agent です。
親 agent から受けた task_goal を **計画** だけに集中して分解してください。

【ロール】
- task_goal を 3-7 step に分解、各 step に依存関係を明示
- ツールは file_read のみ許可 (実装はしない)
- 出力 = 計画のみ、実装案や code は出さない

【出力形式】
1. 短い要約 (≤ 1 行)
2. ステップ列挙 (Markdown リスト)
3. 依存関係 (並列可 / 順次必須を明示)
4. 完了条件 (測定可能な完了判定)

温度は 0.3 (低)、揺れない計画を出すこと。
```

#### 4.3.2 `prompts/subagent_roles/coder.txt` (~20 行)

```
あなたは1bit ローカル LLM の **Coder** sub-agent です。
親 agent から受けた task_goal を **実装** に集中して具体化してください。

【ロール】
- task_goal を実現する code / patch / コマンドを提案
- 1 ツール選択 + 1 file_write を最大 (= 過剰実装禁止)
- 既存コードと矛盾しないこと、ファイル内容は file_read で確認 (項目 136 重ね掛け)

【出力形式】
1. 採用した実装方針 (≤ 2 行)
2. 具体的 patch / 新規ファイル内容 (≤ 30 行)
3. 副作用 (test 影響範囲、依存関係)
4. 代替案 (項目 50 重ね掛け、失敗時の代替手順)

温度は 0.3 (低)、ツール使用前に <think> で意図を必ず述べる (項目 47 重ね掛け)。
```

#### 4.3.3 `prompts/subagent_roles/reviewer.txt` (~18 行)

```
あなたは1bit ローカル LLM の **Reviewer** sub-agent です。
親 agent から受けた task_goal を **第三者視点で批判的に review** してください。

【ロール】
- task_goal の前提誤り / 論理破綻 / 見落とし / 代替案を探す
- ツール file_read のみ許可、実装提案禁止 (代替案の提示はテキストのみ)
- AGREE / DISAGREE / UNCERTAIN の 3 値で総括

【出力形式】
- AGREE: <1 行根拠>
- DISAGREE: <2-3 行根拠と修正提案>
- UNCERTAIN: <不足情報>

温度は 0.7 (高、Coder の癖と異なる視点を必ず提示)。
G1 critic との重複: G1 は step 中、本 role は sub-agent task 完了後の review。重ね掛け運用 OK。
```

#### 4.3.4 `prompts/subagent_roles/tester.txt` (~20 行)

```
あなたは1bit ローカル LLM の **Tester** sub-agent です。
親 agent から受けた task_goal を **test 起草** に集中して具体化してください。

【ロール】
- task_goal を検証する test ケースを Red 優先で起草 (G2 TDD directive 重ね掛け)
- shell tool で `cargo test` 形 / pytest 形のコマンドを 1 つ提案
- 期待される pass/fail 条件を明示、test 名 = 短く検証内容を反映

【出力形式】
1. 起草する test の目的 (≤ 1 行)
2. test 名 + 検証内容 (Markdown コードブロック)
3. 実行コマンド (1 行)
4. 期待結果 (PASS / FAIL の判定基準)

温度は 0.3 (低)、test 起草は仕様の言い換えであり創作ではない。
```

### 4.4 Result aggregation: parent agent 用 merge prompt

各 role の出力を section 化し、parent agent が最終決定するための merge prompt を生成:

```rust
/// 各 role 並列実行の結果を section 化した merge prompt を生成。
/// parent agent はこれを context として読み、最終決定を出す。
///
/// # 出力形式
/// ```text
/// ## 並列 Sub-Agent 役割分業 結果 (4/4 role 完了)
///
/// ### Planner (success, 3 iterations)
/// <planner answer 抜粋 / 切詰 800 文字以内>
///
/// ### Coder (success, 5 iterations)
/// <coder answer 抜粋>
///
/// ### Reviewer (failure, 0 iterations, error: ...)
/// (実行失敗、partial 集約)
///
/// ### Tester (success, 4 iterations)
/// <tester answer 抜粋>
///
/// ---
/// 上記 4 視点を踏まえ、最終決定を出してください。
/// ```
fn build_role_merge_prompt(results: &[RoleResult]) -> String;
```

#### 4.4.1 `RoleResult` 構造体 (新規)

```rust
#[derive(Debug, Clone)]
pub struct RoleResult {
    pub role: SubAgentRole,
    pub task_id: String,
    pub answer: String,
    pub iterations_used: usize,
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone)]
pub struct RoleDelegationResult {
    pub role_results: Vec<RoleResult>,
    pub merge_prompt: String,
}

impl RoleDelegationResult {
    pub fn all_succeeded(&self) -> bool { self.role_results.iter().all(|r| r.success) }
    pub fn success_rate(&self) -> f64 { /* 同 DelegationResult パターン */ }
    pub fn role_disagreement_rate(&self) -> Option<f64> {
        // Reviewer role の DISAGREE / total の割合 (Lab metric 候補、Phase 4 で空計算)
        // (Phase 5 Lab で実装、Phase 1 は struct field のみ準備)
    }
}
```

### 4.5 env opt-in: `BONSAI_PARALLEL_SUBAGENT_ENABLED=1`

| env | default | 効果 |
|---|---|---|
| `BONSAI_PARALLEL_SUBAGENT_ENABLED` | unset (OFF) | unset / "0" / "false" で `execute_roles_parallel` 経路短絡、既存 `execute` (順次/独立並列) 維持 |
| `BONSAI_PARALLEL_SUBAGENT_MAX_ROLES` | `3` | 1-4 で override 可、不正値は default 採用 + warn log |
| `BONSAI_PARALLEL_SUBAGENT_TIMEOUT_MS` | `120000` (= 2 min/role) | 各 role の wall-time timeout、超過時 RoleResult.success=false |

env name 命名は項目 217-219 と整合 (`BONSAI_<feature>_ENABLED`)。

### 4.6 既存挙動互換: env 未設定 default = 既存 `execute` 経路

| state | 動作 | 互換性 |
|---|---|---|
| env unset (production default) | `roles` 引数を **無視** し既存 `execute` (順次/独立並列) | observable 動作 100% 互換 |
| env="1" + tool 引数で `roles` 指定 | `execute_roles_parallel` 経路 | 新経路 |
| env="1" + tool 引数で `roles` 未指定 | 既存 `execute` (env がない場合と同じ) | 互換 |
| env="1" + `roles` len > 4 | 最初 4 を採用、warn log + truncate | safe |
| env="1" + `roles` len > MAX_PARALLEL_ROLES | 最初 N を採用 (N=MAX_PARALLEL_ROLES)、warn log | safe |

### 4.7 Lab metric (informational): parallel_dispatch_count / role_disagreement_rate / wall_time_reduction

Phase 5 Lab effectiveness (別 plan、本 plan delivery 範囲外):

```rust
pub struct ParallelSubAgentStats {
    /// 本 cycle で execute_roles_parallel が起動した回数
    pub parallel_dispatch_count: usize,
    /// Reviewer role が DISAGREE 返却した割合 (denominator = Reviewer 起動回数)
    pub role_disagreement_rate: Option<f64>,
    /// 並列 wall-time / 順次 wall-time (推定) の比 (1.0 未満で短縮効果あり)
    pub wall_time_reduction: Option<f64>,
    /// 各 role 単独の平均 success rate (4 entries: planner/coder/reviewer/tester)
    pub per_role_success_rate: std::collections::HashMap<String, f64>,
}
```

`MultiRunBenchmarkResult` に `Option<ParallelSubAgentStats>` field 追加 (項目 200 RDC/VAF と同 informational pattern、ACCEPT 判定不変)。

TSV 列拡張 (informational only): 既存 12 → 18 列 (項目 200 で 12→15、本 plan で 15→18 = `parallel_dispatch_count / role_disagreement_rate / wall_time_reduction`)。

**Lab ACCEPT 判定** (Phase 5 別 plan): paired t-test で env=ON vs env=unset の `composite_score` Δ で判定。`role_disagreement_rate` / `wall_time_reduction` は副次指標。

### 4.8 audit log 拡張

`observability/audit.rs::AuditAction` に variant 追加:

```rust
pub enum AuditAction {
    // ... 既存
    ParallelRoleDispatch {
        roles: Vec<String>,           // ["planner", "coder", "reviewer", "tester"]
        succeeded_count: usize,
        failed_count: usize,
        total_duration_ms: u64,       // wall-time (max(per-role) 近似)
        disagreement_detected: bool,  // Reviewer DISAGREE が 1 件以上
    },
}
```

`as_str()` 追加: `Self::ParallelRoleDispatch { .. } => "parallel_role_dispatch"`、SQLite 既存 schema で受切れる (action_type TEXT + payload JSON)、**migration 不要**。

## 5. TDD strict 5 phase (test ≥ 7 件)

### Phase 1 — Red (test 7-9 件)

`src/agent/subagent.rs` 末尾 `#[cfg(test)] mod tests` 内に追加 (既存 23 test 末尾)。

| # | test 名 | 期待 (Red 時) |
|---|---|---|
| 1 | `t_subagent_role_short_code_and_default_set` | `SubAgentRole::Planner.short_code() == "planner"` 等 4 role + `default_set().len() == 4` |
| 2 | `t_subagent_role_embedded_prompt_nonempty` | 4 role の `embedded_prompt()` が空でない (include_str! 対象 file 存在確証) |
| 3 | `t_parallel_subagent_env_default_disabled` | env unset で `is_parallel_subagent_enabled() == false` (項目 214 同パターン) |
| 4 | `t_parallel_subagent_env_explicit_true` | `BONSAI_PARALLEL_SUBAGENT_ENABLED=1` で true、"0" / その他で false |
| 5 | `t_execute_roles_parallel_short_circuits_when_disabled` | env unset で `execute_roles_parallel` が `RoleDelegationResult { role_results: vec![], merge_prompt: "..." }` 即 return、backend 呼出ゼロ (MockLlmBackend.calls() で確認) |
| 6 | `t_execute_roles_parallel_truncates_to_max_roles` | env="1" + 4 role 指定 + `MAX_PARALLEL_ROLES=3` (env override で 3 設定) で先頭 3 role のみ実行、warn log emit |
| 7 | `t_execute_roles_parallel_emits_per_role_results` | env="1" + 2 role (Planner + Coder) 指定 + MockLlmBackend で `RoleDelegationResult.role_results.len() == 2`、各 role.short_code が一致 |
| 8 | `t_execute_roles_parallel_error_boundary` | 1 role が backend error で fail でも他 role は success 継続 (RoleResult.success 値別、merge_prompt が partial section 含む) |
| 9 | `t_execute_roles_parallel_audit_log_emitted` | env="1" 実行後に `AuditAction::ParallelRoleDispatch` が SQLite audit に 1 件追加 (file-backed store 経由) |

env mutation race avoidance: module-local `static PARALLEL_SUBAGENT_TEST_LOCK: Mutex<()>` で serialize (項目 218 REVIEW_INJECT_TEST_LOCK pattern 踏襲、`context_inject.rs:639` 同形式)。

期待: `cargo test --lib subagent::tests` で **新規 7-9 件 fail / compile error** で Red 確証 (`SubAgentRole` / `execute_roles_parallel` / `RoleDelegationResult` / `is_parallel_subagent_enabled` / `MAX_PARALLEL_ROLES` 未定義)。

commit: `test(subagent): G3 Phase 1 Red — SubAgentRole + execute_roles_parallel 7-9 test`

### Phase 2 — Green

実装ファイル:

1. **`src/agent/subagent.rs`** — `SubAgentRole` enum + `RoleResult` / `RoleDelegationResult` 構造体 + `is_parallel_subagent_enabled` / `parallel_role_max` 関数 + `execute_roles_parallel` method (~200 行追加、既存 943 → ~1140 行)
2. **`prompts/subagent_roles/{planner,coder,reviewer,tester}.txt`** 4 file 新規 (§ 4.3 内容、各 ≤ 25 行、UTF-8)
3. **`src/observability/audit.rs`** — `AuditAction::ParallelRoleDispatch` variant + `as_str()` arm 追加
4. **`src/tools/`** (subagent tool 実装場所、Phase 2 Green で `rg "subagent" src/tools/` 経由特定) — `parameters_schema` に optional `roles` field 追加 + `call` で env gate + `execute_roles_parallel` dispatch (~30-50 行追加)
5. **`src/agent/benchmark.rs`** — `ParallelSubAgentStats` 構造体 + `MultiRunBenchmarkResult` への `Option<ParallelSubAgentStats>` field (Phase 4 smoke で空集計、本 plan delivery は struct + run_k 集計 hook までで Lab metric 完全配線は Phase 5 別 plan)

`execute_roles_parallel` 実装の核 (既存 `execute_parallel` の `std::thread::scope` パターン踏襲):

```rust
pub fn execute_roles_parallel(
    &self,
    parent_task_id: &str,
    task_goal: &str,
    roles: &[SubAgentRole],
) -> anyhow::Result<RoleDelegationResult> {
    if !self.sub_config.can_delegate() {
        return Ok(RoleDelegationResult {
            role_results: vec![],
            merge_prompt: format!("委任深度上限({MAX_DEPTH})到達、role 並列 dispatch スキップ"),
        });
    }
    let max_roles = parallel_role_max();
    let effective_roles: Vec<_> = roles.iter().take(max_roles).copied().collect();
    if roles.len() > max_roles {
        log_event(LogLevel::Warn, "subagent",
            &format!("roles len {} > MAX_PARALLEL_ROLES {}, truncated", roles.len(), max_roles));
    }
    let store_path: Option<String> = self.store.and_then(|s| s.path().map(String::from));
    if store_path.is_none() {
        // in-memory store は順次経路 (既存 execute_parallel と同 invariant、subagent.rs:173)
        return self.execute_roles_sequential(parent_task_id, task_goal, &effective_roles);
    }
    // ... std::thread::scope で role ごと spawn
    // 各 thread 内で AgentConfig.system_prompt = role.embedded_prompt() で execute_single_subtask
    // 結果集約 + build_role_merge_prompt
    // AuditAction::ParallelRoleDispatch emit
    // ...
}
```

期待:
- `cargo test --lib` で **既存 1150 + 新規 7-9 test = 1157-1159 passed**
- `cargo clippy --lib --tests -- -D warnings` clean
- `cargo fmt --check` clean
- env unset で既存 1150 test 退行ゼロ (`BONSAI_PARALLEL_SUBAGENT_ENABLED` 未設定 = legacy 経路)

commit: `feat(subagent): G3 Phase 2 Green — SubAgentRole + execute_roles_parallel + 4 role prompts`

### Phase 3 — Refactor

- `build_role_merge_prompt` を pure function 化 (testability、外部から呼出可能)
- `execute_single_subtask` を **role 識別 audit log に拡張** (既存 `execute_single_subtask` は無改変、本 plan 専用 wrapper `execute_role_subtask` で role 引数を audit に流す)
- `parallel_role_max` の env パース失敗時 warn log + default 3 (項目 214 pattern 統一)
- docstring 整備 — `SubAgentRole` / `execute_roles_parallel` / `RoleDelegationResult` に「由来 plan: parallel-subagent-roles-impl.md / G3 / 項目 224 候補」明記
- `prompts/subagent_roles/{4 file}.txt` 各 file 冒頭に `# G3 sub-agent role: <Role Name>` ヘッダ統一
- DEFAULT_SYSTEM_PROMPT 16 ルール × 4 role directive cross-check (R6 軽減、本 plan section 13 review log に matrix 追記)
- test mutex 確認 (env mutation race 回避、項目 218 pattern)

commit: `refactor(subagent): G3 Phase 3 — pure merge fn + role audit + docstring + cross-check`

### Phase 4 — Smoke (G-4、3 段)

#### G-4a: 既存経路後方互換 (env unset)
```bash
cargo build --release
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 2>&1 | tee /tmp/g3_g4a.log
```
期待: `parallel_role_dispatch` audit ゼロ、score / pass@k / duration が項目 207 baseline (0.7812) ± variance、TSV `parallel_dispatch_count=0`

#### G-4b: env=ON / 既存 caller (`roles` 未指定) で legacy 維持
```bash
BONSAI_PARALLEL_SUBAGENT_ENABLED=1 \
  BONSAI_LAB_SMOKE=1 \
  ./target/release/bonsai --lab --lab-experiments 0 2>&1 | tee /tmp/g3_g4b.log
```
期待: env=ON でも tool caller 側で `roles` 未指定なら **legacy `execute` 経路維持**、`parallel_role_dispatch=0`、production 動作影響ゼロ

#### G-4c: env=ON / 強制 role dispatch test fixture (1 task で 3 role 起動)
release build に **smoke 専用 fixture** を仕込む (cfg(test) または `BONSAI_PARALLEL_SUBAGENT_FIXTURE=1` で常時有効化、本番 caller 影響ゼロ):
```bash
BONSAI_PARALLEL_SUBAGENT_ENABLED=1 \
  BONSAI_PARALLEL_SUBAGENT_FIXTURE=1 \
  BONSAI_LAB_SMOKE=1 \
  ./target/release/bonsai --lab --lab-experiments 0 2>&1 | tee /tmp/g3_g4c.log
grep "parallel_role_dispatch" /tmp/g3_g4c.log  # ≥ 1 件
```
期待:
- `parallel_role_dispatch` audit ≥ 1
- 各 role の AgentConfig.system_prompt が role 専用に差替 (audit log で確証)
- duration: 純粋並列なら 1 role の 1.5-2x、backend sequential 化前提でも duration ≤ 4x role (4 role spawn だが backend キューイング)
- score: 1bit variance 範囲 (Δ ≥ -0.05 lenient gate)

判定:
- ✅ G-4a: env unset で既存挙動完全互換 (1157-1159 passed 維持、parallel audit 0)
- ✅ G-4b: env=ON でも legacy caller 経路維持 (parallel audit 0)
- ✅ G-4c: 強制 fixture で並列経路 wiring 確認、duration ≤ 4x role、score lenient gate

### Phase 5 — Lab effectiveness (別 plan、本 plan delivery 範囲外)

別 plan ファイル `lab-v20-parallel-subagent.md` を起票:
- AgentFloor 30 task / k=3 / paired t-test (env=ON vs env=unset)
- T4 MultiStepToolChain を中心に tier 別 Δscore 計測
- ACCEPT 判定: (a) T4 paired Δ ≥ +0.015 (b) duration -10% 以下 (cost neutral 以上) (c) 全 tier Δ ≥ -0.01 (副作用なし)
- ACCEPT → defaults 昇格検討 (env default ON、本 plan 7 章 R3 mitigation 参照)
- REJECT → dead-code 候補化 (項目 215 ERL pattern、項目 222 sqlite-vec wiring 削除 pattern 踏襲)

副次評価: 項目 200 RDC/VAF/GDS reliability_decay tier 別 variance 観測、Lab v17 副次 finding pattern (stability 軸 ON 顕著優位なら informational ACCEPT 検討)。

## 6. API 影響

### 6.1 公開 API (新規)

| modulo path | 種別 | 備考 |
|---|---|---|
| `crate::agent::subagent::SubAgentRole` | pub enum | 4 variant + `short_code` + `embedded_prompt` + `default_set` |
| `crate::agent::subagent::RoleResult` | pub struct | 7 field |
| `crate::agent::subagent::RoleDelegationResult` | pub struct | 2 field + `all_succeeded` / `success_rate` / `role_disagreement_rate` |
| `crate::agent::subagent::SubAgentExecutor::execute_roles_parallel` | pub method | 既存 method 並列追加 (signature 不変、新 method) |
| `crate::agent::subagent::is_parallel_subagent_enabled` | pub(crate) fn | env パーサ |
| `crate::agent::subagent::parallel_role_max` | pub(crate) fn | env パーサ + clamp |
| `crate::agent::subagent::build_role_merge_prompt` | pub(crate) fn | pure helper |
| `crate::agent::subagent::MAX_PARALLEL_ROLES` | pub(crate) const | usize=3 |
| `crate::agent::benchmark::ParallelSubAgentStats` | pub struct | informational metric |
| `crate::observability::audit::AuditAction::ParallelRoleDispatch` | enum variant | additive |

### 6.2 内部 API (新規 private / pub(super))

| API | 種別 | 配置 |
|---|---|---|
| `execute_role_subtask` | private fn | `subagent.rs`、role 引数を audit に流す `execute_single_subtask` wrapper |
| `execute_roles_sequential` | private method | `SubAgentExecutor`、in-memory store 経路 |

### 6.3 公開 API 破壊的変更

**ゼロ**:
- `SubAgentExecutor::new` signature unchanged
- `SubAgentExecutor::execute` signature unchanged
- `SubAgentConfig` field unchanged (4 field 維持)
- `execute_single_subtask` signature unchanged
- `DelegationResult` / `SubTaskResult` 既存フィールド unchanged
- `MultiRunBenchmarkResult` への `Option<ParallelSubAgentStats>` 追加は `serde(default)` + `skip_if_none` で additive
- `subagent` tool の `parameters_schema` への optional `roles` field 追加は **既存 caller 互換** (未指定で legacy 経路)

env unset で既存挙動 100% 維持 = 1150 passed 退行ゼロ。

## 7. Risks / Mitigations

| # | Risk | severity | Mitigation |
|---|---|---|---|
| **R1** | M2 16GB で 4 並列 LLM call で メモリ圧迫 (1bit Bonsai-8B 1.28GB × 4 = 5.12GB + llama-server overhead) | **HIGH** | (i) `MAX_PARALLEL_ROLES=3` default で 4 並列禁止 (ii) llama-server は single-instance + sequential request 前提で実機並列度 = 1 (logical 並列のみ、R8 で詳細) (iii) Phase 4 G-4c で `top -l 1 -pid <pid>` メモリ計測、超過時 `MAX_PARALLEL_ROLES=2` に下げる buffer |
| **R2** | role disagreement の resolve 困難 (Reviewer DISAGREE で Coder 出力不採用 → 親 agent が判断不能) | **HIGH** | (i) Phase 1 は merge prompt で **section 化のみ**、resolve は親 agent 任せ (ii) Reviewer DISAGREE は audit log で記録、Lab metric `role_disagreement_rate` で追跡 (iii) Phase 5 Lab で disagreement_rate と score 相関を分析 (高相関 = 親 agent が resolve できている、低相関 = 改善余地) (iv) Phase 2 派生 plan で resolve 戦略を別途設計 (e.g., voting / weighted) |
| **R3** | 1bit Bonsai-8B context window 不足 (n_ctx=16384 で role ごと完全 task context 渡すと 4×4096=16384 で限界) | **HIGH** | (i) 各 role の AgentConfig.n_ctx_budget = parent.n_ctx_budget / 4 で分割 (subagent.rs:71 既存 invariant 拡張) (ii) role 専用 system prompt を簡潔化 (≤ 25 行 = ~500 token) (iii) Phase 4 G-4c で n_ctx_overflow audit が 0 件であること確証、超過時 prompt 短縮 |
| **R4** | tokio runtime nested call 制約 (parent agent が tokio runtime 内で execute_roles_parallel 呼ぶと nested runtime panic) | LOW | bonsai は tokio 非採用、`std::thread::scope` で sync 並列、tokio runtime 不在で R4 該当せず。本 plan は新 async runtime 導入 **しない** (R8 と統合) |
| **R5** | `prompts/subagent_roles/{4 file}.txt` 改変で Lab 結果再現不可 | LOW | `include_str!` で binary 内に埋込、git 履歴で改変追跡可。Phase 5 Lab plan で「prompts/subagent_roles/ git hash」を Experiment metadata に記録 |
| **R6** | DEFAULT_SYSTEM_PROMPT 16 ルール × role directive 矛盾 (e.g., Reviewer "実装提案禁止" vs DEFAULT 「ツール使用前 think」) | MEDIUM | Phase 3 で 16 ルール × 4 directive cross-check matrix を本 plan section 13 review log に追記、矛盾は文面修正、重複は許容 (項目 47/50/136 重ね掛け強調 OK) |
| **R7** | env mutation race で Phase 1 test の決定論性損失 | MEDIUM | test mutex 導入 (`PARALLEL_SUBAGENT_TEST_LOCK`、項目 218 pattern)、Phase 3 Refactor で対応 |
| **R8** | 新 async runtime (tokio) 導入が scope creep + 既存 std::thread::scope パターンとの不整合 | MEDIUM | (i) bonsai 既存 `execute_parallel` (subagent.rs:251-322) は `std::thread::scope` で確立済 → 本 plan も同パターン採用、新 runtime 非導入を明文化 (ii) llama-server は HTTP API + sync request、sync runtime で十分 (iii) tokio 採用は Phase 2 派生 plan で別途検討 (本 plan scope 外) |
| **R9** | 1 role 失敗で他 role の wall-time が無駄になる (early-exit せず全 role 完走待ち) | LOW | (i) `BONSAI_PARALLEL_SUBAGENT_TIMEOUT_MS=120000` で各 role 個別 timeout (ii) thread::scope は join 必須、early-exit は cancel token 経由で実装 (iii) Phase 1 は full-wait、early-exit は Phase 2 派生 plan 候補 |
| **R10** | `subagent` tool の `parameters_schema` 拡張で既存 caller 影響 | LOW | optional field 追加 = `serde(default)` で旧 caller 互換、Phase 4 G-4b で legacy caller smoke で確証 |
| **R11** | Reviewer role が Coder role 出力を見ずに review (役立たず批判) | MEDIUM | (i) Phase 1 は **independent parallel** (各 role は他 role の出力を見ない、merge は parent 段) で割り切り (ii) Phase 2 派生で 2-stage (Coder → Reviewer) sequential サブパス追加候補 (iii) Reviewer role.txt で「自身の事前知識 + task_goal のみで批判」と明記 |
| **R12** | G1 critic と Reviewer role の役割重複で token cost +50% | MEDIUM | (i) G1 = step 中 critic、Reviewer = sub-agent task 完了後 review、時間軸が異なる (ii) 両 ON は user 選択 (env opt-in 2 軸)、Phase 5 Lab で「G1 only / G3 only / 両 ON / 両 OFF」factorial 検証候補 (iii) 本 plan は G3 単独効果のみ計測 |
| **R13** | parent agent の merge prompt token 肥大化で context overflow | MEDIUM | (i) 各 role answer を 800 文字以内に切詰 (build_role_merge_prompt 内) (ii) 4 role × 800 = 3200 char ≈ 1600 token、合計 system prompt + memory blocks + heuristics + merge = 安全圏 (iii) Phase 4 G-4c で context_overflow audit 0 件確証 |

## 8. Quality Gates

| Gate | 内容 | 検証 |
|---|---|---|
| **G-1 (Phase 1 Red)** | 7-9 新規 test 全 fail or compile error | `cargo test --lib subagent::tests` 全 fail 確認、既存 1150 passed 維持 |
| **G-2 (Phase 2 Green)** | 既存 1150 + 新規 7-9 = **1157-1159 passed**、clippy 0、fmt 0 | `cargo test --release && cargo clippy --tests -- -D warnings && cargo fmt --check` |
| **G-3 (Phase 3 Refactor)** | docstring + cross-check matrix (16 ルール × 4 directive) + test mutex + 1157-1159 維持 | self-review + review log を本 plan section 13 に追記 |
| **G-4 (Phase 4 Smoke)** | (a) env unset で既存挙動完全互換、`parallel_dispatch_count=0` (b) env=ON + `roles` 未指定で legacy (c) env=ON + fixture で並列 wiring 確認、duration ≤ 4x role、score Δ ≥ -0.05 (lenient) | `cargo run --release --` × 3 |
| **G-5 (production default OFF 確証)** | env 未設定で `is_parallel_subagent_enabled() == false`、`execute_roles_parallel` 直接呼出も legacy 経路 | unit test `t_parallel_subagent_env_default_disabled` + `t_execute_roles_parallel_short_circuits_when_disabled` |
| **G-6 (Effectiveness、Phase 5 Lab、別 plan)** | Lab v20 paired t-test で AgentFloor 30 task / k=3 / ON-OFF、T4 paired Δ ≥ +0.015 + duration -10% 以下 + 全 tier Δ ≥ -0.01 で ACCEPT | 別 plan |

G-1 〜 G-5 PASS で本 plan delivery 完了 (Phase 5 / G-6 は別 plan)。

## 9. 完了条件

1. ✅ `SubAgentRole` enum (4 variant) + `short_code` + `embedded_prompt` + `default_set` 追加 (`src/agent/subagent.rs`)
2. ✅ `RoleResult` / `RoleDelegationResult` 構造体 + `all_succeeded` / `success_rate` 追加
3. ✅ `SubAgentExecutor::execute_roles_parallel` method + private helper (`execute_role_subtask` / `execute_roles_sequential`) 追加
4. ✅ `prompts/subagent_roles/{planner,coder,reviewer,tester}.txt` 4 file 新規 (§ 4.3 内容、各 ≤ 25 行、UTF-8、Bonsai-8B 1bit 向け簡潔表現)
5. ✅ `subagent` tool の `parameters_schema` に optional `roles` field 追加 + tool `call` で env gate + `execute_roles_parallel` dispatch
6. ✅ `BONSAI_PARALLEL_SUBAGENT_ENABLED` env opt-in、default OFF (項目 214/216-219 と pattern 統一) + `BONSAI_PARALLEL_SUBAGENT_MAX_ROLES` (default 3) + `BONSAI_PARALLEL_SUBAGENT_TIMEOUT_MS` (default 120000)
7. ✅ `AuditAction::ParallelRoleDispatch` variant 追加 (`observability/audit.rs`)
8. ✅ `ParallelSubAgentStats` informational metric (`benchmark.rs`、Phase 4 smoke で空集計)
9. ✅ TDD strict 5 phase 全消化 (Phase 1 Red → Phase 2 Green → Phase 3 Refactor → Phase 4 Smoke 3 段 → Phase 5 別 plan 起票確証)
10. ✅ 既存 1150 passed 維持、新規 7-9 test 追加で **1157-1159 passed**、clippy 0 / fmt clean / API 完全 additive (signature 変更ゼロ、新 method / enum 追加のみ) / smoke G-4a/b/c 全 PASS / CLAUDE.md 項目 224 候補追記 + handoff 起票 + INDEX.md G3 リンク

## 10. 見積もり

| Phase | 内容 | 時間 |
|---|---|---|
| **P0 (調査)** | subagent.rs 既読、tools/ 内 subagent tool 実装場所確認、env opt-in pattern (項目 214/217-219) 確認 | 0.4h |
| **P1 (Red)** | test 7-9 件追加、cargo test 全 fail 確認 | 1.0h |
| **P2 (Green)** | SubAgentRole enum + RoleResult/RoleDelegationResult + execute_roles_parallel + 4 prompt file + AuditAction::ParallelRoleDispatch + subagent tool 拡張 + ParallelSubAgentStats hook | 4.5h |
| **P3 (Refactor)** | pure merge fn + role audit + docstring + 16 ルール × 4 directive cross-check matrix + env mutex | 1.2h |
| **P4 (Smoke 3 段)** | G-4a env unset (5 min) + G-4b env=ON legacy (10 min) + G-4c env=ON fixture (15 min) + 解析 + 修正 buffer | 3.0h (実機 wall ~30 min) |
| **P6 (commit + handoff + CLAUDE.md)** | 5 commits + handoff 起票 + CLAUDE.md 項目 224 + INDEX.md | 0.7h |
| **計** | | **~10.8h ≈ 1.5 day** |

Phase 5 (Lab v20 parallel-subagent effectiveness paired t-test) は別 plan ~6h、本 plan delivery 範囲外。

派生 plan 候補 (本 plan ACCEPT 後):
- `lab-v20-parallel-subagent.md` (paired t-test、~6h、Phase 5 effectiveness)
- `parallel-subagent-resolve-strategy-impl.md` (Reviewer DISAGREE 時の voting / weighted resolve、~1 day、R2 軽減 Phase 2)
- `parallel-subagent-tokio-migration.md` (std::thread::scope → tokio::join! 移行、~0.5 day、R8 / R9 軽減 Phase 2)
- `parallel-subagent-2stage-impl.md` (Coder → Reviewer sequential サブパス追加、~1 day、R11 軽減 Phase 2)

## 11. Quick Start

```bash
# 0. 既存実装確認 (production code 変更ゼロ確証)
rg -n "SubAgentRole|execute_roles_parallel|RoleDelegationResult" src/  # 期待 0 件
rg -n "BONSAI_PARALLEL_SUBAGENT" src/                                   # 期待 0 件
ls /Users/keizo/bonsai-agent/prompts/                                   # heuristic_reflection.txt のみ
ls /Users/keizo/bonsai-agent/prompts/subagent_roles/ 2>/dev/null         # No such file

# 1. Phase 1 Red — test 追加
$EDITOR src/agent/subagent.rs   # 末尾 mod tests に 7-9 件追加
cargo test --lib subagent::tests   # 全 fail / compile error 確認
git commit -m "test(subagent): G3 Phase 1 Red — SubAgentRole + execute_roles_parallel 7-9 test"

# 2. Phase 2 Green — 実装
mkdir -p prompts/subagent_roles
$EDITOR prompts/subagent_roles/planner.txt    # § 4.3.1
$EDITOR prompts/subagent_roles/coder.txt      # § 4.3.2
$EDITOR prompts/subagent_roles/reviewer.txt   # § 4.3.3
$EDITOR prompts/subagent_roles/tester.txt     # § 4.3.4
$EDITOR src/agent/subagent.rs                 # SubAgentRole + execute_roles_parallel + helpers
$EDITOR src/observability/audit.rs            # ParallelRoleDispatch variant
$EDITOR src/agent/benchmark.rs                # ParallelSubAgentStats + run_k 集計 hook
# subagent tool 実装ファイル特定 (Phase 2 Green 着手時 grep "subagent" src/tools/)
$EDITOR src/tools/<subagent_tool>.rs           # parameters_schema に roles field + env gate dispatch
cargo test --release                            # 1157-1159 passed
cargo clippy --tests -- -D warnings
cargo fmt --check
git commit -m "feat(subagent): G3 Phase 2 Green — SubAgentRole + execute_roles_parallel + 4 role prompts"

# 3. Phase 3 Refactor
$EDITOR src/agent/subagent.rs                  # pure merge fn + role audit + docstring + env mutex
$EDITOR .claude/plan/parallel-subagent-roles-impl.md   # § 13 review log (16 ルール × 4 directive matrix)
git commit -m "refactor(subagent): G3 Phase 3 — pure merge fn + role audit + docstring + cross-check"

# 4. Phase 4 Smoke 3 段
cargo build --release

# G-4a: 既存経路後方互換
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 2>&1 | tee /tmp/g3_g4a.log
grep "parallel_role_dispatch" /tmp/g3_g4a.log   # 期待 0

# G-4b: env=ON / 既存 caller (legacy)
BONSAI_PARALLEL_SUBAGENT_ENABLED=1 BONSAI_LAB_SMOKE=1 \
  ./target/release/bonsai --lab --lab-experiments 0 2>&1 | tee /tmp/g3_g4b.log
grep "parallel_role_dispatch" /tmp/g3_g4b.log   # 期待 0 (legacy)

# G-4c: env=ON / fixture で強制 role dispatch
BONSAI_PARALLEL_SUBAGENT_ENABLED=1 BONSAI_PARALLEL_SUBAGENT_FIXTURE=1 BONSAI_LAB_SMOKE=1 \
  ./target/release/bonsai --lab --lab-experiments 0 2>&1 | tee /tmp/g3_g4c.log
grep "parallel_role_dispatch" /tmp/g3_g4c.log   # 期待 ≥ 1

# 5. commit + handoff + CLAUDE.md
$EDITOR CLAUDE.md       # 項目 224 候補追記
$EDITOR .claude/projects/-Users-keizo-bonsai-agent/memory/session_2026_05_XX_handoff.md
$EDITOR .claude/plan/INDEX.md  # G3 リンク追加
git commit -m "docs(subagent): G3 G-1〜G-4 PASS + 項目 224 候補 + handoff"

# 6. Phase 5 別 plan 起票 (本 plan delivery 範囲外)
$EDITOR .claude/plan/lab-v20-parallel-subagent.md
```

## 12. 参考

### 由来 meta-plan
- **`.claude/plan/building-ai-coding-agents-gap-analysis.md`** § 4 G3 (★ 低-中優先、~1.5 day 推定の起票元)

### bonsai 既存 plan (品質基準・TDD strict 5 phase 手本)
- **`.claude/plan/critic-separate-llm-impl.md`** (G1) — Critic 別 LLM 分離、本 plan の Reviewer role と協調設計、env opt-in pattern + AuditAction 拡張 pattern 直接参考
- **`.claude/plan/agent-side-tdd-enforcement-impl.md`** (G2) — 起票中、Tester role の TDD directive 重ね掛け先
- **`.claude/plan/task-aware-system-prompt-impl.md`** (G4) — `prompts/task_aware/` 同居 dir pattern + env opt-in + cross-check matrix の構造手本
- `.claude/plan/agentfloor-tier-eval-impl.md` (CapabilityTier 設計手本、Phase 5 Lab T4 評価基盤)
- `.claude/plan/cerememory-decay-port-impl.md` (env opt-in default OFF + license attribution + SCHEMA pattern)
- `.claude/plan/erl-defaults-off-switch-impl.md` (env name `BONSAI_<feature>_ENABLED` 統一、項目 216 同設計)
- `.claude/plan/lab-v17-erl-effectiveness.md` (Phase 5 Lab paired t-test 別 plan 構造手本)
- `.claude/plan/event-repository-trait-impl.md` (Mock + parity test pattern、項目 209 dividend、Phase 1 test の参考)

### bonsai 既存項目 (本 plan で reference する CLAUDE.md 項目)
- **項目 1**: Reflexion (parent agent merge 段で発火、共存)
- 項目 47: ツール使用前 `<think>` 強制 (デフォルト化、各 role directive 重ね掛け)
- 項目 50: フォールバック戦略 (デフォルト化、Coder/Tester directive 重ね掛け)
- **項目 55**: tool-level 並列 (本 plan = agent-level 並列、両立)
- **項目 120**: SubAgentExecutor 順次委任 (本 plan の base 経路、不変維持)
- 項目 136: 回答前ファイル内容確認 (デフォルト化、Reviewer directive 重ね掛け)
- **項目 160**: SubAgent 深度制限 2 + エラー境界 (本 plan で不変維持)
- 項目 187: ContextOverflowGuard / n_ctx_budget (親→子伝播 invariant 拡張)
- **項目 209**: AgentFloor 6-tier (Phase 5 Lab T4 評価対象)
- 項目 211: Self-Verify Phase 5 `BONSAI_LAB_PHASE5_FOCUS` (Lab focus filter pattern 流用候補)
- 項目 214: Lab v17 toggle (env opt-in pattern)
- **項目 215**: Lab v17 REJECT (天井 7 連続、構造変異枯渇 evidence、ACCEPT 基準先例)
- 項目 216-219: env name `BONSAI_<feature>_ENABLED` 統一、Cerememory 三本柱 + ERL defaults OFF
- **項目 222**: sqlite-vec wiring 削除 (REJECT 後 dead-code 化 pattern、Phase 5 REJECT 時参考)

### bonsai source files (本 plan で grep / 参照、production code 変更ゼロで Phase 2 着手時の対象)
- **`src/agent/subagent.rs`** (943 行、本 plan の中核拡張先、`SubAgentRole` / `execute_roles_parallel` 追加)
- `src/agent/agent_loop/core.rs` (loop 本体、parent agent merge 段で `RoleDelegationResult.merge_prompt` 受取)
- `src/agent/agent_loop/config.rs` (`AgentConfig`、role 別 system_prompt 差替先)
- `src/runtime/inference.rs` (`LlmBackend` trait、role 並列実行で各 thread が同 backend 共有 — invariant: backend は Send + Sync)
- `src/observability/audit.rs` (`AuditAction` enum、本 plan で `ParallelRoleDispatch` 追加)
- `src/agent/benchmark.rs` (`MultiRunBenchmarkResult`、本 plan で `ParallelSubAgentStats` 追加)
- `src/tools/` (subagent tool 実装場所、Phase 2 Green で grep 経由特定、`parameters_schema` 拡張先)
- `prompts/heuristic_reflection.txt` (項目 213 同居先例、本 plan は `prompts/subagent_roles/` 4 file 配置)

### 論文・survey
- **arxiv 2603.05344** — Building AI Coding Agents for the Terminal (G3 由来論文、本 plan の主軸)
- **arxiv 2602.07359** — W&D: Scaling Parallel Tool Calling for Efficient Deep Research (役割分業 + 並列 evidence)
- **arxiv 2604.18509** — MASS-RAG: Multi-Agent Synthesis (Evidence summarizer / Extractor / Reasoner 並列)
- arxiv 2603.21357 — AgentHER ECHO + HSL (項目 201、Tester role と HSL relabel 相性検証 Phase 5 候補)
- arxiv 2605.00334 — How Far Up the Tool Use Ladder Can Small Open-Weight Models Go? (項目 209 AgentFloor 6-tier)

### CODEX_SESSION (for `/ccg:execute` use)
- 新規取得推奨 (本 plan は G3 derivative 起票で項目 213/214/210 の既存 session 不適)
- 既存 session 流用検討時: `019e064a-334c-7692-9735-c5d95231ebf1` (項目 213 ERL plan v2 起票時の session、env opt-in pattern context が近い)

## 13. Phase 3 Review Log (Phase 3 完了後追記)

### 13.1 16 ルール × 4 role directive cross-check matrix
> Phase 3 着手後にここに追記する。各セルは「重複」(= 重ね掛け強調 OK) / 「矛盾」(= 文面修正必須) / 「独立」(= no-op) の 3 値。

| 16 ルール ↓ \ role → | Planner | Coder | Reviewer | Tester |
|---|---|---|---|---|
| 1 簡潔 | (TBD) | (TBD) | (TBD) | (TBD) |
| 2 繰り返し回避 | (TBD) | (TBD) | (TBD) | (TBD) |
| 3 ツール使用前 think (項目 47) | (TBD) | (TBD) | (TBD) | (TBD) |
| 4 代替手順提示 (項目 50) | (TBD) | (TBD) | (TBD) | (TBD) |
| ... 5-15 ... | ... | ... | ... | ... |
| 16 回答前ファイル内容確認 (項目 136) | (TBD) | (TBD) | (TBD) | (TBD) |

### 13.2 SOUL.md base persona との整合
> Phase 3 着手後にここに追記する。SOUL.md base persona 文面 (3 段検索 default) と各 role directive の整合性確認。Reviewer role が「事実誤認・論理破綻・見落とし・代替案」軸で批判する際、SOUL.md の親しみやすさ persona と矛盾しないか確認。

### 13.3 Lab metric 連携計画
> Phase 5 別 plan で `ParallelSubAgentStats` 集計経路を確定。本 plan delivery 範囲は struct + 集計 hook までで、effectiveness 検証は別 plan。

## 14. ★ Phase 5 Effectiveness REJECT 時 handling

Lab v20 paired t-test で全 tier (特に T4) Δscore が +0.015 未満 / duration -10% 未満 / 副作用 -0.01 超:

1. **production default `BONSAI_PARALLEL_SUBAGENT_ENABLED` 未設定維持** (= legacy 既定化、構造変更不要、項目 214/216 同 pattern)
2. **`prompts/subagent_roles/` 全削除候補** (env unset で無効、保守負担削減)、または将来 variant base として残置 (G2 TDD merge 後に Tester directive 単独活用候補)
3. **`SubAgentRole` / `execute_roles_parallel` dead-code 候補化** (項目 215 ERL pattern + 項目 222 sqlite-vec wiring 削除 pattern 踏襲)
4. **副次 finding 検討**:
   - duration 短縮効果のみ ACCEPT 候補 (score Δ ≈ 0 + duration -20% ならコスト面で informational ACCEPT)
   - role_disagreement_rate 高 = Reviewer effective evidence、stability 軸 (std 縮小) で項目 200 RDC/VAF re-eval 候補
   - role 個別 effective なら subset (e.g., Coder + Tester 2 role のみ) で再 plan 起票
5. **CLAUDE.md** に negative finding 記録 (Lab 天井打破失敗 evidence、項目 215 pattern)、**arxiv 2603.05344 P7 知見が 1bit ローカル文脈で効かなかった** ことを external validation の反対側として記録
6. **後続 plan 検討**:
   - `parallel-subagent-tokio-migration.md` で真の async 並列 (M2 メモリ余裕あれば、Phase 2)
   - `parallel-subagent-2stage-impl.md` で Coder → Reviewer sequential (R11 軽減、Phase 2)
   - dead-code 削除 plan は別 session (項目 222 pattern: sqlite-vec wiring 削除と同経路)
