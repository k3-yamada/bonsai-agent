# bonsai-agent アーキテクチャ overview

> Z-1 Phase 2 で CLAUDE.md から分離 (項目 255)。元の CLAUDE.md「アーキテクチャ」セクション verbatim 移行。

## src/ tree

```
src/
├── main.rs / lib.rs / cancel.rs
├── config.rs                      # TOML設定（~/.config/bonsai-agent/config.toml）
│                                  # AgentSettings.soul_path: SOUL.mdペルソナパス
├── domain/                       # ★ 最下層 (Clean Architecture Entities + Ports、ADR-010)
│   ├── conversation.rs           # Message, Role, Session, ToolCall, Attachment
│   ├── tool_schema.rs            # ToolSchema DTO (Tool trait=振る舞いは tools 層)
│   ├── embedder.rs               # Embedder trait + SimpleEmbedder/FastEmbedder + cosine
│   ├── event.rs                  # Event/EventType/TrajectoryCandidate/EventRepository trait
│   └── llm.rs                    # LlmBackend trait + GenerateResult/TokenUsage + MockLlmBackend
├── agent/
│   ├── agent_loop/                # run_agent_loop() — Reflexion + 全パーツ統合 (mod 分割)
│   │                              # core.rs: メインループ + emit_event helper
│   │                              # step.rs: 1 step 実行 (LLM call + tool exec)
│   │                              # outcome.rs: StepOutcome ディスパッチ + Reflexion + Critic
│   │                              # advisor_inject.rs: 計画/検証/critic 注入 + verification metric
│   │                              # support.rs: build_answer / check_invariants / record_*
│   ├── benchmark.rs               # BenchmarkSuite — 8タスク評価 + AgentFloor 30 task (項目 223)
│   │                              # MultiRunConfig/MultiRunTaskScore — pass^k + PASS@(k,T) (項目 225)
│   │                              # run_k() — 各タスクk回実行、pass_at_k/pass_consecutive_k計算
│   ├── parse.rs / validate.rs     # <think>/<tool_call>パーサー、バリデーション
│   ├── error_recovery.rs          # FailureMode(4種), CircuitBreaker, LoopDetector, ContinueSite
│   ├── compaction.rs              # 4段階コンテキストコンパクション + AI+Toolペア保護
│   ├── checkpoint.rs              # git stashチェックポイント/ロールバック
│   ├── task.rs                    # TaskState状態マシン（中断/再開/サブタスク）
│   ├── experiment.rs              # ExperimentLoop — 自律的自己改善ループ
│   │                              # run_factcheck_pass_lab (項目 230、env-gated)
│   │                              # lab_mlx_prewarm (項目 252、env-gated、per-iter timeout 項目 252 M2)
│   ├── experiment_log.rs          # 実験ログ（SQLite+TSV永続化）
│   ├── middleware.rs              # ミドルウェアチェーン（5段）
│   ├── subagent.rs                # SubAgentExecutor — サブタスク順次委任
│   ├── working_memory.rs          # Miller 7±2 hard cap (項目 219、env-gated)
│   └── event_store.rs             # 具象 EventStore<'a> (SQLite)。型/port/純粋ロジックは domain::event
│                                  # EventType::AssistantMessage emit は step.rs 経由 (項目 237)
├── tools/
│   ├── mod.rs                     # Tool トレイト + ToolRegistry（動的選択）
│   │                              # format_schemas_compact — Deferred Schema (トークン80%節約)
│   ├── shell.rs / git.rs / web.rs / repomap.rs
│   ├── file.rs                    # FileReadTool / FileWriteTool (fuzzy SEARCH/REPLACE)
│   ├── plugin.rs                  # TOML定義カスタムツール
│   ├── mcp_client.rs              # MCPクライアント（JSON-RPC over stdio）
│   └── hooks.rs / permission.rs / sandbox.rs
├── runtime/
│   ├── inference.rs               # FallbackBackend (具象)。LlmBackend trait/DTO/Mock は domain::llm
│   ├── llama_server.rs            # LlamaServerBackend（HTTP API）
│   ├── cache.rs                   # 推論キャッシュ (Embedder は domain::embedder へ移動)
│   └── model_router.rs            # ModelRouter + PipelineStage + AdvisorConfig + CriticConfig (項目 226)
├── memory/
│   ├── store.rs                   # MemoryStore — SQLite A-MEM + セッション永続化
│   ├── experience.rs / skill.rs   # 経験記録、スキル自動昇格
│   ├── search.rs                  # ハイブリッド検索（FTS5+ベクトル+graph BFS RRF、項目 228）
│   ├── feedback.rs                # Correction/Reinforcement検出
│   ├── graph.rs                   # KnowledgeGraph — グラフ構造連想記憶（BFS双方向探索）
│   │                              # contains_triple / find_conflicting_edges (項目 230)
│   ├── factcheck.rs               # KG-grounded triple extraction + verify (項目 230、env-gated)
│   ├── heuristics.rs              # ERL Heuristics Pool (項目 213、env-gated default OFF)
│   ├── decay.rs                   # Cerememory power-law decay port (項目 217、env-gated)
│   ├── review.rs                  # Cerememory ReviewState V12 port (項目 218、env-gated)
│   ├── dreams.rs                  # Dreaming Light/Deep分離
│   └── evolution.rs               # arxiv自己進化エンジン + 能動的自己改善
├── knowledge/
│   ├── extractor.rs               # フロー→ストック抽出（6カテゴリ）
│   ├── vault.rs                   # mdファイル蓄積（Karpathyパターン）
│   └── vault_lint.rs              # Vault 整合性 5 軸検出 (項目 246/251/254)
├── safety/
│   ├── secrets.rs                 # 秘密情報フィルタ
│   ├── autonomy.rs / boot_guard.rs / manifest.rs / network.rs
├── observability/
│   ├── audit.rs                   # 監査ログ（AuditAction:LlmCall/ToolCall/SecurityEvent/CriticCall/FactCheck/VaultLint）
│   └── logger.rs                  # 構造化ログ (log_event)
└── db/
    ├── schema.rs / migrate.rs     # SQLite schema V16 (frontier_bucket_scores / frontier_inject_scores、項目 229)
```

## 主要なトレイト

- `LlmBackend` — `generate(messages, tools, on_token, cancel) -> GenerateResult`、`generate_with_params(.., &InferenceParams)` (項目 226 critic temperature)
- `Tool` — `name(), description(), parameters_schema(), permission(), call(args), is_read_only()`
- `Sandbox` — `execute(command, args, limits) -> ExecResult`
- `Embedder` — `embed(texts) -> Vec<Vec<f32>>`
- `EventRepository` — `append / replay / extract_*_trajectories_since_id` (項目 209、SQLite + Mock parity)

## 設計原則

**「Scaffolding > Model」** — 1ビットモデルの改善余地は限定的。ハーネス側で信頼性を底上げする。

p^n 問題 (ステップ蓄積による失敗確率指数的増大) への対策が中心思想:
- AI+Tool ペア保護 (項目 5)
- Continue Sites (項目 2)
- 2 層 LoopDetector (項目 3)
- StallDetector (項目 4)
- Deferred Schema (項目 7、トークン 80% 節約)
- StepOutcome 統一 dispatch (項目 9)
- 計画強制ルール (項目 10、Lab v6.2 唯一の ACCEPT)
- KG-FactCheck (項目 230 Plan A 系列、Bonsai-8B fabricate 検出)
- Vault Lint 5 軸 (項目 246/251/254、orphan draft 検出含む)
- Dynamic Budget (項目 248 Phase 1-4 + Phase 5 plan 起票済)

## Module layer 順

詳細は [module-layer-rules.md](module-layer-rules.md) (Z-4 layer linter rule source) を参照。
層順 (確定、DEP-001 強制、WHITELIST_DEP=空): `domain < db < observability < safety < memory < knowledge < runtime < tools < agent < main` (ADR-010)

## 関連

- CLAUDE.md (Claude Code エントリ) ← 本 file の link source
- docs/INDEX.md (Z-1 Phase 1 で新設) ← ナビゲーション
- memory/harness_patterns_archive.md ← ハーネスパターン項目 1-252 verbatim
