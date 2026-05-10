# Plan: G2 Agent-Side TDD Enforcement — agent harness が test-first 構造を強制 (Phase 1: 同 backend opt-in)

> **由来 meta-plan**: `.claude/plan/building-ai-coding-agents-gap-analysis.md` § 4 G2 (★★ 中優先、P6 完全 gap = 唯一の ❌ 判定)
> **由来項目**: 項目 1/9 (Reflexion / Continue Sites)、項目 10 (計画強制ルール、Lab v6.2 唯一 ACCEPT)、項目 47 (`<think>` 強制、Lab v6.2 ACCEPT)、項目 50 (戦略的 alternate-tool 選択、Lab v6.2 ACCEPT)、項目 120/160 (`SubAgentExecutor`)、項目 209 (`EventRepository` trait + Mock)、項目 213 (ERL Heuristics Pool)、項目 214/216 (env opt-in `BONSAI_<feature>_ENABLED` pattern)、項目 217-219 (Cerememory 三本柱 default OFF)、項目 222 (sqlite-vec wiring 削除 = REJECT 後 dead-code 化先例)
>
> **位置付け (重要)**: meta-plan G2 で **「P6 = 唯一の ❌ gap」** と判定された "agent-side TDD enforcement" を bonsai に追加する **Phase 1 plan**。CLAUDE.md 巻頭の TDD section は **人間運用** 向けで、bonsai agent 自身は test-first で動かない (Reflexion 経由の事後 self-critique のみ)。本 plan は agent harness が **code-modification multi-step task** で **「テストを先に書くか?」を構造的に促す** prompt + tool-pattern monitoring + G1 critic 協調 経路を additive に追加する。
>
> **Phase 1 scope**: env opt-in default OFF、production code 変更ゼロ前提 (本 plan merge 時点では plan ファイル 1 件追加のみ、実装は別 session で `/ccg:execute` 経由)。Lab paired t-test は **別 plan** `lab-v19-tdd-effectiveness.md` 起票 (G1 plan と同 pattern)。
>
> **G1 dependency**: G1 critic plan (`critic-separate-llm-impl.md` 既起票、項目 223 候補) と協調設計。本 plan は G1 と **独立着手可** だが、G1 + G2 連携で本領発揮 (4.6 節 G1 協調経路で具体化)。

## Task Type
- [ ] Frontend
- [x] Backend (新規 module `src/agent/tdd_enforcement.rs` / `prompts/tdd/*.txt` 4 file / `agent_loop/core.rs` の inject 順序に hook 追加 / `tools/shell.rs` は read-only でテスト実行 pattern match のみ / `observability/audit.rs::AuditAction::TddEvent` 追加 / `agent/subagent.rs` は read-only)
- [ ] Fullstack
- [ ] Docs

## 1. 背景

### 1.1 P6 完全 gap (唯一の ❌) 経緯

`building-ai-coding-agents-gap-analysis.md` § 3.2 の 10 pattern 評価で、**P6 Test-driven verification** だけが ❌ (= 既存項目 reference ゼロ) と判定された。9 patterns (P1/P2/P3/P4/P5/P7/P8/P9/P10) は ✅ 完全 or 🟡 部分実装で何らかの reference を持つ。bonsai は CLAUDE.md 巻頭で **「Development Philosophy: TDD (Strict)」** を Red→Verify→Commit→Green→Refactor の 5 step として明文化しているが、これは **人間/Claude Code 運用** 向けで、bonsai agent loop 内部 (`run_agent_loop`) はテストを先に書く構造を持たない。

### 1.2 人間 TDD vs agent TDD の根本差

| 観点 | 人間/Claude Code TDD (CLAUDE.md 既存) | agent-side TDD (本 plan の対象) |
|---|---|---|
| 主体 | Claude Code (or 人間 IDE) | Bonsai-8B 自身 (1bit ローカル) |
| 強制経路 | `~/.claude/rules/development-workflow.md` 文書指示 | agent_loop 内 hook、prompt 注入、tool 監視 |
| 実行タイミング | 機能実装前 | code-write tool 呼出前後 |
| 検証 | 人間レビュー | tool_call patterns + Reflexion + G1 critic |
| 失敗時 | git pre-commit hook で block | session.add_message で agent に修正促す |

CLAUDE.md の TDD は **bonsai agent への運用指針ではなく、bonsai を開発する Claude Code (or 人間) 向けの開発ポリシー**。bonsai 自身が code-modification task を解く時、test を先に書く必要は **ハーネス側で構造的に強制されない限り** モデル側 (1bit Bonsai-8B) の意思に依存する。

### 1.3 Reflexion (項目 1/9) との対比

`agent_loop/advisor_inject.rs::inject_verification_step` (項目 17/18/89) は **iteration > 0 + 複雑タスク** で FinalAnswer 直前に self-critique を挟む。これは **事後検証** (= 実装したコードが妥当か後から問う)。

agent-side TDD は **事前検証**:
- code-write tool (file_write / multi_edit) の **呼出前に「期待挙動の test を先に追加するか?」を agent に問う prompt 注入**
- 実装直後に `cargo test` 等を実行する pattern を tool_call で検出 → TddPhase 遷移
- test を書かず実装した場合 = G1 critic と協調して警告

**両者は補完関係**: Reflexion (事後) + TDD (事前) = 2 段防衛 (項目 1 補強、Lab v15-v17 構造変異枯渇 = 項目 215 への補助線)。

### 1.4 設計参考: Cursor / Aider / Claude Code

| Agent | TDD pattern | bonsai 該当物 |
|---|---|---|
| **Cursor** | "test-first agent mode" (チェックボックス、user opt-in) | (G2 Phase 1 で対応) |
| **Aider** | `auto-test` flag — diff 適用後に `--test-cmd` を自動実行、失敗で自動修正 loop | tool_call で `cargo test` pattern 検出 + TddPhase TestPassing 遷移 |
| **Claude Code** | sandboxed-cli pattern — multi-step plan で test を先に書くよう内部指示 | system prompt 注入 (`prompts/tdd/*.txt`) |
| **Devin** | task type 別 prompt (debug / refactor / new feature) | TaskComplexityHeuristic で multi-step 検出 → TDD 起動 |

3/4 は **「prompt 強化 + test-cmd 検出 + 自動 loop」** を組合せ。Phase 1 は最小実装としてこの 3 要素を opt-in で導入する。

### 1.5 1bit Bonsai-8B 文脈での意義

- **検証ステップ多用問題** (項目 17-24 で advisor max_uses=3 制限導入済) を **構造的に解決** する補助線: test を agent 自身が書けば、verification 自動化 (test 実行 = pass/fail の決定論的 signal) で Self-Verification Dilemma (項目 210/212) も緩和される可能性
- HSL relabel (項目 201-205) との相性: test green = "達成済 subgoal" として hindsight 抽出可能 (failed_trajectory で test red のまま終わった step が relabel 対象)
- 1bit モデルが directive を **無視するリスク高** (Lab v15 で #47 思考強制再生成、自然言語 directive の効果限界) → effective でなければ Phase 5 Lab REJECT で dead-code 化 (項目 222 sqlite-vec wiring 削除 pattern 踏襲)

### 1.6 Lab 天井 7 連続 (項目 215) との関係

Lab v8/v9/v10/v14/v15/v16/v17 全 REJECT、prompt-level / config-level / context-level の 3 軸構造変異枯渇 (CLAUDE.md 項目 207-215)。本 plan は **第 5 軸 (workflow-structural variation = TDD enforcement)** で天井打破候補。期待値は global score 改善ではなく、**T5 ErrorRecovery / T6 LongHorizonPlanning (AgentFloor 6-tier、項目 209)** の局所改善 (4.8 Lab metric)。

## 2. 目的

1. **agent 自身が test-first で動く構造を提供** — code-write tool 呼出前に test 追加を促す prompt 注入経路を opt-in で実装
2. **multi-step task の信頼性向上** — TaskComplexityHeuristic で multi-step を検知 → TDD 起動 → tool_call pattern で TddPhase 遷移を追跡 → Reflexion + G1 critic と協調で誤実装を catch
3. **G1 critic と協調** — critic 段階で「test 書かず実装」を検出 → `CriticDisagreementAction::Inject` 経路で disagreement message 注入 (G1 plan 既設計と接続)
4. **Lab 天井 7 連続打開の補助線** — workflow-structural 変異 (= 第 5 軸) として AgentFloor T5/T6 局所改善を狙う (Phase 5 別 plan で paired t-test、項目 215 ACCEPT 基準 = Δ ≥ +0.015 + p < 0.1 踏襲)

### 非目標

- **人間 TDD ワークフローの強制ではない** — CLAUDE.md 巻頭の Red→Verify→Commit→Green→Refactor (人間運用) は不変、本 plan は agent loop 内挙動のみ拡張
- **単一 file 修正 task は対象外** — TaskComplexityHeuristic で multi-step (subagent 利用 / file_write 連続 N 回 / `<plan>` で N+ step 言及) のみ起動
- **coverage 強制ではない** — 80% カバレッジ等の数値強制はしない (ハーネス側の責務外、Lab effectiveness で間接評価)
- **test framework 完全網羅ではない** — Phase 1 は `cargo test` / `pytest` / `npm test` / `go test` の 4 pattern のみ (extensibility は config で `prompts/tdd/test_runners.toml` 経由、Phase 2 候補)
- **production default ON 化** — env unset で完全互換 (項目 214/216-219 と pattern 統一)、Phase 5 Lab ACCEPT 後に defaults 昇格検討
- **Lab paired t-test の本 plan 内実装** — 別 plan `lab-v19-tdd-effectiveness.md` 起票 (G1 plan と同 pattern)

## 3. 既存項目との関係

| 項目 | 関係 | 本 plan での扱い |
|---|---|---|
| **1 (Reflexion)** | 同一 LLM 内 self-critique、事後検証経路 | 共存。本 plan は **事前検証** (test-first) で並列、Reflexion REJECT 経路 (項目 215) を拡張ではなく代替候補として位置付け |
| **9 (Continue Sites / Replan)** | 停滞時別ロール再計画 | 影響なし。`inject_replan_on_stall` は本 plan と独立 (TDD 中の test 失敗連続は LoopDetector が捕捉) |
| **10 (計画強制ルール、Lab v6.2 ACCEPT)** | multi-step task 計画強制、デフォルト化済 | 重複ではなく **強化**: TDD は計画強制の subset (test-first を計画 step に含めるよう促す)、prompt は重ね掛け強調 (項目 213 同 pattern) |
| **47 (`<think>` 強制、Lab v6.2 ACCEPT)** | ツール使用前 think 強制、デフォルト化済 | TDD prompt 内で「テスト追加を think で明示」を重ね掛け |
| **50 (代替手段 / 別 tool 戦略、Lab v6.2 ACCEPT)** | エラー時別 tool 試行 | TDD failing test → 実装 loop 内で代替手段動作と整合、TestFailing 状態で別 tool 推奨 |
| **120/160 (`SubAgentExecutor`)** | 順次委任、深度制限 2 | TaskComplexityHeuristic の signal の 1 つ = subagent 起動検知で multi-step 判定 |
| **136 (回答前ファイル内容確認、Lab v9 ACCEPT)** | デフォルト化済 | TDD prompt 内で「test 内容を読み返す」step として整合 |
| **161 (Skill 軌跡昇格)** / **201-205 (AgentHER)** | failed/successful trajectory → skill 昇格 | TDD は HSL relabel と相性良好: TestPassing 達成 trajectory = successful subgoal、TestFailing で諦めた trajectory = failed (relabel 対象) |
| **187 (ContextOverflowGuard)** | 累積 token 監視、強制 compaction | TDD prompt 追加で system prompt size 増加、ContextOverflowGuard で吸収 (R6 mitigation) |
| **209 (`EventRepository` trait + Mock)** | trait 化 dividend | TDD test (Phase 1 Red) で Mock 経由 SQLite なし unit test 可能化、本 plan は `&dyn EventRepository` で TddEvent 集計 (Phase 4 smoke で event 抽出) |
| **210 (Self-Verify dynamic skip)** | `AdvisorConfig.dynamic_skip_threshold` | TDD ON 時は Self-Verify 過剰発動が緩和される仮説 (= test pass で verification 不要)、Phase 5 Lab で ON/OFF interaction 観測候補 |
| **211 (Self-Verify Phase 5 `BONSAI_LAB_PHASE5_FOCUS`)** | Lab variant 機構 | 同 pattern で `BONSAI_LAB_PHASE5_FOCUS=tdd_phase` 流用候補 (Phase 5 Lab plan で扱う) |
| **213 (ERL Heuristics Pool)** | `prompts/heuristic_reflection.txt` 同居先例 | `prompts/tdd/*.txt` を `prompts/` 直下 sub-dir に配置、`include_str!` pattern 流用 |
| **214 (Lab v17 toggle 機構)** | `BONSAI_<feature>_DISABLED` pattern (項目 216 で `_ENABLED` 統一) | `BONSAI_TDD_ENFORCEMENT_ENABLED=1` で env opt-in、項目 217-219 と完全統一 |
| **215 (Lab v17 REJECT、天井 7 連続)** | ACCEPT 基準 + dead-code 化先例 | Phase 5 Lab plan で同基準 (Δ ≥ +0.015 + p < 0.1)、REJECT 時 dead-code 化 |
| **216 (ERL defaults OFF 切替)** | env name `BONSAI_<feature>_ENABLED` 統一 + default false | 設計踏襲 |
| **217-219 (Cerememory 三本柱)** | env opt-in default OFF + 完全 additive API | 設計踏襲 (4.7 節) |
| **222 (sqlite-vec wiring 削除)** | REJECT 後 dead-code 化 pattern | Phase 5 REJECT 時の handling 先例として参照 (§ 14) |
| **223 候補 (G1 critic plan)** | 別 LLM critic 機構、`critic-separate-llm-impl.md` 既起票 | **dependency**: G1 + G2 連携で本領発揮、本 plan は G1 と独立着手可 (4.6 G1 協調経路) |

## 4. 設計

### 4.1 `TddEnforcementConfig` struct (新規 module `src/agent/tdd_enforcement.rs`)

新規 module で AdvisorConfig (`runtime/model_router.rs`) や CriticConfig (G1) と並列に追加。AdvisorConfig に乗せず別 module + 別 struct にする (設計判断と理由):

- AdvisorConfig は既に 16 フィールドで肥大化、scope creep 回避 (G1 plan 4.1 と同判断)
- TDD 機構は Reflexion / Critic と直交する **workflow-structural** な軸 = independent module が自然
- Phase 5 REJECT 時の dead-code 化 (項目 222 pattern) が容易

```rust
//! G2 Agent-Side TDD Enforcement (項目 224 候補)。
//!
//! agent harness が code-modification multi-step task で test-first 構造を促す。
//! production default OFF (`BONSAI_TDD_ENFORCEMENT_ENABLED` env unset で観測動作完全互換)、
//! env=1 で opt-in。
//!
//! 由来 meta-plan: building-ai-coding-agents-gap-analysis.md G2

use crate::agent::conversation::{Message, Session, ToolCall};

#[derive(Debug, Clone)]
pub struct TddEnforcementConfig {
    /// `BONSAI_TDD_ENFORCEMENT_ENABLED=1` で opt-in。default OFF で observable 動作完全互換。
    pub enabled: bool,

    /// TaskComplexityHeuristic 起動の閾値 (signal_count >= multi_step_threshold で起動)
    /// default 2 = 「subagent 利用 + file_write 2 回」or 「`<plan>` で N+ step 言及 + file_write」等
    pub multi_step_threshold: usize,

    /// file_write 連続 N 回で multi-step 判定 (default 2)
    pub file_write_chain_min: usize,

    /// `<plan>` 内で N+ step 言及で multi-step 判定 (default 3)
    pub plan_step_min: usize,

    /// 現在の TddPhase (state machine、各 step で更新)
    pub phase: TddPhase,

    /// 各 phase で許容される最大 iteration (TestFailing が連続して loop しない gate)
    pub max_failing_iterations: usize,

    /// 現在 phase に滞在した iteration 数
    pub iterations_in_phase: usize,

    /// test runner pattern 集 (default = ["cargo test", "pytest", "npm test", "go test"])
    /// `prompts/tdd/test_runners.toml` 経由で extensible (Phase 2 候補)
    pub test_runner_patterns: Vec<String>,

    /// G1 critic 協調モード (CriticDisagreementAction::Inject 連携)
    pub critic_coordination: TddCriticCoordination,

    /// disagreement action: TestPending で file_write 検知時の挙動
    pub on_premature_implementation: TddPrematureAction,
}

/// TDD phase state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TddPhase {
    /// 初期 / multi-step 未検出 / TDD 不適用
    #[default]
    NotApplicable,
    /// multi-step task 検出済 (TaskComplexityHeuristic = true)、TDD 起動候補
    Detected,
    /// agent に test-first prompt 注入済、test 追加待ち
    TestRequested,
    /// test 追加 tool_call (file_write *_test.rs 等) 検出済、test 実行待ち
    TestPending,
    /// test 実行で fail 確認済 (Red 確証)、実装許可
    TestFailing,
    /// 実装 file_write 検出済、test 再実行待ち
    Implementing,
    /// test 再実行で pass 確認済 (Green 達成)
    TestPassing,
    /// 完了後 refactor 段 (任意)
    Refactoring,
}

impl TddPhase {
    pub fn label(self) -> &'static str { /* "not_applicable" .. "refactoring" */ }
    /// 次に許容される phase の集合 (state machine 制約)
    pub fn allowed_next(self) -> &'static [TddPhase] {
        match self {
            Self::NotApplicable => &[Self::Detected],
            Self::Detected => &[Self::TestRequested, Self::NotApplicable],
            Self::TestRequested => &[Self::TestPending, Self::NotApplicable],
            Self::TestPending => &[Self::TestFailing, Self::TestPassing, Self::NotApplicable],
            Self::TestFailing => &[Self::Implementing, Self::NotApplicable],
            Self::Implementing => &[Self::TestPending, Self::TestPassing, Self::TestFailing],
            Self::TestPassing => &[Self::Refactoring, Self::NotApplicable],
            Self::Refactoring => &[Self::NotApplicable],
        }
    }
}

/// G1 critic との協調モード
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TddCriticCoordination {
    /// G1 critic 不在時の単独動作 (Phase 1 default)
    #[default]
    Standalone,
    /// G1 critic 連携: test 書かず実装検出時に CriticDisagreementAction::Inject 経路で警告
    CooperativeWithG1,
    /// shadow mode: 検出 log のみ、prompt 注入なし
    LogOnly,
}

/// TestPending phase で file_write (test 以外) 検知時の挙動
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TddPrematureAction {
    /// system message で警告注入、agent 続行 (Phase 1 default)
    #[default]
    InjectWarning,
    /// log only、production 影響ゼロ (smoke / shadow mode)
    LogOnly,
    /// tool_call を一時的に block、replan 強制 (Phase 2 候補)
    BlockAndReplan,
}
```

### 4.2 `TddPhase` 遷移経路 (state machine 詳細)

```
                                                ┌─ TestPassing ── Refactoring ── NotApplicable
                                                │
NotApplicable ── Detected ── TestRequested ── TestPending ── TestFailing ── Implementing ─┐
       ▲                                            │                                      │
       │                                            └─── TestPassing (test 即 pass で skip) │
       │                                                                                    │
       └────────────────────────────────────────────────────────────────────────────────────┘
```

**遷移トリガー**:
- `NotApplicable → Detected`: TaskComplexityHeuristic が `true` 返却 (4.3)
- `Detected → TestRequested`: agent_loop 内で TDD prompt 注入完了 (`prompts/tdd/test_first.txt`)
- `TestRequested → TestPending`: file_write tool_call で path に `_test.rs` / `test_*.py` / `*.test.ts` 等の test naming convention match
- `TestPending → TestFailing`: shell tool_call で test_runner pattern + 出力に "FAILED" / "fail" 検出
- `TestPending → TestPassing`: 同 + 出力に "PASSED" / "ok. " 検出 (test が即 pass = test がトリビアル or 実装済の signal)
- `TestFailing → Implementing`: file_write tool_call で path が test 命名規則 **以外** で hit
- `Implementing → TestPending`: shell tool_call で test_runner pattern 再検出
- いずれの phase からも `→ NotApplicable`: cancel / abort / iterations_in_phase > max_failing_iterations

### 4.3 `TaskComplexityHeuristic` (multi-step task 判定)

```rust
pub struct TaskComplexityHeuristic;

impl TaskComplexityHeuristic {
    /// signal_count を計算、>= config.multi_step_threshold で multi-step 判定。
    ///
    /// signals:
    ///   1. subagent_active: SubAgentExecutor が現在 active (項目 120/160)
    ///   2. file_write_chain >= config.file_write_chain_min: 直近 N tool_call で file_write/multi_edit 連続
    ///   3. plan_step_count >= config.plan_step_min: 最新 `<plan>` ブロックで N+ step 言及
    ///   4. detect_task_complexity(input): 既存 (項目 210、`outcome.rs:131`) で bool=true
    ///
    /// 推定不能 (空 session) で signal_count=0 → false (= NotApplicable 維持)
    pub fn evaluate(
        session: &Session,
        config: &TddEnforcementConfig,
        subagent_active: bool,
        recent_tool_calls: &[ToolCall],
    ) -> bool {
        let mut signal_count = 0usize;
        if subagent_active { signal_count += 1; }

        // signal 2: file_write_chain
        let chain = count_consecutive_write_tools(recent_tool_calls);
        if chain >= config.file_write_chain_min { signal_count += 1; }

        // signal 3: plan_step_count (最新 user/assistant message から `<plan>` 抽出 + step 数えあげ)
        if extract_plan_step_count(session) >= config.plan_step_min { signal_count += 1; }

        // signal 4: 既存 detect_task_complexity 流用 (read-only)
        let last_user = session.last_user_message_text();
        if let Some(text) = last_user {
            if crate::agent::agent_loop::outcome::detect_task_complexity(&text) {
                signal_count += 1;
            }
        }

        signal_count >= config.multi_step_threshold
    }
}
```

**重要設計点**:
- signal は **OR 集約ではなく AND 性集約** (≥ threshold) = false-positive 抑制 (R1 mitigation)
- `detect_task_complexity` は read-only 流用 (既存 bool 判定、項目 210 共有)
- subagent_active は `agent_loop/core.rs` の loop scope で変数として伝播 (signature 拡張は config 経由のみで signature 変更ゼロ)

### 4.4 prompt 注入経路 (`agent_loop/core.rs` の inject 順序)

`src/agent/agent_loop/core.rs:136-142` の既存 inject 順序:
```rust
inject_memory_blocks(session, &config.soul_path, &config.memory_blocks);  // 既存
let injected_heuristic_ids = inject_heuristics(session, &task_context, store);  // 既存 (項目 213)
inject_contextual_memories(session, &task_context, store);  // 既存 (項目 80)
inject_planning_step(session, &task_context);  // 既存 (項目 10)
```

**変更後** (TDD hook を inject_heuristics 直前に挿入、計画強制 `inject_planning_step` の影響範囲外で先行注入):
```rust
inject_memory_blocks(session, &config.soul_path, &config.memory_blocks);  // 既存
// G2 項目 224 候補: TDD enforcement directive (env=BONSAI_TDD_ENFORCEMENT_ENABLED=1 で opt-in)。
// production default unset で no-op (legacy 完全互換)。multi-step 判定で None 返却 = skip。
let _tdd_phase = crate::agent::tdd_enforcement::inject_tdd_directive(
    session,
    &task_context,
    &mut config.tdd_enforcement,
    /* subagent_active: */ false,  // loop scope variable で伝播 (実装時に確定)
    /* recent_tool_calls: */ &[],   // 同上
);
let injected_heuristic_ids = inject_heuristics(session, &task_context, store);  // 既存
inject_contextual_memories(session, &task_context, store);  // 既存
inject_planning_step(session, &task_context);  // 既存
```

注入順序の根拠:
1. `inject_memory_blocks` (SOUL.md persona 先) → 人格基盤
2. **`inject_tdd_directive` (本 plan、ここ)** → workflow 構造指示
3. `inject_heuristics` (項目 213) → 過去経験
4. `inject_contextual_memories` (項目 80) → タスク文脈
5. `inject_planning_step` (項目 10) → 計画 step (TDD directive を **読んだ後** に計画作成)

### 4.5 test 実行検出 (`tools/shell.rs` の tool_call で pattern match)

`tools/shell.rs::ShellTool::call` 内には改変を加えず (read-only)、agent_loop 側で **post-tool-call hook** として pattern match を実装:

```rust
// agent/tdd_enforcement.rs
/// shell tool_call の args + 結果から test runner pattern を検出。
///
/// 検出 pattern (default config.test_runner_patterns):
///   - "cargo test" prefix
///   - "pytest" prefix
///   - "npm test" / "npm run test"
///   - "go test"
///
/// 結果 stdout/stderr の "FAILED" / "fail" / "0 passed" 等で TestFailing、
/// "PASSED" / "ok. " / "passed" 等で TestPassing 判定。
pub fn detect_test_outcome(
    tool_call: &ToolCall,
    tool_result: &str,
    config: &TddEnforcementConfig,
) -> Option<TestOutcome> {
    if tool_call.name != "shell" { return None; }  // 既存 ToolCall.name field
    let cmd = tool_call.args.get("command")?.as_str()?;
    if !config.test_runner_patterns.iter().any(|p| cmd.contains(p)) {
        return None;
    }
    // pattern match for FAILED/PASSED/...
    if tool_result.contains("FAILED") || tool_result.contains("test failed") {
        Some(TestOutcome::Failed)
    } else if tool_result.contains("test result: ok") || tool_result.contains("passed") {
        Some(TestOutcome::Passed)
    } else {
        Some(TestOutcome::Inconclusive)  // phase 遷移しない
    }
}

pub enum TestOutcome {
    Passed,
    Failed,
    Inconclusive,
}
```

agent_loop 側 (post-tool-call、`tool_exec.rs` 直後の hook):
```rust
if let Some(outcome) = detect_test_outcome(&tool_call, &tool_result, &config.tdd_enforcement) {
    config.tdd_enforcement.transition_on_test_outcome(outcome);
}
```

### 4.6 G1 Critic との協調

G1 plan (`critic-separate-llm-impl.md`、項目 223 候補) は `CriticDisagreementAction::InjectAsSystemMessage` (default) で executor に disagree を inject。本 plan は G1 critic の **before answer** hook で「test 書かず実装」検出時に disagreement を発火する経路:

```rust
// G1 critic plan の inject_critic_review 内で本 plan の API を呼ぶ
// (G1 plan で要 import 追加、本 plan は API 提供のみ)

pub fn check_test_first_compliance(
    session: &Session,
    tdd_config: &TddEnforcementConfig,
    answer: &str,
) -> Option<TddViolation> {
    if !tdd_config.enabled { return None; }
    if tdd_config.phase == TddPhase::TestRequested && /* answer indicates implementation */ {
        return Some(TddViolation::ImplementationWithoutTest);
    }
    if tdd_config.phase == TddPhase::TestFailing
        && tdd_config.iterations_in_phase > tdd_config.max_failing_iterations
    {
        return Some(TddViolation::TooManyFailingIterations);
    }
    None
}

pub enum TddViolation {
    ImplementationWithoutTest,
    TooManyFailingIterations,
    PrematureRefactoring,
}
```

G1 plan 側 (`inject_critic_review` 内、`tdd_critic_coordination=CooperativeWithG1` 時のみ):
```rust
// 既存 G1 critic 評価
let mut critic_outcome = run_critic_llm(/* ... */);
// G2 violation を critic disagreement に追加
if let Some(violation) = check_test_first_compliance(&session, &tdd_config, answer) {
    if matches!(critic_outcome, CriticOutcome::Agree { .. }) {
        critic_outcome = CriticOutcome::Disagree {
            raw_response: format!("TDD violation: {:?}", violation),
            suggested_revision: Some("先にテストを追加してください".to_string()),
        };
    }
}
```

**G1 独立着手の根拠**: 本 plan の `check_test_first_compliance` は G1 plan が unmerge でも compile 可 (純関数、G1 import は本 plan API を `Option` で扱う)。G1 + G2 連携で本領発揮だが、G2 単独でも **TddPrematureAction::InjectWarning** 経路で動作 (4.1 設計)。

### 4.7 env opt-in `BONSAI_TDD_ENFORCEMENT_ENABLED`

| env | default | 効果 |
|---|---|---|
| `BONSAI_TDD_ENFORCEMENT_ENABLED` | unset (= OFF) | unset / `0` / `false` で `enabled=false` → `inject_tdd_directive` 冒頭 short-circuit |
| `BONSAI_TDD_MULTI_STEP_THRESHOLD` | `2` | multi-step 判定 signal_count 下限 |
| `BONSAI_TDD_FILE_WRITE_CHAIN_MIN` | `2` | file_write 連続 N 回判定 |
| `BONSAI_TDD_PLAN_STEP_MIN` | `3` | `<plan>` 内 step 言及 N+ 判定 |
| `BONSAI_TDD_MAX_FAILING_ITER` | `3` | TestFailing で max_failing_iterations の loop gate |
| `BONSAI_TDD_CRITIC_COORD` | `standalone` | `standalone` / `cooperative` (G1 連携) / `log_only` (shadow) |
| `BONSAI_TDD_PREMATURE_ACTION` | `inject_warning` | `inject_warning` / `log_only` / `block_replan` (Phase 2 候補) |

env name 命名は項目 217-219 (`BONSAI_<feature>_ENABLED` opt-in 形式) と整合、Cerememory 三本柱と完全統一。

### 4.8 Lab metric: tdd_compliance_rate / cycle 完走率 / TddPhase 遷移時間

`MultiRunBenchmarkResult` への informational metric 追加 (ACCEPT 判定不変、項目 200 Beyond pass@1 / 項目 209 trait dividend pattern):

```rust
pub struct TddStats {
    pub multi_step_detected_count: usize,    // TaskComplexityHeuristic = true 件数
    pub tdd_phase_started_count: usize,      // Detected → TestRequested 達成件数
    pub test_first_compliance_count: usize,  // TestPending 達成件数 (= test を先に書いた件数)
    pub test_passing_count: usize,           // TestPassing 達成件数 (= Green 達成)
    pub premature_implementation_count: usize, // TestRequested → 直接 file_write 検出件数 (violation)
    pub max_failing_iter_exceeded_count: usize, // gate 発動件数
    pub avg_phase_transition_secs: f64,      // phase 遷移時間中央値 (informational)
}

impl TddStats {
    pub fn compliance_rate(&self) -> Option<f64> {
        let denom = self.multi_step_detected_count;
        if denom == 0 { None } else {
            Some(self.test_first_compliance_count as f64 / denom as f64)
        }
    }
    pub fn green_rate(&self) -> Option<f64> {
        let denom = self.tdd_phase_started_count;
        if denom == 0 { None } else {
            Some(self.test_passing_count as f64 / denom as f64)
        }
    }
}
```

TSV 列拡張 (informational only): G1 plan で 15→18 列、本 plan で **18 → 22 列** 追加 (`tdd_detected / tdd_compliance / tdd_green / tdd_premature`)。

**Lab ACCEPT 判定** (Phase 5 別 plan): paired t-test で TDD ON vs OFF の `composite_score` Δ で判定。`compliance_rate` / `green_rate` は副次指標 (ON で T5/T6 の局所改善有無、項目 209 AgentFloor との 2D 解析)。

### 4.9 不能時 graceful skip (デフォルト無効・存在しない設定の扱い)

R1/R2/R6 軽減のため、以下 case で **silent skip** (panic / log error しない):

1. `TaskComplexityHeuristic::evaluate()` が false → `phase = NotApplicable` 維持
2. `prompts/tdd/*.txt` 読込失敗 (file 不在 / 内容空白) → directive 注入 skip
3. `detect_test_outcome()` が `Inconclusive` → phase 遷移なし
4. `iterations_in_phase > max_failing_iterations` → `phase = NotApplicable` に reset、log warn のみ

すべての case で session に余計なメッセージは追加しない = SOUL.md + DEFAULT_SYSTEM_PROMPT のみで legacy 動作と同一。本 plan で言う graceful skip は **既存 inject_* 関数群 (memory_blocks / heuristics / contextual_memories) の `Option<()>` 返却で skip する pattern と完全同型** (`context_inject.rs` 既存先例、項目 80/213)、ad-hoc fallback ではない。

### 4.10 audit log 拡張

```rust
// observability/audit.rs (既存 enum に variant 追加)
pub enum AuditAction {
    // ... 既存 (LlmCall / ToolCall / SecurityEvent / StepOutcome / AdvisorCall / TaskComplete / MultiFileNudge / AdvisorSkip)
    TddEvent {
        phase_from: String,
        phase_to: String,
        trigger: String,           // "task_complexity" | "test_runner_match" | "file_write_test" | "file_write_impl" | "premature_implementation" | "max_iter_exceeded"
        signal_count: usize,
        violation: Option<String>, // "implementation_without_test" | "too_many_failing_iter" | "premature_refactoring" | None
    },
}
```

`as_str()` match arm に `Self::TddEvent { .. } => "tdd_event"` を `audit.rs:107` 付近に追加 (既存 `AdvisorSkip` の隣)。SQLite テーブルは既存 schema で受け切れる (action_type TEXT + payload JSON)、**migration 不要**。

## 5. TDD strict 5 phase

### Phase 1 — Red (test ≥ 8 件、本 plan は 8-12 件で着地)

新規 test ファイル: `src/agent/tdd_enforcement.rs` 末尾 `#[cfg(test)] mod tests` または `tests_tdd_enforcement.rs` 新規 (実装時判断)。

| # | test 名 | 期待 (Red 時) |
|---|---|---|
| 1 | `t_tdd_config_default_disabled` | `TddEnforcementConfig::default().enabled == false`、`phase == NotApplicable`、`multi_step_threshold == 2` |
| 2 | `t_tdd_config_env_enabled_parse` | `BONSAI_TDD_ENFORCEMENT_ENABLED=1` で `enabled == true`、各 env 値の override 確認 |
| 3 | `t_tdd_short_circuits_when_disabled` | `enabled=false` 時 `inject_tdd_directive` が `None` 即 return、session 不変 |
| 4 | `t_task_complexity_heuristic_signals` | (a) subagent_active=true 単独で signal=1 (b) file_write 2 連続で signal=1 (c) plan 3 step 言及で signal=1 (d) detect_task_complexity=true で signal=1、threshold=2 で `evaluate=true` 確認 |
| 5 | `t_tdd_phase_state_machine_allowed_transitions` | 全 phase の `allowed_next()` 集合が § 4.2 図と一致 (8 phase × 平均 2 transition = 16 assertion) |
| 6 | `t_tdd_phase_disallowed_transition_panics_or_skips` | `NotApplicable → TestPassing` (skip 必須遷移) で `transition_on_*` が phase 不変 + warn log |
| 7 | `t_detect_test_outcome_cargo_test_passed` | `tool_call.name=="shell"` + `args.command="cargo test --lib"` + result に "test result: ok" で `Some(Passed)` |
| 8 | `t_detect_test_outcome_cargo_test_failed` | 同 + result に "FAILED" で `Some(Failed)` |
| 9 | `t_detect_test_outcome_non_test_command_returns_none` | `command="ls"` で `None` |
| 10 | `t_check_test_first_compliance_violation` | `phase=TestRequested` + answer 内 implementation indicator で `Some(ImplementationWithoutTest)` |
| 11 | `t_inject_tdd_directive_appends_when_enabled` | env=1 + multi-step signal>=2 で `<context type="tdd_directive:test_first">` メッセージ追加 + `Some(TddPhase::TestRequested)` 返却 |
| 12 | `t_audit_emits_tdd_event_on_transition` | phase 遷移時に `AuditLog` に `TddEvent` action 1 件追加、`phase_from/phase_to` 文字列確認 |

期待: `cargo test --lib tdd_enforcement` で **全 8-12 件 fail / compile error** で Red 確証。

env mutation race avoidance: module-local `static TDD_TEST_LOCK: Mutex<()>` で serialize (項目 218 `REVIEW_INJECT_TEST_LOCK` pattern 踏襲、`context_inject.rs:639` 同形式)。

commit `test(tdd): Phase 1 Red — TddEnforcementConfig + state machine + 8-12 test`

### Phase 2 — Green

実装ファイル:
1. **`src/agent/tdd_enforcement.rs`** 新規 (~280 行) — `TddEnforcementConfig` / `TddPhase` / `TddCriticCoordination` / `TddPrematureAction` / `TaskComplexityHeuristic::evaluate` / `detect_test_outcome` / `check_test_first_compliance` / `inject_tdd_directive` / `transition_on_*` private helpers / `from_env`
2. **`src/agent/mod.rs`** に `pub mod tdd_enforcement;` 追加 (1 行、項目 219 `working_memory` の隣)
3. **`prompts/tdd/`** 新規 dir + 4 file (各 ≤ 25 行、Bonsai-8B 1bit 向け簡潔表現):
   - `prompts/tdd/test_first.txt` — TestRequested 注入用、「期待挙動の test を先に書いてください、cargo test で fail を確認してから実装に進んでください」
   - `prompts/tdd/test_failing.txt` — TestFailing 確証後の実装段、「Red 確認済。最小実装で test を pass させてください」
   - `prompts/tdd/test_passing.txt` — Green 達成後 refactor 推奨、「Green 達成。test を維持しつつ refactor を検討してください」
   - `prompts/tdd/violation_warning.txt` — premature_implementation 警告、「テストを書かずに実装が進んでいます。先に test を追加してください (TDD violation)」
4. **`src/agent/agent_loop/core.rs:136-142`** の inject 順序に `inject_tdd_directive` call 追加 (§ 4.4 設計通り、`inject_heuristics` 直前)
5. **`src/agent/agent_loop/config.rs::AgentConfig`** に `pub tdd_enforcement: TddEnforcementConfig` field 追加 (Default::default で disabled、項目 179 `memory_blocks` の隣)
6. **`src/config.rs::AgentSettings`** に `pub tdd_enforcement: TddEnforcementConfig` field 追加 + `#[serde(default)]` で TOML 後方互換
7. **`src/observability/audit.rs`** に `AuditAction::TddEvent { ... }` variant 追加 + `as_str()` match arm 追加
8. **`src/agent/benchmark.rs`** に `TddStats` 構造体 + `MultiRunBenchmarkResult::tdd_stats: Option<TddStats>` field 追加 (`#[serde(default, skip_serializing_if = "Option::is_none")]`)
9. **post-tool-call hook** (`agent_loop/tool_exec.rs` 後 or `core.rs` の loop 内): `detect_test_outcome` を呼び `transition_on_test_outcome` 経由で `config.tdd_enforcement.phase` 更新 (subagent_active と recent_tool_calls は loop scope 変数)

期待:
- `cargo test --lib` で **既存 1150 passed + 新規 8-12 test = 1158-1162 passed**
- `cargo clippy --lib --tests -- -D warnings` clean
- `cargo fmt --check` clean
- env unset (default) で既存全 test 退行ゼロ (`BONSAI_TDD_ENFORCEMENT_ENABLED` 未設定 = `enabled=false` short-circuit)

commit `feat(tdd): Phase 2 Green — TddEnforcementConfig + state machine + inject + audit + prompts`

### Phase 3 — Refactor

- `prompts/tdd/*.txt` を `include_str!` で binary 埋込 (項目 213 heuristic_reflection.txt と同 pattern、Lab 結果再現性確保 R10 mitigation)
- `detect_test_outcome` の正規表現キャッシュ (`once_cell::sync::Lazy<Regex>` × 4 = passed/failed pattern)
- `TaskComplexityHeuristic::evaluate` 内の signal 集計を early-return ladder に整理 (collapsible_if 警告対策)
- docstring 整備 — 各 pub 構造体に「由来 plan: agent-side-tdd-enforcement-impl.md / G2 / 項目候補 224」明記
- env パース失敗時の挙動を `default + log warn` に統一 (項目 214 toggle と pattern 統一)
- test mutex (`TDD_TEST_LOCK`) を module 直下に追加 (項目 218 pattern 踏襲)
- Phase 3 self-review log を本 plan § 13 に追記 (4 directive × 16 ルール cross-check matrix)

commit `refactor(tdd): Phase 3 — regex cache + include_str + docstring + test mutex + cross-check matrix`

### Phase 4 — Smoke (3 段)

#### G-4a: 既存経路後方互換 (env unset)
```bash
cargo build --release
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0
```
期待: TDD event 0 回、score / pass@k / duration が項目 207 baseline (0.7812) ± variance、TSV `tdd_detected=0`、`tdd_directive` tag 不在。

#### G-4b: TDD ON / standalone / log_only (shadow mode)
```bash
BONSAI_TDD_ENFORCEMENT_ENABLED=1 \
  BONSAI_TDD_CRITIC_COORD=standalone \
  BONSAI_TDD_PREMATURE_ACTION=log_only \
  BONSAI_LAB_SMOKE=1 \
  ./target/release/bonsai --lab --lab-experiments 0
```
期待: smoke 7 task のうち multi-step task (smoke_failure_chain_pair / smoke_partial_success_chain) で TDD detected ≥ 1、`AuditAction::TddEvent` log emit、production 動作影響ゼロ (PrematureAction=LogOnly)、score ± variance、`tdd_directive:test_first` tag が session に挿入確認 (debug log)。

#### G-4c: TDD ON / cooperative G1 (G1 plan merge 前提) / inject (production-like)
```bash
# G1 plan merge 後のみ実行可、未 merge 時は G-4c を skip し note を残す
BONSAI_CRITIC_ENABLED=1 BONSAI_CRITIC_MODE=different_prompt \
  BONSAI_TDD_ENFORCEMENT_ENABLED=1 \
  BONSAI_TDD_CRITIC_COORD=cooperative \
  BONSAI_TDD_PREMATURE_ACTION=inject_warning \
  BONSAI_LAB_SMOKE=1 \
  ./target/release/bonsai --lab --lab-experiments 0
```
期待: G1 critic disagreement に TDD violation が injection、`tdd_compliance_rate >= 0.3` (1bit lenient gate、Phase 5 Lab で適正値測定)、duration +30% 以下 (G1 plan R2 と同 gate)。

判定:
- ✅ G-4a: 既存挙動完全互換 (1158-1162 passed 維持)、TSV 列追加で外部解析 robust 確認 (列 22 で header 駆動 reader OK)
- ✅ G-4b: TDD wiring 確認、production 影響ゼロ、`tdd_directive:test_first` tag 挿入
- ✅ G-4c (G1 merge 後): G1+G2 協調 wiring 確認、score lenient gate (Δ ≥ -0.05、1bit variance 範囲)

### Phase 5 — Lab effectiveness (別 plan、本 plan delivery 範囲外)

- 別 plan ファイル `lab-v19-tdd-effectiveness.md` を起票 (G1 plan の `lab-v18-critic-effectiveness.md` と同 pattern)
- core 22 / k=3 / paired t-test (env=enabled vs env=unset) で `composite_score` Δ を計測
- AgentFloor 30 task / 6-tier (項目 209 merge 前提) で tier 別 paired t-test (T5/T6 の局所改善仮説検証)
- ACCEPT 判定: (a) Δ mean ≥ +0.015 AND (b) one-sided p < 0.1 (項目 215 Lab v17 と同基準)
- 副次評価: `compliance_rate` / `green_rate` / 項目 200 RDC/VAF/GDS reliability_decay tier 別 variance 観測
- ACCEPT → defaults 昇格検討 (env default ON、本 plan 7 章 R3 mitigation 参照)
- REJECT → dead-code 候補化 (項目 215 ERL pattern、項目 222 sqlite-vec wiring 削除 pattern 踏襲、§ 14 で詳細)

## 6. API 影響

### 6.1 公開 API (新規)

| modulo path | 種別 | 備考 |
|---|---|---|
| `agent::tdd_enforcement` | new pub mod | 280 行 |
| `agent::tdd_enforcement::TddEnforcementConfig` | pub struct | 10 field、Default impl 完備 |
| `agent::tdd_enforcement::TddPhase` | pub enum | 8 variant + `label()` + `allowed_next()` |
| `agent::tdd_enforcement::TddCriticCoordination` | pub enum | 3 variant (Standalone / CooperativeWithG1 / LogOnly) |
| `agent::tdd_enforcement::TddPrematureAction` | pub enum | 3 variant (InjectWarning / LogOnly / BlockAndReplan、後者 Phase 2) |
| `agent::tdd_enforcement::TaskComplexityHeuristic::evaluate` | pub fn | (Session, Config, subagent_active, recent_tool_calls) → bool |
| `agent::tdd_enforcement::detect_test_outcome` | pub fn | (ToolCall, tool_result, Config) → Option<TestOutcome> |
| `agent::tdd_enforcement::TestOutcome` | pub enum | 3 variant (Passed / Failed / Inconclusive) |
| `agent::tdd_enforcement::check_test_first_compliance` | pub fn | (Session, Config, answer) → Option<TddViolation>、G1 plan が import |
| `agent::tdd_enforcement::TddViolation` | pub enum | 3 variant |
| `agent::tdd_enforcement::inject_tdd_directive` | pub(crate) fn | (Session, &str, &mut Config, subagent_active, &[ToolCall]) → Option<TddPhase> |
| `agent::tdd_enforcement::TddEnforcementConfig::from_env` | pub fn | env から構築 |
| `agent::benchmark::TddStats` | pub struct | informational metric |
| `config::AgentSettings::tdd_enforcement` | pub field (default disabled) | TOML 後方互換 |
| `agent::agent_loop::AgentConfig::tdd_enforcement` | pub field (default disabled) | Default 経由で後方互換 |
| `observability::audit::AuditAction::TddEvent` | enum variant | 既存 enum に additive |

### 6.2 公開 API 破壊的変更

**ゼロ**:
- `AdvisorConfig` 既存フィールド unchanged
- `inject_verification_step` / `inject_planning_step` / `inject_heuristics` / `inject_memory_blocks` signature unchanged
- `BenchmarkSuite::run_k` signature unchanged (`MultiRunBenchmarkResult` への `Option<TddStats>` 追加は serde default + skip_if_none で additive)
- `MultiRunBenchmarkResult` 既存フィールド unchanged
- `ShellTool::call` / `FileWriteTool::call` / `MultiEditTool::call` 改変なし (read-only pattern match のみ、`tools/shell.rs` は untouched)
- `SubAgentExecutor::execute` signature unchanged (subagent_active は agent_loop scope 変数で伝播)

env unset で既存挙動 100% 維持 = 1150 passed 退行ゼロ。

## 7. Risks / Mitigations

| # | Risk | severity | Mitigation |
|---|---|---|---|
| **R1** | TaskComplexityHeuristic false-positive で **単 file 修正にも TDD 強制発動** → 過剰計画で逆効果 | **HIGH** | (i) signal_count >= multi_step_threshold (default 2) で AND 性集約 (4.3) (ii) `evaluate()` が false 返却で完全 skip (4.9) (iii) Phase 5 Lab で `multi_step_detected_count` を ground truth (BenchmarkTask.capability_tier) と照合、誤判定率 ≥ 30% で classifier 改修 plan 起票 |
| **R2** | 1bit Bonsai-8B が **test 自体を hallucinate** (テスト不在の関数呼出 / 誤 assertion) で偽 Green | **HIGH** | (i) `prompts/tdd/test_first.txt` で「実装前にテスト fail を確認」を厳格指定 (Red 必須) (ii) `detect_test_outcome` で TestPassing 即達成は warn log (要確認 signal) (iii) Phase 4 G-4b で TestFailing → Implementing → TestPassing の遷移ありの cycle 比率 ≥ 50% gate (iv) Phase 5 Lab で `green_rate` 単独 ACCEPT は不採用、`composite_score` Δ ≥ +0.015 必須 |
| **R3** | **lang 別 test runner の多様性** で pattern match 漏れ (rust の `cargo nextest` / python の `unittest` / ts の `jest`) | MEDIUM | (i) Phase 1 default = 4 pattern (cargo test / pytest / npm test / go test)、project 多数派カバー (ii) `BONSAI_TDD_TEST_RUNNERS` env or `prompts/tdd/test_runners.toml` で extensible (Phase 2 候補、本 plan は default 4 のみ) (iii) Inconclusive 判定で phase 遷移なし = pattern 漏れで silent skip (graceful) |
| **R4** | **TddPhase state 永続化リスク** — session 跨ぎ phase 残存で誤動作 | MEDIUM | (i) `TddEnforcementConfig.phase` は in-memory only、SQLite 永続化なし (ii) session.reset / 新 cycle 開始で phase = NotApplicable に reset (Phase 4 で benchmark.rs 集計 hook 内に reset 配線) (iii) `iterations_in_phase > max_failing_iterations` で auto reset (4.9) |
| **R5** | **G1 critic との重複** — 同じ implementation_without_test 検出を G1 と G2 が重ねる | LOW | (i) `TddCriticCoordination::CooperativeWithG1` 時のみ G1 + G2 連携 (ii) Standalone (Phase 1 default) 時は G2 単独動作、G1 critic は G2 violation を読まない (iii) Phase 5 Lab plan で Reflexion / G1 / G2 の 3 因子 factorial 推奨 (G1 plan R7 と同 finding) |
| **R6** | **prompt 追加で system prompt size 増加** → ContextOverflowGuard (項目 187) 発動頻度上昇 | MEDIUM | (i) 4 directive 各 ≤ 25 行 (~500 token 以下、合計 2000 token 以下) (ii) phase に応じて 1 directive のみ active (= 全部同時注入しない) (iii) Phase 4 G-4b で context_overflow_count を audit log で確認、baseline 比 +20% 以下 gate (iv) F2 ContextOverflowGuard で吸収 (項目 187 / handoff 05-08i) |
| **R7** | **env mutation race** で Phase 1 test の決定論性損失 | MEDIUM | test mutex `TDD_TEST_LOCK` 導入 (項目 218 / G1 plan R9 と同 pattern)、Phase 3 Refactor で対応 |
| **R8** | **`TddPrematureAction::BlockAndReplan` (Phase 2) で agent 進行不能** | LOW | Phase 1 では Phase 2 variant `BlockAndReplan` は `unimplemented!()` panic、`from_env` で `block_replan` 設定時は warn log + default `inject_warning` 置換 (G1 plan R11 と同 mitigation) |
| **R9** | **`prompts/tdd/*.txt` 改変で Lab 結果が再現不可能** | LOW | `include_str!` で binary 内に埋込、git 履歴で改変追跡可。Phase 5 Lab plan で「prompts/tdd/ git hash」を Experiment metadata に記録 (G1 plan R10 と同) |
| **R10** | **`AuditAction::TddEvent` payload JSON 肥大化** で audit テーブル inflation | LOW | phase_from / phase_to / trigger / signal_count / violation のみ記録、本文は session message に含まれるため重複保存しない (G1 plan R8 と同) |
| **R11** | **Phase 5 Lab で Reflexion vs Critic vs TDD のどれが効いたか分離不能** | MEDIUM | Phase 5 Lab plan で 8 cell (Reflexion ON/OFF × G1 Critic ON/OFF × G2 TDD ON/OFF) factorial 推奨、本 plan delivery 範囲外 (Lab plan 責務) |
| **R12** | **HSL relabel (項目 201-205) との相性検証で test 命名規則の文化差** (rust `_test.rs` / py `test_*` / ts `*.test.ts`) で TestPending 誤検出 | MEDIUM | (i) Phase 1 default 命名規則は 3 つカバー (ii) Phase 5 Lab で TestPending 比率 < 10% なら命名規則拡張 plan 起票 |

## 8. Quality Gates

### G-1 Phase 1 Red
- 8-12 新規 test 全 fail or compile error (`cargo test --lib tdd_enforcement`)
- 既存 1150 passed 維持

### G-2 Phase 2 Green
- 既存 1150 + 新規 8-12 test = **1158-1162 passed**
- `cargo clippy --lib --tests -- -D warnings` clean (新規 warning ゼロ)
- `cargo fmt --check` clean
- env unset (default) で既存全 test 退行ゼロ

### G-3 Phase 3 Refactor
- `include_str!` で `prompts/tdd/*.txt` 4 file binary 埋込確認
- `once_cell::sync::Lazy` regex cache 導入確認
- 全 pub 構造体 docstring (G2 由来 + 項目候補 224 cross-ref)
- test mutex (`TDD_TEST_LOCK`) 導入 (env race 回避)
- 4 directive × 16 ルール cross-check matrix を本 plan § 13 に追記
- 1158-1162 passed 維持

### G-4 Phase 4 Smoke (3 段)

| Gate | 検証内容 | 期待 |
|---|---|---|
| **G-4a (env unset)** | 既存挙動完全互換 | `tdd_detected=0`、score / duration が項目 207 baseline ± variance、TSV 22 列 reader robust |
| **G-4b (standalone + log_only)** | TDD wiring 確認 | smoke 7 task で `tdd_detected ≥ 1`、AuditAction::TddEvent emit、production 動作影響ゼロ、`tdd_directive:test_first` tag 挿入、context_overflow_count baseline ± 20% (R6 gate)、Inconclusive 比率 ≤ 50% (R3 gate) |
| **G-4c (cooperative + inject、G1 merge 後のみ)** | G1+G2 協調 wiring 確認 | G1 critic disagreement に TDD violation 注入、`compliance_rate ≥ 0.3` (1bit lenient)、score Δ ≥ -0.05 (lenient)、duration +30% 以下 (R6 gate) |

G-4c は G1 plan が merge されていない場合は **skip 可** (本 plan は G1 と独立着手可、4.6 設計)。skip 時は handoff に「G1 merge 後に G-4c 単独 smoke 実行」を TODO 化。

### G-5 G1 協調動作 smoke (G1 merge 後のみ)
- G1 critic plan merge 後、G-4c を再実行
- G1 + G2 連携で `tdd_violation` が `critic_disagreement` に injection されること audit log で確認
- ON/OFF 切替で session.messages 差分が期待通り (G2 violation message が cooperative 時のみ追加)

### G-6 Lab effectiveness (別 plan、本 plan delivery 範囲外)
- core 22 / k=3 / paired t-test
- AgentFloor 30 task / 6-tier (項目 209 merge 前提) で tier 別 paired t-test
- ACCEPT 基準: (a) Δ mean ≥ +0.015 AND (b) one-sided p < 0.1
- 副次 finding (項目 215 同パターン): T5/T6 局所改善有無、`compliance_rate` / `green_rate` で stability 軸 ACCEPT 検討

G-1 〜 G-4 (G-4c は G1 merge 後) PASS で本 plan delivery 完了。G-5 / G-6 は別 plan の責務。

## 9. 完了条件

1. ✅ `src/agent/tdd_enforcement.rs` 新規モジュール ~280 行 + `TddEnforcementConfig` / `TddPhase` 8 variant / `TddCriticCoordination` / `TddPrematureAction` / `TaskComplexityHeuristic::evaluate` / `detect_test_outcome` / `check_test_first_compliance` / `inject_tdd_directive` / `from_env`
2. ✅ `prompts/tdd/test_first.txt` / `test_failing.txt` / `test_passing.txt` / `violation_warning.txt` 4 file 新規 (各 ≤ 25 行、Bonsai-8B 1bit 向け簡潔表現)
3. ✅ `src/agent/mod.rs` に `pub mod tdd_enforcement;` 追加
4. ✅ `src/agent/agent_loop/core.rs:136-142` の inject 順序に `inject_tdd_directive` を `inject_memory_blocks` 直後・`inject_heuristics` 直前に挿入
5. ✅ `src/config.rs::AgentSettings.tdd_enforcement` field 追加 + `#[serde(default)]` で TOML 後方互換
6. ✅ `src/agent/agent_loop/config.rs::AgentConfig.tdd_enforcement` field 追加 + Default::default で empty
7. ✅ `src/observability/audit.rs::AuditAction::TddEvent` variant 追加 + `as_str()` match arm
8. ✅ `src/agent/benchmark.rs::TddStats` 構造体 + `MultiRunBenchmarkResult::tdd_stats: Option<TddStats>` field 追加 (additive)
9. ✅ post-tool-call hook (`tool_exec.rs` 後) で `detect_test_outcome` 呼出 + `phase` 更新
10. ✅ `BONSAI_TDD_ENFORCEMENT_ENABLED` env opt-in、default OFF (項目 214 / 217-219 と pattern 統一)
11. ✅ TDD strict 5 phase 全消化 (Phase 1 Red → Phase 2 Green → Phase 3 Refactor → Phase 4 Smoke 3 段 → Phase 5 別 plan 起票確証)
12. ✅ 既存 1150 passed 維持、新規 8-12 test 追加で 1158-1162 passed、clippy 0 / fmt clean、API 完全 additive、smoke G-4a/b 全 PASS、G-4c は G1 merge 後 PASS、CLAUDE.md 項目 224 候補追記 + handoff 起票 + INDEX.md G2 リンク

## 10. 見積もり

| Phase | 内容 | 時間 |
|---|---|---|
| **P0 (調査)** | judge.rs / advisor_inject.rs / outcome.rs / subagent.rs / tools/shell.rs 既読、env opt-in pattern 確認、G1 plan 既起票確認 | 0.4h |
| **P1 (Red)** | test 8-12 件追加、cargo test 全 fail 確認、env mutex 設計 | 1.2h |
| **P2 (Green)** | TddEnforcementConfig + 4 enum + state machine + TaskComplexityHeuristic + detect_test_outcome + check_test_first_compliance + inject_tdd_directive + AgentConfig/AgentSettings field + AuditAction + TddStats + 4 prompts file + core.rs hook + post-tool-call hook | 5.5h |
| **P3 (Refactor)** | regex cache + include_str + docstring + env mutex + 4 directive × 16 ルール cross-check matrix | 1.3h |
| **P4 (Smoke 3 段)** | G-4a env unset (5 min) + G-4b log_only smoke (15 min) + G-4c (G1 merge 後、20 min) + 解析 + 修正 buffer | 3.5h (実機 wall ~40 min、G-4c は G1 merge 後 +20 min) |
| **P6 (commit + handoff + CLAUDE.md)** | 5 commits + handoff 起票 + CLAUDE.md 項目 224 + INDEX.md G2 リンク | 0.8h |
| **計 (G1 dependency なし)** | | **~12.7h ≈ 1.5-2 day** |

G1 plan 既 merge 想定で +0.3h (G-4c 実行時間)、未 merge で G-4c skip = -0.5h。

Phase 5 (Lab v19 TDD effectiveness paired t-test) は別 plan ~6h、本 plan delivery 範囲外。

派生 plan 候補 (本 plan ACCEPT 後):
- `tdd-block-and-replan-phase2-impl.md` (`BlockAndReplan` 実装、tool_call 一時 block + replan 強制、~1 day)
- `lab-v19-tdd-effectiveness.md` (paired t-test、AgentFloor T5/T6 tier 別、~6h)
- `tdd-test-runners-toml-impl.md` (extensible test runner config、~0.5 day)

## 11. Quick Start

```bash
# 0. 既存実装確認 (production code 変更ゼロ確証)
rtk grep -n "TddEnforcementConfig\|inject_tdd_directive\|TddPhase\|TaskComplexityHeuristic" src/  # 期待 0 件
rtk grep -n "BONSAI_TDD" src/                                                                     # 期待 0 件
ls /Users/keizo/bonsai-agent/prompts/                                                              # heuristic_reflection.txt のみ
rtk grep -n "tdd_enforcement\|tdd_event" src/                                                     # 期待 0 件
rtk grep -n "CriticConfig\|critic-separate-llm" .claude/plan/                                     # G1 plan 起票確認

# 1. Phase 1 Red — test 追加
$EDITOR src/agent/tdd_enforcement.rs           # 新規 module、todo!() 含む 8-12 test
$EDITOR src/agent/mod.rs                       # pub mod tdd_enforcement
rtk cargo test --lib tdd_enforcement           # 全 8-12 件 fail / compile error 確認
git commit -m "test(tdd): Phase 1 Red — TddEnforcementConfig + state machine + 8-12 test"

# 2. Phase 2 Green — 実装
$EDITOR src/agent/tdd_enforcement.rs           # struct + enum + heuristic + detect + inject + from_env
mkdir -p prompts/tdd
$EDITOR prompts/tdd/test_first.txt
$EDITOR prompts/tdd/test_failing.txt
$EDITOR prompts/tdd/test_passing.txt
$EDITOR prompts/tdd/violation_warning.txt
$EDITOR src/config.rs                          # AgentSettings.tdd_enforcement field
$EDITOR src/agent/agent_loop/config.rs         # AgentConfig.tdd_enforcement field
$EDITOR src/agent/agent_loop/core.rs           # inject_tdd_directive 1 行追加 (memory_blocks 直後・heuristics 直前)
$EDITOR src/agent/agent_loop/tool_exec.rs      # post-tool-call hook で detect_test_outcome
$EDITOR src/observability/audit.rs             # TddEvent variant
$EDITOR src/agent/benchmark.rs                 # TddStats + run_k 集計 hook
rtk cargo test --lib                           # 1158-1162 passed
rtk cargo clippy --lib --tests -- -D warnings
rtk cargo fmt --check
git commit -m "feat(tdd): Phase 2 Green — TddEnforcementConfig + state machine + inject + audit + prompts"

# 3. Phase 3 Refactor
$EDITOR src/agent/tdd_enforcement.rs           # regex cache + include_str + docstring + env mutex
$EDITOR .claude/plan/agent-side-tdd-enforcement-impl.md  # § 13 review log + 4 directive × 16 ルール cross-check matrix
git commit -m "refactor(tdd): Phase 3 — regex cache + include_str + docstring + env mutex + cross-check matrix"

# 4. Phase 4 Smoke 3 段
rtk cargo build --release

# G-4a: 既存経路後方互換
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 2>&1 | tee /tmp/tdd_g4a.log
grep "tdd_detected\|tdd_event" /tmp/tdd_g4a.log  # 期待 0

# G-4b: log_only shadow mode
BONSAI_TDD_ENFORCEMENT_ENABLED=1 \
  BONSAI_TDD_CRITIC_COORD=standalone \
  BONSAI_TDD_PREMATURE_ACTION=log_only \
  BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 2>&1 | tee /tmp/tdd_g4b.log
grep "tdd_event\|TddEvent\|tdd_directive" /tmp/tdd_g4b.log  # 期待 ≥ 1

# G-4c: cooperative G1 (G1 plan merge 後のみ実行)
# BONSAI_CRITIC_ENABLED=1 BONSAI_CRITIC_MODE=different_prompt \
#   BONSAI_TDD_ENFORCEMENT_ENABLED=1 BONSAI_TDD_CRITIC_COORD=cooperative \
#   BONSAI_TDD_PREMATURE_ACTION=inject_warning \
#   BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 2>&1 | tee /tmp/tdd_g4c.log

# 5. commit + handoff + CLAUDE.md
$EDITOR CLAUDE.md       # 項目 224 候補追記
$EDITOR .claude/projects/-Users-keizo-bonsai-agent/memory/session_2026_05_XX_handoff.md
$EDITOR .claude/plan/INDEX.md  # G2 リンク追加
git commit -m "docs(tdd): G-1〜G-4 PASS + 項目 224 候補 + handoff"

# 6. Phase 5 別 plan 起票 (本 plan delivery 範囲外)
$EDITOR .claude/plan/lab-v19-tdd-effectiveness.md
```

## 12. 参考

### 由来 meta-plan
- **`.claude/plan/building-ai-coding-agents-gap-analysis.md`** § 4 G2 (★★ 中優先、P6 唯一の ❌、~2 day 推定の起票元)

### 関連 G plan (G1 dependency / G4 alignment)
- **`.claude/plan/critic-separate-llm-impl.md`** — G1 critic 別 LLM 分離 plan (★★★)、本 plan の `TddCriticCoordination::CooperativeWithG1` 連携先、`CriticDisagreementAction::Inject` 経路で TDD violation injection
- **`.claude/plan/task-aware-system-prompt-impl.md`** — G4 task-aware prompt plan (★★)、本 plan の TaskComplexityHeuristic と `TaskComplexityClassifier` の coarse 軸 vs fine 軸の役割分担参考

### bonsai 既存 plan (品質基準・TDD strict 5 phase 手本)
- **`.claude/plan/agentfloor-tier-eval-impl.md`** — TDD strict 5 phase + Quality Gates G-1〜G-5 構造手本、CapabilityTier 6-tier (項目 209) は本 plan Phase 5 Lab で T5/T6 局所改善仮説検証の base
- **`.claude/plan/cerememory-decay-port-impl.md`** — env opt-in default OFF + license attribution + SCHEMA migration pattern
- **`.claude/plan/cerememory-review-state-v12-impl.md`** — env opt-in `BONSAI_REVIEW_ENABLED` + `REVIEW_INJECT_TEST_LOCK` test mutex pattern (本 plan R7 mitigation 手本)
- **`.claude/plan/erl-heuristics-pool-impl-v2.md`** — `prompts/heuristic_reflection.txt` 同居先例 (項目 213)、本 plan は `prompts/tdd/*.txt` 4 file を同 dir 配置
- **`.claude/plan/lab-v17-erl-effectiveness.md`** — Phase 5 Lab paired t-test 別 plan の構造手本 (項目 214-215)、本 plan の `lab-v19-tdd-effectiveness.md` 起票時の手本
- **`.claude/plan/event-repository-trait-impl.md`** — Mock + parity test pattern (項目 209、Phase 1 test の参考、Mock 経由 SQLite なし unit test 化)
- **`.claude/plan/agenther-option-a-migration.md`** — TDD strict 5 phase + signature 必須化 pattern (本 plan は signature 不変方針なので非該当だが、Phase 1 Red の test 数手本)

### bonsai 既存項目 (本 plan で reference する CLAUDE.md 項目)
- **項目 1**: Reflexion (同一 LLM self-critique、事後検証) — 共存対象、本 plan は事前検証で並列
- **項目 9**: 停滞時 Replan / Continue Sites — 影響なし
- **項目 10**: 計画強制ルール (Lab v6.2 唯一 ACCEPT、デフォルト化) — 本 plan は subset 強化
- **項目 47**: ツール使用前 `<think>` (Lab v6.2 ACCEPT) — TDD prompt で重ね掛け
- **項目 50**: 代替 tool 戦略 (Lab v6.2 ACCEPT) — TestFailing 段階で別 tool 整合
- **項目 80**: contextual memory injection — タグ統一方針 `<context type="...">` 準拠
- **項目 120/160**: SubAgentExecutor — TaskComplexityHeuristic signal 1 つ
- **項目 136**: 回答前ファイル内容確認 (Lab v9 ACCEPT) — TDD prompt で重ね掛け
- **項目 161**: Skill 軌跡昇格 — HSL relabel 相性良好
- **項目 187**: ContextOverflowGuard — R6 mitigation
- **項目 201-205**: AgentHER hindsight relabel — TestPassing/TestFailing が relabel 対象
- **項目 209**: EventRepository trait + Mock + AgentFloor 6-tier (Phase 5 Lab で tier 別 paired t-test)
- **項目 210**: Self-Verify dynamic skip + `detect_task_complexity` (TaskComplexityHeuristic で read-only 流用)
- **項目 211**: Self-Verify Phase 5 `BONSAI_LAB_PHASE5_FOCUS` (Phase 5 Lab variant 機構手本)
- **項目 213**: ERL Heuristics Pool / `prompts/heuristic_reflection.txt`
- **項目 214**: Lab v17 toggle 機構 (env opt-in pattern)
- **項目 215**: Lab v17 REJECT (天井 7 連続、構造変異枯渇 evidence、ACCEPT 基準先例)
- **項目 216**: ERL defaults OFF 切替 (`BONSAI_<feature>_ENABLED` 統一)
- **項目 217-219**: Cerememory 三本柱 (env opt-in default OFF pattern)
- **項目 222**: sqlite-vec wiring 削除 (REJECT 後 dead-code 化 pattern、§ 14 で参照)

### bonsai source files (本 plan で grep / 参照、Phase 2 着手時の対象)
- `src/agent/tdd_enforcement.rs` (新規、~280 行)
- `src/agent/mod.rs` (pub mod 1 行追加)
- `src/agent/agent_loop/core.rs` (loop 本体、136-142 行の inject 順序に hook 配置先)
- `src/agent/agent_loop/config.rs` (`AgentConfig`、本 plan で `tdd_enforcement` field 追加)
- `src/agent/agent_loop/advisor_inject.rs` (Reflexion injection 経路、本 plan は **改変なし** で並列)
- `src/agent/agent_loop/outcome.rs` (`detect_task_complexity` 既存 bool、131-152 行 read-only 流用)
- `src/agent/agent_loop/tool_exec.rs` (post-tool-call hook 追加先)
- `src/agent/conversation.rs` (`ToolCall` struct、76 行、本 plan は read-only 利用)
- `src/agent/subagent.rs` (`SubAgentExecutor`、111 行、subagent_active loop scope 変数で伝播)
- `src/observability/audit.rs` (`AuditAction` enum、本 plan で `TddEvent` variant 追加、`as_str()` match arm 102-108 行)
- `src/agent/benchmark.rs` (`MultiRunBenchmarkResult`、本 plan で `TddStats` 追加)
- `src/config.rs` (`AgentSettings`、321-366 行、本 plan で `tdd_enforcement` field 追加)
- `src/runtime/model_router.rs` (`AdvisorConfig`、本 plan は **改変なし**、G1 plan で `CriticConfig` 追加先)
- `src/tools/shell.rs` (`ShellTool`、11 行、本 plan は **read-only** で pattern match のみ)
- `src/tools/file.rs` (`FileWriteTool` / `MultiEditTool`、61/191 行、本 plan は **read-only**)
- `prompts/heuristic_reflection.txt` (項目 213 同居先例、本 plan は `prompts/tdd/*.txt` 4 file を同 dir 配置)

### 論文・survey
- **arxiv 2603.05344** — Building AI Coding Agents for the Terminal (G2 由来論文、本 plan の主軸、P6 TDD verification の論文知見)
- arxiv 2602.03485 — Self-Verification Dilemma (項目 210 由来、Reflexion 過剰発動の課題、本 plan の動機補強)
- arxiv 2603.21357 — AgentHER ECHO + HSL (項目 201、TestPassing/TestFailing が hindsight relabel 対象との相性検証候補 Phase 5)
- Cursor 公式 docs — "test-first agent mode" 設計参考
- Aider 公式 docs — `--auto-test` flag、`--test-cmd` 自動実行 loop 設計参考

### CODEX_SESSION (for `/ccg:execute` use)
- 新規取得推奨 (本 plan は G2 derivative 起票で項目 213/214/210 の既存 session 不適)
- 既存 session 流用検討時: `019e064a-334c-7692-9735-c5d95231ebf1` (項目 213 ERL plan v2 起票時の session、env opt-in pattern context が近い、G1 plan も同 session 流用候補)

## 13. Phase 3 Review Log (Phase 3 完了後追記)

### 13.1 4 directive × 16 ルール cross-check matrix
> Phase 3 着手後にここに追記する。各セルは「重複」(= 重ね掛け強調 OK) / 「矛盾」(= 文面修正必須) / 「独立」(= no-op) の 3 値。

| 16 ルール ↓ \ directive → | test_first | test_failing | test_passing | violation_warning |
|---|---|---|---|---|
| 1 簡潔 | (TBD) | (TBD) | (TBD) | (TBD) |
| 2 繰り返し回避 | (TBD) | (TBD) | (TBD) | (TBD) |
| ... | ... | ... | ... | ... |

### 13.2 SOUL.md base persona との整合
> Phase 3 着手後にここに追記する。SOUL.md base persona の文面 (3 段検索でのデフォルト) と各 TDD directive の整合性を確認。

### 13.3 G1 critic.txt との整合 (G1 merge 後)
> G1 plan merge 後にここに追記する。G1 critic.txt の文面と本 plan の 4 directive (特に violation_warning.txt) の役割分担、重複ゼロ確証。

## 14. ★ Phase 5 Effectiveness REJECT 時 handling (項目 222 sqlite-vec wiring 削除 pattern)

Lab v19 paired t-test で TDD-on の Δscore < +0.015 or one-sided p ≥ 0.1:

1. **production default `BONSAI_TDD_ENFORCEMENT_ENABLED` 未設定維持** (= legacy 既定、本 plan default と同じ、構造変更不要)
2. **`TddEnforcementConfig` / `inject_tdd_directive` を dead-code 候補化** (項目 215 ERL pattern、項目 222 sqlite-vec wiring 削除 pattern 踏襲)
3. **`prompts/tdd/` 全削除候補** (env unset で無効、保守負担削減)、または将来の variant base として残置 (G4 plan § 14 と同設計)
4. **CLAUDE.md** に negative finding 記録 (副次知見: `compliance_rate` / `green_rate` / T5/T6 tier 別 std 縮小有無を項目 215 pattern で報告)
5. **dead-code 削除 plan** は別 session で起票 (項目 222 pattern: sqlite-vec wiring 削除 plan `.claude/plan/sqlite-vec-wiring-removal-impl.md` と同経路、~280 行 net delete 想定)
6. 後続 plan 検討:
   - 派生 `tdd-block-and-replan-phase2-impl.md` で **強制力高い `BlockAndReplan` 経路** で再測定 → ACCEPT 可能性が残る
   - 副次 finding (T5/T6 局所改善あり) があれば項目 200 RDC/VAF re-eval 候補
   - 1bit モデル directive 効果限界が確証された場合 = 第 5 軸 (workflow-structural variation) の追加変異候補は MCP-Bench / vllm-mlx 等 (research_arxiv_2026_05_07.md 領域 6 参照)
