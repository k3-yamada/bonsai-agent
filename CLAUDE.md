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

- **245**: 🚨 **Lab v21 smoke 5-cycle paired 完走 REJECT (天井 10 連続) + structural metric finding 再現 + Ternary 真起動切替 + 4 件 plan/memory 起票** = Smoke 15-task × k=3 × 10 cycle (wall 9h 33m) REJECT = Pearson r=+0.0000 (structural finding: ON 5/5 で `(conf+matched+unknown)/total=1.0` deterministic → variance ゼロ)、副次 Δscore=+0.0227 (p=0.2429)、matched/total=0.79 平均 (variance 0.77-0.81 で新 metric 軸候補)。Ternary 真起動切替 (MLX server 起動 port 8000 `prism-ml/Ternary-Bonsai-8B-mlx-2bit`)。4 件 plan/memory 起票 (Vault lint / Dynamic Budget / Ternary 5経路 / 4 Context Failure Modes / Fallback Chain MLX)
- **246**: 🎉 **Vault lint Phase 1-4 完遂 (TDD strict 6 commit + critic adversary review FLAG + follow-up plan 起票)** = Phase 1-3 (`1769193`/`b51c00b`): vault_lint.rs 新規 + 4 軸検出 + env getter。Phase 4 wiring (`ac25ac2`/`30a38d6`/F2 fix commit): `is_vault_lint_lab_enabled()` env gate (BONSAI_VAULT_LINT_LAB=1) + `handle_lab_mode` pre-lab sanity gate (ECC `oh-my-claudecode:architect` 助言経由 = pre-lab 1 回 emit + strict bail + 暗黙短絡) + `AuditAction::VaultLint` variant + audit_log 永続化 (action_type='vault_lint')。critic adversary review (`oh-my-claudecode:critic`): FLAG (1 MAJOR + 3 FLAG)、M1 (`try_into().unwrap_or(u64::MAX)` 安全 cast) + F2 (session_id `lab-vault-lint-{timestamp}` uniquify) 修正、F1 (bail branch test) は `vault-lint-bail-branch-test.md` (項目 251 候補) で follow-up。1294→1319 passed (+25) / clippy clean / fmt clean
- **247**: 🟡 **Lab v22 Metric Redesign Phase A-D 実装 + CCG synthesis (進行中)** = (a) Phase B (`lab_v22_metric.py`、Wilcoxon + Cohen's dz + factcheck 補助 + Pearson r 診断 + ACCEPT/REJECT 統合 + A/A mode) (b) Phase C (`BONSAI_LAB_TEMP` env、TDD strict 6 test + main.rs wiring) (c) Phase A 起動 (`lab_v22_aa_test.sh`、両側 OFF×OFF + T=0 で σ_Δ noise floor) (d) Phase D ready (`lab_v22_paired.sh`)。CCG synthesis: Pearson r 廃止 → paired Δscore + Wilcoxon + dz 主軸、smoke=10/decision=20/strict=27 cycle 推奨。補強 plan 3 件 (b/c/d) 起票済
- **248**: 🎉 **Dynamic Budget Compaction Phase 1-3 完遂 (TDD strict 2 commit、Zenn 4 ratio 配分)** = Phase 1 Red (`2546d79`): `src/agent/compaction.rs` BudgetRatios skeleton + 5 failing test。Phase 2 Green + Phase 3 Refactor (`5109219`): BudgetRatios::default = 0.4/0.3/0.2/0.1 (plan §3.1)、allocate() 4 軸按分 (余り buffer)、adjusted() new_ratio = base × (1 + (rel-0.5) × α) で動的調整、env getter SSOT (`BONSAI_DYNAMIC_BUDGET_RATIOS` 4要素 sum 1.0 / `BONSAI_DYNAMIC_BUDGET_ALPHA` 範囲 [0.0,1.0] default 0.2)。Phase 4 wiring (runtime 統合) は別 phase 候補
- **249**: 🚨 **Lab Runtime Stabilization Phase 1-3 完遂 + Phase 4 F1+F2 検証 REJECT (実機 finding 重要)** = Phase 1-3 (`5a01a45`): F1 (`BONSAI_LAB_LONG_SSE=1` で sse_chunk_timeout 60→180) + F2 (`BONSAI_LAB_MLX_ONLY=1` で fallback chain clear) + F3 (`BONSAI_LAB_TASK_LIMIT` で task pool 縮小) env-gated 実装、134 lines / 3 files。**Phase 4 F2 拡張 (今 commit)**: 実機 Smoke で F2 が fallback クリアのみで primary 未切替の構造的 bug を検出 → primary backend を `ServerBackend::MlxLm` + server_url 8000 + model_id `prism-ml/Ternary-Bonsai-8B-mlx-2bit` に切替する拡張実装。**Phase 4 Smoke G-RT REJECT**: F1+F2+T=0+SMOKE=1 (5 task × k=3) で wall **103m42s (1h43m)** = target ≤35 min を **3x 超過**、SSE timeout 5 回発火 (F1 180s でも MLX 初トークン latency catch しきれず) + 非ストリーミング fallback。**finding**: MLX 2-bit primary は llama-server 1-bit gguf より latency が高い (Ternary 精度 +5pt の対価 ~2x latency)、F1+F2 だけでは Lab v22 paired 5h 完走基準未達 = 次 phase で F4 設計 (MLX server pre-warm or non-streaming default 等) 必要

## Lab実機テスト結果 (最新のみ、詳細は memory/lab_history*.md)

### Lab 天井連続記録 (Bonsai-8B 1bit, k=3, 10 cycle paired)
| Lab | 軸 | 結果 | 数値 | 教訓 |
|---|---|---|---|---|
| v17 | ERL Heuristics Pool | REJECT | Δ=−0.0014 / p=0.5072 | 天井 7 連続 |
| v19 | Frontier (score 軸) | REJECT | Δ=+0.0072 / p=0.4262 | 天井 8 連続 |
| v19 (案 A) | Frontier (context-length 軸) | **ACCEPT** | bucket 0→1 gradient = -0.1944 (基準 1.94x) | 第 6 軸 baseline 確立 |
| v20 | KG-FactCheck (Pearson r) | **REJECT** | r=0.0 / Δ=-0.0038 / p=0.5316 / wall 19h 9m | **天井 9 連続** + structural finding (matched=0 で variance ゼロ) |
| v21 smoke | KG-FactCheck (matched 軸 + Pearson r) | **REJECT** | r=0.0 / Δ=+0.0227 / p=0.2429 / wall 9h 33m | **天井 10 連続** + structural metric 再現 (`(conf+matched+unknown)/total=1.0` deterministic、matched 軸単独なら variance 0.77-0.81 で新 metric 軸候補) |
| v22 Phase A | A/A test (項目 247 noise floor σ_Δ 採取試行) | KILL | cycle 1=78 min / cycle 2=81 min (target 30 min の 2.6x slowdown 観測 → kill) | Phase A 完走前に項目 249 で SSE timeout + fallback retry overhead を根本対処する判断、kill 12:37 |
| 249 Phase 4 Smoke G-RT | F1+F2 (long_sse + mlx_only primary 切替) Lab Runtime Stabilization 検証 | **REJECT** | wall **103m42s** (target ≤35 min を 3x 超過) / SSE timeout 5 回発火 / 非ストリーミング fallback / 5 task × k=3 | F1 (180s) でも MLX 2-bit 初トークン latency catch しきれず、F1+F2 単独では Lab v22 paired 完走基準未達 = 次 phase F4 設計 (MLX pre-warm or non-streaming default) 必要、MLX 2-bit は llama-server 1-bit gguf より ~2x latency finding |

### Bonsai-8B 能力プロファイル (LADDER + AgentFloor、項目 224)
- T1 Instruct=**0.68** / T2 SingleTool=0.52 / T3 ToolSelect=0.77 / T4 MultiStep=0.64 / T5 ErrorRecov=0.70 / **T6 LongHorizon=0.47** (weakest)
- tier-targeted 変異の優先攻略 = T6 偏向 (Lab v22+ HypothesisGenerator 改修の前提)

### Plan A 系列完結 (項目 230 → 234 → 235 → 236 → 237 → 238 → 239 → 240 → 241 → 242 → 243)
- 3 段配線: (a) 230 wiring (b) 235 trajectory scope (c) 237 event emit → G-6b で **factcheck total=5 / conflicting=3 = Bonsai-8B fabricate 検出初成功**
- 項目 242 Phase 4 G-7b 実機: **total=11 matched=8 unknown=0 conflicting=3 mean_path_len=1.00** (Lab v20 structural finding 解消、matched 軸 variance 復活確証)
- 項目 243 G-7c 実機 (input 書換後): **total=15 matched=12 unknown=0 conflicting=3 mean_path_len=1.00 / score=0.7613 / pass@k=0.8889** = matched +50% / score +40.6% 大幅改善 + 副作用解消、Lab v21 paired 起動 ready
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
