# bonsai-agent DESIGN_SPEC

`.claude/plan/` 散在を集約した index。**各ソースは削除せず、本ファイルは目次のみ**。
agents-cli の `DESIGN_SPEC.md` パターン（ADK P3 取込候補）に準拠。

更新は手動。新規プラン追加時に「7. アクティブ計画」へ 1 行追加するだけで足りる。

---

## 1. 設計原則

- **Scaffolding > Model**: 1bit Bonsai-8B の改善余地は限定的、ハーネス側で信頼性を底上げ
- **巻き戻し禁止**: Edit/Write 後の clippy 由来の変更巻き戻しは禁止（CLAUDE.md 注意事項）
- **Lab 中の release ビルド禁止**: 進行中 Lab を競合させない（`cargo test --lib` のみ非干渉）
- **TDD Red 先行 commit**: 巻き戻し対策として失敗テストを先行 commit する

## 2. 全体アーキテクチャ

`src/agent/` を中心とした階層構造。詳細は `CLAUDE.md` のディレクトリツリーを参照。

| 層 | 役割 | 主要ファイル |
|---|---|---|
| **Agent loop** | Reflexion 統合、ステップ実行 | `agent/agent_loop.rs`（要分割、§7） |
| **Benchmark** | pass^k 評価、軌跡評価 | `agent/benchmark.rs` |
| **Judge** | LLM-as-judge rubric 評価（Phase A1 Red） | `agent/judge.rs` |
| **Memory** | A-MEM SQLite + KnowledgeGraph + Skill | `memory/store.rs` 他 |
| **Runtime** | LlmBackend 抽象、HttpAdvisor、Cache | `runtime/llama_server.rs`, `runtime/model_router.rs` |
| **Safety** | Secrets フィルタ、Sandbox、Manifest | `safety/*.rs` |
| **Observability** | AuditLog（粗粒度）+ EventStore（順序） | `observability/audit.rs`, `agent/event_store.rs` |

## 3. 主要トレイト

- `LlmBackend` — `generate(messages, tools, on_token, cancel) -> GenerateResult`
- `Tool` / `TypedTool<Args>` — `name(), description(), parameters_schema(), call(args)`
- `Sandbox` — `execute(command, args, limits) -> ExecResult`
- `Embedder` — `embed(texts) -> Vec<Vec<f32>>`
- `Middleware` — `before_step()` / `after_step()` フック（5 段チェーン）
- `LlmJudge` — `evaluate(task, response, trajectory) -> RubricScore`（ADK P0、Phase B1 Green 予定）

## 4. Lab 自己改善ループ

- `cargo run -- --lab` で起動、k=3 / 22 タスクで pass^k 評価
- 変異候補生成 → 事前スクリーニング → フル評価 → ACCEPT/REJECT
- LabStagnationDetector でベスト不変 / variance 崩壊を検知 → Dreamer 早期起動
- 履歴: `memory/lab_history_v1_v6.md`、最新 v9–v12 は `CLAUDE.md` 末尾に記載

## 5. 実験運用ルール

- **release バイナリ稼働中はソース編集 OK だが `cargo build --release` 禁止**
- Lab 完走前は `cargo test --lib` のみ（バイナリ非干渉）
- 進行中の `task_id` は TaskOutput で `block=false` 監視

## 6. ADK 取込ロードマップ（Phase A〜D）

| Phase | スコープ | ステータス |
|---|---|---|
| **A1** | LLM-as-judge TDD Red（型 + パース + Stub Err） | ✅ 完了（commit `1fa204a`） |
| **A2** | DESIGN_SPEC.md 雛形 | ✅ 本ファイル |
| **B1** | Judge Green: try_remote_with_prompt / try_claude_code_with_prompt wire | ✅ 完了（923テスト、監査対応済） |
| **B2** | experiment.rs 統合（judge_threshold = 0.7） | 📋 設計済（[`.claude/plan/phase-b2-judge-gate.md`](../.claude/plan/phase-b2-judge-gate.md)） |
| **C** | Generator-Critic 分離（SubAgentExecutor 拡張） | 🔵 候補 |
| **D** | Workflow primitive（Sequential/Parallel/Loop trait） | 🔵 リスク高、保留 |

詳細: [`.claude/plan/adk-integration.md`](../.claude/plan/adk-integration.md)

## 7. アクティブ計画（`.claude/plan/`）

| ファイル | テーマ |
|---|---|
| `adk-integration.md` | Google ADK 知見の取り込み（Phase A/B/C/D、推奨 β path） |
| `agent-loop-split-validated.md` | `agent_loop.rs` 2661 行 → 8 モジュール分割設計 |
| `next-actions-2026-04-25.md` | Lab v12 並行作業候補（残作業整理） |
| `next-actions-2026-04-25-v2.md` | 〃（Codex 分析統合版） |
| `structural-improvements-v2.md` | 構造改善ロードマップ v2（残タスク + Lab v11 計測） |
| `continuation-2026-04-25.md` | Lab v11 後継 / EventStore 統合 / ベンチマーク拡張 |
| `lab-v11-accept-analysis.md` | Lab v11 ACCEPT 詳細調査（defaults 化見送り判断） |
| `lab-v12-accept-analysis.md` | Lab v12 ACCEPT 詳細調査（temperature 0.7 を v13 確認実験で再検証推奨） |
| `phase-c-and-refactor-draft.md` | Phase C ベンチマーク拡張 + リファクタ草稿 |
| `phase-b2-judge-gate.md` | ADK Phase B2: judge_threshold=0.7 を ACCEPT ゲートに統合する設計 |

## 8. 参照

- `CLAUDE.md` — プロジェクト概要 + ハーネスパターン 162 項目 + Lab 履歴
- `memory/MEMORY.md` — 永続メモリ index（個別知見ファイルへのリンク）
- `memory/google_adk_learnings.md` — ADK 2.0 / agents-cli 全体像、P0–P4 取込候補
