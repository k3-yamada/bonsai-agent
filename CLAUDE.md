# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## プロジェクト概要

`bonsai-agent` — Bonsai-8B（1ビット量子化Qwen3-8B、1.28GB）で動作するRust製自律型エージェント。
Mac M2 16GB上で単一バイナリとして完結（LLM推論をインプロセスで実行、外部サーバー不要）。

## ビルド・テストコマンド

```bash
cargo build                    # ビルド
cargo test                     # ユニットテスト（125テスト）
cargo test -- --ignored        # 統合テスト（モデルDL必要）
cargo test <テスト名>           # 単一テスト
cargo check                    # 型チェック（高速）
cargo clippy -- -D warnings    # リント
cargo fmt -- --check           # フォーマットチェック
```

## Rust Edition

Rust **2024 edition**。ネイティブasyncトレイト、let chains等を使用可能。

## アーキテクチャ

```
src/
├── main.rs                    # CLIエントリーポイント
├── lib.rs                     # モジュール宣言
├── cancel.rs                  # CancellationToken（全レイヤー伝播）
├── runtime/
│   ├── inference.rs           # LlmBackend トレイト + MockLlmBackend
│   └── cache.rs               # 推論結果キャッシュ（model_id含むキー）
├── agent/
│   ├── agent_loop.rs          # run_agent_loop() — Plan→Execute→Reflect
│   ├── parse.rs               # parse_assistant_output() — <think>/<tool_call>パーサー
│   ├── validate.rs            # validate_tool_call() — バリデーション+危険パターン検出
│   ├── conversation.rs        # Message, Session, ToolCall, ParsedOutput
│   └── error_recovery.rs      # FailureMode, CircuitBreaker, LoopDetector
├── tools/
│   ├── mod.rs                 # Tool トレイト + ToolRegistry（動的選択）
│   ├── permission.rs          # Permission(Auto/Confirm/Deny) + DaemonPolicy
│   ├── sandbox.rs             # Sandbox トレイト + DirectSandbox（ulimit付き）
│   └── shell.rs               # ShellTool
├── memory/
│   ├── store.rs               # MemoryStore — SQLite A-MEM（FTS5検索）
│   └── experience.rs          # ExperienceStore — 成功/失敗/insight自動記録
└── db/
    ├── schema.rs              # 全SQLiteスキーマ（13テーブル）
    └── migrate.rs             # マイグレーション機構
```

## 主要なトレイト

- `LlmBackend` — LLM推論の抽象化。`generate(messages, tools, on_token, cancel) -> GenerateResult`
- `Tool` — ツール定義。`name(), description(), parameters_schema(), permission(), call(args)`
- `Sandbox` — コマンド実行の隔離。`execute(command, args, limits) -> ExecResult`

## テストパターン

- `MockLlmBackend` — スクリプト化されたレスポンスを返すモック
- `MemoryStore::in_memory()` — インメモリSQLiteでテスト
- 実モデルが必要なテストには `#[ignore]` を付ける
