# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## プロジェクト概要

`bonsai-agent` — Bonsai-8B（1ビット量子化Qwen3-8B、1.28GB）で動作するRust製自律型エージェント。
Mac M2 16GB上でllama-server HTTP API経由で推論。1348 unit test (2026-05-21 時点)、95+ ソースファイル。

設計原則: **「Scaffolding > Model」** — 1ビットモデルの改善余地は限定的。ハーネス側で信頼性を底上げする。

## ビルド・テスト・実行

→ [docs/execution/runbook.md](docs/execution/runbook.md) (env 一覧 + Smoke G-RT2 起動例含む)

## アーキテクチャ + 主要トレイト

→ [docs/architecture/overview.md](docs/architecture/overview.md) (src/ tree + trait 群 + 設計原則)
→ [docs/architecture/module-layer-rules.md](docs/architecture/module-layer-rules.md) (Z-4 layer linter rule source)

## ハーネスパターン

p^n問題（ステップ蓄積による失敗確率指数的増大）への対策。**項目 1-252 は `~/.claude/projects/-Users-keizo-bonsai-agent/memory/harness_patterns_archive.md` (Claude Code session memory) に verbatim アーカイブ**。CLAUDE.md は索引 + デフォルト化済み変異 + 直近項目の 1 行サマリーのみ保持。

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

- **247**: 🟡 **Lab v22 Metric Redesign Phase A-D 実装 + CCG synthesis (進行中)** = (a) Phase B (`lab_v22_metric.py`、Wilcoxon + Cohen's dz + factcheck 補助 + Pearson r 診断 + ACCEPT/REJECT 統合 + A/A mode) (b) Phase C (`BONSAI_LAB_TEMP` env、TDD strict 6 test + main.rs wiring) (c) Phase A 起動 (`lab_v22_aa_test.sh`、両側 OFF×OFF + T=0 で σ_Δ noise floor) (d) Phase D ready (`lab_v22_paired.sh`)。CCG synthesis: Pearson r 廃止 → paired Δscore + Wilcoxon + dz 主軸、smoke=10/decision=20/strict=27 cycle 推奨。補強 plan 3 件 (b/c/d) 起票済
- **248**: 🎉 **Dynamic Budget Compaction Phase 1-3 + Phase 4 wiring 完遂 (TDD strict 4 commit、Zenn 4 ratio 配分 + middleware 統合)** = Phase 1-3 (`2546d79` + `5109219`): BudgetRatios skeleton + default 40/30/20/10 + allocate() 4 軸按分 + adjusted() new_ratio = base × (1 + (rel-0.5) × α) + env getter SSOT (`BONSAI_DYNAMIC_BUDGET` / `_RATIOS` 4 要素 sum 1.0 / `_ALPHA` 範囲 [0.0,1.0] default 0.2)。**Phase 4 wiring (今 session 2 commit `f03666e` Red + `623d81f` Green)**: `CompactionConfig.budget_ratios: Option<BudgetRatios>` field (Default = None backward compat) + `with_dynamic_budget_from_env(self) -> Self` factory (env=1 で Some 設定) + `dynamic_budget_for_compaction(&config) -> Option<AllocatedBudget>` helper + `compact_if_needed` に log emit hook (`[INFO][compaction.budget] buffer=N summary=N entities=N kg=N total=N`) + `CompactionMiddleware::with_n_ctx_budget` / `Default` 2 constructor に chain 統合。env unset で完全 backward compat、env on で Some + log emit (将来 4 軸個別 prune の hook 点)。1323→1327 passed (+4) / clippy clean / fmt clean。Smoke G-10a/b/c 実機検証は次 phase
- **249**: 🚨 **Lab Runtime Stabilization Phase 1-3 + Phase 4 F1+F2 REJECT (実機 finding + F4 plan 起票)** = Phase 1-3 (`5a01a45`): F1 (`BONSAI_LAB_LONG_SSE=1` で sse_chunk_timeout 60→180) + F2 (`BONSAI_LAB_MLX_ONLY=1` で fallback chain clear + primary 切替) + F3 (`BONSAI_LAB_TASK_LIMIT`) env-gated 実装。Phase 4 Smoke G-RT REJECT: F1+F2+T=0+SMOKE=1 (5 task × k=3) で wall **103m42s** = target ≤35 min を **3x 超過**、SSE timeout 5 回発火 (F1 180s でも MLX 初トークン latency catch しきれず) + 非ストリーミング fallback。**finding**: MLX 2-bit primary は llama-server 1-bit gguf より latency ~2x 高い (Ternary +5pt の対価)、F1+F2 単独では Lab v22 paired 5h 完走未達。**次手 = 項目 250 (F4 plan 起票完了)**
- **250**: 🟡 **項目 249 F4 設計 plan 起票 (planner agent 並列実行、planning-only)** = `oh-my-claudecode:planner` agent を background で並列起動 (本 session 並列 task の 1 つ)、Claude 単独で 4 案比較。`.claude/plan/lab-runtime-stabilization-f4-mlx-latency.md` (413 行) を生成、案 A=MLX server pre-warm / 案 B=non-streaming default 化 / 案 C=1-bit gguf primary 回帰 (Ternary 諦め、Lab cycle 完走優先) / 案 D=HTTP/2 multiplex を 5 軸 (Lab v22 5h 完走可能性 / 工数 / Ternary 精度維持 / リスク / rollback 容易性) で比較、推奨案 + TDD strict 3-phase 実装 outline + Phase 4 smoke 合格基準 (cycle ≤ 35 min) 込み。production code 変更ゼロ。commit `9626dbb`
- **251**: 🎉 **Vault Lint bail 分岐 test 追加 (項目 246 critic F1 follow-up、TDD strict 3 commit + helper extraction)** = Phase 1 Red (`863e816`): `vault_lint.rs` に `run_vault_sanity_gate(vault_root, stale_days, strict, audit) -> Result<VaultLintReport>` helper stub (`unimplemented!()`) + 4 failing test (clean+strict / dirty+warn_only / **dirty+strict→Err 核心 bail** / audit_log emit COUNT==1)。Phase 2 Green (`f74440c`): helper 本実装 (Vault::new + lint_vault_for_lab + warn_log + audit.log + strict bail) + `main.rs::handle_lab_mode` inline ブロック (53 行) を helper 呼出 (23 行) にリファクタ (warn-only mode の Vault::new 失敗 contract 維持)。Phase 3 Refactor (`fabf931`): cargo fmt 整形。1319→1323 passed (+4) / clippy clean / fmt clean。silent regression で `bail!` → `Ok(())` 改変を CI が catch 可能化 = 項目 246 critic adversary review F1 解消
- **252**: 🎉 **Lab MLX pre-warm F4 案 A Phase 1+2+2.5 完遂 + 項目 248 critic follow-up (TDD strict 4 commit + main.rs wiring + critic F1+F2+F4+F6 解消)** = Phase 1 Red (`3faa422`) + Phase 2 Green (`fa2e7b9`): src/config.rs に env getter `is_lab_mlx_warmup()` / `lab_mlx_warmup_count() -> Option<usize>` (range 1..=10) + 4 test。Phase 2.5 Red (`ade5457`) + Phase 2 Green (`76e82f9`): src/agent/experiment.rs に `lab_mlx_prewarm(backend, count) -> usize` (Phase 2 Green = `backend.generate(&[user("ping")], ...)` を count 回 + log_event Info/Warn) + src/main.rs::handle_lab_mode に raw backend (CachedBackend wrap 前) 経由 env-gated 配線 (`BONSAI_LAB_MLX_WARMUP=1` + optional `_COUNT=N` default 3)。**項目 248 Phase 4 critic adversary review (`oh-my-claudecode:critic` REVISE 2 MAJOR + 4 MINOR) 対応**: F1 (eprintln→log_event 経由化、`9aebb48`) + F4 (BudgetRatios::is_normalized で finite+≥0.0 guard 追加) + F6 (rustdoc env value list 明示) + F2 (wiring test 5 件追加 `05dd056`、CompactionMiddleware.config を pub(crate)、env override 有効/無効分岐 test)。1323→1339 passed (+16) / clippy clean / fmt clean。**項目 252 critic adversary review follow-up (3 commit pushed)**: M1 (cancel forward、`b8f4b31`、lab_mlx_prewarm signature に `cancel: &CancellationToken` 追加 + 各 iter 冒頭で `is_cancelled()` 早期 bail + caller の cancel を `backend.generate` に forward、Ctrl+C 中断応答性確保) + F3 (succ==0 stderr 警告、`323b714`、main.rs::handle_lab_mode で全 fail 時 operator visibility 確保、silent failure 防止) + F1 (Err-path test、`c39ce33`、`AlwaysFailBackend` (test-only LlmBackend impl) 追加 + `t_lab_mlx_prewarm_all_fail_returns_zero` で graceful degradation 契約確証)。1339→1340 passed (+1) / clippy clean / fmt clean。**項目 252 M2 plan 起票完了** (`5830814`、`.claude/plan/lab-mlx-prewarm-timeout-bound.md` 235 行) = 案 A (per-iter wall budget、env `BONSAI_LAB_MLX_WARMUP_TIMEOUT_SECS` default 180s) 推奨 + TDD strict 3-phase outline + Phase 4 Smoke G-PWT1/2/3 + G-RT2 acceptance + rollback strategy。**項目 248 Phase 5 plan 起票完了** (`ada37b5`、`.claude/plan/dynamic-token-budget-phase5-axis-prune.md` 542 行)。**code-reviewer MEDIUM 3 件 atomic fix** (`0f55ebe`) = M-1 (Message::user("ping") loop 外 hoist、重複 alloc 回避) + M-2 (eprintln 表記混同回避、BONSAI_LAB_MLX_WARMUP_COUNT={n} 明示) + M-3 (rustdoc cancel 早期 bail 明記)。次手=**Phase 4 Smoke G-RT2** = `cargo build --release` + MLX server 起動 + `BONSAI_LAB_LONG_SSE=1 BONSAI_LAB_MLX_ONLY=1 BONSAI_LAB_MLX_WARMUP=1 BONSAI_LAB_TEMP=0 BONSAI_LAB_TASK_LIMIT=5 ./scripts/lab_v22_aa_test.sh` 実機検証 (cycle wall ≤ 35 min ACCEPT、Lab v22 paired 5h 完走 prerequisite)、M2 解消後に最終 smoke 推奨。**項目 252 M2 (per-iter wall budget) Phase 1 Red + Phase 2 Green 完遂 (2 commit)**: Phase 1 Red (`4a883a4`): `lab_mlx_warmup_timeout_secs() -> u64` env getter stub (常に 0 sentinel return) + config.rs::tests に 3 test (3 FAIL 期待、`LAB_RUNTIME_ENV_TEST_LOCK` で cross-test serialize)。Phase 2 Green (`22a2368`): env parse 本実装 (range 1..=600、default 180 = F1 sse_chunk_timeout 整合、env=0 sentinel で素朴 loop = M1 fix path 維持) + lab_mlx_prewarm に thread::scope + mpsc::recv_timeout 経路追加 (timeout=0 で既存挙動 100% 互換、>0 で per-iter wall budget enforce、LlmBackend trait の Send+Sync (inference.rs:31) 活用)。1345→1348 passed (+3) / clippy clean / fmt clean / `cargo build --release` clean (27.99s)。**M1+M2+M3+F1+F3 全 critic finding 解消**、Phase 4 Smoke G-RT2 prerequisite 全件完了 (binary 準備済、MLX server 起動は user 環境)

- **254**: 🎉 **Vault frontmatter status state machine — vault_lint 5 軸目 unreviewed_aged 完遂 (TDD strict 4 commit + Phase 4 wiring test)** = 2do BRAIN article (Qiita YushiYamamoto/62bafac9b4cf3961b3eb) 適用案 I-3 plan `.claude/plan/vault-status-state-machine.md` に基づき、項目 246/251 vault_lint への 1 軸増分 incremental 実装。Phase 1 Red (`00f56b4`): `VaultLintReport::unreviewed_aged_entries: Vec<(cat, ts_str, excerpt, age_days)>` field 追加 + `vault_unreviewed_days()` env getter stub + 4 test (2 FAIL 期待)。Phase 2 Green (`f30dcbb`): `vault_unreviewed_days()` env parse 本実装 (`BONSAI_VAULT_UNREVIEWED_DAYS` range 1..=90 default 14) + `lint_vault_for_lab` の incomplete block に age > unreviewed_threshold 判定追加 + `is_clean()` / `warn_log()` 拡張 + `AuditAction::VaultLint` variant に `unreviewed_aged: usize` field 追加 (`#[serde(default)]` で audit_log 既存 row 100% backward compat) + `run_vault_sanity_gate` の audit emit + bail message 統合 + audit.rs::test_audit_action_vault_lint_round_trip fixture 更新。Phase 3 Refactor (`2ac7e88`): module docstring に 5 軸目追記 + 2do BRAIN 対応関係明示。Phase 4 wiring test (`df58cb4`): 5 軸目 populate 確証 + is_clean()=false + strict bail 発火の 3 段確証 test 1 件追加 (silent regression catch、項目 251 bail pattern 適用)。**実 vault フォーマット適応 finding**: 記事の frontmatter `status: draft|reviewed|stale` は per-page 形式だが、bonsai Vault は line-based markdown (`- [ts] content`) のため frontmatter 列挙不可、代替として `incomplete (TODO/FIXME/WIP) AND aged > N days` で 2do BRAIN の Obsidian Dataview `WHERE status != "reviewed"` 等価検出を実現 (incomplete 軸のサブセット軸、orphan draft 検出の温床)。1340→1345 passed (+5) / clippy clean / fmt clean。次手 = Phase 5 smoke (G-VS-1..4 実機、~30 min) は production vault 未満で手動検証推奨、本実装は既存 `BONSAI_VAULT_LINT_LAB=1` + `BONSAI_VAULT_LINT_STRICT=1` の opt-in 経路に seamless 統合済

- **255**: 🎉 **Z-1 Zenn Codex Harness 適用 — CLAUDE.md slimming + docs/ ナレッジベース整備完遂 (5 phase + critic CRITICAL/HIGH 修正)** = Zenn dragon1208/66547a030c0236「Codex でエージェント駆動開発プラットフォームを設計する」記事 (Step 1+2+5、AGENTS.md 100 行原則 + docs/ Single Source of Truth) 適用、`.claude/plan/agents-md-docs-knowledge-base.md` 案 B (gradual 5 phase) 採用。Phase 1: `docs/INDEX.md` 新設 (ナビゲーション hub)。Phase 2: `docs/architecture/{overview.md, module-layer-rules.md}` (overview = CLAUDE.md「アーキテクチャ」+「主要なトレイト」verbatim 移行、module-layer-rules = Z-4 layer linter rule source、層順 `db<observability<safety<memory<knowledge<runtime<tools<agent<main`)。Phase 3: `docs/quality/{lab-history.md, scores.md}` (lab-history = CLAUDE.md「Lab 実機テスト結果」verbatim 移行 / scores = 1348 passed / 0 clippy / AgentFloor T1-T6 baseline、Z-3 自動更新 hook 点)。Phase 4: `docs/execution/runbook.md` (CLAUDE.md「ビルド・テストコマンド」+「Rust Edition」+「テストパターン」 verbatim 移行 + env 15 件 table、Smoke G-RT2 起動例)。Phase 5: CLAUDE.md final reduce **202→88 行 (-56%)**、Zenn 推奨 100 行原則達成。**code-reviewer CRITICAL 1 + HIGH 3 + MEDIUM 2 atomic fix**: (a) CLAUDE.md → archive dead link 修正 (`../../.claude/...` invalid → `~/.claude/...` absolute) (b) docs/decisions/README.md 新規作成 (404 解消) (c) docs/INDEX.md memory/ path 誤誘導修正 (「project root の」→「project root **外部**、Claude Code session memory dir」) (d) 絶対ルール placeholder 解消 (SSOT 参照 + 主要 5 ルール inline) (e) CLAUDE.md test 数 1278→1348 同期。production code 変更ゼロ (markdown only)、cargo test --lib 1348 passed (退行不可能)、clippy clean / fmt clean。**統合 ROI**: agent context overhead 大幅削減 (Claude Code auto-load 入力 token ~半分)、Codex (AGENTS.md) + Claude Code (CLAUDE.md) 両 IDE 対応の foundation 確立、Z-4 layer linter (項目 256) の rule source 完備

- **256**: 🎉 **Z-4 Layer Architecture Linter — tests/structural.rs 4 軸 lint 完遂 (TDD strict Phase 1 Red + Phase 2 Green + clippy fix)** = Zenn dragon1208 Step 4 (verbatim 6 種 linter codes: DEP/LOG/SIZE/NAME×2/TYPE) bonsai 適用、`.claude/plan/layer-architecture-linter.md` 案 B (tests/structural/ integration test) 採用。Phase 1 Red (`?a?`): tests/structural.rs (~220 LOC) 新規、4 軸 lint test 追加: t_no_new_src_file_over_800_lines (SIZE-001) / t_layer_order_no_upward_dep (DEP-001) / t_no_eprintln_in_production (LOG-001、cfg(test) 以前 catch) / t_lint_error_messages_include_docs_link (META、docs/ link 強制)。**Phase 1 Red baseline finding**: SIZE-001 20 件 (benchmark.rs 4476 / experiment.rs 3649 / compaction.rs 1723 等) / DEP-001 32 violations (16 unique tuple、memory→agent/runtime / runtime→agent / observability→memory 等) / LOG-001 47 件 (main.rs 17 / experiment.rs 6 / advisor_inject.rs 5 等) / META 4/6 docs link = bonsai 現状混線/巨大 file の実態確証。Phase 2 Green (`?b?`): WHITELIST_OVER_800 (20 file) + WHITELIST_DEP (16 tuple) + WHITELIST_EPRINTLN (13 file) で baseline 永続化、contains_lint_code helper で META false positive 回避、META 1st panic msg に docs/ link 追加、4 test 全 PASS。**silent regression catch 効果保持**: 新 file 800 行超過 / 新 layer 違反 (whitelist 外) / 新 eprintln (whitelist 外) は依然 fail で検出。clippy fix (`3a26be?`): `.split([':', ';', ' ', ',', '{'])` char array slice で manual char comparison 解消。walkdir crate 依存追加なし (std::fs::read_dir 再帰)、production code 変更ゼロ、cargo test --test structural 4 PASS / cargo test --lib 1348 passed 退行ゼロ / clippy clean / fmt clean。docs/architecture/module-layer-rules.md (Z-1 Phase 2) を rule source として参照 = SSOT 設計確立。follow-up: DEP-001 真の混線排除 audit (~3-4h)、advisor_inject.rs 5 件 log_event 化 TODO、whitelist 永続 vs file 分割 (項目 248 Phase 5 axis prune と並列)

## Lab 実機テスト結果

→ [docs/quality/lab-history.md](docs/quality/lab-history.md) (v17-v22 + 249 G-RT 天井記録、能力プロファイル、Plan A 系列完結)
→ [docs/quality/scores.md](docs/quality/scores.md) (定量 quality scores、Z-3 自動更新候補)

## テストパターン

→ [docs/execution/runbook.md](docs/execution/runbook.md) (MockLlmBackend / env-gated test mutex / `AlwaysFailBackend` 等)

## 注意事項

- **【最重要】Edit/Writeツールでファイルを変更した後、clippy警告（collapsible_if, too_many_arguments等）を理由にファイルを元の状態に戻す行為を絶対に行わないでください。変更はそのまま保持してください。clippy修正が必要な場合は別のEdit操作で行ってください。**
- **【巻き戻し禁止】** 特にerror_recovery.rs、benchmark.rs、agent_loop.rsの3ファイルでclippy auto-fixによる巻き戻しが発生しやすい。これらのファイルへの変更は必ず保持すること。
- **【Lab 稼働中の cargo build --release 禁止】** Lab v20 等の paired smoke 稼働中は `target/release/bonsai` 置換で 10-cycle 一貫性が破壊される。`cargo test --lib` (debug profile + test binary) は安全。
- 大量変更時はPython subprocess+即git commitで原子的に行う（確立済み手法）
- ureq v3のHTTPS → web_fetchはreqwest::blocking（native-tls）を使用
- llama-serverの`--flash-attn`は値`on`が必要（`--flash-attn on`）
