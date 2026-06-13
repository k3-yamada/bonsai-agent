# bonsai-agent

[English](README.en.md) | **日本語**

Bonsai-8B（1ビット量子化Qwen3-8B、1.28GB）で動作するRust製自律型AIエージェント。

Mac M2 16GBで完結。外部クラウドAPI不要。ローカルLLMだけで自律的にタスクを実行し、経験から学習する。

## 特徴

- **1.28GBのLLM** — Bonsai-8B（1ビット量子化）でツール呼び出し・コード理解・Web検索が可能
- **自己進化** — 経験を自動記録、3回成功でスキルに昇格、arxiv論文を自動収集して知識を蓄積
- **フロー→ストック** — 会話の中から意思決定・学び・TODOを自動抽出しmdファイルに蓄積（Karpathyパターン）
- **安全設計** — Sandbox、パスガード、秘密情報フィルタ、段階的自律レベル、セーフモード
- **拡張可能** — TOMLプラグイン、MCPクライアント、pre/postフック
- **豊富なハーネスパターン** — 「Scaffolding > Model」原則で1ビットモデルの信頼性を底上げ（~1,500 テスト、設計原則は [CLAUDE.md](CLAUDE.md) / [docs/quality/lab-history.md](docs/quality/lab-history.md)）
- **設計思想の錨（[VALUES.md](docs/VALUES.md)）** — V1〜V7 の価値観を明文化、Goodhart's Law 監視（指標の形骸化検出、env-gated）で自己の変質を警戒
- **LLM-as-judge 評価基盤** — Judge Gate + ルーブリック採点でベンチマークを 22→40 タスクに拡張
- **MLXバックエンド対応** — llama-serverに加え、mlx-lm（Apple Silicon最適化）でも推論可能
- **メモリ最適化 sidecar** — `start-mlx-sidecar.sh` で KV cache 量子化 (-71%) + `mx.set_cache_limit` 制御。M2 16GB で swap 阻止
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

Ternary Bonsai 8B の MLX 版を 2 通りで起動可能:

```bash
# セットアップ（初回のみ）
./scripts/setup_mlx_ternary.sh

# A. cubist mlx-openai-server（port 8000、標準）
./scripts/start-mlx-server.sh &

# B. メモリ最適化 sidecar（port 8888、M2 16GB 推奨）
./scripts/start-mlx-sidecar.sh &
```

**B（sidecar）** は KV cache 量子化 (-71%, kv4@6417tok) と `mx.set_cache_limit` による swap 阻止を実装。`BONSAI_MLX_CACHE_LIMIT_GB` / `BONSAI_MLX_KV_BITS` / `BONSAI_MLX_QUANTIZED_KV_START` 等の env で制御。詳細は [docs/execution/runbook.md](docs/execution/runbook.md) §Phase 2 メモリ最適化。

config.toml でバックエンドを切替（port は使用するスクリプトに合わせる）:

```toml
[model]
backend = "mlx-lm"
server_url = "http://localhost:8888"  # sidecar 使用時。cubist は 8000
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
cargo run -- --list-tools              # 登録ツール一覧（whitelist 適用後の live registry）
cargo run -- --ingest <PATH>           # .md/.txt を memory に取り込む（知識デーモン）
cargo run -- --dashboard               # 統合統計ダッシュボード
cargo run -- --checkpoints             # チェックポイント一覧
cargo run -- --rollback <id>           # チェックポイント復元
cargo run -- --lab                     # 自律的自己改善ループ
cargo run -- --init                    # config.tomlテンプレート生成
cargo run -- --skills-export           # スキルをMarkdownにエクスポート
cargo run -- --diagnose                # サーバー接続診断
cargo run -- --evolve                  # arxiv収集+自己改善
cargo run -- --serve --api-port <PORT> # REST API サーバー
cargo run -- --mcp-server              # MCP サーバーとして起動
cargo run -- --server-url <URL>        # カスタムサーバーURL
```

### 環境変数（主要）

完全な一覧は [docs/execution/runbook.md](docs/execution/runbook.md) を参照。

| 環境変数 | 既定 | 用途 |
|----------|------|------|
| `BONSAI_DB_PATH` | OS data_dir | memory DB パス上書き（テスト隔離・本番非汚染に使用） |
| `BONSAI_ENABLED_TOOLS` | 未設定 | deny-by-default ツール whitelist。列挙したツールのみ有効化（未設定で全ツール） |
| `BONSAI_LAB_SMOKE` | 未設定 | smoke モード。readonly ツールのみ自動許可 + コンテキスト縮小 |

```bash
# 例: readonly ツールだけを有効化して安全に動作確認
BONSAI_ENABLED_TOOLS=file_read,recall cargo run -- --list-tools
# => 登録ツール 2 件: file_read / recall

# smoke モード（readonly default = file_read/repo_map/recall/web_fetch/web_search/arxiv_search）
BONSAI_LAB_SMOKE=1 cargo run -- --list-tools
```

## ビルド feature flags

`Cargo.toml` の `default = ["cli", "tree-sitter", "embeddings"]` で **3 機能すべてデフォルト ON**。`cargo build` / `cargo run` では追加 flag 不要で本格構成 (sqlite-vec vec0 KNN + fastembed + tree-sitter) が有効。

| feature | 内容 | デフォルト |
|---|---|---|
| `cli` | clap CLI flags | ✅ ON |
| `tree-sitter` | RepoMap (Rust/Python/TS/JS/Go) | ✅ ON |
| `embeddings` | fastembed (AllMiniLML6V2) + sqlite-vec vec0 ANN 検索 | ✅ ON |

**hash-only / 軽量 build** (CI、test、組込み等) は明示的に opt-out:

```bash
cargo build --release --no-default-features --features cli,tree-sitter
```

この場合 `HybridSearch::vector_search` は線形 scan path に切替 (compile-time exclusive、ランタイム分岐なし)。embedding は SimpleEmbedder (ハッシュベース) のため semantic search は機能しないが、ビルド/テストは完走する。

### ローカル埋め込み（offline / MLX 経由）

`embeddings` feature は内部で `ort`（ONNX Runtime）の prebuilt バイナリを **ビルド時に DL** し、`fastembed` モデルを **実行時に Hugging Face から DL** する。ネットワーク制限環境ではこの両方が失敗する。

これを避けてローカル完結させるには、MLX sidecar の `/v1/embeddings`（OpenAI 互換）を使う:

```bash
# 1. sidecar を起動（mlx-embeddings 同梱、初回リクエストまで埋め込みモデルは lazy load）
./scripts/start-mlx-sidecar.sh &

# 2. bonsai 側で HttpEmbedder を有効化
export BONSAI_EMBED_URL=http://localhost:8888
cargo run --no-default-features --features cli,tree-sitter
```

`BONSAI_EMBED_URL` を設定すると `create_embedder()` が `HttpEmbedder`（fastembed/ONNX 非依存）を最優先で採用するため、**ort のバイナリDLなしに実埋め込みが使える**。sidecar が落ちている場合は hash 埋め込みに graceful fallback する。埋め込みモデルは `BONSAI_MLX_EMBED_MODEL`（既定 `mlx-community/all-MiniLM-L6-v2-4bit`）で変更可能。

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
| `remember` | Auto | 事実・好みを memory に保存（知識デーモン） |
| `recall` | Auto | 過去の memory を検索（知識デーモン、FTS5+ベクトル） |
| **プラグイン** | 設定可能 | TOML定義でカスタムツール追加 |
| **MCP** | Confirm | MCPサーバーのツールを利用 |

`BONSAI_ENABLED_TOOLS` / `BONSAI_LAB_SMOKE` で有効化するツールを制限できる（deny-by-default、上記「環境変数」参照）。`--list-tools` で実際に有効なツール集合を確認可能。

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
# macOS: ~/Library/Application Support/bonsai-agent/vault/
# Linux: ~/.local/share/bonsai-agent/vault/
vault/
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
- ユーザーの修正（「違う」「訂正」）/強化（「完璧」「なるほど」）を agent loop で検出しログ記録（日英対応、`detect_feedback`。DB永続化・検索重み更新は今後の課題）
- arxiv APIから関心領域の論文を自動収集（`EvolutionEngine`）
- 定期振り返りレポート（`Dreamer`）
- **能動的自己改善**（`apply_improvements`）:
  - 3回以上失敗したコマンドパターンを自動検出 → 警告メモリに蓄積
  - スキル不足を検出 → スキル化の提案を自動記録
  - 成功率低下を検出 → プロンプト改善の提案を自動記録
  - 頻出ツールを検出 → 精度向上の優先度を自動記録
  - 全てメモリに蓄積され、次回セッションでプロンプトに自動注入

## Lab実機テスト結果

Bonsai-8B 1bit、k=3、10 cycle paired による変異評価。全履歴・詳細は [docs/quality/lab-history.md](docs/quality/lab-history.md) を参照。

### 現状（2026-06時点）
- **天井 10 連続 REJECT**（v17〜v21）— デフォルト設定が最適解に収束し、ハーネス機構は最適化済み
- **paired evidence 規律**（[ADR-003](docs/decisions/ADR-003-paired-evidence-over-unpaired.md)）— unpaired single-cycle の ACCEPT が paired re-eval で覆る例を複数確認 → cherry-picked noise を決定的に排除
- **能力プロファイル**（AgentFloor T1-T6）: T1 Instruct=0.68 / T3 ToolSelect=0.77 / **T6 LongHorizon=0.47（最弱）** — tier-targeted 変異は T6 偏向で攻略
- baseline score ≈ 0.82（smoke: score=0.8209 / pass@k=1.0）

### デフォルト化済み変異（Lab ACCEPT → 恒久適用）
- 「計画強制」ルール（+0.025、v1）
- 「ツール使用前に `<think>` で意図記述」（+0.032、v5）
- 「フォールバック戦略」（+0.001、v5）
- 「回答前ファイル内容確認」（+0.0157、v9）

## ハーネスパターン

「Scaffolding > Model」設計原則に基づく、1ビットモデルの信頼性向上パターン（代表例）:

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

設計原則と代表的なパターンは [CLAUDE.md](CLAUDE.md) を参照（網羅的な実験ログは開発者向けの内部ノートに保管）。

## 開発

```bash
cargo test --lib               # ~1,500 テスト
cargo test --test structural   # レイヤー/サイズ/eprintln lint（Z-4）
cargo clippy --lib -- -D warnings  # リント
cargo fmt -- --check           # フォーマット
```

開発フローの詳細（Lab 起動、env 一覧、smoke 手順）は [docs/execution/runbook.md](docs/execution/runbook.md)、設計判断は [docs/decisions/](docs/decisions/)（ADR-001〜011）、設計思想は [docs/VALUES.md](docs/VALUES.md) を参照。

## 必要環境

- macOS (Apple Silicon) or Linux
- Rust 1.80+ (edition 2024)
- llama-server（[PrismML Bonsai-demo](https://github.com/PrismML-Eng/Bonsai-demo)から取得）
- または mlx-lm + mlx-openai-server（`./scripts/setup_mlx_ternary.sh` でセットアップ）

## ライセンス

MIT
