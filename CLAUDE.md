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

p^n問題（ステップ蓄積による失敗確率指数的増大）への対策。**項目 1-219 は [memory/harness_patterns_archive.md](../../.claude/projects/-Users-keizo-bonsai-agent/memory/harness_patterns_archive.md) にアーカイブ**（2026-05-07 分離 / 202-219 を 2026-05-10 追加）。

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

### 直近項目（181-220、1 行サマリー）
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
202. AgentHER runtime 組込（Lab 末尾 hook、HindsightSummary、1055 passed）+ Phase 4 で event flow disconnect 発見（Phase 5 化）
203. AgentHER event flow 修復（Option B export hook + scoping、5 commits、1057 passed、smoke `failed=3 successful=3` PASS）
204. AgentHER HSL relabel 実機実証（mixed-success task 追加、smoke `relabels=1 skills=1`、HSL 真効力初実証）+ Option A 移行 plan 起票
205. AgentHER Option A 移行完遂（`Option<&MemoryStore>`→必須化、3 commits、-107 行、smoke score=0.7344 = +6.7% vs 05-07i baseline）
206. handoff TODO 消化（reset rename + current_max_id helper、+2 tests = 1058）+ skills=0 真因解析 = `skill.rs:181` dedup deterministic 確認
207. **Lab v15 long run**（89 min, core 22 / k=3 / `--lab-experiments=3`）= baseline=**0.7812** Zone A 突入、3 実験全 pre-screen REJECT、**天井 5 連続確定**
208. arxiv 構造変異 plan 3 件並列起票（ERL Heuristics / AgentFloor 6-tier / Self-Verify Dilemma、~1130 行、planning-only、推奨順 Self-Verify→ERL→AgentFloor）
209. EventRepository trait 化完遂（10 method + Mock 構造体、4 commits、1064 passed、SQL parity 保証、後続 store の trait 化 template 確立）
210. Self-Verification Dilemma 動的 skip 完遂（`AdvisorConfig::dynamic_skip_threshold`、default OFF、3 commits、1075 passed）
211. Self-Verify Phase 5 Lab variant 機構（`SetAdvisorThreshold(f64)` + focus filter、+4 tests = 1079、Lab v16 effectiveness 検証経路）
212. **Lab v16 完走**（298 min、3 experiments 全 REJECT、**天井 6 連続確定**）+ smoke pre-screen 信頼性低の副次知見、production default 0.0 維持確定
213. **ERL Heuristics Pool Phase 2 Green 完遂**（commit `41b6ac3`、Codex audit 5 件反映、SCHEMA_V10、+20 tests = 1099、production default ON で empty pool no-op）
214. Lab v17 toggle 機構 Phase 1-3（`BONSAI_ERL_DISABLED=1` で ERL 全 skip、+5 tests = 1104、F10 falsifiable hypothesis 検証経路）
215. **Lab v17 完走 + REJECT 確定**（5 paired / 12 cycle / 15h 37min / mean Δ=−0.0014 / **p=0.5072**、**天井 7 連続**）+ Cerememory port plan 3 件起票
216. ERL defaults OFF 切替完遂（env rename `BONSAI_ERL_ENABLED` default false、Lab v17 REJECT 反映、3 commits、1104 passed）
217. **Cerememory power-law decay port 完遂**（`src/memory/decay.rs` 4 純関数 + SCHEMA_V11 stability 列、`BONSAI_DECAY_ENABLED` opt-in、3 commits、+12 tests = 1116）
218. **Cerememory ADR-011 ReviewState port 完遂**（`src/memory/review.rs` Strength/Freshness 構造分離 + SCHEMA_V12 9 列、`BONSAI_REVIEW_ENABLED` opt-in、3 commits、+18 tests = 1134）
219. **Phase G Working Memory Cap 完遂**（`src/agent/working_memory.rs` Miller 7±2 hard cap、`BONSAI_WORKING_CAP_ENABLED` opt-in、+9 tests = 1143、Cerememory 三本柱完成）


220. **sqlite-vec Activation Phase 0-5 完遂 (Step A 採用 + Step B Milvus Lite REJECT 確定)** — vec0 brute-force exact KNN 移行（G-4.1 3.63x speedup / G-4.2 score=0.7420 Δ=-0.0174 PASS / G-4.3 RSS 246MB / G-4.4 backfill 188ms/10K / G-4.5 recall@10=1.0000 perfect、1143→1150 passed、7 commits、handoff 05-09h 詳細）


## Lab実機テスト結果（Bonsai-8B 1bit, k=3, 10サイクル）

### v17結果（2026-05-08〜09）— 5 paired 完走、REJECT 確定、天井 7 連続
- 完走時間: **15h 37min** wall (12 cycle × 平均 75.4 min)、PID 53005 完走 2026-05-09 13:41:30
- ベースライン (warmup): warmup_1=**0.7594** (62.2 min) / warmup_2=**0.6964** (76.3 min)
- 5 paired (ON / OFF / Δ): (1) 0.7000/0.7255/−0.0255 (2) 0.7228/0.7250/−0.0022 (3) 0.7134/0.6544/**+0.0590**★ (4) 0.7156/0.6977/+0.0179 (5) 0.6518/0.7080/**−0.0562**
- **paired t-test**: ON mean=0.7007 / OFF mean=0.7021 / **Δ mean=−0.0014 / t=−0.0718 (df=4) / one-sided p=0.5072**
- **ACCEPT 判定**: (a) Δ≥+0.015 NG / (b) p<0.1 NG → **REJECT 確定** (項目 213 ERL 機構 dead-code 候補化)
- **天井 7 連続**: Lab v8/v9/v10/v14/v15/v16/v17、prompt+config+context level の 3 軸構造変異全失敗
- 副次 finding (★ 項目 200 RDC/VAF で再評価候補): ON pair 1-4 variance std≈0.010 vs OFF std≈0.034、stability 軸で ON 顕著優位
- ON 5 急落 (0.7156→0.6518): pool 成熟による陳腐化 inject 比率増加 = Cerememory ADR-011 命題の実機実証 = Plan B (freshness gate) 必要性確証
- heuristics pool 134 件 (verification 66 / failure_recovery 52 / efficiency 16)、cycle 跨ぎ蓄積健全
- production code 変更ゼロ・1104 passed 維持、3 commits (script + Plan A/B + roadmap D-G)



### v15結果（2026-05-08）— 3実験完了、項目 205 Option A 移行の長時間安定性検証 PASS
- ベースライン: score=**0.7812**, pass@k=0.8889, pass_consec=0.8750（61.6 min, llama-only `-c 16384`, **core 22 タスク**）
- handoff 05-08 smoke (0.7344) **比 +0.0468 (+6.4%)** / handoff 05-05b core 22 (0.7560) **比 +0.0252** → **Zone A (>0.78) 突入**
- 3 実験全 pre-screen REJECT: ツール思考強制(-0.1583, #47重複) / エラー分析(-0.3917) / フォールバック戦略(-0.3967, #50重複)
- 承認率: **0/3 (0%)** — **天井 5 連続確定** (v8/v9/v10/v14/v15)
- 完走時間 **89 min** (見積もり 3-4h を大幅短縮、pre-screen 効率向上効果)
- crash/panic ゼロ・退行ゼロ、production code 変更ゼロ・1058 passed 維持
- 副次知見: HypothesisGenerator が既デフォルト #47/#50 を再生成 → tried_details 54 件履歴がプロンプトレベル変異枯渇、**次の打開点 = 構造的変異** (subagent / memory / compaction)

### v1〜v14 アーカイブ（2026-04-14〜2026-04-29）
- アーカイブ済 → memory/lab_history_v1_v6.md（v8/v9/v10/v14 を 2026-05-10 追加拡張、v1/v3/v5/v6.2 は 2026-04-25 分離）
- 派生デフォルト化変異: 項目10（計画強制）/ 項目47（思考強制）/ 項目50（フォールバック戦略）/ 項目136（事実確認、v9）

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
