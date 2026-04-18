# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## プロジェクト概要

`bonsai-agent` — Bonsai-8B（1ビット量子化Qwen3-8B、1.28GB）で動作するRust製自律型エージェント。
Mac M2 16GB上でllama-server HTTP API経由で推論。694テスト、64ソースファイル。

設計原則: **「Scaffolding > Model」** — 1ビットモデルの改善余地は限定的。ハーネス側で信頼性を底上げする。

## ビルド・テストコマンド

```bash
cargo build                    # ビルド
cargo test                     # ユニットテスト（694テスト）
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
│   ├── middleware.rs               # ミドルウェアチェーン（5段: Audit/ToolTrack/Stall/Compact/TokenBudget）
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
│   └── model_router.rs            # ModelRouter + PipelineStage + AdvisorConfig
├── memory/
│   ├── store.rs                   # MemoryStore — SQLite A-MEM + セッション永続化
│   ├── experience.rs / skill.rs   # 経験記録、スキル自動昇格（3シグナルスコアリング）
│   ├── dreams.rs                  # Dreaming Light/Deep分離（dream_light/dream_deep）
│   ├── search.rs                  # ハイブリッド検索（FTS5+ベクトルRRF融合）
│   ├── feedback.rs                # Correction/Reinforcement検出
│   ├── graph.rs                   # KnowledgeGraph — グラフ構造連想記憶（BFS双方向探索）
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
    ├── schema.rs / migrate.rs     # 15テーブルSQLiteスキーマ（V5: knowledge_graphテーブル追加）
```

## 主要なトレイト

- `LlmBackend` — `generate(messages, tools, on_token, cancel) -> GenerateResult`
- `Tool` — `name(), description(), parameters_schema(), permission(), call(args), is_read_only()`
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
11. **RepoMap多言語対応**: Rust/Python/TS/JS + Go/Java/C/C++ + Kotlin/Swift、行番号付き
12. **段階分離パイプライン**: 複雑タスク検出→計画プレステップ自動注入（1bitモデル精度向上）
13. **Vault選択的注入**: ナレッジVaultの関連カテゴリのみをコンテキストに注入
14. **Event Sourcing**: 統一イベントストリーム（events テーブル、リプレイ・分析対応）
15. **Advisor Toolパターン**: 簡潔化指示（トークン35-45%削減）+ 完了前自己検証ステップ
16. **PipelineStage**: Explore/Plan/Execute/Verify/Advise の5段階ステージ定義
17. **AdvisorConfig**: max_uses制御 + 将来のAPI連携エンドポイント抽象化（AgentConfigに統合済、完了前自己検証を3回上限）
18. **inject_verification_step**: 検証ステップ関数化 + verification_prompt設定可能化（build_verification_prompt()で将来API差替）
19. **HttpAdvisor**: try_remote_advice()でOpenAI互換APIに検証委託（api_key/api_model/timeout_secs対応、失敗時ローカルフォールバック）
20. **AdvisorSettings**: config.toml [advisor]セクション + APIキー環境変数自動検出（OPENAI_API_KEY/ANTHROPIC_API_KEY）
21. **StallDetector→Advisor連携**: 停滞検出時にAdvisorRole::Replan経由で再計画注入、AdvisorRole enumで検証/再計画分離
22. **AuditAction::AdvisorCall**: 監査ログにadvisor呼出を記録（role/source/prompt_len/duration_ms）
23. **Advisorキャッシング**: 同一role+task_contextでHTTP重複回避（per-sessionキャッシュ、自動リセット）
24. **AdvisorStats**: AuditLog::advisor_stats(session?)でtotal/role別/source別/avg_prompt_len/avg_remote_duration_ms集計
25. **SQLiteチェックポイント**: SCHEMA_V4 checkpoints テーブル + CheckpointManager::with_persistence/load_persisted（再起動後復元可）
26. **自動チェックポイント**: AgentConfig.auto_checkpoint=true でタスク開始時に自動CP（git stash+DB永続化）
27. **CheckpointStats**: CheckpointManager::stats(session?)でtotal/rolled_back/with_git_ref、rollback_rate/git_capture_rate集計
28. **Rollback CLI**: `--checkpoints` 一覧表示 + `--rollback <id>` で復元（DB+git stash apply）
29. **ダッシュボード**: `--dashboard` でAdvisor/Checkpoint/Lab実験/監査ログの統合統計ビュー
30. **RepoMap v2**: Kotlin/Swift対応 + 行番号(L42:) + pub(crate)/async def/interface/type対応（依存追加なし）
31. **Advisor起動ログ**: log_startup()でリモート/ローカル設定を表示、新規セッション開始時のみ
32. **--init CLI**: config.tomlテンプレート生成 + Advisor設定ガイド表示
33. **LoopState構造体**: 6変数を集約、ミドルウェアチェーン化の基盤
34. **handle_outcome()**: outcomeディスパッチを単一関数に分離、OutcomeAction enum
35. **kv_cache_type反映**: spawn()にパラメータ追加（config.toml→llama-server直通）
36. **--flash-attn on**: llama-server仕様要件の値`on`追加（性能バグ修正）
37. **ExperimentConfig config.toml統合**: [experiment]セクション、dreamer_interval/max_experiments設定可能
38. **構造化ログ**: observability/logger.rs — LogLevel(Error/Warn/Info/Debug) + BONSAI_LOG envフィルタ
39. **eprintln→log_event変換**: agent_loop/compaction/secrets/embedder の12+箇所をカテゴリ付きログに
40. **fuzzyマッチ7戦略化**: 空白正規化/Trim/インデント柔軟/Unicode正規化/エスケープ/Blockアンカー/境界Trim
41. **Context fencing**: メモリ/経験/Vault注入を`<memory-context>`タグで囲み（hermes-agentパターン）
42. **FileStuckGuard**: ファイル単位スタック段階エスカレーション（3回→nudge, 6回→give up、macOS26/Agent知見）
43. **Anti-Hallucinationルール**: ツール結果なし主張禁止+同ファイル連続再読込禁止（macOS26/Agent知見）
44. **TokenBudgetTracker**: 累積トークン予算追跡+diminishing returns検出（macOS26/Agent知見）
45. **型強制(Type Coercion)**: ツール引数の文字列→数値/bool自動変換（hermes-agent知見）
46. **Handoff framing圧縮**: compact_level3に「引継ぎサマリー」(Resolved/Remaining Work)追加
47. **Lab ACCEPT変異デフォルト化**: 「ツール使用前に必ず<think>で意図と期待結果を書く」(+0.032実証)
48. **構造化エラー分類12種**: FailureMode+4種(ContextOverflow/RateLimited/NetworkError/ServerDisconnect) + RecoveryHint
49. **Ternary Bonsai config**: ModelConfig.gguf_path + model_id="ternary-bonsai-8b" + context_length=65536対応
50. **ACCEPT変異2デフォルト化**: 「ツール結果が期待と違う場合、別のツールを試す」(Lab +0.001実証)
51. **XMLタグプロンプト**: `<tool_persistence>`でツール使用強制（hermes-agent最終知見）
52. **Ternary DLスクリプト**: scripts/download_ternary.sh（HF CLI + llama-server起動例）
53. **llama-server死活監視**: wait_for_health()で自動復帰待機（macOS26/Agent知見）
54. **ミドルウェアチェーン**: trait Middleware + MiddlewareChain（DeerFlow知見、5段パイプライン: Audit→ToolTrack→Stall→Compact→TokenBudget）
55. **読取ツール並列実行**: Tool::is_read_only() + std::thread::scope（連続読取2件以上で並列化、書き込みはバリア逐次）
56. **MLXバックエンド対応**: ServerBackend enum（llama-server/mlx-lm）、mlx-lmデフォルトポート8000自動検出、setup_mlx_ternary.shスクリプト
57. **ミドルウェアチェーン統合**: handle_outcome Continue分岐でMiddlewareChain.run_after_step()実行、LoopStateに組込
58. **InferenceParams設定可能化**: config.toml [model.inference]でtemperature/top_p/top_k/min_p/max_tokens/repeat_penalty調整可能
59. **全OutcomeにMW適用**: FinalAnswer/Abortedにもミドルウェアチェーンで監査ログ統一
60. **ヘルスチェック統一**: wait_until_healthy/is_healthyに/v1/modelsフォールバック（MLX対応）
61. **ストリーミング応答**: SSEパース+on_tokenリアルタイムコールバック+非ストリーミングフォールバック
62. **ベンチマーク12タスク化**: コード生成/マルチステップ/エラー処理/要約の4タスク追加
63. **MLX互換パラメータ**: mlx_compatibleフラグでtop_k/min_p除外+repetition_penalty変換
64. **--diagnose CLI**: サーバー接続+モデル一覧+テストプロンプト+InferenceParams表示の診断コマンド
65. **Tool Send+Sync安全性**: std::thread::scope並列呼び出しテスト+MiddlewareChainスレッド安全性テスト
66. **エラーメッセージ改善**: バックエンド種別+トラブルシューティングヒント付きエラー表示
67. **ストリーミングトークン概算**: SSE usageなし時にバイト数*0.4でトークン概算フォールバック
68. **Hyperagentsメタ変異**: AcceptedMutationアーカイブ+MetaMutationGenerator複合変異+estimate_mutation_effect効果推定
69. **InferenceParamsプリセット**: mlx_optimized/llama_server_defaultプリセット（ワンライン設定切替）
70. **タスク種別ツール制限**: TaskType enum + detect_task_type + select_relevant_with_type（不要ツール除外でコンテキスト節約）
71. **グラフ構造連想記憶**: KnowledgeGraph — SQLite V5、ノード+エッジ+BFS双方向探索（連想検索の精度向上）
72. **Agent Skills段階的開示**: summary/tags + format_schemas_progressive（タスク種別に応じたスキーマ段階開示）
73. **seed注入**: with_seed()でMLX決定論的出力対策（再現性向上）
74. **RepoMap tree-sitter AST**: Rust/Python/TS/JS/Goはtree-sitter ASTベースのシンボル抽出（正確なネスト構造、impl trait for対応）、Java/C/C++/Kotlin/Swiftは正規表現フォールバック
75. **PageRank RepoMap**: ファイル間依存グラフ+PageRankで重要ファイルを上位表示（Aider方式、gen_map_ranked()がデフォルト）
76. **Vault Rules vs Docs分離**: Decision/Patternは常時注入（Rules）、Fact/Insight/Preference/Todoはタスクコンテキスト連動（Docs）
77. **Experience Replay注入**: 類似タスクの成功/失敗/学び経験を<experience-context>タグで注入（同じ失敗の回避）
78. **コンパクションlevel3改善**: 引継ぎサマリーにツール使用統計+最終成果+未解決事項を保持
79. **ToolResultCache**: 読取専用ツール結果のセッション内キャッシュ（get/put/invalidate/stats）
80. **コンテキスト注入タグ統一**: 全注入を`<context type="xxx">`フォーマットに統一（memory/experience/vault-rules/vault-docs/skills/graph）
81. **ToolResultCache統合**: LoopState組込+execute_validated_callsでキャッシュヒット/保存+書込後invalidate
82. **agent_loop.rsリファクタ**: tool_exec.rs（ツール実行）+context_inject.rs（コンテキスト注入）に分割、2144行→1817行
83. **Preserved Thinking保持**: コンパクション時に<think>ブロックの結論部分を[Preserved Thinking]セクションで保持（GLM-5.1知見）
84. **重要度ベース適応的コンパクション**: メッセージ重要度スコア（User=1.0〜Toolエラー=0.2）で削除優先度決定（GLM-5.1 DSA知見）
85. **TrialSummary試行記憶**: 失敗した試行の履歴を構造化保持、Replan時に注入して同じ失敗を回避（GrandCode知見）
86. **環境障害フィルタ**: ServerDisconnect/NetworkErrorを分類し再計画ではなくリトライ優先（GLM-5.1知見）
87. **仮説→検証→計画ループ**: inject_planning_stepを仮説提案→小テスト検証→計画作成の3段階に強化（GrandCode知見）
88. **検証チェックリスト**: inject_verification_stepに根拠確認・未検証仮定・エッジケースのチェックリスト追加（PaperOrchestra知見）
89. **Claude Code Advisorバックエンド**: `claude -p`サブプロセスでAdvisor応答取得（Pro/Team契約内、API料金ゼロ、config.toml backend="claude-code"）

## Lab実機テスト結果（Bonsai-8B 1bit, k=3, 10サイクル）

### v6.2結果（2026-04-17）— ベースライン計測（進行中）
- ベースライン: score=0.8517, pass@k=0.9167
- v8系ハーネス改善（69-73項目）後の計測

### v5結果（2026-04-17）— 承認率40%（v3の4倍）
- ベースライン: score=0.8429, pass@k=0.9167（v3比-3.8%、advisor検証が原因→修正済）
- **ACCEPT 1**: 「ツール使用前に思考を強制」(+0.032, score=0.8749) → **デフォルト化済**
- **ACCEPT 2**: 「フォールバック戦略」(+0.001, score=0.8762) → **デフォルト化済**
- REJECT: エラー分析強制(-0.019), 不明(-0.042), 不明(-0.004)
- 承認率: **2/5 (40%)** — v3の1/10(10%)から大幅改善

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
