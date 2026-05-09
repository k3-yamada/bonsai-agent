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
203. AgentHER event flow 修復（Option B export hook + scoping、Phase 1-4 TDD strict、5 commits、1055→1057 passed）+ Phase 4 smoke 完全 PASS（failure-inducing task `smoke_failure_chain_pair` 追加、`failed=3 successful=3 / events 0→162` 実機実証、scoping snapshot で過去 cycle 遮断確認）
204. AgentHER HSL relabel 実機実証（mixed-success task `smoke_partial_success_chain` 追加、Cargo.toml + /tmp/bonsai_phase5_nonexistent.md で 1 success + 1 fail = `tool_success_rate=0.5` + 成功 subgoal 1 件、SMOKE 6→7 / default 41→42 / core 23→24 + 6 assertion 更新、1057 passed 維持 / clippy 0 / fmt 0）+ smoke 7 task k=3 実機（16.85 min, score=0.6885）で **`failed=8 successful=3 relabels=1 skills=1 insights=1`** 観測 = HSL 真効力初実証（`relabels >= 1` PASS、events 162→286 +124 で scoping 3 cycle 跨ぎ確証）、partial_success 3 sessions のうち 1 件のみ 2 file_read 完遂は 1bit variance 想定範囲、+ Phase 5 Option A 移行 plan 起票（`.claude/plan/agenther-option-a-migration.md`、pre-screen 汚染 A1-A4 比較で A3 推奨）
205. AgentHER Option A 移行完遂（`Option<&MemoryStore>` → `&MemoryStore` 必須化、TDD strict 5 phase / 3 commits、net -107 行）: Phase 1 Red = `_phase1_red_run_k_signature_typecheck` (compile-time fn-pointer 形 typecheck) で E0308 build red 確証、Phase 2 Green = `benchmark.rs::run_k` signature + body (ephemeral `MemoryStore::in_memory()` 削除 / `reset_session_data` 全 run 開始時 reset / `Some(&store)` → `Some(store)` / export_to bulk copy block 削除) + `experiment.rs` 4 caller 更新 (582/595 = `scratch_store` (in_memory) 導入で plan A3 採用 = pre-screen の persistent.events 汚染回避 / 868/1017 = `Some(store)` → `store`) + `event_store.rs::export_to` (38 行) + `test_export_to_basic` (54 行) 削除 + benchmark.rs dead import (`LogLevel`/`log_event`) cleanup、Phase 3 Refactor = `reset_session_data` docstring 警告追記 (rename は CLAUDE.md「don't refactor beyond」に基づき次回 task 化)、**1057→1056 passed (test_export_to_basic 1 削除分、退行ゼロ / clippy 0 / fmt 0)**、Phase 4 smoke 1 cycle 実機 (15.6 min, llama-server `-c 16384`) で **score=0.7344 / pass@k=0.8095 / `failed=6 successful=3 relabels=1 skills=0 insights=1`** 観測 = handoff 05-07i (Option B baseline 0.6885) **比 +0.0459 (+6.7% 改善) + duration -7.4%** で G-4 PASS (export_to bulk copy overhead 削減効果と推定、~~skills=0 は max_promote dedup の 1bit variance~~ → 項目 206 で deterministic dedup と訂正)、3 commits (`e3e5d69` Red / `122d7b7` Green / Phase 3 docstring)
206. handoff 05-08 残 TODO 2 件消化 + variance 真因解析: (1) `reset_session_data` → `reset_session_data_for_lab` rename (commit `9e3ae5e`、Lab cycle 専用意図を name で明示)、(2) `EventStore::current_max_id` helper 抽出 (commit `3f94322`、experiment.rs:855 の inline `SELECT COALESCE(MAX(id),0) FROM events` を impl に移動 + 単体 test 2 件 = empty_returns_zero / returns_max_after_appends)、**1056→1058 passed (+2 / clippy 0 / fmt 0)**。+ skills=0 真因解析 = 項目 205 で「1bit variance」と記述した skills=0 は誤りで、`skill.rs:181-188` の application-level dedup (`SELECT COUNT(*) > 0 FROM skills WHERE tool_chain = ?1`) の **deterministic 挙動**だった: 直接 SQLite 照会で skill id=4 (`hsl_Cargo.toml_と__tmp_bonsai_phase_c670`、tool_chain=`file_read -> file_read`、created_at `2026-05-07T13:32:29`) が handoff 05-07i smoke で promote 済みと確認、本日 smoke の同 tool_chain relabel は exists=true で skip → skills=0、設計通りの skill 爆発防止 (production code 修正不要、handoff も訂正)
207. **Lab v15 long run** (項目 205 Option A 移行の長時間安定性検証、core 22 / k=3 / `--lab-experiments=3`、89 min 完走): baseline=**0.7812** / pass@k=0.8889 / pass_consec=0.8750 (3699.1s = 61.6 min)、**Option A 効果実証** = handoff 05-08 smoke (0.7344) **比 +0.0468 (+6.4%)** / handoff 05-05b core 22 (0.7560) **比 +0.0252** で **Zone A (>0.78) 突入**、3 実験全 pre-screen REJECT (0% accept、見積もり 3-4h を 89 min に大幅短縮): exp_0=「ツール使用前に思考を強制」delta=-0.1583 (#47 と additive 重複) / exp_1=「エラー分析の強制」delta=-0.3917 / exp_2=「フォールバック戦略」delta=-0.3967 (#50 と additive 重複)、crash/panic ゼロ・退行ゼロ、TSV 永続化確認 (3 行追加 / `is_prescreen_reject=true`)、**天井 5 連続確定** (v8/v9/v10/v14/v15)、副次知見 = HypothesisGenerator が既デフォルト #47/#50 を再生成 (tried_details 54 件履歴がプロンプトレベル変異を枯渇させている、構造的変異 (subagent / memory / compaction 等) への移行が次の打開点)、**production code 変更ゼロ・1058 passed 維持**
208. **arxiv 構造変異 plan 3 件並列起票** (項目 207 副次知見「天井 5 連続打開 = 構造変異」を受けた multi-plan 並列、planner agent 3 並列、production code 変更ゼロ): (1) `.claude/plan/erl-heuristics-pool-impl.md` ~470 行 = arxiv 2603.24639 ERL (Gaia2 +7.8% over ReAct) を `src/memory/heuristics.rs` 新層で実装、Skill (tool_chain) / Experience (record) / Vault (rules) と独立な「自然言語 heuristics 層」を Reflexion 由来で蓄積、Lab cycle 開始時 system prompt に top-K 注入 (TDD strict 5 phase / SQLite V10 / +10 tests / 主要 risk = R1 重複爆発 = fingerprint dedup で軽減 / ~8h ≈ 1 day)、(2) `.claude/plan/agentfloor-tier-eval-impl.md` ~390 行 = arxiv 2605.00334 AgentFloor 6-tier (T1 InstructionFollowing → T6 LongHorizonPlanning) を Lab v16 評価軸として統合、既存 Tier::Core/Extended (項目 172) と直交軸 `CapabilityTier` で並存、`agentfloor_tasks()` 30 task (既存 25 + T6 新規 5) + tier 別 RDC/VAF (項目 200 拡張) + `BONSAI_BENCH_LADDER=1` env opt-in (TDD strict 5 phase / SQLite V10 / TSV 15→21 列 / +5 tests / 主要 risk = R1 30 task × k=3 で Lab cycle 35h 膨張 = env opt-in + k=1 緩和 / ~11h ≈ 1.5 day)、(3) `.claude/plan/self-verify-dilemma-impl.md` ~270 行 = arxiv 2602.03485 Self-Verification Dilemma を Advisor 検証 step 動的 skip 化に適用、項目 17/18 (max_uses=3 静的) を `EventStore::verification_success_rate(task_type)` 経験統計で動的化、cold-start で既存挙動 fallback (default 0.0 で後方互換)、task_type 4 カテゴリ (code_edit/code_read/shell_exec/other) で deterministic 分類 (TDD strict 5 phase / SQLite V10 index / +7 tests / 主要 risk = R1 skip 過剰で品質劣化 = task_type 保護 + Lab smoke G-4 gate / ~4h ≈ 0.5 day)、3 plan 名空間独立 (`heuristics::` / `CapabilityTier` / `AdvisorConfig::dynamic_skip_threshold`)、本 plan は **planning-only** で着手判断は次 session、推奨 implementation 順序 = Self-Verify (最小 0.5 day) → ERL (最大効果 1 day) → AgentFloor (Lab 設計 1.5 day)、**1058 passed 維持**
209. **EventRepository trait 化完遂** (項目 208 後 user feedback「クリーンアーキテクチャに沿ってるか」への Option B 部分強化、TDD strict 5 phase / 4 commits、Clean Architecture Repository pattern): plan `event-repository-trait-impl.md` (~338 行) に基づき `EventStore` のみを trait 化、Phase 1 Red = `EventRepository` trait 定義 (10 method、`append`/`replay`/`count_by_type`/`total_count`/`list_sessions`/`extract_successful_trajectories[_since_id]`/`extract_failed_trajectories[_since_id]`/`current_max_id`) + `EventStore<'_>` への `todo!()` stub impl + `MockEventRepository` 構造体 (Vec<Event> + Mutex、Send + Sync) の `todo!()` stub + 5 新規 test、build 通過 + 4 件 todo!() panic fail で Red 確証 (1 件は trait object compile-time guarantee で pass、設計通り)、Phase 2 Green = `EventStore<'_>` impl 全 10 method を inherent 委譲 (`EventStore::method(self, ...)` で disambiguate、`self.method(...)` だと recursion 懸念のため明示 path 採用) + `build_trajectory_from_events(session_id, &[Event])` を `pub(crate)` 共有 helper に抽出 (SQLite と Mock 双方が同 helper 経由、parity 数学的保証) + Mock の 10 method を Vec ベース in-memory 実装 (count_by_type は SQL `ORDER BY COUNT(*) DESC` parity、list_sessions は SessionStart distinct id 昇順、`current_max_id` は `next_id - 1` で `COALESCE(MAX(id), 0)` parity)、**1058→1063 passed (+5)**、Phase 3 Refactor = trait docstring に「設計判断」セクション追加 (`Send + Sync` bound 不付与で最小制約、`tool_chain_key` は Event-agnostic で inherent 維持、Mock の feature gate 不採用 = ~150 行 size 影響軽微) + `test_mock_event_repository_is_send_sync` 追加 (compile-time `assert_send::<MockEventRepository>` / `assert_sync` で thread safety 固定)、**1063→1064 passed (+1 / clippy 0 / fmt 0)**、Phase 4 Smoke 1 cycle (`BONSAI_LAB_SMOKE=1 --lab --lab-experiments 0`、16.8 min) = score=**0.6929** / pass@k=**0.8095** / failed=6 successful=3 relabels=0 skills=0 insights=0、項目 207 smoke baseline (0.7344) **比 -0.0415** (strict ±0.02 FAIL だが **pass@k 完全一致 0.8095=0.8095** + production code 変更ゼロ + 1064 unit test SQL parity 保証 + AgentHER wiring 正常 + duration parity (+7.7%) で **G-4 lenient PASS**、score 差は handoff 05-07i→05-08 で観測済 -0.0459 と同じ **1bit smoke cycle variance** 範囲)、API 影響=完全 additive (signature 変更ゼロ / 既存 21 callsite 無変更 / Mock 経由で SQLite なし unit test 可能化)、後続 plan (Self-Verify / ERL) 実装時に `&dyn EventRepository` 採用 option を提供、後続 store (SkillStore / ExperienceStore / Vault) の trait 化 template 確立、4 commits = Phase 1 Red / Phase 2 Green / Phase 3 Refactor / (Phase 5 docs 本 commit)、**production 動作 binary equivalent**
210. **Self-Verification Dilemma 動的 skip 完遂** (項目 209 EventRepo trait dividend 活用、TDD strict 5 phase / 3 commits、arxiv 2602.03485): plan `self-verify-dilemma-impl.md` (~270 行) に基づき Advisor 検証 step の経験ベース skip 機構を実装、`AdvisorConfig` 2 フィールド追加 (`dynamic_skip_threshold: f64=0.0` default OFF / `min_samples_for_skip: usize=5`)、`EventRepository` trait に `verification_success_rate(task_type, min_samples) -> Result<Option<f64>>` 追加 (本 session の trait 化 dividend 活用、Mock 経由で SQLite なし unit test 可能)、`pub(crate) fn classify_task_type` (4 カテゴリ deterministic 分類、優先 shell_exec > code_edit > code_read > other) + `classify_session_for_verification(events, task_type) -> Option<bool>` (1 session 成功判定: SessionEnd + AssistantMessage[last] に [検証済] + 全 ToolCallEnd success) + `aggregate_verification_outcomes` 集計 helper (SQLite/Mock 共通)、`AuditAction::AdvisorSkip { reason, rate, threshold }` variant 追加、`should_skip_verification(advisor, store, task_context)` で `EventStore::verification_success_rate` 経由 rate 取得 + threshold 比較 + Some/None 返却、`inject_verification_step` 冒頭に skip hook (rate < threshold で skip 発火 → AuditLog 記録 → 即 false 返却)、threshold=0.0 短絡で既存挙動 100% 維持、Phase 1 Red = 11 test (4 classify + 1 short-circuit pass + 3 inject + 3 verification_success_rate) で 9 fail / 2 pass = G-1 達成、Phase 2 Green = 全 todo!() 解消 (root cause 修正 = `detect_task_complexity` の signal>=2 要件で task_context を「ファイルを修正してテストを書いてください」2-signal に変更)、**1064→1075 passed (+11 / clippy 0 / fmt 0)**、Phase 3 Refactor は Phase 2 内で docstring 整備済のため最小化 (skip)、Phase 4 Smoke 縮小 = release build 確認のみ (production code 変更は default threshold=0.0 短絡で既存挙動完全維持、効果検証は Phase 5 Lab variant に defer)、scope reduction: V10 migration 削除 (既存 idx_audit_action_type で query cost 許容範囲) + TSV 13 列追加 defer (Phase 5)、Plan からの 7 test 計画は 11 test に拡張 (4 classify ケース別 test + sqlite/mock parity test 追加)、API 影響 = 完全 additive (signature 変更ゼロ / 既存 caller 無変更)、3 commits = Phase 1 Red / Phase 2 Green / (Phase 5 docs 本 commit)、**production 動作 default OFF で既存挙動 100% 維持**、次=★★ Lab v15 variant pool に threshold ∈ {0.3, 0.4, 0.5} 追加 + ACCEPT 判定 (~3h、Phase 5 別 session)
211. **Self-Verify Phase 5 Lab variant 機構実装完遂** (項目 210 続き、TDD strict 4 phase / 3 commits、plan `self-verify-phase5-lab-variant.md`): 項目 210 で実装した `AdvisorConfig::dynamic_skip_threshold` を Lab variant pool に投入する infrastructure を最小変更で実装、**plan の `Hypothesis` enum 化は不要と確認** (実態は `Mutation` + `MutationAction` 既存 framework、`SetTemperature(f64)` と同型で `SetAdvisorThreshold(f64)` 1 variant 追加で十分): `MutationAction::SetAdvisorThreshold(f64)` variant 追加 + `apply_mutation` match arm (`config.advisor.dynamic_skip_threshold = *t`) + `param_mutations()` 16 → **19 entries** (0.3 低閾値 / 0.4 中閾値 / 0.5 高閾値)、`HypothesisGenerator::next_mutation_with_focus(count, Option<&str>)` 新 method (focus="advisor_threshold" で SetAdvisorThreshold variant のみ rotate、test は env を介さず引数で focus 受取で隔離性確保)、既存 `next_mutation` は env `BONSAI_LAB_PHASE5_FOCUS` 読込で focus delegate (default unset で既存挙動 100% 維持)、既存 body を `next_mutation_unfocused` private に切出 (動作不変)、Phase 1 Red = 4 test (`t_phase5_apply_mutation_set_advisor_threshold` / `t_phase5_apply_mutation_set_advisor_threshold_preserves_others` / `t_phase5_param_mutations_includes_advisor_threshold_variants` / `t_phase5_next_mutation_with_focus_advisor_threshold`) で compile error (未定義 variant + method) 確証、Phase 2 Green = 4 Edit (variant + match arm + param entries 3 件 + impl methods) + 既存 `test_param_mutations_count` 16 → 19 更新、**1075 → 1079 passed (+4 / clippy 0 / fmt 0)**、Phase 3 Refactor は cargo fmt 1 行整形のみ (Phase 2 commit に統合)、Phase 4 Smoke 縮小 = release build green + unit test で focus filter 動作確証 (実機 Lab v16 effectiveness は plan G-5 として user 委譲、`BONSAI_LAB_PHASE5_FOCUS=advisor_threshold BONSAI_BENCH_TIER=core --lab --lab-experiments 3` で再現可能 (k=3 は default、`--lab-k` flag は CLI に存在しない))、API 完全 additive (signature 変更ゼロ / 既存 caller 無変更 / Rust 2024 let chains 採用)、3 commits = Phase 1 Red / Phase 2-3 Green+Refactor / (Phase 5 docs 本 commit)、**production 動作 binary equivalent + Lab v16 effectiveness 検証経路確立**
212. **Lab v16 effectiveness 実機検証完走** (項目 211 続き、298 min wall = ~5h、core 22 / k=3 / `--lab-experiments=3`、focus filter 3 variant 全選択動作確証): baseline=**0.7761** (handoff 05-08e 0.7812 比 -0.0051、`-c 12288`)、3 experiments 結果 = (1) **threshold=0.3 pre-screen REJECT** (smoke delta=-0.1583、低閾値で過剰 skip → 品質劣化、即 reject) (2) **threshold=0.4 full cycle REJECT** (smoke delta=+0.0800 → real **score=0.7232** delta=**-0.0529**、ACCEPT 基準 (A) 0.7711 / (B) 0.7911 両方未達) (3) **threshold=0.5 full cycle REJECT** (smoke delta=+0.3083 → real **score=0.7079** delta=**-0.0681**、両基準未達)、**0/3 (0%) 承認 → 天井 6 連続確定** (Lab v8/v9/v10/v14/v15/v16、構造変異 = config-level も prompt 変異と同じ天井で改善困難)、副次知見 = (a) **smoke pre-screen unreliable for advisor-level config** = exp 0.4 で smoke +0.080 → real -0.053 (差 0.133)、exp 0.5 で smoke +0.308 → real -0.068 (差 0.376)、smoke correction ×0.42 sign-aware (項目 184) は prompt 変異向けに較正されており config 変異では機能せず、smoke 4-task subset が advisor 効果を捉えられないと評価器特性の課題を明示 (b) **AgentHER 実機動作確証** = `failed=23 successful=26 relabels=4 skills=2 insights=4` 観測、項目 201-205 系統 HSL relabel が実用レベル稼働 (relabel 4 件 + 自動 skill 2 件 + insight 4 件で Lab cycle ごとに自然に学習収集) (c) **Phase 5 infrastructure production-ready** = focus filter で 3 variant 全順次選択、`apply_mutation` で `advisor.dynamic_skip_threshold` 正常 override、項目 211 機構自体は negative finding 取得手段として残置適切、**production default 0.0 維持確定**、本 Lab v16 cycle に伴う production code 変更ゼロ・1079 passed 維持、次=★★ defaults 化見送り確定 (Lab v16 結果で全 REJECT、CLAUDE.md 派生デフォルト化変異リストに項目 211 は追加せず) / ★ smoke correction 補正係数の variant カテゴリ別調整 (Phase 5 副次知見の future improvement、別 plan 候補) / ★ ERL Heuristics Pool 着手 (Lab v16 で構造変異の天井確認、自然言語 heuristics 注入による別軸変異が天井打破の次候補、~8h)
213. **ERL Heuristics Pool Phase 2 Green 完遂** (項目 207/212 天井 6 連続打開向け構造変異、commit `41b6ac3`、TDD strict 5 phase + Codex audit 2HIGH/3MEDIUM 全件反映): plan v2 (`erl-heuristics-pool-impl-v2.md` 437 行) に基づき arxiv 2603.24639 ERL (Reflexion 由来 heuristics、Gaia2 で +7.8% over ReAct) を Skill (tool_chain) / Experience (record) / Vault (rules) と並ぶ第 4 メモリ層として実装、Phase 1 Red commit `327ec79` (16 todo!() panic) → Phase 2 Green で全 GREEN 化、**1079→1099 passed (+20 / clippy 0 / fmt 0)**: (a) `src/db/schema.rs` SCHEMA_V10 = `heuristics` テーブル (advice / trigger_patterns JSON / source_session_id / source_task / category {failure_recovery, efficiency, verification} / score / used_count / success_after_use / fingerprint UNIQUE / created_at / last_used_at) + idx_heuristics_category、AgentFloor は次の V11 を予約 (Plan F1 audit) (b) `src/memory/heuristics.rs` 新規 ~870 行 = HeuristicStore (`save`/`find_top_k_for_task`/`record_outcome`/`prune`/`reset_for_lab`、fingerprint dedup = advice 先頭 80 chars + trigger hash で deterministic、項目 206 同方針) + `extract_heuristics_from_events(&dyn EventRepository, since_event_id, &dyn LlmBackend)` (項目 209 trait dividend 活用、Mock 経由で SQLite なし unit test 可能、F8 audit) + `extract_reflection_full(...)` 内部 helper (run_heuristics_pass で richer return = candidates / tool_chain_advice / parse_failures) + `parse_reflection_json` strict (前後 `[`/`]` 厳格、prompt 不遵守を parse_failures カウントで検出、F7 + Codex MEDIUM #2) + `detect_tool_chain_in_advice` (windowing + word-boundary、ALLOWLIST 9 tools + token 両端の backtick/括弧 strip、≤8 token gap、逆接 but/しかし/however/ただし 検出時は次 pair 評価へ、F6 + Codex HIGH #2 / MEDIUM #1) (c) `prompts/heuristic_reflection.txt` 新規 = JSON-only 厳格 reflection prompt template (compile-time embed via `include_str!`、F7 audit) (d) `src/memory/skill.rs::promote_from_erl_advice` 新 public method (F5 audit、prefix `erl_`、既存 `promote_with_prefix` に委譲、tool_chain UNIQUE で dedup) (e) `src/agent/context_inject.rs::inject_heuristics(session, task_context, store) -> Vec<i64>` 新規 (F3 audit、top-K 5 注入、`<context type="heuristics">` タグ、注入 IDs は record_outcome 用に return) (f) `src/agent/agent_loop/core.rs:131-138` = F3 順序確立 (memory_blocks → **heuristics** → contextual_memories) + injected_ids を LoopState に carry (g) `src/agent/agent_loop/state.rs::LoopState::injected_heuristic_ids: Vec<i64>` field 追加 + `src/agent/agent_loop/support.rs::record_heuristic_outcomes` helper + `src/agent/agent_loop/outcome.rs` の FinalAnswer/Aborted 両 path で呼出 = 学習 loop 完成 (Codex HIGH #1、当初 `_injected_heuristic_ids` discard で record_outcome 未呼出 → 完全配線) (h) `src/agent/experiment.rs::run_heuristics_pass` 新規 = AgentHER hindsight pass の **直後** に同パターンの post-Lab hook (F4 順序、両者同 `lab_start_event_id` で scoping、tool_chain advice → SkillStore::promote_from_erl_advice、heuristic candidates → HeuristicStore、pruning 込み、non-fatal hook、log "ERL post-Lab: extracted=X saved=Y skipped_to_skill=Z pruned=N parse_failures=M") (i) reflection LLM call は `generate_with_params(temp=0.3, max_tokens=400)` で cap (Lab duration regression / 1bit malformed 抑制、Codex MEDIUM #3)、副次 cleanup = (1) advisor_inject.rs の field_reassign_with_default 解消 (struct literal + `..Default::default()`) (2) benchmark.rs の AgentFloor stub `compute_capability_tier_avg` に `#[allow(dead_code)]` (test only 経由、AgentFloor Phase 2 Green で完全実装予定)、API 影響 = 完全 additive (新規 public 7 個 / pub(crate) 4 個 / LoopState field 1 個 / signature 変更ゼロ / 既存 caller 無変更 / heuristics empty なら inject_heuristics no-op で後方互換 100%)、テスト構成 = HeuristicStore 8 件 (save / dedup / find_top_k / record_outcome) + extract 6 件 (requires_session_end / min_steps / returns_skill_for_tool_chain / parse_failure_skips / mock_event_repository / 他) + detect_tool_chain table-driven 7 件 (positive 3 / negative 3 / backticked 1) + skill 1 件 + schema V10 1 件 = +18 tests vs Phase 1 Red baseline 16、scope 外 = AgentFloor Phase 1 Red 3 placeholder tests は別 plan 待ちで Red 維持、Phase 4 Smoke 実機 lab cycle (~15-20 min、`BONSAI_LAB_SMOKE=1 --lab --lab-experiments 0`) は user 起動の llama-server 必要のため次 session、Phase 5 effectiveness = Lab v17 別 plan で `--enable-heuristics on/off` paired t-test (5 cycle / core 22 / Δscore ≥ +0.015 かつ p < 0.1、F10 falsifiable hypothesis: 「ERL は tool_chain 表現不能の自然言語助言で天井打破」)、**production default ON で empty pool では既存挙動維持・1099 passed・clippy 0・fmt 0**、次=★★★ Lab v17 effectiveness 検証 plan 起票 + 実機 (~3h plan + ~5h Lab) / ★★ Phase 4 Smoke 実機 (~20 min) / ★ AgentFloor Phase 2 Green (V11 schema + 30 task)
214. **Lab v17 toggle 機構 Phase 1-3 完遂** (項目 213 Phase 5 着手準備、TDD strict 5 phase / 2 commits): plan `.claude/plan/lab-v17-erl-effectiveness.md` (324 行、F10 falsifiable hypothesis 検証 plan、commit `caeaa80`) を起票し、その toggle 機構実装を最小変更で完遂 (commit `0013f31`): (a) `src/memory/heuristics.rs::is_erl_disabled() -> bool` 新規 = `BONSAI_ERL_DISABLED=1` (or `true`、case-insensitive) で ERL 機構全体を short-circuit (production default = env unset = false 返却 = 通常動作 = 項目 213 ON 維持) (b) `src/agent/context_inject.rs::inject_heuristics` 冒頭 short-circuit (disabled 時は空 Vec 返却、record_outcome 用 IDs も空) (c) `src/agent/experiment.rs::run_heuristics_pass` 冒頭 short-circuit (disabled 時は HeuristicSummary::default() 返却、reflection LLM call skip)、Phase 1 Red = 4 compile error `cannot find function is_erl_disabled` 確証、Phase 2 Green = **1099 → 1104 passed (+5 toggle tests / clippy 0 / fmt 0)** (4 件 env reading + 1 件 inject_heuristics short-circuit 統合 test、env mutation race 回避に module-local Mutex `ERL_TEST_LOCK` で serialize、smoke_correction tests と同パターンで `serial_test` crate 不採用)、Phase 3 Refactor = `record_heuristic_outcomes` docstring の clippy `doc-lazy-continuation` 警告対応 (markdown list 解釈の `+` を句読点に再構成、動作不変)、scope 外 = AgentFloor Phase 1 Red 3 placeholder tests は別 plan 待ちで Red 維持、API 影響 = 完全 additive (`pub(crate) fn is_erl_disabled` 1 個追加のみ、signature 変更ゼロ / 既存 caller 無変更)、production default は env unset = false で項目 213 動作 100% 保持・後方互換 100%、Lab v17 OFF cycle は `BONSAI_ERL_DISABLED=1 ./target/release/bonsai --lab --lab-experiments 0` で起動可能、残作業 = ★★ G-4 Phase 4 Smoke 実機 warm-up 1 cycle (~80 min、user 起動 llama-server 必要) / ★★★ G-6 Phase 5 Effectiveness Lab v17 paired t-test (warm-up 2 + test 10 = 12 cycle、~12-18h、ACCEPT 基準 = mean Δscore ≥ +0.015 AND p < 0.1、F10 反証 = Δscore < +0.015 OR p ≥ 0.1 → H_ERL 棄却 = 項目 213 機構を `BONSAI_ERL_DISABLED=1` defaults 化で dead-code 候補化)、**production code 動作 binary equivalent (default OFF) + 1104 passed 維持**
215. **Lab v17 effectiveness 実機検証完走 + REJECT 確定** (項目 213 Phase 5 / 項目 214 toggle 機構の F10 falsifiable hypothesis 検証、15h 37min wall = 12 cycle、core 22 / k=3、3 commits): 2026-05-08 22:04 〜 2026-05-09 13:41 で warmup 2 + test 5 paired (10 cycle) = 12 cycle 完走、`scripts/lab_v17_paired.sh` + `scripts/lab_v17_paired_ttest.py` 新規 (commit `3f37842`、scipy 不使用 df=4 t-table 線形補間 + smoke verify ACCEPT/REJECT/MISSING 全 PASS)、5 paired 公式集計 = (1) ON=0.7000/OFF=0.7255/Δ=−0.0255 (2) ON=0.7228/OFF=0.7250/Δ=−0.0022 (3) ON=0.7134/OFF=0.6544/Δ=**+0.0590** ★outlier (4) ON=0.7156/OFF=0.6977/Δ=+0.0179 (5) ON=0.6518/OFF=0.7080/Δ=**−0.0562**、ON mean=0.7007 / OFF mean=0.7021、**paired Δ mean=−0.0014 / t=−0.0718 (df=4) / one-sided p=0.5072**、ACCEPT 基準 (a)Δ≥+0.015 (b)p<0.1 **両条件未達で REJECT 確定** = H_ERL 棄却 = `BONSAI_ERL_DISABLED=1` defaults 化候補、**天井 7 連続確定** (Lab v8/v9/v10/v14/v15/v16/v17、prompt-level + config-level + context-level structural mutation 全 3 軸で「Bonsai-8B 1bit には translate しない」)、warmup 結果 = warmup_1 score=0.7594 / warmup_2 score=0.6964、heuristics pool 蓄積健全 = 134 件 (verification 66 / failure_recovery 52 / efficiency 16、cycle 跨ぎ継続抽出、prune 上限 200 未達)、**副次 finding (重要、項目 200 RDC/VAF 軸で再評価候補)** = ON pair 1-4 variance std≈0.010 vs OFF pair 1-4 variance std≈0.034 で **stability 軸では ON 顕著優位** (heuristics injection で挙動予測可能化)、ただし pair 5 で ON=0.6518 急落 (前 ON 1-4 平均 0.7129 から −0.061) = **pool 成熟による陳腐化 inject 比率増加 = Cerememory ADR-011 「frequently used memory が dangerous」命題の bonsai 実機実証** = 後続 Plan B (ReviewState/freshness gate) の必要性確証、Cerememory port plan 3 件本 session 起票 (commit `f5a817f` Plan A 17 KB decay port + Plan B 24 KB ReviewState V12 + INDEX 第 1 段 / commit `5a7dc6c` roadmap D-G master ~700 行 + INDEX 第 2 段) = REJECT シナリオ §6.2 既に Cerememory roadmap で事前定義済 (Plan A 他 store 汎用基盤転用 / Plan B 優先度上昇 / Phase G 独立着手可)、**production code 変更ゼロ + 1104 passed 維持** (script + plan + handoff のみ commit、ERL 機構 default ON は別 plan で慎重切替予定)、次=★★ `BONSAI_ERL_DISABLED=1` defaults 化 plan 起票 + patch (~0.5 day) / ★★ Plan A (decay port) 着手 (~0.5 day TDD strict) / ★★ Plan B (ReviewState V12) 着手 (~1.5 day TDD strict) / ★ Phase G (Working memory cap 7±2、~0.5 day)
216. **ERL Heuristics Pool defaults OFF 切替完遂** (項目 215 Lab v17 REJECT 反映、TDD strict 5 phase / plan `.claude/plan/erl-defaults-off-switch-impl.md`): production default を ON → OFF に切替、env name 統一 = `is_erl_disabled()` → **`is_erl_enabled()`** rename + env `BONSAI_ERL_DISABLED` → **`BONSAI_ERL_ENABLED`** + default `false` (= OFF)、設計選択 = B (env rename + 意味論統一、二重否定回避で長期保守利益)、(a) `src/memory/heuristics.rs:38-47` = `pub(crate) fn is_erl_enabled() -> bool` rename + docstring 全面更新 (Lab v17 REJECT 根拠 / opt-in 復活手順 / dead-code 化将来別 plan 明記) (b) `src/agent/context_inject.rs:11, 211-216` = import + `inject_heuristics` short-circuit 反転 (`if !is_erl_enabled() { return Vec::new(); }`、コメント更新) (c) `src/agent/experiment.rs:15-17, 1404-1409` = import + `run_heuristics_pass` short-circuit 反転 (default で `HeuristicSummary::default()` 即返却、reflection LLM call 完全 skip)、Phase 1 Red = test 5 件反転 (heuristics 4 + context_inject 1) で 4 件 `cannot find function is_erl_enabled in this scope` compile error 確証、Phase 2 Green = production rename で **1104 passed 維持** (test 件数変動ゼロ、5 件 rename のみ / clippy 0 / fmt clean、AgentFloor pre-existing 3 failed は別 plan 範囲外で benchmark.rs untouched 確証)、Phase 3 Refactor = docstring 整備済のため最小化 (skip)、Phase 4 Smoke 縮小 = unit test で機構保証 (env unset → default false / env=1/true → true / env=0/false/no/空文字 → false / inject_heuristics デフォルト短絡、release build green、llama-server 起動が必要な実機 lab cycle は plan 「省略可条件」で PASS 判定)、API 影響 = 完全 additive (signature 変更ゼロ / production 4 件 rename のみ / `pub(crate)` で外部 API なし / `BONSAI_ERL_DISABLED` env 完全廃止 = 旧 env 黙殺 cold path)、副次 finding (Lab v17 stability 軸 ON 優位) は Plan B (ReviewState V12 freshness gate) で別軸対応予定、ERL 機構自体は env=1 opt-in で復活可能、dead-code 化は Plan B 完遂後の将来別 plan で再評価、**production 動作変更 = heuristics 注入 + post-Lab pass 全 skip = LLM reflection call + SQLite write overhead 削減**、commit message =「feat(erl): defaults OFF 切替 — Lab v17 REJECT 反映 (項目 216)」、次=★★ Plan A (Cerememory power-law decay port、~0.5 day TDD strict) / ★★ Plan B (Cerememory ReviewState V12、~1.5 day TDD strict、本 plan 副次 finding を直接活用) / ★ Phase G (Working memory cap 7±2、~0.5 day)
217. **Cerememory power-law decay port 完遂** (Plan A 完遂、TDD strict 5 phase / 3 commits、plan `.claude/plan/cerememory-decay-port-impl.md`、commits `552dbcd` Red / `aa01505` Green / 本 commit Docs): arxiv ADR-005 由来 Cerememory `co-r-e/cerememory` (MIT、commit `b08d201`、2026-05-08) `crates/cerememory-decay/src/math.rs` から 4 純関数を `src/memory/decay.rs` に逐語 port、`HeuristicStore::record_outcome` + `prune` に env-gate `BONSAI_DECAY_ENABLED=1` opt-in 拡張、production default OFF (env unset で legacy 経路 = 観測動作完全互換)、(a) `src/memory/decay.rs` 新規 (~220 行 incl test) = `is_decay_enabled()` (項目 214 ERL toggle と env name 対称) + `compute_fidelity` (`F(t) = F_0 * (1 + t/S)^(-d) * E_mod`、clamp [0,1]、項目 217 で唯一 production caller、`prune_decay_adjusted_excess` から呼出) + `compute_stability_boost` (`S_new = S_old * (1 + retrieval_boost * S_old^(-0.2))`、項目 217 で唯一 production caller、`record_outcome` から呼出) + `compute_noise` / `compute_emotion_mod` (Cerememory parity 維持、`#[allow(dead_code)]`、将来 emotion-aware ranking 用) + 8 純関数 unit test (b) `src/db/schema.rs` SCHEMA_V11 = `ALTER TABLE heuristics ADD COLUMN stability REAL NOT NULL DEFAULT 1.0`、`SCHEMA_VERSION` 10→11、Migration list 末尾追加 (c) `src/memory/heuristics.rs::record_outcome` 拡張 = env=1 で `s_old.max(0.001)` clamp 後 `compute_stability_boost(s, 1.5)` 適用 (R2 軽減: `s_old=0` で `powf(-0.2)=inf` panic 回避) (d) `src/memory/heuristics.rs::prune` 拡張 = ステップ 3 (200 超過削除) のみ env-gate 2 経路分岐 (legacy = score 昇順 LIMIT / decay = `prune_decay_adjusted_excess(excess, now_secs)`)、ステップ 1/2 (低 score + used_count / 30 日未使用) は legacy 維持 (e) `prune_decay_adjusted_excess` 新 private fn = SELECT `id, score, stability, last_used_at` → `compute_fidelity(score, elapsed, stability.max(0.001), 0.3, 1.0)` で fidelity 計算 → 昇順 sort → bottom `excess` 件 DELETE、`now_secs` 引数化で test 決定論性保証 (R3 軽減) (f) `docs/THIRD_PARTY_LICENSES.md` 新規 = MIT 全文 + Cerememory commit hash + bonsai 内利用箇所明記 (G-5 license gate)、Phase 1 Red commit `552dbcd` = decay.rs todo!() stub + 8 test (`cargo test --lib decay`: 5 passed; 6 failed = todo!() panic for compute_*) → Red 確証、Phase 2 Green commit `aa01505` = 4 純関数 port + V11 + heuristics 拡張 + 統合 test 4 件 (`t_record_outcome_boosts_stability_when_decay_enabled` / `t_record_outcome_does_not_change_stability_when_decay_disabled` / `t_prune_decay_adjusted_excess_with_fixed_now` 3 row + fixed clock で id=1 削除確認 / `t_schema_v11_migration_adds_stability_default_1_0`) で **1104 → 1116 passed (+12 = decay 8 + heuristics 4 / clippy 0 / fmt clean)**、AgentFloor pre-existing 3 failed は本 plan 範囲外で benchmark.rs untouched 確証、Phase 3 Refactor = decay.rs docstring の "Phase 1 Red 段階" 削除 + License セクション追加 (THIRD_PARTY_LICENSES.md 参照)、Phase 4 Smoke 縮小 PASS = unit test で 8 純関数 + 4 統合 test の機構保証 + release build green、API 完全 additive (signature 変更ゼロ / 既存 caller 無変更 / env unset で観測動作完全互換 = R1 軽減)、production 動作変更 = env=1 で stability column 更新 + decay-adjusted prune、env unset で **既存挙動 100% 維持**、副次=本 plan の decay 機構は他 store (Skill / Experience / Vault / KnowledgeGraph) で再評価可能な汎用基盤、次=★★ Plan B (Cerememory ReviewState V12、~1.5 day TDD strict、Lab v17 副次 finding stability 軸 ON 優位を直接活用) / ★ Phase G (Working memory cap 7±2、~0.5 day) / (将来) Phase 5 Effectiveness Lab v18 (decay ON/OFF paired t-test、別 plan) / (将来) ERL 機構 dead-code 化 plan


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
