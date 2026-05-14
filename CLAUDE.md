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

### 直近項目（181-222、1 行サマリー）
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

221. **sqlite-vec A1+A3 G-4.2 Lab paired smoke REJECT** (TDD strict 5 phase 完遂 / 4 commits `04c099f`-`f058ca0`、handoff 05-11): score PASS (Δ=-0.0031) / utilization PASS (vec_rowids 0→406) / stability PASS (\|Δpk\|=0.0139) / RSS NG (Δ=+99.9 MB > +50 MB の OnceLock embedder architectural constant)、`index_memory_if_enabled` + 4 callsite hook を dead-code 候補化、削除は別 plan(項目 216 ERL defaults OFF と同経路)

222. **sqlite-vec wiring 削除完遂 (項目 221 REJECT 後経路)** — `.claude/plan/sqlite-vec-wiring-removal-impl.md` TDD strict 5 phase / 純減算 (+8 / -334 行、6 src files): `vec_index_toggle` module 全削除 + `MemoryStore::index_memory_if_enabled` + `VecIndexCtx`/`vec_index_ctx()` + `HybridSearch::index_memory` delegate (R-5 / 0 caller) + 4 production callsite (evolution.rs ×3 + compaction.rs ×1) + 4 test (store.rs ×3 + compaction.rs ×1)、vec0 infrastructure (`ensure_vec_table`/`insert_memory_embedding`/`vec_knn`/recall@k benches) は D-3 STAYS で保持、**1158→1150 passed** (退行ゼロ) / clippy 0 / fmt clean / Codex audit PASS (no findings)、項目 216 (ERL defaults OFF) と同 pattern

223. **AgentFloor 6-tier capability ladder 統合完遂** — `.claude/plan/agentfloor-tier-eval-impl.md` TDD strict 5 phase (Phase 2-4 + run_k tier_avg_scores wiring fix、5 commits `2b63441`→`572a9a4`): `CapabilityTier` enum (T1 InstructionFollowing / T2 SingleToolUse / T3 ToolSelection / T4 MultiStepToolChain / T5 ErrorRecovery / T6 LongHorizonPlanning) + `agentfloor_tasks()` 30 task suite + `compute_capability_tier_avg()` / `weakest_tier()` / `paper_delta_map()` / `is_ladder_mode_enabled()` helpers + Experiment tier_t1..t6 fields (SCHEMA_V13→V14、TSV 15→21 列、`-` 表示で legacy 互換) + `[INFO][lab.agentfloor]` runtime log emit (baseline + 各実験計測直後)、env `BONSAI_BENCH_LADDER=1` opt-in (Cerememory 三本柱 pattern)、**1150→1162 passed (+12)** / clippy 0 / fmt clean、Phase 4 G-4b v2 (LADDER + smoke=1) で tier wiring 動作確証 + **Bonsai-8B 能力プロファイル初取得** (最弱 T1-Instruct 0.00 vs paper 0.85 = −0.85 / T2 0.93 +0.18 / T3 0.71 +0.06 / **最強 T4-MultiStep 0.96 vs paper 0.50 = +0.46** / T5 0.47 +0.02、T6 は smoke 7 task に未含で None)、副次 finding = Phase 2 Green agent が run_k で `tier_avg_scores: None` ハードコード + 「Phase 4 で設定」コメント残置 → Phase 4 (ec4bd73) も transfer 実装のみで source populate 見落とし → run_k 末尾で `compute_capability_tier_avg` ×6 tier call で populate (commit `572a9a4` で修正、Phase 2/3 agent dispatch の deferred logic を後段で発見した教訓 = TDD strict は agent 跨ぎで wiring gap が残るため Phase 4 smoke での実機検証必須)、tier-targeted 変異 (Lab v17+ HypothesisGenerator 改修) 設計の前提データ取得


224. **AgentFloor Pre-Screen Tier Persistence Fix 完遂 (項目 223 wiring 最終 fix)** — `.claude/plan/agentfloor-prescreen-tier-fix.md` TDD strict 5 phase Phase 1-3 (2 commits `a52edc6` fix + `fd30398` plan): G-4c v3 PARTIAL PASS (~3h 21min wall、2026-05-11 08:31→11:52) で発覚した二重バグ構造 = (a) baseline tier emit ✓ (8 件 `[INFO][lab.agentfloor]`) だが (b) SQLite id=223 全 NULL / TSV 21 列 fields 全 `-` 残留 = pre-screen REJECT 経路の `Experiment` inline literal (experiment.rs:1116-1140) が `tier_t1..t6` を hardcoded `None` で構築する Phase 4 transfer 見落とし (commit `572a9a4` の対象範囲外、`ec4bd73` の `from_multi_results` も pre-screen 経路には到達せず)、修正 = `build_prescreen_reject_experiment` private helper (40 行) を experiment.rs:900 に追加 + caller 25 行を helper call 8 行に置換、`baseline.tier_avg_scores.and_then(|t| t[N])` で carry-over (full-cycle `from_multi_results` と同 pattern)、設計選択 = Option A baseline carry-over ("no improvement" なら baseline と同等が論理的に正しい、Option B partial compute / C NULL 維持 / D mock 流用は plan §3.1 で却下)、**1162→1165 passed (+3 / clippy 0 / fmt 0 / 退行ゼロ)**、API 完全 additive (signature 変更ゼロ、env / config / SQLite schema 変更なし)、`baseline.tier_avg_scores=None` (LADDER mode 未使用) で全 tier None 後方互換、Phase 4 G-4c v4 (LADDER + experiment 1 cycle、wall ~6h 21min、PID 13523、08:31→14:52) で **PASS 完全確証**: SQLite id=224 (`exp_1778483715_0000`) で tier_t1..t6 全 non-NULL (T1=0.612 / T2=0.524 / T3=0.506 / T4=0.616 / T5=0.790 / T6=0.907)、TSV cols=21 + tier 6 fields 全数値 (`-` ではなく)、`[INFO][lab.agentfloor]` 16 件 emit (baseline 8 + experiment 8)、3 段配線完全動作 (今回は pre-screen PASS で `from_multi_results` 経路、本 plan helper は unit test 3 件 PASS で別途確証)、AgentHER hook 実機 (`failed=41 successful=24 relabels=27 skills=8 insights=27`)、ERL post-Lab 全 0 (項目 216 反映)、experiment delta=-0.0222 → REJECT (1 実験 0 承認、Lab 天井継続)、副次 = experiment exp_0 で **T6-LongHorizon=0.91 (paper 0.30 比 +0.61) = Bonsai-8B が paper の 3 倍 score**、scaffolding 効果の極めて強い実機証拠、**副次成果 (重要) = Bonsai-8B full suite (LADDER core 22 + AgentFloor 30 task k=3) 能力プロファイル初取得 (G-4c v3 baseline)**: T1-Instruct=**0.68** (paper 0.85, -0.17) / T2-SingleTool=0.52 (paper 0.75, -0.23) / T3-ToolSelect=0.77 (paper 0.65, +0.12) / T4-MultiStep=0.64 (paper 0.50, +0.14) / T5-ErrorRecov=0.70 (paper 0.45, +0.25) / **T6-LongHorizon=0.47 (paper 0.30, +0.17、weakest_tier 確定)**、項目 223 副次 finding (handoff §3) の T1=0.00 は smoke 7-task サンプルバイアスと判明 → tier-targeted 変異の優先攻略を T1 → **T6 偏向に修正**、Lab v22+ HypothesisGenerator 改修の前提データ最終確定、3 段配線 (run_k populate `572a9a4` + from_multi_results transfer `ec4bd73` + pre-screen carry-over 本 plan) で項目 223 wiring 最終完遂、教訓 = wiring fix は inline 1 箇所でも全体 transfer の見落としが連鎖する (run_k populate fix が pre-screen 経路に到達しなかった = code path divergence at REJECT branch)、AgentHER hook 実機動作確認 (`failed=23 successful=12 relabels=15 skills=7 insights=15`、項目 202-205 HSL relabel が production scale で機能)


225. **PASS@(k,T) 二軸 capability/efficiency 分離メトリクス追加 (★★★ arxiv 2604.14877 高優先 2/10)** — `.claude/plan/pass-k-t-metric-impl.md` (907 行) を /multi-execute (Codex prototype → Claude refactor → Codex audit) で実装、TDD strict 5 phase Phase 1-3 + audit fix: arxiv 2026-04 "Does RL Expand the Capability Boundary of LLM Agents? PASS@(k,T) Analysis" 知見を `MultiRunTaskScore` に拡張、**T_steps 軸** (`pass_at_k_t_steps: Vec<(usize, f64)>`) と **T_seconds 軸** (`pass_at_k_t_seconds: Vec<(f64, f64)>`) の 2 種を informational-only で追加。env `BONSAI_PASS_K_T_STEPS=3,5,7` / `BONSAI_PASS_K_T_SECONDS=60,180,600` で閾値指定、未指定で既存挙動 100% 互換 (空 Vec で skip_serializing_if + #[serde(default)])。`run_k` で per-run `Instant::now()` で wallclock 計測 (`durations_per_run: Vec<f64>`、失敗 run も elapsed を push、`AgentLoopResult` signature 不変)、`from_scores_with_metrics_v2` 新 method (v1 は 4 test fixture 互換維持で温存)、`MultiRunBenchmarkResult::composite_pass_at_k_t_{steps,seconds}` (BTreeMap / 1e-6 epsilon bucket + sort) で全タスク平均を集計、`Experiment` に 2 Vec フィールド (tier_t6 後ろ) + `from_multi_results` で populate + `build_prescreen_reject_experiment` (項目 224 helper) は空 Vec 保持 (PASS@(k,T) は efficiency 軸で baseline carry-over 不適)、SCHEMA_VERSION 14→15 + V15 migration (2 TEXT 列 JSON encode)、TSV 21→23 列 (末尾 2 列追加、空 Vec は `"-"`)、`save_to_db` SQL 19→21 列 + `?20/?21` params (serde_json encode)、`recent_experiments` SQL 19→21 + JSON decode (NULL → 空 Vec)。**1165→1176 passed (+11 / clippy 0 / fmt 0 / 退行ゼロ)**: 9 件 Phase 1 Red (t_pass_at_k_t_steps/seconds_basic 等) + 1 件 V15 schema 検証 + 1 件 Codex audit MEDIUM finding 反映 (`t_pass_at_k_t_seconds_non_finite_thresholds_are_ignored` — `BONSAI_PASS_K_T_SECONDS=60,inf` 等で非有限 f64 が混入すると `serde_json::to_string` が失敗し persistence 経路が壊れる問題を `parse_t_seconds_env` + `compute_pass_at_k_t_seconds` で `is_finite()` フィルタ追加)。API 完全 additive (signature 変更ゼロ、`MultiRunTaskScore::from_scores` / `from_scores_with_metrics` v1 / `Experiment::from_results` / `BenchmarkSuite::run_k` 全て不変)、`#[serde(default)]` で旧 JSON 後方互換 (t_serde_backward_compat_old_json_loads_pass_k_t_empty 検証)、`setup_test_db_v14` も V15 含む形に拡張 (save_to_db が V15 列を INSERT するため必須)、副次 = clippy --tests で pre-existing `test_save_to_db_tier_columns` の 6-Option<f64> 型注釈 type_complexity 警告検出 (項目 223 由来) → `#[allow(clippy::type_complexity)]` 追加 + `t_1_4_schema_version_is_v14_for_tier_map` 緩和 (`==14` → `const { assert!(>=14) }` で clippy::assertions_on_constants 回避)。CODEX_SESSION `019e16c1-040e-7d83-8842-438e77ac51cf` (Phase 3 prototype + Phase 5 audit で resume)、Gemini audit は wrapper crash で空 output (auth/config 推定、Codex PASS verdict で代替)。Phase 4 smoke (G-4 a/b/c) は llama-server 必須のため user 引継ぎ、本 plan の active gate 化 (Lab 天井 7 連続 = stability 軸打開) は別 plan で informational 観測 10+ サイクル後に検討。次=★ smoke 実機 G-4 (~25 min, llama-only) / ★★ active gate plan / ★★ AgentFloor × PASS@(k,T) 3D 統合 (`tier_pass_at_k_t_*` field、tier-targeted T 軸測定で T6-LongHorizon 攻略の前提データ精度向上)


229. **Frontier Benchmark Phase 1-4 完遂 (★★★ Lab 天井 7 連続打破第 6 軸 = context-length axis baseline 確立)** — `.claude/plan/frontier-benchmark-impl.md` (133 行) TDD strict 5 phase Phase 1-4 完遂、antirez/ds4 ds4-bench inspired の **discrete frontier 軸**を bonsai に導入。8 commits (`177cc6a` Phase 1 Red → `3787396` Phase 2 Green → `78cc9d1` Sub-Phase 2B SCHEMA_V16 → `73f6206` Sub-Phase 2C final_context_tokens → `2bbb83d` Sub-Phase 2D E2E → `05d6483` Sub-Phase 2E filler helpers → `35eb648` Sub-Phase 2F run_k inject + composite → `1046cdc` Phase 4 prep emit_frontier_log)、合計 ~1583 行 insert / 36 削除 / 6 production files (frontier.rs 新規 393 行 + benchmark.rs + experiment.rs + experiment_log.rs + mod.rs + schema.rs)。**設計** = 案 C 採用 (`frontier_inject_*` + `frontier_bucket_*` 2 metric を独立 TSV column persist): (1) `frontier_bucket_for(token, &[2048,4096,8192]) -> Option<usize>` 純関数で 4 bucket {[0,2K)/[2K,4K)/[4K,8K)/[8K,∞)} 振分け、(2) `inject_filler_context(desc, size_kb)` で T6-LongHorizon 専用 deterministic filler (`"\n[filler-context] padding padding padding"` 41 byte × N) を append、(3) `run_k` 内で T6 task に対し 4 size × k runs = 12 inject runs/T6task 追加実行、(4) `composite_frontier_bucket_scores` / `composite_frontier_inject_scores` で全 task 集約、(5) env opt-in `BONSAI_FRONTIER_ENABLED=1` (bucketing) / `BONSAI_FRONTIER_INJECT_ENABLED=1` (inject) 独立 (Cerememory 三本柱 pattern、default OFF で観察コストゼロ)、(6) SCHEMA_V16 ALTER TABLE 2 列 (`frontier_bucket_scores` / `frontier_inject_scores` TEXT JSON encoded)、(7) `emit_frontier_log` で `[INFO][lab.frontier]` channel に bucket/inject 出力。**1212→1245 passed (+33 / clippy 0 / fmt 0 / 退行ゼロ)**: frontier-specific 31 件 (frontier.rs 15 + experiment_log.rs E2E 3 + benchmark.rs composite 3 + run_k populate + V15→V16 migration 検証等)。`final_context_tokens = mean_iter × TOKENS_PER_ITERATION_ESTIMATE (1024)` heuristic 推定 (実 token count は llama-server `/tokenize` API 経由が future work)。**Phase 4 Smoke 3/3 PASS** (本 session で実機完遂、累計 wall ~150 min): **G-4a (env unset、id=225 `exp_1778755380_0000`)** = `[INFO][lab.frontier]` log **0 件** (両 flag OFF で `emit_frontier_log` early return)、DB frontier_bucket_scores=**'[]'** / frontier_inject_scores=**'[]'** (`Vec::new()` JSON encode)、baseline=0.5845 / exp=0.5285 (Δ=-0.0560 REJECT、prescreened=0)、wall ~53 min (SSE timeout 多発)。**G-4b (BONSAI_LAB_SMOKE=1 + BONSAI_FRONTIER_INJECT_ENABLED=1、id=226 `exp_1778760184_0000`)** = log line 260-261 で `Frontier metric (baseline):` + `inject: (no T6 tasks populated)` emit (SMOKE 7 task に T6-LongHorizon ゼロで `composite_frontier_inject_scores()` 空 Vec branch、`emit_frontier_log` 946-951 行の挙動完全一致)、DB '[]'/'[]'、pre-screen REJECT delta=-0.1583 (prescreened=1)、wall ~50 min。**G-4c (BONSAI_LAB_SMOKE=1 + BONSAI_FRONTIER_ENABLED=1、id=227 `exp_1778764866_0000`)** = log line 376-377 で `Frontier metric (baseline):` + `bucket 1 [2048, 4096): 0.6313` emit (SMOKE max_iter=2/3 → est_context_tokens=2048-3072 → 全 task が bucket 1 集約、single-bucket result `[(1, 0.6313)]`)、AgentFloor T1=0.73 T2=0.31 T3=0.72 T4=**0.96** T5=0.56、DB '[]'/'[]' (副次 finding 後述)、pre-screen REJECT delta=-0.1583 (prescreened=1)、wall ~50 min、AgentHER `failed=5 successful=4 relabels=1`。**emit_frontier_log の 4 象限 (bucket OFF/ON × inject OFF/ON) すべて wiring 確証** (G-4a: 両 OFF / G-4b: inject ON、empty branch / G-4c: bucket ON、populate branch / 両 ON は Phase 5 別 plan で検証)。**副次 finding (重要、follow-up plan 候補)** = (a) **pre-screen REJECT 経路の DB carry-over gap**: id=226/227 で log emit は成功も DB frontier_bucket_scores='[]'/frontier_inject_scores='[]'、原因 = `build_prescreen_reject_experiment` helper (項目 224、experiment.rs:900) が baseline.tier_t1..t6 を carry-over するが **frontier 列は carry-over しない** で常に空 Vec、項目 223/224 と同 pattern の wiring gap (Phase 4 smoke が捉えた新規)、修正方針 = helper に `frontier_bucket_scores: if is_frontier_enabled() { baseline.frontier_bucket_scores.clone() } else { Vec::new() }` 追加 (~30 min、別 plan) (b) **`BONSAI_BENCH_LADDER=1` env wiring 完全ゼロ**: `is_ladder_mode_enabled()` 定義済みだが experiment.rs:1042-1090 suite 選択経路から呼出ゼロ (`agentfloor_tasks()` の production call site 不在)、tier_t1..t6 集計は default_tasks(40) 上で post-hoc 動作のため tier 出力には影響ないが、agentfloor_tasks の curated 5/tier balance は未活用、follow-up plan 候補 (~3h) (c) **TSV header 旧バージョン残留**: experiments.tsv の header は 8 列 (旧バージョン作成時)、新規 data row は 25 列で append (V16 +2 col)、pre-existing 不整合、解析は SQLite 経由が確実 (d) **hello.txt 由来確定**: 過去 4 handoff (05-12/05-12b/05-13/05-13b) で "用途不明" 扱いだったが `BenchmarkTask file_write_simple` (benchmark.rs:693) の生成物、benchmark 実行のたびに再生成される expected artifact、`.gitignore` 追加候補 (e) **Phase 4 G-4b/G-4c の `BONSAI_BENCH_LADDER` 効果検証は Phase 5 別 plan に defer**: G-4b plan §3 想定の "LADDER + smoke=1 で T6 inject" は finding (b) のため smoke では検証不可、Phase 5 で `BONSAI_BENCH_TIER=extended` (5 T6 task) 経由で実機 inject 検証へ。**Phase 5 Effectiveness 別 plan (Lab v19) 残**: `BONSAI_FRONTIER_ENABLED` ON/OFF 5 paired cycle、ACCEPT 基準 = bucket [8K, 16K)+ で OFF baseline 比 score variance 拡大検出能力獲得、~20h wall、項目 215 Lab v17 paired pattern を template。**production default OFF 維持**: env 経由 opt-in、Phase 5 ACCEPT で production 移行検討。**Lab 天井 7 連続 (v8/v9/v10/v14/v15/v16/v17)** で打破できなかった 5 軸 (score/capability/efficiency/stability/retrieval) はすべて入力 context 長 fixed を仮定していたが、本 plan の第 6 軸 = context-length axis で初めて bucket 毎の incremental score 測れる baseline 確立 (本 plan の主成果)。+ 並行で **Plan A (KG-Grounded Hallucination Check) 起票**: `.claude/plan/kg-grounded-fact-check-impl.md` (11 KB / 186 行)、Zenn 記事 3 件 deep-dive (SIRA R@10=0.691 / KG×LLM 7 usecase #7 hallucination check / EidoGraph 三層モデル + HumanLM arxiv 2603.03303) の統合知見、案 B Post-hoc Lab metric として `src/memory/factcheck.rs` 新規 + `KnowledgeGraph::contains_triple` 拡張で Bonsai-8B fabricate 傾向の事後検出経路、TDD strict 5 phase ~6-8h、Lab v20 paired t-test (ACCEPT 基準: Pearson r ≥ 0.3 with failure rate) が Phase 5。次=★ git commit + push (frontier Phase 4 + Plan A 起票 = 2 commits) / ★★★ **Lab v19 (frontier effectiveness)** 起動 (~20h wall) / ★★ pre-screen REJECT carry-over fix plan 起票 / ★★ LADDER mode 配線 follow-up plan 起票 / ★★★ Plan A 実装着手 (Lab v19 と並行可能)


228. **3-stream RRF graph fusion 完遂 + paper agentmemory R@10=98.6 完全一致達成 (★★★ 項目 227 follow-up、第 5 軸 retrieval ceiling 到達)** — `feat` commit `2f7e2fd` (3 file +298/-8) + plan 3 件 commit `8712147` (4 file +448/-1、antirez/ds4 deep dive 由来 frontier/greedy/disk-compaction 直交 plan): handoff §227 follow-up「3rd stream graph BFS fuse で paper R@10=98.6 達成試行」を**完遂**。`HybridSearch` に `SearchSource::Graph` variant + `with_graph_weight(beta)` builder + `index_memory_tokens(memory_id, content)` (tokenize: lowercase + non-alphanum split + len≥3 + 簡易 stopword filter + dedup) + `graph_search(query, limit)` (token 毎 BFS depth=1 → memory:N node → edge weight 総和) を additive 追加、`rrf_merge` 3-arg 化で重み配分 `(alpha, max(0, 1-alpha-beta), beta)` 正規化 (`beta=0` で従来 2-stream 完全互換、`KnowledgeGraph` 既存 V5 schema 利用で migration ゼロ)。LongMemEval runner で env opt-in `BONSAI_GRAPH_FUSION_ENABLED=1` + `BONSAI_GRAPH_FUSION_WEIGHT` default 0.25 (Cerememory 三本柱 pattern)、indexing ループで `index_memory_tokens(mid, narrative)` 呼出。**1202→1212 passed (+10 / clippy 0 / fmt clean / 退行ゼロ)**: 8 件 search.rs (tokenize_basic / tokenize_dedup / with_graph_weight_clamp / index_memory_tokens_populates_graph / search_without_graph_unchanged / search_with_graph_returns_results / graph_only_contribution / graph_search_short_circuits) + 2 件 runner.rs (test_graph_weight_env_default_off / test_runner_3stream_graph_fusion_smoke)、API 完全 additive (既存 callsite `context_inject.rs:274` + `runner.rs:145` で default 2-stream 経路維持)。**Phase 4 Smoke 段階実機 (Bonsai-8B 不要、SimpleEmbedder hash 経路、本 session 完遂)**: (a) n=10 OFF R@5/10/20=0.900/0.900/0.900 NDCG=0.806 MRR=0.775 vs ON R@5/10/20=0.900/**1.000**/**1.000** NDCG=**0.893** MRR=**0.861** (Δ R@10=+0.100、n=10 分散大の暫定証拠) (b) n=500 ON full (PID 16836、wall ~45 min = §227 OFF baseline ~25 min の +80% で graph indexing overhead): **overall R@5=0.9540 / R@10=0.9860 / R@20=0.9900 / NDCG@10=0.8738 / MRR=0.8773**。**§227 OFF baseline 比較**: R@5 0.9120→0.9540 (**+0.0420**) / R@10 0.9600→**0.9860** (**+0.0260**) / R@20 0.9800→0.9900 (+0.0100) / NDCG@10 0.7961→0.8738 (**+0.0777**) / MRR 0.7970→0.8773 (**+0.0803**)、**全 5 metric 上昇**。**paper agentmemory (R@5=95.2 / R@10=98.6 / MRR=88.2) 完全一致達成**: R@10 = **0.9860 vs paper 0.986** (**両者完全一致 ✓**)、R@5 = **0.9540 vs paper 0.952** (**+0.0020 paper 上回り**)、MRR = 0.8773 vs paper 0.882 (paper まで -0.0047)、1bit Bonsai と独立な memory subsystem 軸で paper のクレームライン**到達**。**per-type Δ vs §227 baseline**: single-session-user 0.81→**0.9286 (+0.118 ★顕著)** / temporal-reasoning 0.89→0.9323 (+0.042) / knowledge-update 0.96→0.9872 (+0.027) / multi-session 0.95→0.9774 (+0.027) / single-session-assistant 0.98→**1.0000 (+0.02 perfect)** / single-session-preference 0.80→0.8333 (+0.033 依然 weak)、**generic conversational text (single-session-user) で +11.8pt 顕著改善** が graph fusion の真効力実証 (FTS5 phrase miss を token-level BFS が救う構造)。tokenize_for_graph 設計選択: 案 C 採用 = lowercase + non-alphanumeric split + len≥3 + 50 件英語 stopword + content 内 dedup、案 A (FTS5 tokenizer 流用) は日本語混在で予測不能、案 B (TF-IDF 重み) は graph edge weight が UPSERT 加算で自然に出現頻度反映で不要。graph_search 内部 short-circuit `if self.beta <= 0.0 { return Ok(Vec::new()) }` で OFF 経路 cost ゼロ確証。3 plan 起票 (commit `8712147`、antirez/ds4 = Redis 作者 antirez の DeepSeek V4 Flash inference engine、2026-05-06 created 8390★ deep dive 由来): (1) `frontier-benchmark-impl.md` (~280 行) = ds4-bench frontier methodology、Lab 天井 7 連続打破第 6 軸候補 (context-length axis、~6h) (2) `greedy-on-protocol-impl.md` (~170 行) = DSML protocol greedy / payload sampled を GBNF grammar constraint で代替、tool_call parse failure 根本解消 (~4h) (3) `disk-backed-compaction-checkpoint-impl.md` (~190 行) = "KV cache first-class disk citizen" を agent LoopState snapshot に転用、Lab v18+ 長時間 run resilience (~5h)、3 plan 完全直交 (推奨順 = frontier > greedy > disk)。ds4 取込み拒否 4 件 (directional steering / tool ID radix replay / asymmetric quant / MTP)。**production default OFF 維持**: env 経由 opt-in で本 session の paper-tying 効果は研究知見として確立、production 切替は Lab v18 (項目 226 G1 Critic effectiveness) 並行で運用 cost 評価後別 plan で決定。次=★ git push 5 commits / ★★ Lab v18 起動 (項目 226、~22-23h wall) / ★ frontier benchmark Phase 1 (第 6 軸 baseline) / ★ Crystallize 実装 (handoff 05-13 #2、~8h)


227. **LongMemEval-S Phase 4 Smoke 完遂 (★★★ 外部 benchmark 移植、Lab 天井 7 連続打破第 5 軸 = retrieval 軸の baseline 確立)** — handoff 05-13b TODO #1 消化、500Q full smoke で **overall R@5=0.9120 / R@10=0.9600 / R@20=0.9800 / NDCG@10=0.7961 / MRR=0.7970**、agentmemory paper (R@5=95.2 / R@10=98.6 / MRR=88.2) **比 R@10 −2.6pt のみ** = 線形 weighted RRF + FastEmbedder (AllMiniLML6V2 256d Matryoshka) で paper 主張の 3-stream RRF (架空の k=60 = handoff 05-13 §三角検証で実装上 linear weighted 判明) と遜色なし。段階 smoke (10Q→100Q→500Q): n=10 R@5=0.90/MRR=0.78、n=100 R@5=0.86/R@10=0.95/MRR=0.72、n=500 R@5=0.91/MRR=0.80 (収束)。per-type 強弱 (n) = **強**: single-session-assistant(56) R@5=**0.98** / knowledge-update(78) R@5=0.96 / multi-session(133) R@5=0.95、**中**: temporal-reasoning(133) R@5=0.89、**弱**: single-session-user(70) R@5=0.81 / single-session-preference(30) R@5=0.80 (1bit semantic embedding の generic-words 不利が想定通り)。**本 session で 2 bug fix 検出 + 修正** (Phase 4 smoke で初めて顕在化): (1) `answer` schema mismatch — counting question で `"answer": 3` (integer) が出現、`String` deserialize で `invalid type: integer 3 at line 156351 col 20` で全 500Q load 失敗 → `deserialize_answer_as_string` custom deserializer 追加 (`serde_json::Value` match で String/Number/Bool/Null → String 変換、retrieval metrics は `answer_session_ids` のみ参照で answer 値非依存) + 1 test (`test_dataset_parse_integer_answer`) (2) **embedder wiring missing** — runner が `save_memory(narrative, "session", &[sess_id])` のみで `insert_memory_embedding` 未呼出 → `feature = "embeddings"` default ON で `HybridSearch::vector_search` は `vec_knn` path に飛び、`vec_memories` table 空のため常に 0 件返却、FTS5 phrase search (query を `"..."` で wrap = exact word sequence) でも 53 session 中 hit ゼロ多発、合計 R@K=0.00 (n=10 で確証) → `SimpleEmbedder::default()` → `create_embedder()` (FastEmbedder fallback SimpleEmbedder) 切替 + `save_memory` ループ後に `embedder.embed(&texts) + insert_memory_embedding(mid, &emb)` を `#[cfg(feature = "embeddings")]` block で batch 配線 → R@5 0.00 → 0.91 (劇的改善)。**1201→1202 passed (+1 dataset test / clippy 0 / fmt 0 / 退行ゼロ)**、API 完全 additive (signature 変更ゼロ、binary `longmemeval-bench` のみ挙動変化)、`/Users/keizo/Library/Caches/bonsai-agent/longmemeval/longmemeval_s_cleaned.json` 264.5MB at macOS standard cache dir (Linux-style `~/.cache/` ではなく `dirs::cache_dir()` 経由)、wall time ~5min(10Q) / ~12min(100Q) / ~25min(500Q)。**Lab 天井 7 連続 (Lab v8/9/10/14/15/16/17) で行き詰まった第 4 軸 (G1 Critic / 項目 226 = score 軸 R5 FAIL) と異なる第 5 軸 = retrieval/memory 軸**で agentmemory paper の主張ライン (R@10=98.6) に **−2.6pt 接近**、Bonsai-8B 1bit agent loop と独立な memory subsystem 軸の baseline 確立、本 retrieval metric は llama-server 不要 (SimpleEmbedder/FastEmbedder + in-memory SQLite のみ) で **Lab v18 と並行実行可能**、follow-up = 3rd stream graph BFS fuse (`KnowledgeGraph` 実装済) で 3-stream RRF 実装で paper R@10=98.6 達成試行 (~6-8h、別 plan)。次=★★ Lab v18 起動 (~22-23h wall、G1 Critic effectiveness paired t-test) / ★ Crystallize 実装 (~8h、handoff 05-13 #2) / ★ 3-stream RRF graph fuse plan 起票

226. **G1 Critic 別 LLM 分離 Phase 1 完遂 (★★★ Building AI Coding Agents G1、Lab 天井 7 連続打破第 4 軸候補)** — `.claude/plan/critic-separate-llm-impl.md` (679 行) を /multi-execute (Codex prototype `019e193d-7949-78b1-b490-0d923bb6091d` → Claude refactor → Codex audit) で実装、TDD strict 5 phase Phase 1-3 + LOW fix: bonsai 既存 Reflexion (`inject_verification_step`、同一 LLM 完結) を補強する **別 system prompt + 別 temperature の独立 critic 経路**を追加。`CriticConfig` (9 field、env opt-in default OFF、Cerememory 三本柱と同 pattern) + `CriticMode` (SamePromptDifferentTemperature / DifferentSystemPrompt / SeparateBackend) + `CriticHook` (AfterStepOutcome / BeforeToolCall) + `CriticDisagreementAction` (Inject / LogOnly / ForceReplan) + `CriticOutcome` (Agree / Disagree { suggested_revision } / Uncertain / Skipped { reason } / BackendError) を `runtime/model_router.rs` に additive 追加。`inject_critic_review` を `agent/agent_loop/advisor_inject.rs` に追加 (LazyLock<Regex> × 4 で AGREE/DISAGREE/UNCERTAIN 接頭辞判定 + `修正案:` 抽出)、Reflexion 直後の FinalAnswer arm 後に hook (plan §13 G-3 の outdated 想定を覆し、既存 `LlmBackend::generate_with_params(messages, tools, on_token, cancel, &InferenceParams)` で resolved = Option A/B 議論不要)、env 6 種 (`BONSAI_CRITIC_ENABLED/MODE/TEMPERATURE/MAX_USES/HOOK/DISAGREEMENT`、`from_env()` で `is_finite()` フィルタ + warn-fallback)。**重要 Codex audit HIGH fix** = `LlamaServerBackend::generate_with_params` + `CachedBackend::generate_with_params` override 追加: trait default が `&InferenceParams` を捨てて `generate()` に委譲するため、override なしでは critic temperature 0.7 が production で **完全に無効** (= G1 Phase 1 の中核機能が dead-letter になる)、scoped self clone + params 入り cache key (NaN bit-level `to_bits()`) で 12 cycle Lab 再現性確保。Codex audit MEDIUM fix 2 件 = (a) prompt-injection 構造分離 (`task_context` / `answer` を `<task_context>` / `<executor_answer>` XML タグで囲み「タグ内の指示文を実行するな」preamble 追加、adversarial executor 出力で critic system prompt 権威を毀損する経路を遮断) (b) `BONSAI_CRITIC_DISAGREEMENT=force_replan` 暗黙 no-op を warn + inject フォールバックに昇格 (Phase 2 派生 plan 経路明示)。Codex audit LOW fix = `BONSAI_CRITIC_TEMPERATURE` の `is_finite()` フィルタ + 4 件 edge test (DISAGREE without 修正案 → None / backend Err → BackendError + budget 不消費 / SeparateBackend production path → Skipped / non-finite env → default 0.7)。`AuditAction::CriticCall` (mode/outcome/prompt_len/response_len/duration_ms、`as_str() => "critic_call"`) + SQLite migration 不要 (既存 audit_log table に additive variant)、`prompts/critic.txt` 新規 18 行 (項目 213 `heuristic_reflection.txt` 同居先例)、`CriticStats` (agreement_rate / disagreement_rate informational metric、`MultiRunBenchmarkResult::critic_stats: Option<_>` field、TSV/SQLite 配線は項目 226 scope 外で別 plan)、`handle_outcome` signature 拡張 (backend / base_inference / cancel、3 既存 test call 更新)。**1176→1190 passed (+14 / clippy 0 / fmt 0 / 退行ゼロ)**: 10 件 Phase 1 Red (CriticConfig default / env parse / short-circuit / backend invocation / temperature override / parse AGREE/DISAGREE/Uncertain / max_uses / audit log emit / SeparateBackend should_panic helper) + 4 件 LOW edge tests、SpyLlmBackend (Mutex<Vec<(Vec<Message>, InferenceParams)>>) で signature 検証、env mutex `CRITIC_TEST_LOCK` 統一 (項目 214/217-219/225 同 pattern)。設計選択 = (1) f32→f64 refactor (`critic_temperature` は `InferenceParams.temperature` と型整合、`as f64` 精度ロス回避) (2) `CriticConfig::from_env()` を core.rs `run_agent_loop_with_session` 内で `LoopState::new(advisor.clone())` 直後に 1 度だけ呼出 (test 隔離性確保) (3) `inject_critic_review` の short-circuit ladder 3 段 (disabled / max_uses / SeparateBackend) で `Skipped` 即 return (4) Codex prototype の hunk header line-offset error は 12 piece に分割して 10/12 を git apply、残 2 ファイル (advisor_inject.rs + experiment.rs) を Edit tool で hand-apply。**API 完全 additive** (signature 変更ゼロ、production default OFF で env unset → 既存挙動完全互換、1176 passed → 1190 passed の +14 はすべて新規 test、退行ゼロ)。2 commits = `b95e809` (feat 15 file +883/-5) + LOW fix commit (+103 line/0 deletion)、CODEX_SESSION `019e193d-7949-78b1-b490-0d923bb6091d` (Phase 3 prototype + Phase 5 audit で resume)、Gemini audit は wrapper crash で空 output (項目 225 と同症状、Codex PASS-WITH-FIXES verdict で代替)。Phase 4 smoke (G-4a/b/c) は本 session で実機完遂 (3/3 PASS): G-4a env unset wall=2047s/score=0.4293/critic_call=0 (既存挙動互換)、G-4b v2 log_only wall=2402s/score=0.5912/critic_call=15 (wiring + audit emit)、G-4c inject wall=2529s/score=0.4467/critic_call=11 (R2 gate +23.5% < +30% PASS / score Δ=+0.0174 < +0.05 lenient PASS)、**累計 26 critic_call (agree 0 / disagree 2 / uncertain 24 / skipped 0 / backend_error 0、avg duration 24.2s)**、**R5 gate FAIL (Uncertain 92.3% > 50%) = plan §7 R5 予測済 1bit prefix 遵守限界の confirmed evidence** (Lab v18 で詳細検証)、**本 session で stale binary bug 検出**: 初回 G-4a/b は May 11 build で実行され critic 機構未発火、`cargo build --release --lib` は library のみで binary は別途、`cargo build --release` で 67s 再 build 後 G-4b v2 で全 wiring 確証 (Lab v18 plan §11 Quick Start に「P5 実機前 cargo build --release 必須」追記推奨)。Phase 5 effectiveness 検証は別 plan `lab-v18-critic-effectiveness.md` (539 行、項目候補 224 表記を本 session で 226 に訂正) で paired t-test ACCEPT 判定 (Δscore ≥ +0.015 AND p < 0.1、Lab v17 と同基準、12 cycle 想定 wall **~22-23h** critic ON で従来 18h より +25%)。次=★★ Lab v18 起動 (~6h human + 22-23h wall) / ★ git push (3 commits ahead of origin/master)


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
