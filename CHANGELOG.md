# Changelog

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
