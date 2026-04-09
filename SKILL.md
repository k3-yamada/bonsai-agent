---
name: bonsai-agent-patterns
description: Rust製自律型エージェント（1ビットLLM、Reflexion、A-MEM）の設計・実装パターン
version: 1.0.0
source: local-design-analysis
---

# bonsai-agent パターン

## アーキテクチャ原則

### 単一バイナリ自己完結
- LLM推論はインプロセスFFI（llama-cpp-2経由）。外部サーバー不要
- モデルは`hf-hub`で自動ダウンロード、`~/.cache/huggingface/`にキャッシュ
- `cargo run`だけで動作する

### トレイトによる抽象化
- `LlmBackend` — 推論バックエンド（LlamaCppBackend実装、テスト時はMock）
- `Tool` — ツール定義（name, description, parameters_schema, permission, call）
- テストではトレイトのモック実装を使い、実モデル不要でエージェントループをテスト

### 2段階生成パイプライン
- `<think>`タグ内は自由形式でCoT推論（制約なし）
- `<tool_call>`タグ内のみGBNF文法でJSON構造を強制
- 構造化出力の強制は推論能力を劣化させるため、思考と出力を分離する

## コーディング規約

### Rust 2024 edition
- `edition = "2024"` — ネイティブasyncトレイト使用可能（`async-trait`クレート不要）
- `gen`キーワード予約に注意

### エラーハンドリング
- `anyhow::Result`を関数の戻り値に使用
- ユーザー向けメッセージは日本語（理由 + 対処法）
- 内部エラーは`FailureMode` enumで分類（ParseError / ToolExecError / LoopDetected）

### 命名
- 変数・関数: snake_case（英語）
- 型・トレイト: PascalCase（英語）
- コメント: 「Why」を書く、「What」は書かない
- コミット: `type(scope): subject`（日本語）

## テストパターン

### TDD厳守（Red → Green → Refactor）
1. テストを先に書く（`#[cfg(test)] mod tests`）
2. `cargo test`で失敗確認
3. 最小限の実装でパス
4. リファクタ

### モック戦略
- `LlmBackend`トレイトに対して`MockLlmBackend`を実装
- スクリプト化されたレスポンスを`Vec<String>`で返す
- 実モデル不要のテスト: エージェントループ、パーサー、ツールレジストリ
- 実モデル必要のテスト: `#[ignore]`で分離

### テスト配置
- 各ファイル末尾の`#[cfg(test)] mod tests`にユニットテスト
- 統合テストは`tests/`ディレクトリ
- `cargo test` — ユニットテスト（高速、モデル不要）
- `cargo test -- --ignored` — 統合テスト（モデルDL必要）

## ツールシステム

### 動的ツール選択
- 全ツールをプロンプトに入れない（小型モデルの精度低下を防ぐ）
- `ToolRegistry::select_relevant(query, max=5)`で関連ツールのみ注入
- キーワードマッチングでスコアリング

### 権限モデル
- `Auto` — 確認なしで実行（FileRead等）
- `Confirm` — ユーザー確認後に実行（Shell, FileWrite等）
- `Deny` — 実行禁止

## メモリ設計（A-MEM式）

### 原子的エントリ
- 各メモリは独立した原子的ノート（content + tags）
- `memory_links`テーブルでノート間をリンク（related_to, derived_from, contradicts）
- SQLite FTS5で全文検索

### 階層的コンテキスト
- 直近のメッセージ: フル内容
- 古いメッセージ: 要旨（gist）に圧縮
- コンテキスト上限（4096トークン）到達で自動要約

## エージェントループ（Reflexion）

### 基本フロー
1. メモリから関連コンテキスト検索
2. 動的ツール選択（上位5件）
3. LLMにリクエスト（インプロセス）
4. 2段階パース
5. ツール呼び出し → 成功なら要旨圧縮して継続 / 失敗ならReflexion

### エラーリカバリ
- `ParseError` → プロンプト修正してリトライ
- `ToolExecError` → エラー情報をコンテキストに追加してリトライ
- `LoopDetected` → 即座に打ち切り
- 最大リトライ: 3回
