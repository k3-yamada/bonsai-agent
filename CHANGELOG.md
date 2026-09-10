# Changelog

## Unreleased

### 追加
- **サブエージェント体制の刷新と最適化（Track A & B）**:
  - **開発支援サブエージェント（Track A）**: `bonsai_architect`（DEP-001/設計）、`bonsai_implementer`（Rust 2024/完全同期/Clippy巻き戻し絶対禁止/Lab releaseビルド保護）、`bonsai_tdd_verifier`（1,480+テスト保護/回帰ラチェット）、`bonsai_lab_evaluator`（ADR-003 Paired Evidence統計検証）、`bonsai_values_auditor`（docs/VALUES.md監査）の5専任ロール体制を確立し、`.claude/agents/` および `.agents/agents/` の不整合（tokio等）を完全解消。
  - **ランタイム・サブエージェント（Track B）**: `src/agent/subagent.rs` に `SubAgentRole`（`General`, `Explorer`, `Builder`, `Verifier`）を導入。ロール別プロンプトの動的注入（`prompt_instruction()`）を TDD で実装。`SubAgentConfig.allowed_tools` のツール allowlist enforcement を実装（Issue #25）。`AgentConfig.allowed_tools`（既定 `None` = 全許可で既存挙動不変）へ `build_sub_config()` が伝播し、`agent_loop::step::execute_step` の提示フィルタ（許可外ツールのスキーマをLLMに提示しない）とdispatchガード（`ToolRegistry::get()` 到達前に遮断し拒否理由をセッションへ返す）の2段で強制する。`Some([])` は全禁止。判定は完全一致のみ（glob/ワイルドカードは非対応）。`Verifier` の `shell` は多層防御側で抑える方針のため維持（`docs/architecture/subagents-guide.md` §3に明文化）。`known_tool_names()` のSSOT化は #28 で追跡。
  - **提示フィルタのtop-k溢れ修正（Issue #29）**: `ToolRegistry::select_relevant_split` / `select_relevant_split_semantic` に `allowed: Option<&[String]>` 引数を追加し、allowlistによる絞り込みをtop-k切り詰めの**前段**（候補収集ループ）で行うよう変更。従来はtop-k選択後に`retain`していたため、allowlistツールが意味検索スコア下位に落ちるとtop-k選択枠から溢れ、提示スキーマがゼロになりうる不具合（fail-closedで安全側だが機能制約）を解消。`tools`層は`agent`層に依存できない（DEP-001）ため、判定ロジックは`agent_loop::config::is_tool_allowed()`とは独立に`tools::mod`内の`is_name_in_allowlist`として実装。
  - **ドキュメント運用ガイドライン**: `docs/architecture/subagents-guide.md` を新設し、サブエージェント運用とドキュメント同期規約（SSOT統治）を体系化。
  - **`known_tool_names()` の SSOT 化（Issue #28）**: builtin tool 名とその登録処理を `src/tools/builtin.rs`（`BUILTIN_TOOL_NAMES` const + `register_builtin_tools()` factory）へ集約。`main.rs::setup_tools()` と `agent::subagent::known_tool_names()`（H-1回帰防止テスト用）の双方が同一SSOTを参照するようになり、これまで4箇所に存在していたtool名ミラーの乖離リスクを解消。`tests/structural.rs` に `t_setup_tools_registers_only_via_builtin_factory` を追加し、`setup_tools()` が factory をバイパスして直接 `registry.register(Box::new(..))` することを構造的に検出する。
  - **allowlistセレクタ群の一貫性強化（Issue #31）**: `tools::is_name_in_allowlist`（重複実装）を `pub(crate)` 化し、`agent_loop::config::is_tool_allowed` との等価性を代表的入力110通り（直積）+ 意味論アンカー6件の契約テストで固定。`ToolRegistry::select_relevant_split_semantic` のキャッシュ構築ロジックを `build_semantic_cache`（private ヘルパ、embedderの出所に非依存）へ抽出し、`#[cfg(test)] prime_semantic_cache` seam を新設（`create_embedder()` の `BONSAI_EMBED_URL`/HTTP/fastembed に一切触れず決定的な embedder を注入可能）。これにより Issue #29 のtop-k溢れ修正（allowlistをtop-k切り詰め前段で適用）を semantic 版でも直接カバーする回帰テストを4件追加（`AdversarialEmbedder` による敵対的ケース含む）。呼び出し元ゼロだった `ToolRegistry::select_relevant` / `select_relevant_with_type` / `TaskType::allowed_prefixes`（善意のcontributorが将来配線するとfail-open再発のリスク面）を削除（ADR-002「Scaffolding > Model」準拠）。`.claude/skills/rust-local-llm-agent/SKILL.md:113` の `select_relevant` 言及は本番APIの逐語コピーではなく設計パターンの例示コードのため据え置き。
- **ローカル埋め込み (MLX 経由 / offline 対応)**: MLX sidecar に OpenAI 互換 `/v1/embeddings` エンドポイントを追加し、Rust 側に `HttpEmbedder` を新設。`BONSAI_EMBED_URL` 設定時に `create_embedder()` が fastembed より優先採用する。`embeddings` feature (fastembed/ONNX) 非依存のため、`ort` バイナリの **ビルド時 DL** と fastembed モデルの **実行時 HF DL** の両方を回避でき、ネットワーク制限環境でもローカル完結で実埋め込みが使える。リモート失敗時は hash 埋め込みに graceful fallback (dim=256 維持)。埋め込みモデルは `BONSAI_MLX_EMBED_MODEL` (既定 `mlx-community/all-MiniLM-L6-v2-4bit`、初回まで lazy load)。
- **MCP stdio子プロセスのプロセスグループ化とグループkill（Issue #33）**: `src/tools/mcp_client.rs::build_stdio_command` で unix系子プロセスに `process_group(0)` を適用し、子プロセス自身を新しいプロセスグループのリーダーにした上で `Drop for McpConnection` が `libc::kill(-pid, SIGKILL)` によりグループ全体（`npx` → `node` 等の孫プロセス含む）を後始末する。レビュー指摘対応として、killpg直前に `libc::waitid(P_PID, .., WEXITED | WNOHANG | WNOWAIT)` でreapせずに子プロセスの終了状態をpeekし、`Alive`（生存中）または `ExitedNotReaped`（終了済みだが未reap＝pid再利用の懸念なし）の場合のみグループkillし、reapはDropの `child.wait()` のみが行う不変条件を確立（`src/tools/sandbox/direct.rs::kill_group_and_reap` と揃え、reap済みpidへの誤爆によるプロセス誤殺を防止）。当初検討した `try_wait()` による再確認は、呼び出し時点でreapしてしまうため採用しなかった。`McpToolWrapper::call` からの自動再接続経路で毎回reapが発生し、旧 `McpConnection` の `Drop` 側が常に「reap済み」と誤判定してkillpgが一度も発火せず、`npx` の孫 `node` プロセスがリークする欠陥（Q-1）を招くためである。既知の制約として、`process_group(0)` により子プロセスが端末の前景プロセスグループから外れ、bonsai-agent自体がシグナル等でDropを経由せず終了した場合に子プロセスグループが孤児化しうる点をコード内に明記（フル対応は別Issue）。テストモジュールを800行超過防止のため `src/tools/mcp_client/tests.rs` へ分離。
- **ユーザー取消（Ctrl+C）を学習信号から排除（Issue #34 / ADR-016）**: base実装として、`agent::tool_exec::apply_tool_result` が `r.cancelled` を見て `circuit_breaker`/`trial_summary`/`KnowledgeGraph`（error pattern）/`FileStuckGuard` の4つの学習記録をスキップするとともに、`tool_call_start`/`tool_call_end` payload に `"cancelled": true` を刻むようにした。この刻印を読む述語として `domain::event::is_tool_call_cancelled` を新設し、`domain::event`（`has_user_cancelled_tool_call` 経由）と `memory::experience` が共有する除外判定の基盤とした（docs/VALUES.md V1/V4: ユーザーの割り込みはツールの欠陥ではないため、負の学習信号にしない）。qa-reviewerの検証では、`build_trajectory_from_events` がcancelledなtool callを`tool_sequence`等の計算から個別に除外していたため、分子だけでなく分母（試行総数）からも消え、中断で未完遂だったセッションが `tool_success_rate` 1.0 の完全な成功trajectoryとして記録される欠陥が見つかった。follow-up実装として、セッション内にユーザー取消由来のtool callを1件でも含む場合は成功・失敗いずれの学習信号にもしないセッション単位のgateを `domain::event::has_user_cancelled_tool_call` として新設し、`build_trajectory_from_events` と `classify_session_for_verification`（従来は「全tool_call_endがcancelled」の場合のみ除外する非対称な実装だった）の両方に適用した。判断の経緯は [ADR-016](docs/decisions/ADR-016-user-cancellation-signal-exclusion.md) にまとめた。

### 修正
- `parse_vm_stat_value` / `parse_vm_stat_page_size` (macOS 専用 vm_stat ヘルパー) に `#[cfg(target_os = "macos")]` を付与。非 macOS ビルドでの dead_code 警告 (= `clippy -D warnings` 失敗) を解消。

## v0.1.0 (2026-04-10)

初回リリース。

### エージェントコア
- Reflexionエージェントループ（Plan→Execute→Reflect）
- `<think>` / `<tool_call>` パーサー
- バリデーション + 危険パターン検出
- エラー分類（6種FailureMode）+ サーキットブレーカー + ループ検出
- 4段階コンテキストコンパクション（L0大出力→ディスク、L1プレースホルダー、L2要約、L3緊急切り詰め）
- チェックポイント/ロールバック（git stashベース）
- タスク状態マシン（Pending/InProgress/WaitingForHuman/Completed/Failed + サブタスク）

### ツール（9種 + プラグイン + MCP）
- `shell` — Sandbox経由シェルコマンド実行
- `file_read` / `file_write` — ファイル操作（search/replace差分 + git-firstスナップショット）
- `git` — Git操作（status/diff/log/commit/add/branch）
- `web_search` — DuckDuckGo Instant Answer API
- `web_fetch` — URL取得（reqwest native-tls）
- `repo_map` — コード構造マップ（regex抽出、Aider方式）
- プラグインシステム — TOML定義でカスタムツール追加
- MCPクライアント — JSON-RPC over stdioでMCPサーバーと通信
- pre/postフック — ツール実行前後にスクリプト実行

### 推論
- `LlmBackend` トレイト + `MockLlmBackend`
- `LlamaServerBackend` — llama-server HTTP API
- 推論キャッシュ（model_id対応）
- `Embedder` トレイト（SimpleEmbedder + FastEmbedder）
- `ModelRouter` — タスク特性+RAM残量でBonsai/Gemma4自動切替
- マルチモデルパイプライン（intent分類→モデル選択チェーン）

### メモリ・学習
- A-MEM（SQLite FTS5、Zettelkasten式タグ付き）
- 経験メモリ（成功/失敗/insight自動記録）
- スキルシステム（3回成功→自動昇格）
- ハイブリッド検索（FTS5 + ベクトルKNN + RRF融合）
- Correction/Reinforcement検出（DeerFlow方式、日英対応）
- Dreamingシステム（exbrain方式、データ駆動の振り返り+パターン検出）
- arxiv自己進化エンジン（論文自動収集+知識蓄積+改善提案）
- 能動的自己改善（apply_improvements: 失敗パターン警告、スキル化提案、成功率改善提案を自動記録）（論文自動収集+知識蓄積+改善提案）
- ナレッジVault（フロー→ストック自動抽出、mdファイル蓄積）
- セッション永続化 + 再開（--resume）

### 安全
- DirectSandbox（ulimit付きコマンド実行）
- PathGuard（パスガード + 秘密情報フィルタ）
- 段階的自律レベル（ReadOnly/Supervised/Full）
- セーフモード（連続起動失敗→最小機能起動）
- ネットワークフィルタ（ドメインホワイトリスト）
- ケイパビリティ・マニフェスト

### 可観測性
- 監査ログ（全ツール呼び出しをSQLiteにappend-only記録）

### CLI
- 対話モード（REPL）
- 単発実行（--exec）
- モックモード（--mock）
- セッション一覧（--sessions）/ 再開（--resume）
- タスク一覧（--tasks）
- 監査ログ（--audit）
- ナレッジVault（--vault）
- マニフェスト（--manifest）

### インフラ
- GitHub Actions CI（macOS: test + clippy + fmt）
- `cargo install` 対応（`bonsai` バイナリ名）
- TOML設定ファイル
- MIT LICENSE
