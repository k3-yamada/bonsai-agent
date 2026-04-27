# `agent_loop/` モジュール構成

`structural-improvements-v2.md` Step 7（commit `72d969d`〜`3019cb6` の 9 commits）で
`agent_loop.rs` 2661 行モノリスを 9 ファイルに分割した結果のモジュール責務マップ。

## ファイル一覧

| ファイル | 行数 | 責務 | 主要型・関数 |
|---|---:|---|---|
| `mod.rs` | 28 | ファサード（純粋な `pub use` のみ） | — |
| `config.rs` | 108 | エージェント設定 + 推論パラメータ派生 | `AgentConfig`, `inference_for_task`, `DEFAULT_SYSTEM_PROMPT` |
| `state.rs` | 206 | 型定義集約（ループ状態の DTO 群） | `AgentLoopResult`, `StepOutcome`, `LoopState<'a>`, `TokenBudgetTracker`, `OutcomeAction`, `StallDetector`, `StepContext<'a>` |
| `support.rs` | 126 | 補助関数（pub(super)、stateless） | `compute_output_hash`, `check_invariants`, `record_success`, `record_abort`, `build_answer`, `clean_response` |
| `advisor_inject.rs` | 240 | Advisor 連携 + ステップ注入 | `AdvisorResolution`, `resolve_advisor_prompt`, `log_advisor_call`, `inject_replan_on_stall`, `inject_verification_step`, `inject_planning_step` |
| `outcome.rs` | 146 | Outcome ディスパッチ | `handle_outcome`, `detect_task_complexity` |
| `step.rs` | 181 | 1 ステップ実行 | `execute_step` |
| `core.rs` | 315 | メインループ + EventStore emit + 自動 CP | `run_agent_loop`, `run_agent_loop_with_session`, `create_task_start_checkpoint`, `emit_event` |
| `tests.rs` | 1439 | 全モジュールの統合テスト（68 件） | — |

## 公開 API（`mod.rs` から `pub use` 経由）

```rust
pub use config::{AgentConfig, inference_for_task};
pub use core::{run_agent_loop, run_agent_loop_with_session};
pub use state::{
    AgentLoopResult, LoopState, OutcomeAction, StallDetector,
    StepContext, StepOutcome, TokenBudgetTracker,
};
pub use step::execute_step;

pub(crate) use core::emit_event;  // tool_exec.rs から呼出
```

外部呼出は 100% 後方互換。`crate::agent::agent_loop::run_agent_loop` 等のパスは
分割前と同じ。

## モジュール間依存（topological order）

```
config         → (no internal deps)
state          → config (AgentConfig 参照)
support        → state (LoopState/Session 操作)
advisor_inject → state, outcome (detect_task_complexity)
outcome        → state, advisor_inject, support
step           → state, support, config (inference_for_task)
core           → state, step, outcome, advisor_inject, support, config
tests          → super::* + 各 sub module
```

循環なし（Rust 標準の visibility ルールで担保）。

## 設計指針

1. **ファサード化**: `mod.rs` はモジュール宣言と `pub use` のみで責務ゼロ。
   呼出元の `use crate::agent::agent_loop::X` パスは分割前と完全互換。

2. **`pub(super)` の採用**: サブモジュール間の API は `pub(super)` で
   `agent_loop` モジュール内のみに閉じる。クレート全体への公開は
   `mod.rs` の `pub use` でのみ意図的に行う。

3. **型定義集約 (state.rs)**: `LoopState<'a>` などライフタイム付き型を
   一箇所にまとめ、各モジュールから `use super::state::LoopState` で参照。

4. **テスト分離 (tests.rs)**: 1422 行の `#[cfg(test)] mod tests { ... }` を
   別ファイル化することで `mod.rs` を 28 行のファサードに保つ。

## 編集時の注意

- **巻き戻し禁止**: `agent_loop/{mod,core,state,step,outcome,support,advisor_inject,config,tests}.rs`
  の clippy 起因リバートは禁止（CLAUDE.md「巻き戻し禁止」リスト対象）。
- **`mod.rs` は触らない**: 新規型/関数追加は適切なサブモジュールへ。
  公開が必要な場合のみ `mod.rs` に `pub use` を追加。
- **テスト件数チェック**: 編集後 `cargo test --lib` で 950 維持を確認。

## 関連ドキュメント

- 設計プラン: `.claude/plan/agent-loop-split-validated.md`（事前検証）
- Phase D 評価: `.claude/plan/phase-d-evaluation.md`（Workflow primitive 形式化見送り判定）
- 全体ロードマップ: `.claude/plan/structural-improvements-v2.md`
