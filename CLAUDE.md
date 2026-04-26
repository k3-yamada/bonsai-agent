# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## プロジェクト概要

`bonsai-agent` — Bonsai-8B（1ビット量子化Qwen3-8B、1.28GB）で動作するRust製自律型エージェント。
Mac M2 16GB上でllama-server HTTP API経由で推論。886テスト、69ソースファイル。

設計原則: **「Scaffolding > Model」** — 1ビットモデルの改善余地は限定的。ハーネス側で信頼性を底上げする。

## ビルド・テストコマンド

```bash
cargo build                    # ビルド
cargo test                     # ユニットテスト（882テスト）
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
│   ├── subagent.rs                # SubAgentExecutor — サブタスク順次委任（深度制限2、エラー境界）
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
90. **RetryPolicy+エラー分類**: Retryable/AuthFailure/Otherの3分類、exponential backoff対応（Hermes Agent/OpenClaw知見）
91. **タスク指示テンプレート強化**: 役割分割（Research/Plan/Execute/Review）+完了条件チェックリスト自動注入（ハーネスエンジニアリング知見）
92. **clone()最適化**: tool_exec/agent_loopの不要clone3件削除（キャッシュ直接参照+std::mem::take）
93. **memory/モジュール統合**: dreams.rs内部メソッドpub化+evolution.rs analyze_deep統合（重複除去）
94. **Adam's Law高頻度表現リライト**: プロンプト40箇所を訓練データ高頻度表現に変換（列挙形式→箇条書き、検証→確認、環境障害→通信エラー等）
95. **Evaluator定量基準化**: 確認チェックリストに定量閾値追加（ツール成功率80%/コンパイル通過/完了条件チェック）
96. **不変条件チェック**: check_invariants()でタスク完了時の品質検証（ツール成功率/回答長チェック、非ブロッキングWarn）
97. **TTL情報鮮度管理**: expires_atカラム+purge_all_expired()自動パージ（DBスキーマV6、セッション開始時実行）
98. **ADR自動生成**: AdrWriter — Replan/Advisor意思決定をMarkdownでVaultに蓄積（knowledge/adr.rs）
99. **SKILL.mdエクスポート**: export_markdown()+--skills-export CLI（スキルのMarkdown書き出し、ゼロショット知識転送）
100. **型駆動ツール定義**: TypedTool<Args: Deserialize+JsonSchema>トレイト+schemarsブランケット実装（全8ツール移行、スキーマ自動生成+型安全パース）
101. **ツール数上限ガード**: max_tools_in_context設定（デフォルト8）、select_relevantで自動制限+warn_if_exceeded警告ログ（ハーネスエンジニアリング知見: 10+ツールで非線形劣化）
102. **MCPマルチサーバー+ToolRegistry統合**: ネームスペース付きツール名（"server:tool"形式）、setup_mcp_server()でgraceful起動、config.toml [[mcp.servers]]から自動登録
103. **タスク複雑度ベース動的InferenceParams**: inference_for_task()でTaskType別温度制御（FileOp/CodeExec→0.3精密、Research→0.6探索）、LlmBackend::generate_with_params()トレイト拡張
104. **ツール出力サイズ制限**: truncate_output()でmax_tool_output_chars（デフォルト4000）超過分をUnicode安全に切り詰め（ハーネスエンジニアリング知見: ツール出力がコンテキストの67.6%）
105. **inference_for_task実機統合**: execute_step()でdetect_task_type→inference_for_task→generate_with_params呼出（TaskType別温度制御が実際に推論に反映）
106. **Vault↔KnowledgeGraph相互リンク**: record_to_graph()でVaultエントリをグラフノード/エッジに自動記録（LLM Wikiパターン、カテゴリ→エントリのcontainsエッジ+ソースのextracted_fromエッジ）
107. **Lab変異1テーマ制約**: MutationTheme enum（Precision/Exploration/Efficiency/Robustness）+ from_cycle()固定マッピング（経験的プロンプトチューニング知見: 1 iteration 1 theme）
108. **MCP自動復旧**: McpConnection::is_alive()生存チェック + McpToolWrapper::call()でプロセス死亡時自動再接続（DevTools MCP知見: 障害時自動リカバリ）
109. **タスク完了メトリクス**: AuditAction::TaskComplete（task_summary/total_steps/tool_success_rate/duration_ms）プロダクトメトリクス第3層（LLMオブザーバビリティ知見）
110. **TaskCompleteダッシュボード**: TaskCompleteStats集計（avg_steps/avg_tool_success_rate/avg_duration_ms）+ --dashboard表示セクション
111. **MultiEditツール**: 単一ファイル複数箇所一括編集（アトミック操作: 全成功 or ロールバック、既存fuzzyマッチ再利用、OpenCode知見）
112. **SSEチャンクタイムアウト**: ureq timeout_recv_body設定でSSEストリーム読取タイムアウト（デフォルト60秒、config.toml sse_chunk_timeout_secs、OpenCode知見、**MLXバックエンドでは180秒推奨**：初トークンレイテンシが長くLab中に多発確認）
113. **Invalidツールハンドラ**: 不明ツール呼出時にLevenshtein距離で類似ツール名を提案（edit_distance+suggest_similar_tool、OpenCode知見）
114. **Prune 2層閾値**: prune_minimum_chars（削除最小閾値）+ prune_protect_tokens（直近保護トークン数）でコンパクション制御改善（OpenCode知見）
115. **fuzzy戦略8 Levenshtein距離BlockAンカー**: 先頭/末尾行が類似（距離≤30%）かつ中間行50%以上類似で一致（8段階fuzzyマッチ化、OpenCode知見）
116. **Truncationスピルオーバー**: truncate_tool_output()でmax_tool_output_chars超過分を/tmpに保存+パス参照（Unicode安全境界、OpenCode知見）
117. **fuzzy戦略9 ContextAwareReplacer**: コンテキスト行アンカーで重複コードブロックを区別（9段階fuzzyマッチ化、OpenCode知見）
118. **ModelCapability**: モデル能力別ツール切替（Full/EditFocused/ReadExecute、ServerBackend自動マッピング、OpenCode知見）
119. **ツール説明外部化**: descriptions.rsに全ツールDESCRIPTION集約（Lab変異実験基盤、OpenCode知見）
120. **サブエージェント委任**: SubAgentExecutor — サブタスク順次実行+エラー境界+深度制限2+TaskManager連携+コンテキスト注入
121. **FunctionGemmaパーサー**: parse.rsに`parse_functiongemma_output()`+`format_functiongemma_declaration()`（正答率25%で不採用）
122. **ツール選択精度ベンチマーク**: tool_selection_bench.rs新規（16テストケース、実機計測対応）
123. **Lab変異pre-screening**: estimate_mutation_effect_with_baseline()統合、4タスク×k=1で事前評価、delta<-0.01で早期REJECT（フル評価の8%コスト）
124. **Streamable HTTP MCP**: McpTransport enum（Stdio/Http）、reqwest::blocking使用、config.toml url指定で自動切替
125. **Lab14サイクル化**: temperature/max_tool_output_chars変異+新プロンプトルール4件追加（探索空間2.4倍）
126. **ベンチマーク22タスク化**: rename/git_diff/permission_error/json_parse/sort/search 6タスク追加
127. **file.rsカバレッジ向上**: fuzzyマッチ全9戦略のテスト+境界条件+統合テスト（+21件）
128. **config.toml prescreening統合**: ExperimentConfigにenable_prescreening/prescreening_threshold追加
129. **タスク単位タイムアウト**: AgentConfig.task_timeout（Duration）、config.toml [experiment] task_timeout_secs（デフォルト300秒）、ウォールクロックベースのスタック防止
130. **BitNetバックエンド完成**: create_backend()でServerBackend::BitNet分岐、デフォルトポート8090、エラーメッセージにバックエンド名表示
131. **適応的変異生成**: プロンプトルール8→20種、パラメータ変異10→16種、試行済み変異DB追跡+重複スキップ、変異空間36候補（枯渇解消）
132. **HTTP MCPステータスコード検証**: send_request()でHTTP 4xx/5xxを事前チェック+エラーボディ付きメッセージ（根本原因の可視化）
133. **HTTP MCP通知ボディ消費**: send_notification()でresp.text()呼出（Keep-Alive接続の正常解放）
134. **HTTP MCP死活チェック改善**: is_alive()でHTTPサーバーにtools/listリクエスト（5秒タイムアウト、従来の常時true→実際の疎通確認）
135. **HTTPタイムアウト分離**: connect_timeout(10秒)とtimeout(60秒)を個別設定（接続遅延と大レスポンスを区別）
136. **ACCEPT変異3デフォルト化**: 「回答を出す前にファイルの内容を確認する」(Lab v9 +0.0157実証)
137. **MCP/ビルトイン分離選択**: select_relevant_split()でビルトイン枠(max_tools_in_context=8)とMCP枠(max_mcp_tools_in_context=3)を独立管理（MCPツール追加時のビルトイン押出し防止）
138. **構造化フィードバックリトライ**: StructuredFeedback（EVALUATION/MISSING/SUGGESTIONS構造）でReplan/Verification注入を改善、inject_verification_stepに失敗履歴コンテキスト追加（NAT SelfEvaluatingAgentWithFeedback知見）
139. **軌跡評価TrajectoryScore**: LCSベースsequence_accuracy+ツールカバー率+余分呼出ペナルティで「正しい理由で正解したか」を評価（NAT Trajectory Evaluation知見）
140. **オラクルフィードバック変異生成**: extract_worst_reasoning()でREJECT最悪delta変異を抽出、add_worst_reasoning_insights()で逆向き変異候補自動生成（NAT GA Optimizer知見）
141. **適応的トリガーLabStagnationDetector**: Stagnation(ベスト不変N回)/VarianceCollapse(delta分散<0.001)の2条件でLab停滞検出、Dreamer早期起動基盤（NAT check_adaptive_triggers知見）
142. **before_stepフック**: Middlewareトレイトにbefore_step()デフォルト実装+MiddlewareSignal::Abort追加、LLM呼出前の介入ポイント確保（NAT pre-step hook知見）
143. **before_stepフック実ループ統合**: run_agent_loop_with_session()のメインループでexecute_step前にrun_before_step()呼出、Abort時はループ中断（NAT知見の実動作化）
144. **LabStagnationDetector実ループ統合**: run_experiment_loopにStagnation/VarianceCollapse検出→Dreamer早期起動+oracle feedback注入の自動トリガー
145. **oracle feedback実ループ統合**: 停滞検出時にextract_worst_reasoning()でREJECT失敗パターン抽出→add_worst_reasoning_insights()で逆向き変異候補を自動生成・注入
146. **セッション保存トランザクション化**: save_session()をBEGIN IMMEDIATE/COMMIT単一トランザクションに最適化+prepare_cachedでINSERT再利用（N+2 fsync→1 fsync）
147. **RAM実測値検出**: get_available_ram()をvm_stat実行結果（free+inactive+purgeable pages）に改善、get_total_ram()分離、フォールバック付き
148. **ツール説明集約参照**: 全9ツールのconst DESCRIPTIONをdescriptions.rsの定数参照に統一（Lab変異実験で1ファイル変更のみ）
149. **RRFクローン除去**: rrf_merge()シグネチャを所有権受取に変更、into_iter()でclone不要化（検索毎のN回アロケーション削除）
150. **experiments複合インデックス**: SCHEMA_V8でidx_experiments_accepted_detail追加（load_tried_detailsのcovering index）
151. **ベンチマークDB再利用**: run_k()でタスク毎1 DB+reset_session_data()（66→22 DB作成に削減）
152. **server.rs unwrap除去**: 全5エンドポイントのserde_json::to_string().unwrap()→unwrap_or_else（APIパニック防止）
153. **with_capacity最適化**: repomap.rs/file.rsのホットパスVec::new()→with_capacity（事前アロケーション）
154. **rollbackエラーログ**: main.rs handle_rollback_modeのgit checkout失敗時にエラーログ出力（サイレント失敗防止）
155. **safety/テスト強化**: network.rs +4テスト（Default/strict_blocks/domain_no_scheme/empty）
156. **middleware.rs unsafe除去**: ライフタイムパラメータ化（`MiddlewareChain<'a>`, `AuditMiddleware<'a>`）、raw pointer削除、UAFリスク解消、build_default_chain()をsafe化
157. **fastembed v5 API修正**: `TextInitOptions::new(EmbeddingModel::AllMiniLML6V2)`への更新、`Mutex<TextEmbedding>`で`&self`embedding維持（internal mutability）、embeddings featureデフォルト有効化
158. **セマンティックツール選択**: `select_relevant_split_semantic()` — ローカルONNX埋め込み（AllMiniLML6V2）+コサイン類似度0.7 + キーワード0.3ハイブリッド、`SemanticCache`遅延初期化+register時自動invalidate、embedder失敗時はキーワード版にフォールバック
159. **セマンティックコンパクション（P1 Step 4）**: `SemanticScorer` — タスクコンテキスト埋め込みとメッセージ埋め込みのコサイン類似度を算出、固定役割スコアと6:4でブレンド（動的重要度）。`compact_level1_with_scorer()` で削除候補を順位付け（既存compact_level1は無変更、opt-in設計、6テスト追加、892テスト）
160. **サブエージェント並列実行（P0 Step 2）**: `SubAgentExecutor::execute()` を独立性検出＋store条件ベースのディスパッチャに変更。`check_independence()` で日本語/英語の依存マーカー（"前の"/"上記"/"previous"/"then "等 20種）を検査し、file-backed store + 2件以上 + 独立判定で `std::thread::scope` 並列化（スレッド毎に `MemoryStore::open()` で独立Connection）、それ以外は従来の順次実行にフォールバック。`MemoryStore` に `path` フィールド追加（in-memory=None）、`AgentConfig` に `Clone` 追加。6テスト追加（898テスト）
161. **成功軌跡抽出→スキル自動昇格（P1 Step 5）**: `EventStore::extract_successful_trajectories(min_rate, min_steps)` で append-only events から成功軌跡を抽出、`TrajectoryCandidate` 構造体で session_id/task_description/tool_sequence/tool_success_rate/total_steps/duration_ms を保持。`SkillStore::promote_from_trajectory()` で軌跡からスキル自動生成（tool_chain_key ベース重複判定、安定ハッシュサフィックス付きスキル名）。SessionEnd必須・閾値フィルタ（rate/steps）・空 tool_sequence スキップの3段階ガード、runtime からの event 発行はforward-compat（テストフィクスチャで検証、908テスト、+10: event_store 7件＋skill 3件）
162. **EventStore ランタイム統合（P1 Step 5 完了）**: `agent_loop::emit_event()` 疎結合ヘルパー（`store=None` 時 no-op、append失敗時 log_event(Warn) で握る、コアループは止めない）。`run_agent_loop_with_session` で SessionStart + UserMessage を開始時に emit、4 つの return path（timeout / before_step abort / OutcomeAction::Return / max_iterations）で SessionEnd を emit。`tool_exec::apply_tool_result` でツール実行毎に ToolCallStart + ToolCallEnd を emit（cache hit 経路含む）。`AuditLog`（粗粒度メトリクス）と `EventStore`（シーケンス保存）を役割分離。Lab v11 ACCEPT 2件は誤差〜中程度＋既存ルール再注入のため defaults 化見送り（`.claude/plan/lab-v11-accept-analysis.md`）。909テスト（+1: test_run_agent_loop_emits_events、SessionStart/UserMessage/ToolCallStart/End/SessionEnd 順序＋成功率100%＋tool_sequence 検証）
163. **ADK Phase B2 Judge Gate + Phase C ベンチマーク 22→40 タスク拡張**: B2 — `agent/judge.rs` の `LlmJudge` トレイトと `HttpAdvisorJudge<'a>` を `experiment.rs` に配線。`MultiRunTaskScore` に `last_response`/`last_trajectory` フィールド追加（serde skip_serializing_if、後方互換）、`benchmark::run_k()` で各タスク最終 run のキャプチャ。`judge_gate_check(judge, result, threshold, sample_size, descs)` でサンプル N 件を rubric 評価し、`mean_composite >= threshold` を ACCEPT 二次ゲートとして強制（fail-open: judge 失敗・空スコア時は通す）。`ExperimentLoopConfig.judge_threshold: Option<f64>`/`judge_sample_size: usize=4` を `[experiment]` セクション経由で opt-in 設定可能化。Phase C — `default_tasks()` に MultiFileEdit/LongRun/ToolChain/ErrorRecovery/McpInteg/Semantic/Reasoning/Summarization/Verification 各 ×2 計 18 タスクを追加（既存 TaskCategory 再利用、新 enum 追加なし）。909→**949 テスト**（+22 ScriptedJudge＋fail-open + 18 phase_c_*、`#[ignore]` 解除）
164. **agent_loop.rs 8 分割完了（structural-improvements-v2 Step 7 完遂）**: `src/agent/agent_loop.rs`（2661行モノリス）を `src/agent/agent_loop/` ディレクトリ配下 8 モジュール（mod=28, tests=1439, core=315, advisor_inject=240, state=206, step=181, outcome=146, support=126, config=108 行）に分割完了。8 commits（`72d969d` リネーム → `c7a8e3d` config → `87c3e75` state → `68ac7f3` support → `2319f4f` advisor_inject → `07d59ce` outcome → `d6ce2be` step → `a1971ba` core → `3019cb6` tests.rs 分離）。`mod.rs` は 28 行のファサード（`pub use core::{run_agent_loop, run_agent_loop_with_session}` 等で 100% 後方互換、middleware/benchmark/experiment/main は無変更）。各 commit で `cargo test --lib` を維持。**最大 production ファイル長 2661→315 行（約 1/8）**。
165. **重複 #[test] アトリビュート削除（カノニカル化）**: `src/agent/experiment.rs:1541` の orphan `#[test]`（コメント直前、二重指定で実機 test entry 重複）を削除。`tools/mod.rs:942` の不要 `let mut reg` を `let reg` に修正（`warn_if_exceeded` は `&self`）。**949→948 テスト**（重複登録の解消によるカノニカル化）。
166. **ADK Phase D YAGNI 判定 = 見送り**: `.claude/plan/phase-d-evaluation.md` で Workflow primitive 形式化を5項目評価（Q1〜Q5）し加重 1.0/17.0（閾値12.0）で **着手見送り**。代替として軽量改善 D-α（SubAgentExecutor docstring 強化）と D-γ（experiment.rs コメント追加）を提案。再評価トリガー: ①複合 Workflow 要件発生 ②Lab pass^k 改善天井（3サイクル全 REJECT）③ADK 2.0+ primitive 標準化、いずれか発生で再判定。

## Lab実機テスト結果（Bonsai-8B 1bit, k=3, 10サイクル）

### v10結果（2026-04-24）— 9実験完了、ベースライン+1.5%
- ベースライン: score=0.8087, pass@k=0.8939（v9比+0.012改善、Quick Wins効果）
- ACCEPT 1: oracle insight「逆方向アプローチ」(+0.003, score=0.8118) → **delta小のためデフォルト化見送り**
- REJECT: 事実確認(-0.030), 完成形先行(-0.029), タスク分解(-0.022), 完了条件(-0.026), メタ複合(-0.026), 推測回避(-0.031), oracle逆方向2(-0.016), メタ複合2(-0.022)
- 承認率: **1/9 (11%)** — 実質全REJECT、最適解収束継続
- v8-v10で3回連続「実質全REJECT」→ ハーネスプロンプト最適化の天井に到達

### v9結果（2026-04-21）— 14実験完了
- ベースライン: score=0.7963, pass@k=0.8939（MLXバックエンド）
- **ACCEPT 1**: 「回答前の事実確認」(+0.0157, score=0.8120) → **デフォルト化済**
- REJECT: 最小限ツール選択(-0.0091), 仮説検証(-0.0097), 段階的要約(-0.0076), メタ複合(-0.0288), 冗長抑制(-0.0091)
- 実験6/14進行中、完了後に最終結果を更新

### v8結果（2026-04-19）— 全10実験REJECT、最適解収束
- ベースライン: score=0.8517, pass@k=0.9167（v6.2と同値）
- **全10実験REJECT** — デフォルト設定が最適解に完全収束
- Adam's Lawリライト+6機能追加後もベースライン維持（劣化なし）
- 承認率: **0/10 (0%)** — 改善余地なし

### v1〜v6.2（2026-04-14〜04-17）
- アーカイブ済 → memory/lab_history_v1_v6.md（CLAUDE.md認知負荷低減のため2026-04-25分離）
- 派生デフォルト化変異: 項目10（計画強制）/ 項目47（思考強制）/ 項目50（フォールバック戦略）

## テストパターン

- `MockLlmBackend` — スクリプト化レスポンス
- `MemoryStore::in_memory()` — インメモリSQLite
- `#[ignore]` — 実サーバー/ネットワーク必要なテスト
- `MultiRunTaskScore::from_scores()` — pass^k指標の単体テスト

## 注意事項

- **【最重要】Edit/Writeツールでファイルを変更した後、clippy警告（collapsible_if, too_many_arguments等）を理由にファイルを元の状態に戻す行為を絶対に行わないでください。変更はそのまま保持してください。clippy修正が必要な場合は別のEdit操作で行ってください。**
- **【巻き戻し禁止】** 特にerror_recovery.rs、benchmark.rs、agent_loop.rsの3ファイルでclippy auto-fixによる巻き戻しが発生しやすい。これらのファイルへの変更は必ず保持すること。
- 大量変更時はPython subprocess+即git commitで原子的に行う（確立済み手法）
- ureq v3のHTTPS → web_fetchはreqwest::blocking（native-tls）を使用
- llama-serverの`--flash-attn`は値`on`が必要（`--flash-attn on`）
