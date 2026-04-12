# Contributing

bonsai-agentへの貢献を歓迎します。

## 開発環境セットアップ

```bash
git clone https://github.com/k3-yamada/bonsai-agent.git
cd bonsai-agent
cargo build
cargo test
```

## 開発フロー

1. ブランチを作成: `git checkout -b feat/your-feature`
2. テストを先に書く（TDD）
3. 実装する
4. 全テスト通過を確認: `cargo test`
5. リント通過を確認: `cargo clippy -- -D warnings`
6. フォーマット: `cargo fmt`
7. コミット: `git commit -m "feat: 機能の説明"`
8. プッシュしてPR作成

## コミットメッセージ

```
<type>: <description>

Types: feat, fix, refactor, docs, test, chore, perf
```

## テスト

- 全ての新機能にユニットテストを追加
- 実サーバー/ネットワークが必要なテストには `#[ignore]` を付ける
- `MockLlmBackend` でエージェントループをテスト
- `MemoryStore::in_memory()` でDB操作をテスト

## ツール追加

新しいツールを追加するには:

1. `src/tools/` に新ファイルを作成
2. `Tool` トレイトを実装（`name`, `description`, `parameters_schema`, `permission`, `call`）
3. `src/tools/mod.rs` にモジュール追加
4. `src/main.rs` で `tools.register()` に追加

## プラグインツール（コード不要）

`~/.config/bonsai-agent/config.toml` に追加するだけ:

```toml
[[plugins.tools]]
name = "my_tool"
command = "echo {message}"
description = "メッセージを表示"
permission = "auto"
[plugins.tools.parameters]
message = { type = "string", description = "表示するメッセージ" }
```

## コードスタイル

- `cargo fmt` に従う
- `cargo clippy -- -D warnings` を通す
- `dynamic` / ハードコーディング禁止
- コメントは「Why」（意図）を書く
