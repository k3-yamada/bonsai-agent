# agent_loop.rs 分割設計検証（Step 1, β path）

**Date:** 2026-04-25
**Source file:** `src/agent/agent_loop.rs` (2661 行、テスト含む)
**Status:** Read-only 検証ドキュメント — 実コード変更は Lab v12 完走後の Step 7 で実施

---

## 1. 目的

`structural-improvements-v2.md` Step 7 で計画されている「2497 行 → 4-5 モジュール分割」を、実コード（2661 行に増加）の最新構造で再検証し、モジュール境界・公開 API・commit 単位の確度を高める。

## 2. 現状マップ（公開 API＋ファイルレベル関数）

| 行 | 種別 | 名前 | 可視性 | 備考 |
|----:|------|------|--------|------|
| 32 | struct | `AgentConfig` | pub | コンフィグ集約 |
| 53 | impl | `Default for AgentConfig` | — | 初期値 |
| 72 | fn | `inference_for_task` | pub | TaskType→InferenceParams 派生 |
| 156 | struct | `AgentLoopResult` | pub | 戻り値 |
| 164 | enum | `StepOutcome` | pub | 1ステップ結果 |
| 177 | struct | `LoopState<'a>` | pub | ループ状態集約（middleware/cache 参照保持） |
| 217 | struct | `TokenBudgetTracker` | pub | 予算追跡 |
| 225 | impl | `TokenBudgetTracker` | — | 計算ロジック |
| 273 | impl | `Default for TokenBudgetTracker` | — | — |
| 280 | enum | `OutcomeAction` | pub | handle_outcome の戻り |
| 288 | struct | `StallDetector` | pub | 停滞検出 |
| 294 | impl | `StallDetector` | — | — |
| 319 | impl | `Default for StallDetector` | — | — |
| 336 | struct | `StepContext<'a>` | pub | execute_step 引数 |
| 347 | fn | `execute_step` | pub | 1ステップ実行（〜507、約160行） |
| 508 | fn | `run_agent_loop` | pub | エントリーポイント |
| 542 | fn | `run_agent_loop_with_session` | pub | 本体ループ（〜738、約197行） |
| 739 | fn | `create_task_start_checkpoint` | priv | CP 補助 |
| 780 | fn | `handle_outcome` | priv | アウトカムディスパッチ（〜883、約103行） |
| 884 | fn | `detect_task_complexity` | priv | プランニング判定 |
| 908 | struct | `AdvisorResolution` | priv | resolve_advisor の返り値 |
| 915 | fn | `resolve_advisor_prompt` | priv | Advisor 呼び分け |
| 969 | fn | `log_advisor_call` | priv | 監査ログ |
| 997 | fn | `inject_replan_on_stall` | priv | Stall→Advisor 注入 |
| 1044 | fn | `compute_output_hash` | priv | StallDetector 補助 |
| 1065 | fn | `inject_verification_step` | priv | 検証ステップ注入 |
| 1115 | fn | `inject_planning_step` | priv | 計画ステップ注入 |
| 1153 | fn | `check_invariants` | priv | 完了時不変条件 |
| 1184 | fn | `record_success` | priv | 成功記録 |
| 1211 | fn | `record_abort` | priv | 中断記録 |
| 1228 | fn | `build_answer` | priv | 最終回答整形 |
| 1236 | fn | `clean_response` | priv | レスポンス clean |
| 1254 | mod | `tests` | priv | テスト（〜2661、約1407行） |

## 3. 提案分割（5+α モジュール）

ディレクトリ: `src/agent/agent_loop/` を新設し、`mod.rs` でファサード化。

| モジュール | 推定行数 | 含まれる項目 | 役割 |
|-----------|---------:|-------------|------|
| `mod.rs` | ~50 | `pub use ...` ファサード | 既存 import 互換のため、`crate::agent::agent_loop::run_agent_loop` 等を温存 |
| `config.rs` | ~150 | `AgentConfig`(32)＋`Default`＋`inference_for_task`(72) | 設定とパラメータ派生 |
| `state.rs` | ~190 | `AgentLoopResult`(156)/`StepOutcome`(164)/`LoopState<'a>`(177)/`TokenBudgetTracker`(217-279)/`OutcomeAction`(280)/`StallDetector`(288-335)/`StepContext<'a>`(336) | 型定義集約（pub 型のため `pub use` 必須） |
| `step.rs` | ~165 | `execute_step`(347-507) | 1ステップ実行 |
| `core.rs` | ~330 | `run_agent_loop`(508)/`run_agent_loop_with_session`(542)/`create_task_start_checkpoint`(739) | メインループ |
| `outcome.rs` | ~110 | `handle_outcome`(780)/`detect_task_complexity`(884) | アウトカムディスパッチ |
| `advisor_inject.rs` | ~210 | `AdvisorResolution`(908)/`resolve_advisor_prompt`(915)/`log_advisor_call`(969)/`inject_replan_on_stall`(997)/`inject_verification_step`(1065)/`inject_planning_step`(1115) | Advisor連携＋ステップ注入 |
| `support.rs` | ~95 | `compute_output_hash`(1044)/`check_invariants`(1153)/`record_success`(1184)/`record_abort`(1211)/`build_answer`(1228)/`clean_response`(1236) | 雑多な補助関数 |
| `tests.rs` | ~1407 | `mod tests` | テストはまるごと移設（`use super::*;` のみ調整） |

合計行数（コードのみ）: 約 1255 行 → 平均 / モジュール = 約 180 行（最大 core.rs 330 行）。8 モジュールに分けることで、最大ファイル長が 2661 → 330（約 1/8）になる。

## 4. 公開 API 互換性

外部から呼ばれる `pub` シンボルは以下のみ:

- `run_agent_loop`, `run_agent_loop_with_session`
- `AgentConfig`, `AgentLoopResult`
- `LoopState`, `TokenBudgetTracker`, `StallDetector`, `StepContext`, `StepOutcome`, `OutcomeAction`
- `execute_step`, `inference_for_task`

**互換戦略:** `mod.rs` で全件 `pub use` リエクスポートし、既存呼出を一切変更しない。

```rust
// src/agent/agent_loop/mod.rs（イメージ）
mod config;
mod state;
mod step;
mod core;
mod outcome;
mod advisor_inject;
mod support;
#[cfg(test)] mod tests;

pub use config::{AgentConfig, inference_for_task};
pub use state::{
    AgentLoopResult, StepOutcome, LoopState,
    TokenBudgetTracker, OutcomeAction, StallDetector, StepContext,
};
pub use step::execute_step;
pub use core::{run_agent_loop, run_agent_loop_with_session};
```

`src/agent/mod.rs` は `pub mod agent_loop;` のまま据置（変更不要）。

## 5. 内部依存（モジュール間 use チェーン）

```
config        → (no internal)
state         → config (TokenBudgetTracker は AgentConfig::token_budget を参照)
support       → state (LoopState は record_success/abort で参照)
advisor_inject→ state (LoopState)
outcome       → state, advisor_inject (inject_replan_on_stall), support (record_*)
step          → state, advisor_inject (resolve_advisor_prompt), support (compute_output_hash)
core          → state, step, outcome, advisor_inject, support, config
```

循環は無し（topological order: config → state → support → advisor_inject → outcome → step → core）。

## 6. リスク棚卸

| Risk | 影響 | Mitigation |
|------|------|------------|
| `LoopState<'a>` のライフタイム伝播がモジュール跨ぎで複雑化 | コンパイル不通 | state.rs に集約、関数シグネチャは `&mut LoopState<'a>` で統一 |
| pub フィールドの crate 内利用 | 子モジュール跨ぎで `pub(crate)` 必要 | 移設時に `pub` → `pub(crate)` に意図的に絞る |
| テスト分割で `use super::*` が崩れる | テスト不通 | テストは `tests.rs` 一本にして全 import を整理 |
| clippy 巻戻し（CLAUDE.md 注意事項） | 既存挙動劣化 | commit 直後ルール: 分割 commit と clippy 適用 commit を分離 |
| Lab 中の作業重複 | スケジュール衝突 | 本実装は Lab v12 完走後（Step 7）に持ち越し済 |

## 7. Commit 単位（推奨）

8 commit に分割（各々独立してコンパイル可能を保証）:

1. `refactor: agent_loop/ ディレクトリ作成 + mod.rs ファサード雛形（テスト維持）`
2. `refactor: agent_loop::config 抽出（AgentConfig + inference_for_task）`
3. `refactor: agent_loop::state 抽出（型定義集約）`
4. `refactor: agent_loop::support 抽出（補助関数）`
5. `refactor: agent_loop::advisor_inject 抽出（Advisor連携＋注入）`
6. `refactor: agent_loop::outcome 抽出（handle_outcome）`
7. `refactor: agent_loop::step 抽出（execute_step）`
8. `refactor: agent_loop::core 抽出（run_agent_loop 系、agent_loop.rs 削除）`

各 commit 後に `cargo test --lib` を必須実行。

## 8. 期待効果

- **最大ファイル長**: 2661 → 330 行（約 8 分の 1）
- **平均モジュール長**: 約 180 行
- **責務明示**: 各モジュール 1 トピック
- **手戻り削減**: Step 7 着手時の構造再分析が不要 → **20-30% 短縮**
- **公開 API**: 100% 後方互換（`pub use` ファサード）

## 9. 着手前提条件

- [ ] Lab v12 ベースライン計測完走（推定 18:43 以降のポーリング）
- [ ] テスト数現状: 912 passed + 26 ignored（本ドキュメント作成時点）
- [ ] `agent_loop.rs` への直接編集禁止（CLAUDE.md 巻戻し禁止リスト対象）
- [ ] 8 commit いずれも単独で `cargo test --lib && cargo clippy -- -D warnings` を pass

---

## 付録: 検証根拠

- `wc -l src/agent/agent_loop.rs` → 2661
- `cargo test --lib` → `912 passed; 0 failed; 26 ignored`（2026-04-25 時点、SSE timeout テスト3件追加後）
- `grep -nE '^(pub )?(fn |struct |enum |impl |mod )' src/agent/agent_loop.rs` で全 33 項目を抽出
- 公開 API は `grep -rn 'agent_loop::' src/` で外部 import 経路を確認可能
