# bonsai-agent

Bonsai-8B（1ビット量子化Qwen3-8B、1.28GB）で動作するRust製自律型AIエージェント。

Mac M2 16GBで完結。外部クラウドAPI不要。ローカルLLMだけで自律的にタスクを実行し、経験から学習する。

## 特徴

- **1.1GBのLLM** — Bonsai-8B（1ビット量子化）でツール呼び出し・コード理解・Web検索が可能
- **自己進化** — 経験を自動記録、3回成功でスキルに昇格、arxiv論文を自動収集して知識を蓄積
- **フロー→ストック** — 会話の中から意思決定・学び・TODOを自動抽出しmdファイルに蓄積（Karpathyパターン）
- **安全設計** — Sandbox、パスガード、秘密情報フィルタ、段階的自律レベル、セーフモード
- **拡張可能** — TOMLプラグイン、MCPクライアント、pre/postフック

## クイックスタート

### 1. Bonsai-demoセットアップ（初回のみ）

```bash
cd ~
git clone https://github.com/PrismML-Eng/Bonsai-demo.git
cd Bonsai-demo
sh scripts/download_binaries.sh
curl -L -o models/gguf/8B/Bonsai-8B.gguf \
  "https://huggingface.co/prism-ml/Bonsai-8B-gguf/resolve/main/Bonsai-8B.gguf"
```

### 2. llama-server起動

```bash
cd ~/bonsai-agent
./scripts/start-server.sh
```

### 3. bonsai-agent起動

```bash
cargo run
```

```
bonsai> このディレクトリのファイル一覧を見せて
bonsai> Cargo.tomlの中身を読んで
bonsai> Rustについて検索して
bonsai> exit
```

### モックモード（LLMなしで動作確認）

```bash
cargo run -- --mock --exec "こんにちは"
cargo run -- --mock    # 対話モード
```

## CLI

```
cargo run                              # 対話モード
cargo run -- --exec "..."              # 単発実行
cargo run -- --mock                    # モックモード（LLMなし）
cargo run -- --sessions                # セッション一覧
cargo run -- --resume <ID>             # セッション再開
cargo run -- --tasks                   # 未完了タスク一覧
cargo run -- --audit                   # 監査ログ
cargo run -- --vault                   # ナレッジVault概要
cargo run -- --manifest                # ケイパビリティ一覧
cargo run -- --server-url <URL>        # カスタムサーバーURL
```

## ツール

| ツール | 権限 | 機能 |
|--------|------|------|
| `shell` | Confirm | シェルコマンド実行（Sandbox経由） |
| `file_read` | Auto | ファイル読み取り |
| `file_write` | Confirm | ファイル書き込み（全文 or search/replace差分） |
| `git` | Confirm | Git操作（status/diff/log/commit/add/branch） |
| `web_search` | Auto | Web検索（DuckDuckGo API） |
| `web_fetch` | Auto | URLからテキスト取得 |
| `repo_map` | Auto | コード構造マップ（関数/構造体名抽出） |
| **プラグイン** | 設定可能 | TOML定義でカスタムツール追加 |
| **MCP** | Confirm | MCPサーバーのツールを利用 |

## アーキテクチャ

```
ユーザー入力
 ↓
ハイブリッド検索（FTS5+ベクトル）→ 関連メモリをプロンプトに注入
 ↓
過去の経験（成功/失敗）→ プロンプトに注入
 ↓
LLM推論（Bonsai-8B via llama-server）
 ↓
パース → バリデーション → ツール実行
 ↓                              ↓
秘密フィルタ適用              監査ログ記録
 ↓
経験自動記録 → 3回成功でスキル昇格
 ↓
ナレッジVault（フロー→ストック自動抽出 → mdファイル蓄積）
 ↓
セッション永続化
```

## 設定

`~/.config/bonsai-agent/config.toml`（オプション、なくてもデフォルト値で動作）

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

[hooks]
pre_tool = ["echo $BONSAI_TOOL_NAME"]
post_tool = ["logger -t bonsai $BONSAI_TOOL_NAME"]

[[plugins.tools]]
name = "weather"
command = "curl -s 'wttr.in/{location}?format=3'"
description = "天気を取得する"
permission = "auto"
[plugins.tools.parameters]
location = { type = "string", description = "都市名" }

[[mcp.servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
```

## ナレッジVault

会話（フロー）から重要な情報（ストック）を自動抽出し、mdファイルに蓄積。

```
~/.local/share/bonsai-agent/vault/
├── decisions.md      # 意思決定（「〜にした」）
├── facts.md          # 事実（「〜である」）
├── preferences.md    # 好み（「〜がいい」）
├── patterns.md       # パターン
├── insights.md       # 洞察（「〜とわかった」）
└── todos.md          # やるべきこと（「〜する必要がある」）
```

Obsidian等で直接閲覧・編集可能。

## 自己進化

```
経験自動記録 → メモリ蓄積 → 次回検索・注入 → スキル昇格
                                          ↑
arxiv論文自動収集 → 知識蓄積 ──────────────┘
```

- 成功/失敗/洞察を自動記録（`ExperienceStore`）
- 同じツールチェーンが3回成功 → スキルに自動昇格（`SkillStore`）
- ユーザーの修正（「違う」）/強化（「完璧」）を検出し高信頼度で記録
- arxiv APIから関心領域の論文を自動収集（`EvolutionEngine`）
- 定期振り返りレポート（`Dreamer`）
- **能動的自己改善**（`apply_improvements`）:
  - 3回以上失敗したコマンドパターンを自動検出 → 警告メモリに蓄積
  - スキル不足を検出 → スキル化の提案を自動記録
  - 成功率低下を検出 → プロンプト改善の提案を自動記録
  - 頻出ツールを検出 → 精度向上の優先度を自動記録
  - 全てメモリに蓄積され、次回セッションでプロンプトに自動注入

## 開発

```bash
cargo test                     # 302テスト
cargo clippy -- -D warnings    # リント
cargo fmt -- --check           # フォーマット
cargo build --features full    # fastembed有効化ビルド
```

## 必要環境

- macOS (Apple Silicon) or Linux
- Rust 1.80+ (edition 2024)
- llama-server（[PrismML Bonsai-demo](https://github.com/PrismML-Eng/Bonsai-demo)から取得）

## ライセンス

MIT
