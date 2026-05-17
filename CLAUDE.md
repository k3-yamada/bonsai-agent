# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## プロジェクト概要

`bonsai-agent` — Bonsai-8B（1ビット量子化Qwen3-8B、1.28GB）で動作するRust製自律型エージェント。
Mac M2 16GB上でllama-server HTTP API経由で推論。1278テスト、69ソースファイル。

設計原則: **「Scaffolding > Model」** — 1ビットモデルの改善余地は限定的。ハーネス側で信頼性を底上げする。

## ビルド・テストコマンド

```bash
cargo build                    # ビルド
cargo test                     # ユニットテスト（1278テスト）
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
│   ├── conversation.rs            # Message, Session, ToolCall
│   ├── error_recovery.rs          # FailureMode(4種), CircuitBreaker, LoopDetector, ContinueSite
│   ├── compaction.rs              # 4段階コンテキストコンパクション + AI+Toolペア保護
│   ├── checkpoint.rs              # git stashチェックポイント/ロールバック
│   ├── task.rs                    # TaskState状態マシン（中断/再開/サブタスク）
│   ├── experiment.rs              # ExperimentLoop — 自律的自己改善ループ
│   │                              # run_factcheck_pass_lab (項目 230、env-gated)
│   ├── experiment_log.rs          # 実験ログ（SQLite+TSV永続化）
│   ├── middleware.rs              # ミドルウェアチェーン（5段）
│   ├── subagent.rs                # SubAgentExecutor — サブタスク順次委任
│   ├── working_memory.rs          # Miller 7±2 hard cap (項目 219、env-gated)
│   └── event_store.rs             # Event Sourcing（統一イベントストリーム）
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
│   ├── inference.rs               # LlmBackend + MockLlmBackend
│   ├── llama_server.rs            # LlamaServerBackend（HTTP API）
│   ├── cache.rs / embedder.rs     # 推論キャッシュ、Embedder
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
│   └── vault.rs                   # mdファイル蓄積（Karpathyパターン）
├── safety/
│   ├── secrets.rs                 # 秘密情報フィルタ
│   ├── autonomy.rs / boot_guard.rs / manifest.rs / network.rs
├── observability/
│   └── audit.rs                   # 監査ログ（AuditAction:LlmCall/ToolCall/SecurityEvent/CriticCall/FactCheck）
└── db/
    ├── schema.rs / migrate.rs     # SQLite schema V16 (frontier_bucket_scores / frontier_inject_scores、項目 229)
```

## 主要なトレイト

- `LlmBackend` — `generate(messages, tools, on_token, cancel) -> GenerateResult`、`generate_with_params(.., &InferenceParams)` (項目 226 critic temperature)
- `Tool` — `name(), description(), parameters_schema(), permission(), call(args), is_read_only()`
- `Sandbox` — `execute(command, args, limits) -> ExecResult`
- `Embedder` — `embed(texts) -> Vec<Vec<f32>>`
- `EventRepository` — `append / replay / extract_*_trajectories_since_id` (項目 209、SQLite + Mock parity)

## ハーネスパターン

p^n問題（ステップ蓄積による失敗確率指数的増大）への対策。**項目 1-239 は [memory/harness_patterns_archive.md](../../.claude/projects/-Users-keizo-bonsai-agent/memory/harness_patterns_archive.md) に verbatim アーカイブ**。CLAUDE.md は索引 + デフォルト化済み変異 + 直近 5 項目の 1 行サマリーのみ保持。

### カテゴリ索引（archive 参照用）

**Core 機構**
- Core ハーネス: 項目 1-10（pass^k / Continue Sites / 2層 LoopDetector / StallDetector / fuzzy / AI+Tool ペア保護 / Deferred Schema / SOUL.md / StepOutcome / 計画強制）
- Tool / RepoMap: 項目 11, 30, 70, 74, 75, 100, 101, 119, 148
- Compaction / Context: 項目 6, 12, 41, 46, 78, 81, 82, 158, 159, 178, 187
- Backend / Inference: 項目 35, 36, 49, 53, 56, 60, 61, 63, 67, 90, 103, 105, 130, 167, 168, 174, 195, 198
- Checkpoint / Audit / Logger: 項目 25-29, 38, 39, 109, 110, 152, 175
- Advisor / Critic: 項目 15-24, 89, **226** (G1 Critic 別 LLM、env-gated)
- MCP: 項目 102, 108, 124, 132-135, 137, 180, 182
- Safety / Filter / Anti-Halluc: 項目 42, 43, 44, 47, 50, 51, 88, 95, 96, 175, 178, **230/234/235/237** (KG-FactCheck Plan A 系列), **239** (regex dash fix)
- Subagent: 項目 120, 160

**Memory / Knowledge**
- Memory / Knowledge 基盤: 項目 13, 71, 76, 77, 80, 83, 84, 106, 161, 162, 177, 179
- Cerememory 三本柱: **217** (power-law decay) / **218** (ReviewState V12 freshness) / **219** (Working Memory 7±2 cap)
- AgentHER hindsight: 項目 201-205
- Graph fusion (paper agentmemory R@10=98.6 達成): **228**
- sqlite-vec: **220** (Step A 採用 + Step B REJECT) → **221** (Lab REJECT) → **222** (wiring removal)

**Lab 実験基盤 / Benchmark**
- Lab 実験基盤: 項目 107, 123, 125, 131, 138-145, 173, 184, 185, 188-198
- Beyond pass@1 RDC/VAF/GDS: **200**
- PASS@(k,T) 二軸 metric: **225**
- AgentFloor 6-tier ladder (T1-T6): **223/224** (Bonsai-8B 能力プロファイル: T1=0.68/T2=0.52/T3=0.77/T4=0.64/T5=0.70/T6=0.47、weakest=T6)
- LongMemEval-S 移植 + 500Q baseline (R@5=0.91): **227**
- Frontier Benchmark (context-length axis): **229/231/232/233** (第 6 軸 baseline 確立、bucket 0→1 gradient = -0.1944 ACCEPT)
- Refactor / Quality: 項目 64-66, 82, 92-94, 146-156, 164-166, 209 (EventRepository trait 化)

### デフォルト化済み変異（Lab ACCEPT → 恒久適用）

- 項目 10: 計画強制ルール（Lab v6.2 唯一の ACCEPT）
- 項目 47: ツール使用前 `<think>` で意図記述（+0.032 実証）
- 項目 50: フォールバック戦略（+0.001 実証）
- 項目 136: 回答前ファイル内容確認（Lab v9 +0.0157 実証）

### 直近 5 項目 (詳細は archive 参照)

- **238**: Lab v20 KG-FactCheck harness Phase 1-3 + Lab v20 Phase 5 起動 (PID 32568)
- **239**: Pattern 1 regex dash 対応 (factcheck.rs `RE_IS_THE_OF` を Pattern 2 と統一、1278 passed)
- **240**: CLAUDE.md archive automation Phase 1-3 (scripts/claudemd_archive.py、3 mode CLI、8 unit test、stdlib only)
- **241**: 🎉 **Lab v20 完走 (wall 19h 9m、10/10 cycle) + Pearson r REJECT 確定 (天井 9 連続) + structural finding** = ON 5/5 で `(conf+unk)/total = 1.0` deterministic (matched=0 で variance ゼロ → Pearson r=0.0)、ただし **conf=3 deterministic 5/5 = Plan A 真効力安定確証** (Bonsai-8B fabricate 検出機構は production-ready)、副次 paired t-test Δ=-0.0038 / p=0.5316 (factcheck 設計上 score 寄与なし実機確認)、対応 = 案 A KG seed 拡張で matched>0 シナリオ生成 + Lab v21 再 paired (別 plan)
- **242**: 🎉 **Lab v21 KG seed 拡張 Phase 1-4 完遂 (Lab v20 structural finding 解消の実機実証)** = Phase 1-3 (commit `8190889`): success_fact 5 task 追加 (benchmark.rs、Pattern 1 + Pattern 2 両軸 cover) + `seed_kg_for_factcheck_lab` 3→8 fact additive 拡張 (factcheck.rs)、1278→1286 passed (+8)。Phase 4 補完: SMOKE_TASK_IDS 10→15 拡張 (success_fact 5 含む)、G-7a PASS = env unset で factcheck emit=0 / score=0.5302 / 47 min (後方互換)、**G-7b PASS = factcheck `total=11 matched=8 unknown=0 conflicting=3 mean_path_len=1.00` / audit_log id=12394 / score=0.5415 / 65 min** = matched 軸 variance 復活確証 + AgentHER successful=4/5 (Bonsai-8B 1bit は context hint 込みで正解可能)、Lab v21 paired で Pearson r 計算可能化確実 (別 session ~15-20h wall)

## Lab実機テスト結果 (最新のみ、詳細は memory/lab_history*.md)

### Lab 天井連続記録 (Bonsai-8B 1bit, k=3, 10 cycle paired)
| Lab | 軸 | 結果 | 数値 | 教訓 |
|---|---|---|---|---|
| v17 | ERL Heuristics Pool | REJECT | Δ=−0.0014 / p=0.5072 | 天井 7 連続 |
| v19 | Frontier (score 軸) | REJECT | Δ=+0.0072 / p=0.4262 | 天井 8 連続 |
| v19 (案 A) | Frontier (context-length 軸) | **ACCEPT** | bucket 0→1 gradient = -0.1944 (基準 1.94x) | 第 6 軸 baseline 確立 |
| v20 | KG-FactCheck (Pearson r) | **REJECT** | r=0.0 / Δ=-0.0038 / p=0.5316 / wall 19h 9m | **天井 9 連続** + structural finding (matched=0 で variance ゼロ) |

### Bonsai-8B 能力プロファイル (LADDER + AgentFloor、項目 224)
- T1 Instruct=**0.68** / T2 SingleTool=0.52 / T3 ToolSelect=0.77 / T4 MultiStep=0.64 / T5 ErrorRecov=0.70 / **T6 LongHorizon=0.47** (weakest)
- tier-targeted 変異の優先攻略 = T6 偏向 (Lab v22+ HypothesisGenerator 改修の前提)

### Plan A 系列完結 (項目 230 → 234 → 235 → 236 → 237 → 238 → 239 → 240 → 241 → 242)
- 3 段配線: (a) 230 wiring (b) 235 trajectory scope (c) 237 event emit → G-6b で **factcheck total=5 / conflicting=3 = Bonsai-8B fabricate 検出初成功**
- 項目 242 Phase 4 G-7b 実機: **total=11 matched=8 unknown=0 conflicting=3 mean_path_len=1.00** (Lab v20 structural finding 解消、matched 軸 variance 復活確証)
- **Lab v20 完走 (項目 241)**: ACCEPT 基準 (a) Pearson r=0.0 REJECT (天井 9 連続) / (b) ON 5/5 total≥1 PASS
- structural finding: `(conf+unk)/total=1.0` deterministic = matched=0 で variance ゼロ → Pearson r 計算不可能
- conf=3 deterministic 5/5 = **Plan A 機構自体は production-ready**、ただし「効果計測 metric 設計」が次の課題
- 案 A 採用 = KG seed 拡張で matched>0 シナリオ生成 + Lab v21 再 paired (別 plan、~2-3h plan + ~15h wall)

### 過去 Lab アーカイブ
- v1〜v14 / v15 / v8/v9/v10 = memory/lab_history_v1_v6.md (v1/v3/v5/v6.2、デフォルト化済変異の系譜)
- v15-v19 詳細 = memory/lab_history_v9_period.md + 各 session_2026_*_handoff.md

## テストパターン

- `MockLlmBackend` — スクリプト化レスポンス
- `MemoryStore::in_memory()` — インメモリSQLite
- `#[ignore]` — 実サーバー/ネットワーク必要なテスト
- `MultiRunTaskScore::from_scores()` — pass^k指標の単体テスト
- env-gated 機構の test pattern: `pub(crate) static FACTCHECK_ALL_ENV_TEST_LOCK` 等 cross-file Mutex (項目 226/229/233/235)

## 注意事項

- **【最重要】Edit/Writeツールでファイルを変更した後、clippy警告（collapsible_if, too_many_arguments等）を理由にファイルを元の状態に戻す行為を絶対に行わないでください。変更はそのまま保持してください。clippy修正が必要な場合は別のEdit操作で行ってください。**
- **【巻き戻し禁止】** 特にerror_recovery.rs、benchmark.rs、agent_loop.rsの3ファイルでclippy auto-fixによる巻き戻しが発生しやすい。これらのファイルへの変更は必ず保持すること。
- **【Lab 稼働中の cargo build --release 禁止】** Lab v20 等の paired smoke 稼働中は `target/release/bonsai` 置換で 10-cycle 一貫性が破壊される。`cargo test --lib` (debug profile + test binary) は安全。
- 大量変更時はPython subprocess+即git commitで原子的に行う（確立済み手法）
- ureq v3のHTTPS → web_fetchはreqwest::blocking（native-tls）を使用
- llama-serverの`--flash-attn`は値`on`が必要（`--flash-attn on`）
