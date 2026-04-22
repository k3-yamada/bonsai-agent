# bonsai-agent

Bonsai-8B（1ビット量子化Qwen3-8B、1.28GB）で動作するRust製自律型AIエージェント。

Mac M2 16GBで完結。外部クラウドAPI不要。ローカルLLMだけで自律的にタスクを実行し、経験から学習する。

## 特徴

- **1.1GBのLLM** — Bonsai-8B（1ビット量子化）でツール呼び出し・コード理解・Web検索が可能
- **自己進化** — 経験を自動記録、3回成功でスキルに昇格、arxiv論文を自動収集して知識を蓄積
- **フロー→ストック** — 会話の中から意思決定・学び・TODOを自動抽出しmdファイルに蓄積（Karpathyパターン）
- **安全設計** — Sandbox、パスガード、秘密情報フィルタ、段階的自律レベル、セーフモード
- **拡張可能** — TOMLプラグイン、MCPクライアント、pre/postフック
- **148のハーネスパターン** — 1ビットモデルの信頼性をスキャフォールディングで底上げ（880テスト、69ソースファイル）
- **MLXバックエンド対応** — llama-serverに加え、mlx-lm（Apple Silicon最適化）でも推論可能
- **ミドルウェアチェーン** — DeerFlow知見による5段パイプライン（Audit→ToolTrack→Stall→Compact→TokenBudget）
- **読取ツール並列実行** — 連続読取2件以上で自動並列化（書き込みはバリア逐次）
- **型駆動ツール定義** — schemars JsonSchema deriveでスキーマ自動生成+型安全パース（TypedToolトレイト）
- **TTL情報鮮度管理** — expires_atカラム+セッション開始時自動パージで陳腐化情報を防止
- **ADR自動生成** — Replan/Advisor介入時の意思決定をMarkdownでVaultに蓄積
- **不変条件チェック** — タスク完了時にツール成功率・回答品質を自動検証

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

### MLXバックエンド（Apple Silicon向け代替）

Ternary Bonsai 8BのMLX版を使う場合:

```bash
# セットアップ（初回のみ）
./scripts/setup_mlx_ternary.sh

# サーバー起動
~/.venvs/bonsai-mlx/bin/mlx-openai-server launch \
  --model-path prism-ml/Ternary-Bonsai-8B-mlx-2bit \
  --model-type lm --port 8000
```

config.tomlでバックエンドを切替:

```toml
[model]
backend = "mlx-lm"
server_url = "http://localhost:8000"
model_id = "ternary-bonsai-8b"
context_length = 65536
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
cargo run -- --dashboard               # 統合統計ダッシュボード
cargo run -- --checkpoints             # チェックポイント一覧
cargo run -- --rollback <id>           # チェックポイント復元
cargo run -- --lab                     # 自律的自己改善ループ
cargo run -- --init                    # config.tomlテンプレート生成
cargo run -- --skills-export           # スキルをMarkdownにエクスポート
cargo run -- --diagnose                # サーバー接続診断
cargo run -- --evolve                  # arxiv収集+自己改善
cargo run -- --server-url <URL>        # カスタムサーバーURL
```

## ツール

| ツール | 権限 | 機能 |
|--------|------|------|
| `shell` | Confirm | シェルコマンド実行（Sandbox経由） |
| `file_read` | Auto | ファイル読み取り（並列実行対応） |
| `file_write` | Confirm | ファイル書き込み（全文 or search/replace差分、fuzzy 9戦略） |
| `multi_edit` | Confirm | 単一ファイル複数箇所一括編集（アトミック操作） |
| `git` | Confirm | Git操作（status/diff/log/commit/add/branch） |
| `web_search` | Auto | Web検索（DuckDuckGo API） |
| `web_fetch` | Auto | URLからテキスト取得 |
| `repo_map` | Auto | コード構造マップ（Rust/Python/TS/JS/Go/Java/C/C++/Kotlin/Swift対応） |
| `arxiv_search` | Auto | arxiv論文検索 |
| **プラグイン** | 設定可能 | TOML定義でカスタムツール追加 |
| **MCP** | Confirm | MCPサーバーのツールを利用 |

読取専用ツール（`file_read`, `web_search`, `web_fetch`, `repo_map`）は`is_read_only()`トレイトにより、連続2件以上で自動並列実行される。書き込みツールはバリアとして逐次実行を保証。

## アーキテクチャ

```
ユーザー入力
 ↓
ハイブリッド検索（FTS5+ベクトル）→ 関連メモリをプロンプトに注入
 ↓
過去の経験（成功/失敗）→ プロンプトに注入
 ↓
LLM推論（Bonsai-8B via llama-server / mlx-lm）
 ↓
パース → バリデーション → ツール実行
 ↓                              ↓
秘密フィルタ適用              監査ログ記録
 ↓
ミドルウェアチェーン（5段パイプライン）
 ├── AuditMiddleware      — ステップ監査
 ├── ToolTrackMiddleware   — ツール使用追跡
 ├── StallMiddleware       — 停滞検出→再計画
 ├── CompactMiddleware     — コンテキスト圧縮
 └── TokenBudgetMiddleware — トークン予算管理
 ↓
経験自動記録 → 3回成功でスキル昇格
 ↓
ナレッジVault（フロー→ストック自動抽出 → mdファイル蓄積）
 ↓
セッション永続化
```

## 設定

`~/Library/Application Support/bonsai-agent/config.toml`（macOS）または `~/.config/bonsai-agent/config.toml`（Linux）。オプション、なくてもデフォルト値で動作。

`cargo run -- --init` でテンプレートを生成可能。

```toml
[model]
server_url = "http://localhost:8080"
model_id = "bonsai-8b"
context_length = 16384
# backend = "mlx-lm"  # MLXバックエンドを使う場合

[model.inference]
temperature = 0.5
top_p = 0.85
top_k = 20
min_p = 0.05
max_tokens = 1024
repeat_penalty = 1.15

[agent]
max_iterations = 10
max_retries = 3
shell_timeout_secs = 30
auto_checkpoint = true

[advisor]
# api_key は環境変数 OPENAI_API_KEY / ANTHROPIC_API_KEY から自動検出
# api_model = "gpt-4o-mini"
# timeout_secs = 30

[experiment]
dreamer_interval = 5
max_experiments = 10

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

# HTTP transport（Streamable HTTP MCP）
# [[mcp.servers]]
# name = "remote"
# command = ""
# args = []
# url = "http://localhost:3000/mcp"
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

## Lab実機テスト結果

Bonsai-8B 1bit、k=3、自律的自己改善ループによる変異評価。

### v8結果（最新、2026-04-19）— 全10実験REJECT、最適解収束
- ベースライン: **score=0.8517**, pass@k=0.9167
- 全10実験REJECT — デフォルト設定が最適解に完全収束
- Adam's Lawリライト+6機能追加後もベースライン維持（劣化なし）

### v6.2結果
- ベースライン: score=0.8517, pass@k=0.9167

### v5結果 — 承認率40%（v3の4倍）
- ベースライン: score=0.8429, pass@k=0.9167
- **ACCEPT 1**: 「ツール使用前に思考を強制」(+0.032) → デフォルト化済
- **ACCEPT 2**: 「フォールバック戦略」(+0.001) → デフォルト化済
- 承認率: **2/5 (40%)**

### v3結果 — ベースライン+1.9%
- ベースライン: **score=0.8762**, pass@k=1.0（完全安定）
- 全変異REJECT → デフォルト設定が最適解に収束

### v1結果 — 初回計測
- ベースライン: score=0.8596, pass@k=1.0
- 唯一のACCEPT: 「計画強制」ルール（+0.025）→ デフォルト化

## ハーネスパターン（135項目）

「Scaffolding > Model」設計原則に基づく、1ビットモデルの信頼性向上パターン:

- **pass^k評価**: 各タスクk回実行、連続成功率で変異効果を検出
- **Continue Sites**: 連続失敗→リトライ→再計画→安全停止の3段エスカレーション
- **2層LoopDetector**: salient hash + 頻度閾値 + 循環パターン検出
- **StallDetector**: 進捗なし検出→Advisor連携で再計画注入
- **fuzzyマッチ9戦略**: 空白正規化/Trim/インデント柔軟/Unicode/エスケープ/Blockアンカー/境界Trim
- **Deferred Schema**: ツールスキーマ名+説明のみでトークン80%節約
- **段階分離パイプライン**: 複雑タスク検出→計画プレステップ自動注入
- **Event Sourcing**: 統一イベントストリーム（リプレイ・分析対応）
- **Advisor Tool**: 簡潔化指示 + 完了前自己検証 + HttpAdvisor（OpenAI互換API委託）
- **ミドルウェアチェーン**: trait Middleware + MiddlewareChain（5段パイプライン）
- **読取ツール並列実行**: is_read_only() + std::thread::scope
- **MLXバックエンド**: ServerBackend enum（llama-server/mlx-lm切替）
- **InferenceParams**: temperature/top_p/top_k/min_p/max_tokens/repeat_penalty設定可能
- **構造化エラー分類12種**: FailureMode拡張 + RecoveryHint
- **ヘルスチェック統一**: /health + /v1/modelsフォールバック（MLX対応）

全135項目の詳細はCLAUDE.mdを参照。

## 開発

```bash
cargo test                     # 840テスト
cargo clippy -- -D warnings    # リント
cargo fmt -- --check           # フォーマット
cargo build --features full    # fastembed有効化ビルド
```

## 必要環境

- macOS (Apple Silicon) or Linux
- Rust 1.80+ (edition 2024)
- llama-server（[PrismML Bonsai-demo](https://github.com/PrismML-Eng/Bonsai-demo)から取得）
- または mlx-lm + mlx-openai-server（`./scripts/setup_mlx_ternary.sh` でセットアップ）

## ライセンス

MIT
