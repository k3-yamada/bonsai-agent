# bonsai-agent Quality Scores

> Z-1 Phase 3 で雛形作成 (項目 255)。Z-3 drift monitor Phase 4 (項目 257 候補) で auto update 候補。

## 概要

bonsai-agent の定量品質指標を集約する SINGLE SOURCE OF TRUTH:
- coverage (cargo llvm-cov、module 別)
- clippy warning 数
- Lab スコア (詳細は `lab-history.md`)
- AgentFloor T1-T6 tier scores (詳細は `lab-history.md`)

## 現状 baseline (2026-05-20、本 session 末時点)

| 指標 | 値 | 出典 |
|---|---|---|
| cargo test --lib | **1348 passed** | 本 session 末 (CLAUDE.md 項目 254 完遂後) |
| clippy warnings | 0 (`-D warnings`) | 本 session 検証 |
| cargo fmt | clean | 本 session 検証 |
| cargo build --release | clean (~28s) | 本 session 検証 (Smoke G-RT2 prerequisite) |
| AgentFloor T1 (Instruct) | 0.68 | 項目 224 |
| AgentFloor T2 (SingleTool) | 0.52 | 項目 224 |
| AgentFloor T3 (ToolSelect) | 0.77 | 項目 224 |
| AgentFloor T4 (MultiStep) | 0.64 | 項目 224 |
| AgentFloor T5 (ErrorRecov) | 0.70 | 項目 224 |
| AgentFloor **T6** (LongHorizon) | **0.47** (weakest) | 項目 224、tier-targeted 攻略の優先 |
| Lab 天井連続 | **10 連続 REJECT** (v17-v22 + 249 G-RT) | `lab-history.md` |

## Module 別 coverage (Z-3 Phase 4 で自動更新)

| Module | Lines | Coverage % | Last update |
|---|---|---|---|
| agent | TBD | TBD | (Z-3 Phase 4 実装後) |
| memory | TBD | TBD | (Z-3 Phase 4 実装後) |
| knowledge | TBD | TBD | (Z-3 Phase 4 実装後) |
| tools | TBD | TBD | (Z-3 Phase 4 実装後) |
| runtime | TBD | TBD | (Z-3 Phase 4 実装後) |
| safety | TBD | TBD | (Z-3 Phase 4 実装後) |
| observability | TBD | TBD | (Z-3 Phase 4 実装後) |
| db | TBD | TBD | (Z-3 Phase 4 実装後) |

## 更新方法

- 手動: cargo llvm-cov + 集計 → 上記 table を edit
- 自動: Z-3 drift monitor Phase 4 で `cargo llvm-cov --workspace --summary-only` → upsert
- 履歴: Lab 完走後 hook で `lab-history.md` も連動更新

## 関連

- `docs/quality/lab-history.md` ← Lab スコア詳細
- `.claude/plan/drift-monitor-weekly-gc.md` (Z-3、項目 257 候補) ← 自動更新 source
- `.claude/plan/agents-md-docs-knowledge-base.md` (Z-1 Phase 3) ← 本 file 雛形 source
