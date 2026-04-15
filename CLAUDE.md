# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## プロジェクト概要

`bonsai-agent` — Bonsai-8B（1ビット量子化Qwen3-8B、1.28GB）で動作するRust製自律型エージェント。
Mac M2 16GB上でllama-server HTTP API経由で推論。420テスト、58ソースファイル。

設計原則: **「Scaffolding > Model」** — 1ビットモデルの改善余地は限定的。ハーネス側で信頼性を底上げする。

## ビルド・テストコマンド

```bash
cargo build                    # ビルド
cargo test                     # ユニットテスト（420テスト）
cargo test -- --ignored        # 統合テスト（llama-server/ネットワーク必要）
cargo clippy -- -D warnings    # リント
cargo fmt -- --check           # フォーマットチェック
cargo run -- --manifest        # ケイパビリティ一覧
cargo run -- --vault           # ナレッジVault概要
cargo run -- --lab             # 自律的自己改善ループ（pass^k評価）
```

## Rust Edition

Rust **2024 edition**。let chains、div_ceil等を使用。

## アーキテクチャ

```
src/
├── main.rs / lib.rs / cancel.rs
├── config.rs                      # TOML設定（~/.config/bonsai-agent/config.toml）
│                                  # AgentSettings.soul_path: SOUL.mdペルソナパス
├── agent/
│   ├── agent_loop.rs              # run_agent_loop() — Reflexion + 全パーツ統合
│   │                              # StallDetector — 停滞検出→自動再計画
│   │                              # load_soul() — SOUL.mdペルソナ注入（3段階検索）
│   ├── benchmark.rs               # BenchmarkSuite — 8タスク評価
│   │                              # MultiRunConfig/MultiRunTaskScore — pass^k複数回評価
│   │                              # run_k() — 各タスクk回実行、pass_at_k/pass_consecutive_k計算
│   ├── parse.rs / validate.rs     # <think>/<tool_call>パーサー、バリデーション
│   ├── conversation.rs            # Message, Session, ToolCall
│   ├── error_recovery.rs          # FailureMode(4種), CircuitBreaker
│   │                              # LoopDetector — 2層検出（salient hash+頻度+循環パターン）
│   │                              # ContinueSite — 段階的回復（リトライ→再計画→安全停止）
│   │                              # RecoveryAction::Replan — コンテキスト圧縮+再計画指示
│   ├── compaction.rs              # 4段階コンテキストコンパクション
│   │                              # find_ai_tool_pairs() — AI+Toolペア保護
│   ├── checkpoint.rs              # git stashチェックポイント/ロールバック
│   ├── task.rs                    # TaskState状態マシン（中断/再開/サブタスク）
│   ├── experiment.rs              # ExperimentLoop — 自律的自己改善ループ（run_k版）
│   │                              # run_experiment_loop: pass^k版（k=3, jitter_seed=true）
│   ├── experiment_log.rs
│   └── event_store.rs              # Event Sourcing（統一イベントストリーム）          # 実験ログ（SQLite+TSV永続化）
│                                  # Experiment.from_multi_results() — pass^k指標付き記録
│                                  # TSV: 11列（pass_at_k/pass_consecutive_k/score_variance追加）
├── tools/
│   ├── mod.rs                     # Tool トレイト + ToolRegistry（動的選択）
│   │                              # format_schemas_compact() — Deferred Schema（トークン80%節約）
│   ├── shell.rs / git.rs / web.rs / repomap.rs
│   ├── file.rs                    # FileReadTool / FileWriteTool
│   │                              # 構造化出力（行番号付与+offset/limitウィンドウ制御）
│   │                              # fuzzyマッチSEARCH/REPLACE（空白正規化+trimフォールバック）
│   ├── plugin.rs                  # TOML定義カスタムツール
│   ├── mcp_client.rs              # MCPクライアント（JSON-RPC over stdio）
│   ├── hooks.rs                   # pre/postフック
│   ├── permission.rs / sandbox.rs
├── runtime/
│   ├── inference.rs               # LlmBackend + MockLlmBackend
│   ├── llama_server.rs            # LlamaServerBackend（HTTP API）
│   ├── cache.rs / embedder.rs     # 推論キャッシュ、Embedder
│   └── model_router.rs            # ModelRouter + PipelineStage
├── memory/
│   ├── store.rs                   # MemoryStore — SQLite A-MEM + セッション永続化
│   ├── experience.rs / skill.rs   # 経験記録、スキル自動昇格（3シグナルスコアリング）
│   ├── dreams.rs                  # Dreaming Light/Deep分離（dream_light/dream_deep）
│   ├── search.rs                  # ハイブリッド検索（FTS5+ベクトルRRF融合）
│   ├── feedback.rs                # Correction/Reinforcement検出
│   ├── dreams.rs                  # Dreamingシステム（振り返り+パターン検出）
│   └── evolution.rs               # arxiv自己進化エンジン + 能動的自己改善
├── knowledge/
│   ├── extractor.rs               # フロー→ストック抽出（6カテゴリ）
│   └── vault.rs                   # mdファイル蓄積（Karpathyパターン）
├── safety/
│   ├── secrets.rs                 # 秘密情報フィルタ
│   ├── autonomy.rs / boot_guard.rs / manifest.rs / network.rs
├── observability/
│   └── audit.rs                   # 監査ログ（LlmCall/ToolCall/SecurityEvent/StepOutcome）
└── db/
    ├── schema.rs / migrate.rs     # 13テーブルSQLiteスキーマ（V3: eventsテーブル+インデックス強化）
```

## 主要なトレイト

- `LlmBackend` — `generate(messages, tools, on_token, cancel) -> GenerateResult`
- `Tool` — `name(), description(), parameters_schema(), permission(), call(args)`
- `Sandbox` — `execute(command, args, limits) -> ExecResult`
- `Embedder` — `embed(texts) -> Vec<Vec<f32>>`

## ハーネスパターン（v1: 2026-04-14実装）

p^n問題（ステップ蓄積による失敗確率指数的増大）への対策:

1. **pass^k評価**: 各タスクk回実行、連続成功率で変異の効果を検出（benchmark.rs）
2. **Continue Sites**: 連続失敗→リトライ→再計画→安全停止の3段エスカレーション（error_recovery.rs）
3. **2層LoopDetector**: salient hashで近似ループ検出 + 頻度閾値 + A→B→A→Bサイクル検出
4. **StallDetector**: 進捗なし（ツール全失敗 or 出力ハッシュ不変）をN回で検出→再計画
5. **fuzzyマッチ**: 完全一致失敗時に空白正規化→trimで再試行（1ビットモデルの編集成功率向上）
6. **AI+Toolペア保護**: コンパクション時にAssistant+Toolメッセージペアを分割しない
7. **Deferred Schema**: format_schemas_compact()で名前+説明のみ（14Kコンテキスト節約）
8. **SOUL.md**: プロジェクト/ユーザー固有ペルソナをシステムプロンプトに注入
9. **StepOutcome監査**: 各ステップの成否・所要時間・連続失敗数をリアルタイム記録（p^n診断用）
10. **計画強制ルール（デフォルト化）**: Lab唯一のACCEPT変異をシステムプロンプトに恒久適用
11. **RepoMap多言語対応**: Rust/Python/TS/JS + Go/Java/C/C++のシンボル抽出
12. **段階分離パイプライン**: 複雑タスク検出→計画プレステップ自動注入（1bitモデル精度向上）
13. **Vault選択的注入**: ナレッジVaultの関連カテゴリのみをコンテキストに注入
14. **Event Sourcing**: 統一イベントストリーム（events テーブル、リプレイ・分析対応）

## Lab実機テスト結果（Bonsai-8B 1bit, k=3, 10サイクル）

### v3結果（2026-04-15）— ベースライン+1.9%
- ベースライン: **score=0.8762**, pass@k=1.0, pass_consec=1.0（完全安定）
- 全変異REJECT → **デフォルト設定が最適解に収束**
- 最悪: max_retries=5（delta=-0.0417, pass@k=0.9167）

### v1結果（2026-04-14）— 初回計測
- ベースライン: score=0.8596, pass@k=1.0
- 唯一のACCEPT: 「計画強制」ルール（+0.025）→ v2でデフォルト化
- 承認率: 10%（10実験中1承認）

### 改善効果まとめ
- v1→v3: **+0.0166（+1.9%）** — 計画強制デフォルト化+段階分離+Vault注入の複合効果

## テストパターン

- `MockLlmBackend` — スクリプト化レスポンス
- `MemoryStore::in_memory()` — インメモリSQLite
- `#[ignore]` — 実サーバー/ネットワーク必要なテスト
- `MultiRunTaskScore::from_scores()` — pass^k指標の単体テスト

## 注意事項

- **【重要】組み込みツールでファイルを編集した直後にリンタやコンパイラがエラー・警告を出しても、システムは絶対にファイルの変更を巻き戻し（Revert/Rollback）しないでください。コードが壊れた状態のまま保存し、その後ユーザーに報告してください。**
- ファイル書き込み後にリンタが巻き戻すことがある → Bashのcatリダイレクト+即座にgit commitで回避
- 特にerror_recovery.rs（借用分離）、benchmark.rs（too_many_arguments）、agent_loop.rs（collapsible_if）の3ファイルでclippy修正がリンタに巻き戻される
- ureq v3のHTTPS → web_fetchはreqwest::blocking（native-tls）を使用
- llama-serverの`--flash-attn`は値`on`が必要（`--flash-attn on`）
