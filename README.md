# bonsai-agent

Bonsai-8B（1ビット量子化Qwen3-8B、1.28GB）で動作するRust製自律型AIエージェント。

Mac M2 16GBで単一バイナリとして完結。外部クラウドAPI不要。

## クイックスタート

### 1. Bonsai-demoセットアップ（初回のみ）

```bash
cd ~
git clone https://github.com/PrismML-Eng/Bonsai-demo.git
cd Bonsai-demo
sh scripts/download_binaries.sh    # llama-server (macOS ARM64)
sh scripts/download_models.sh      # Bonsai-8B GGUF (~1.15GB)
```

### 2. llama-server起動

```bash
# ターミナル1: llama-server
cd ~/bonsai-agent
./scripts/start-server.sh
```

### 3. bonsai-agent起動

```bash
# ターミナル2: エージェント
cargo run
```

### モックモード（LLMなしでテスト）

```bash
cargo run -- --mock --exec "こんにちは"
cargo run -- --mock    # 対話モード
```

## CLI

```
bonsai-agent [OPTIONS]

Options:
  --server-url <URL>   llama-serverのURL [default: http://localhost:8080]
  --exec <TEXT>        単発実行
  --mock               モックモード
  --sessions           セッション一覧
  -h, --help           ヘルプ
  -V, --version        バージョン
```

## ツール

| ツール | 権限 | 機能 |
|--------|------|------|
| shell | Confirm | シェルコマンド実行 |
| file_read | Auto | ファイル読み取り |
| file_write | Confirm | ファイル書き込み（全文 or diff適用） |
| git | Confirm | Git操作（status/diff/log/commit/add/branch） |

## 設定

`~/.config/bonsai-agent/config.toml`（自動生成されない。必要時に作成）

```toml
[model]
server_url = "http://localhost:8080"
model_id = "bonsai-8b"
context_length = 16384

[agent]
max_iterations = 10
max_retries = 3
shell_timeout_secs = 30

[safety]
deny_paths = ["~/.ssh", "~/.gnupg", "~/.aws"]
```

## 開発

```bash
cargo test                     # 154テスト
cargo clippy -- -D warnings    # リント
cargo fmt -- --check           # フォーマット
```

## ライセンス

MIT
