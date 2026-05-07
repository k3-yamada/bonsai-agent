# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## プロジェクト概要

`bonsai-agent` — Bonsai-8B（1ビット量子化Qwen3-8B、1.28GB）で動作するRust製自律型エージェント。
Mac M2 16GB上でllama-server HTTP API経由で推論。886テスト、69ソースファイル。

設計原則: **「Scaffolding > Model」** — 1ビットモデルの改善余地は限定的。ハーネス側で信頼性を底上げする。

## ビルド・テストコマンド

```bash
cargo build                    # ビルド
cargo test                     # ユニットテスト（882テスト）
cargo test -- --ignored        # 統合テスト（llama-server/ネットワーク必要）
cargo clippy -- -D warnings    # リント
cargo fmt -- --check           # フォーマットチェック
cargo run -- --manifest        # ケイパビリティ一覧
cargo run -- --vault           # ナレッジVault概要
cargo run -- --lab             # 自律的自己改善ループ（pass^k評価）
```

## Rust Edition

Rust **2024 edition**。let chains、div_ceil等を使用。

## アーキテクチャ

```
src/
├── main.rs / lib.rs / cancel.rs
├── config.rs                      # TOML設定（~/.config/bonsai-agent/config.toml）
│                                  # AgentSettings.soul_path: SOUL.mdペルソナパス
├── agent/
│   ├── agent_loop.rs              # run_agent_loop() — Reflexion + 全パーツ統合
│   │                              # StallDetector — 停滞検出→自動再計画
│   │                              # load_soul() — SOUL.mdペルソナ注入（3段階検索）
│   ├── benchmark.rs               # BenchmarkSuite — 8タスク評価
│   │                              # MultiRunConfig/MultiRunTaskScore — pass^k複数回評価
│   │                              # run_k() — 各タスクk回実行、pass_at_k/pass_consecutive_k計算
│   ├── parse.rs / validate.rs     # <think>/<tool_call>パーサー、バリデーション
│   ├── conversation.rs            # Message, Session, ToolCall
│   ├── error_recovery.rs          # FailureMode(4種), CircuitBreaker
│   │                              # LoopDetector — 2層検出（salient hash+頻度+循環パターン）
│   │                              # ContinueSite — 段階的回復（リトライ→再計画→安全停止）
│   │                              # RecoveryAction::Replan — コンテキスト圧縮+再計画指示
│   ├── compaction.rs              # 4段階コンテキストコンパクション
│   │                              # find_ai_tool_pairs() — AI+Toolペア保護
│   ├── checkpoint.rs              # git stashチェックポイント/ロールバック
│   ├── task.rs                    # TaskState状態マシン（中断/再開/サブタスク）
│   ├── experiment.rs              # ExperimentLoop — 自律的自己改善ループ（run_k版）
│   │                              # run_experiment_loop: pass^k版（k=3, jitter_seed=true）
│   ├── experiment_log.rs
│   ├── middleware.rs               # ミドルウェアチェーン（5段: Audit/ToolTrack/Stall/Compact/TokenBudget）
│   ├── subagent.rs                # SubAgentExecutor — サブタスク順次委任（深度制限2、エラー境界）
│   └── event_store.rs              # Event Sourcing（統一イベントストリーム）          # 実験ログ（SQLite+TSV永続化）
│                                  # Experiment.from_multi_results() — pass^k指標付き記録
│                                  # TSV: 11列（pass_at_k/pass_consecutive_k/score_variance追加）
├── tools/
│   ├── mod.rs                     # Tool トレイト + ToolRegistry（動的選択）
│   │                              # format_schemas_compact() — Deferred Schema（トークン80%節約）
│   ├── shell.rs / git.rs / web.rs / repomap.rs
│   ├── file.rs                    # FileReadTool / FileWriteTool
│   │                              # 構造化出力（行番号付与+offset/limitウィンドウ制御）
│   │                              # fuzzyマッチSEARCH/REPLACE（空白正規化+trimフォールバック）
│   ├── plugin.rs                  # TOML定義カスタムツール
│   ├── mcp_client.rs              # MCPクライアント（JSON-RPC over stdio）
│   ├── hooks.rs                   # pre/postフック
│   ├── permission.rs / sandbox.rs
├── runtime/
│   ├── inference.rs               # LlmBackend + MockLlmBackend
│   ├── llama_server.rs            # LlamaServerBackend（HTTP API）
│   ├── cache.rs / embedder.rs     # 推論キャッシュ、Embedder
│   └── model_router.rs            # ModelRouter + PipelineStage + AdvisorConfig
├── memory/
│   ├── store.rs                   # MemoryStore — SQLite A-MEM + セッション永続化
│   ├── experience.rs / skill.rs   # 経験記録、スキル自動昇格（3シグナルスコアリング）
│   ├── dreams.rs                  # Dreaming Light/Deep分離（dream_light/dream_deep）
│   ├── search.rs                  # ハイブリッド検索（FTS5+ベクトルRRF融合）
│   ├── feedback.rs                # Correction/Reinforcement検出
│   ├── graph.rs                   # KnowledgeGraph — グラフ構造連想記憶（BFS双方向探索）
│   ├── dreams.rs                  # Dreamingシステム（振り返り+パターン検出）
│   └── evolution.rs               # arxiv自己進化エンジン + 能動的自己改善
├── knowledge/
│   ├── extractor.rs               # フロー→ストック抽出（6カテゴリ）
│   └── vault.rs                   # mdファイル蓄積（Karpathyパターン）
├── safety/
│   ├── secrets.rs                 # 秘密情報フィルタ
│   ├── autonomy.rs / boot_guard.rs / manifest.rs / network.rs
├── observability/
│   └── audit.rs                   # 監査ログ（LlmCall/ToolCall/SecurityEvent/StepOutcome）
└── db/
    ├── schema.rs / migrate.rs     # 15テーブルSQLiteスキーマ（V5: knowledge_graphテーブル追加）
```

## 主要なトレイト

- `LlmBackend` — `generate(messages, tools, on_token, cancel) -> GenerateResult`
- `Tool` — `name(), description(), parameters_schema(), permission(), call(args), is_read_only()`
- `Sandbox` — `execute(command, args, limits) -> ExecResult`
- `Embedder` — `embed(texts) -> Vec<Vec<f32>>`

## ハーネスパターン（v1: 2026-04-14〜）

p^n問題（ステップ蓄積による失敗確率指数的増大）への対策。**項目 1-180 は [memory/harness_patterns_archive.md](../../.claude/projects/-Users-keizo-bonsai-agent/memory/harness_patterns_archive.md) にアーカイブ**（2026-05-07 分離、CLAUDE.md 認知負荷低減）。

### カテゴリ索引（archive 参照用）
- **Core ハーネス**: 項目 1-10（pass^k / Continue Sites / 2層 LoopDetector / StallDetector / fuzzy / AI+Tool ペア保護 / Deferred Schema / SOUL.md / StepOutcome / 計画強制）
- **Tool / RepoMap**: 項目 11, 30, 70, 74, 75, 100, 101, 119, 148
- **Memory / Knowledge**: 項目 13, 71, 76, 77, 80, 83, 84, 106, 161, 162, 177, 179
- **Compaction / Context**: 項目 6, 12, 41, 46, 78, 81, 82, 158, 159, 178, 187
- **Backend / Inference**: 項目 35, 36, 49, 53, 56, 60, 61, 63, 67, 90, 103, 105, 130, 167, 168, 174, 195, 198
- **Advisor**: 項目 15-24, 89
- **Checkpoint / Audit / Logger**: 項目 25-29, 38, 39, 109, 110, 152, 175
- **MCP**: 項目 102, 108, 124, 132-135, 137, 180, 182
- **Safety / Filter / Anti-Halluc**: 項目 42, 43, 44, 47, 50, 51, 88, 95, 96, 175, 178
- **Subagent**: 項目 120, 160
- **Lab 実験基盤**: 項目 107, 123, 125, 131, 138-145, 173, 184, 185, 188-198
- **Refactor / Quality**: 項目 64-66, 82, 92-94, 146-156, 164-166

### デフォルト化済み変異（Lab ACCEPT → 恒久適用）
- 項目 10: 計画強制ルール（Lab v6.2 唯一の ACCEPT）
- 項目 47: ツール使用前 `<think>` で意図記述（+0.032 実証）
- 項目 50: フォールバック戦略（+0.001 実証）
- 項目 136: 回答前ファイル内容確認（Lab v9 +0.0157 実証）

### 直近項目（181-201、1 行サマリー）
181. 作業ツリー cleanup + clippy 既存警告 6 件整理（Phase A、private test のみ変更）
182. MCP detach 効果の core 22 ベンチマーク定量化（score +0.0492、duration ほぼ同等、恒久承認）
183. MLX vs llama-server smoke 比較（MLX 単独 +0.0969 / +28.7% 遅い、項目 173 と矛盾観測）
184. MLX core 22 評価 → 仮説 173「MLX 環境劣化」最終 REJECT（score=0.7976、Zone A 寄りの C）
185. MLX 再現性確認 + FallbackChain 実機検証 + smoke 補正係数 ×0.42 sign-aware 実装
186. Multi-plan 並列 + Plan 2 真因確定（HTTP 400 = H6 CONTEXT_OVERFLOW = n_ctx burst）
187. Step 14 ContextOverflowGuard 実装（F2、累積 token 監視 + 強制 compaction、TDD Red→Green）
188. F1 (`-c 12288`) + Phase 2d smoke（B1a MLX-primary=0.5862 / B1b llama-only=0.7440 +27%）
189. Lab v15 core 22 llama-only baseline=0.7560 / F2 regression check PASS（duration -22% 高速化）
190. F3 RequestSizeGuard 実装（CCG review 12 件 finding + TDD 5 phase、smoke fire=0）
191. F3 core 22 検証 → fire=0 / score=0.7849 / 副作用ゼロ確証（Layer 1 支配仮説）
192. F3 extended tier 検証 → fire=0、Layer 1（項目 116）完全支配確定（120/120 run）
193. F3 threshold 半減 smoke → fire=0/180run、**F3 完全削除**（dead-code 確定、net -475 行）
194. textual tool_call leak 調査 → parser regression test 2 件追加（真因=LLM 出力 + console 可視化）
195. MLX sticky fix（`recover_after_n_success` 機構、TDD 4 件追加、後方互換）
196. leak fix (a) — system prompt rule 16 で think 内 JSON literal 抑止
197. Layer 1 緩和実験（4000→8000）→ Δscore +0.0226 / Δduration +12.8%、棚保留判定
198. MLX sticky recovery 実機検証（core 22 score=0.7837 Zone A、recovery probe 2 件発動、+33.7% vs sticky、production default 残置決定）
199. A-RAG hierarchical retrieval framework との整合（docs validation、bonsai は superset 5 拡張）
200. Beyond pass@1 RDC/VAF/GDS 信頼性メトリクス追加（TDD 5 phase、SQLite V8→V9、TSV 12→15 列）
201. AgentHER hindsight relabel 実装（ECHO + HSL、3 新規 API、TDD 3 phase、1041→1051 passed）
202. AgentHER runtime 組込（Lab 末尾 hook、HindsightSummary、TDD 3 phase、1051→1055 passed）+ Phase 4 smoke 実機検証（wiring PASS / event flow disconnect 発見、benchmark.rs:1009 の `MemoryStore::in_memory()?` で events が ephemeral store に閉じ込められ persistent store に到達せず、Phase 5 案件として TODO 化）


## Lab実機テスト結果（Bonsai-8B 1bit, k=3, 10サイクル）

### v14結果（2026-04-28〜29）— 4実験完了+1中断、プロンプト天井 4 連続確定
- ベースライン: score=**0.5192**, pass@k=0.5667, pass_consec=0.5667（2h56m, MLX backend, **40タスク化**）
- v9/v10 baseline 0.79〜0.81 から **-35% 退行** — ベンチマーク 22→40 拡張が主要因、MLX SSE timeout fallback 品質劣化が副次要因
- ACCEPT 1: 「フォールバック戦略」(+0.0058) → **既存 defaults #50 の再評価でデフォルト化見送り** (delta < +0.015 閾値)
- REJECT: ツール思考強制(-0.026=#47再評価), エラー分析(-0.006), ファイル存在確認(-0.051)
- 真新規変異 ID 176/178 = 0/2 ACCEPT、**プロンプトチューニングは天井 4 連続 (v8/v9/v10/v14)**
- 17.5h 完走 (Step 13 socket timeout の効果で v13 19.5h hang から hang ゼロに改善)
- Step 11 nudge 発火 0件 / Step 10 DiffStore 再評価ゲート未達 (YAGNI 維持)
- 詳細: `.claude/plan/lab-v14-result.md`、次サイクル候補: 構造改善 v3 / ベンチマーク階層分離 / MLX→llama-server 切替試験

### v10結果（2026-04-24）— 9実験完了、ベースライン+1.5%
- ベースライン: score=0.8087, pass@k=0.8939（v9比+0.012改善、Quick Wins効果）
- ACCEPT 1: oracle insight「逆方向アプローチ」(+0.003, score=0.8118) → **delta小のためデフォルト化見送り**
- REJECT: 事実確認(-0.030), 完成形先行(-0.029), タスク分解(-0.022), 完了条件(-0.026), メタ複合(-0.026), 推測回避(-0.031), oracle逆方向2(-0.016), メタ複合2(-0.022)
- 承認率: **1/9 (11%)** — 実質全REJECT、最適解収束継続
- v8-v10で3回連続「実質全REJECT」→ ハーネスプロンプト最適化の天井に到達

### v9結果（2026-04-21）— 14実験完了
- ベースライン: score=0.7963, pass@k=0.8939（MLXバックエンド）
- **ACCEPT 1**: 「回答前の事実確認」(+0.0157, score=0.8120) → **デフォルト化済**
- REJECT: 最小限ツール選択(-0.0091), 仮説検証(-0.0097), 段階的要約(-0.0076), メタ複合(-0.0288), 冗長抑制(-0.0091)
- 実験6/14進行中、完了後に最終結果を更新

### v8結果（2026-04-19）— 全10実験REJECT、最適解収束
- ベースライン: score=0.8517, pass@k=0.9167（v6.2と同値）
- **全10実験REJECT** — デフォルト設定が最適解に完全収束
- Adam's Lawリライト+6機能追加後もベースライン維持（劣化なし）
- 承認率: **0/10 (0%)** — 改善余地なし

### v1〜v6.2（2026-04-14〜04-17）
- アーカイブ済 → memory/lab_history_v1_v6.md（CLAUDE.md認知負荷低減のため2026-04-25分離）
- 派生デフォルト化変異: 項目10（計画強制）/ 項目47（思考強制）/ 項目50（フォールバック戦略）

## テストパターン

- `MockLlmBackend` — スクリプト化レスポンス
- `MemoryStore::in_memory()` — インメモリSQLite
- `#[ignore]` — 実サーバー/ネットワーク必要なテスト
- `MultiRunTaskScore::from_scores()` — pass^k指標の単体テスト

## 注意事項

- **【最重要】Edit/Writeツールでファイルを変更した後、clippy警告（collapsible_if, too_many_arguments等）を理由にファイルを元の状態に戻す行為を絶対に行わないでください。変更はそのまま保持してください。clippy修正が必要な場合は別のEdit操作で行ってください。**
- **【巻き戻し禁止】** 特にerror_recovery.rs、benchmark.rs、agent_loop.rsの3ファイルでclippy auto-fixによる巻き戻しが発生しやすい。これらのファイルへの変更は必ず保持すること。
- 大量変更時はPython subprocess+即git commitで原子的に行う（確立済み手法）
- ureq v3のHTTPS → web_fetchはreqwest::blocking（native-tls）を使用
- llama-serverの`--flash-attn`は値`on`が必要（`--flash-attn on`）
