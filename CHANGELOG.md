# Changelog

## Unreleased

### 追加
- **サブエージェント体制の刷新と最適化（Track A & B）**:
  - **開発支援サブエージェント（Track A）**: `bonsai_architect`（DEP-001/設計）、`bonsai_implementer`（Rust 2024/完全同期/Clippy巻き戻し絶対禁止/Lab releaseビルド保護）、`bonsai_tdd_verifier`（1,480+テスト保護/回帰ラチェット）、`bonsai_lab_evaluator`（ADR-003 Paired Evidence統計検証）、`bonsai_values_auditor`（docs/VALUES.md監査）の5専任ロール体制を確立し、`.claude/agents/` および `.agents/agents/` の不整合（tokio等）を完全解消。
  - **ランタイム・サブエージェント（Track B）**: `src/agent/subagent.rs` に `SubAgentRole`（`General`, `Explorer`, `Builder`, `Verifier`）を導入。ロール別プロンプトの動的注入（`prompt_instruction()`）を TDD で実装。`SubAgentConfig.allowed_tools` / `default_tools()` はロールごとの推奨ツール一覧を設定として保持するのみで、実行時（`SubAgentExecutor::run` 等）でのツール呼び出し enforcement は未実装（今後の課題。現時点で `SubAgentExecutor` に production 呼び出し元がないため実害なし）。
  - **ドキュメント運用ガイドライン**: `docs/architecture/subagents-guide.md` を新設し、サブエージェント運用とドキュメント同期規約（SSOT統治）を体系化。
- **ローカル埋め込み (MLX 経由 / offline 対応)**: MLX sidecar に OpenAI 互換 `/v1/embeddings` エンドポイントを追加し、Rust 側に `HttpEmbedder` を新設。`BONSAI_EMBED_URL` 設定時に `create_embedder()` が fastembed より優先採用する。`embeddings` feature (fastembed/ONNX) 非依存のため、`ort` バイナリの **ビルド時 DL** と fastembed モデルの **実行時 HF DL** の両方を回避でき、ネットワーク制限環境でもローカル完結で実埋め込みが使える。リモート失敗時は hash 埋め込みに graceful fallback (dim=256 維持)。埋め込みモデルは `BONSAI_MLX_EMBED_MODEL` (既定 `mlx-community/all-MiniLM-L6-v2-4bit`、初回まで lazy load)。

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
