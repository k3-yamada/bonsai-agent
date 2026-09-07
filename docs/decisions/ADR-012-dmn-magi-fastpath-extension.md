# ADR-012: 自律知能拡張 — FastPath（反射層代替）、DMN自発思考ループ、MAGI三重合議監視

## Status: Accepted (2026-09-07)

## Context

Bonsai-8B（1-bit 量子化モデル、1.28GB メモリフットプリント）はリソース制約の厳しいエッジ環境（M2 Mac 16GB）で動作する自律エージェントである。
従来のアーキテクチャでは以下の課題が存在した：

1. **受動的待機の限界**: ユーザーからの対話入力がない限りエージェントは一切思考を行わず、過去の失敗や知覚イベントから自発的に教訓を深める機会（VALUES.md V3「経験からの自己更新」）が欠落していた。
2. **反射層導入のトレードオフ**: 「知覚 ➔ 反射 ➔ 意識 ➔ 無意識」という多層神経系モデルにおいて、反射層として別個の軽量モデル（1B〜3B）を常駐させる案が検討されたが、16GB 統合メモリの圧迫、および完全同期アーキテクチャにおけるプロセス間通信オーバーヘッドが過大と判定された。
3. **1-bit モデルの過信・脱線（Goodhart's Law / Hallucination）**: 小規模モデル特有の過信（Overconfidence）や指標最適化への偏重、VALUES.md からの逸脱を、単一プロンプトだけで抑止することには物理的限界があった。

## Decision

「Scaffolding > Model（ADR-002）」原則に基づき、以下の 4 要素を統合設計・実装した。

### 1. 反射層の軽量代替: FastPathDispatcher
- 別プロセス/別モデルの反射層を常駐させる代わりに、`src/agent/fast_path.rs` に軽量ルールベースの `FastPathDispatcher` を新設。
- 定型的な ping や挨拶、明白な即答クエリを REPL の最前線でインターセプトし、LLM 推論とコンテキスト構築をバイパスして即答（ゼロレイテンシ・トークン消費ゼロ）。
- 通常の推論タスクはそのまま透過させ、メインの Agent ループへ委譲する。

### 2. DMN (デフォルト・モード・ネットワーク) 自発的思考ループ
- `src/agent/dmn/` に、完全同期スレッドモデル（`std::thread` + `CancellationToken`）に準拠した常駐ワーカー `DmnRunner` / `DmnWorker` を新設。
- ユーザー入力待ちやタスク完了後のアイドル状態（`is_busy = false`）を自動検知して自発的に思考を巡らせる。
- **三段階ゲート制御**:
  - Gate 0: アイドル判定（`is_busy == false`）
  - Gate 1: スケジューラ判定（ポアソン過程に基づく指数分布ディレイ）
  - Gate 2: 沈黙 / 発話閾値判定（重要度スコア `significance >= speak_threshold`）
- **多層記憶・ナレッジ多重還元**:
  - 内省成果を `experiences` テーブル（エピソード記憶）に記録。
  - 発話相当の気づきは `KnowledgeVault`（`insights.md`）、`KnowledgeGraph`（三つ組エッジ）、A-MEM（`memories` FTS5 テーブル）、直前エピソードとの連想リンク（`reflects_on`）へ自動還元。

### 3. MAGI 三重監視合議パネル (Step-Level Guardrail)
- `src/agent/magi/` にエヴァのMAGIシステムに着想を得た 3-Judge 独立合議パネル `MagiPanel` を新設。
  - **SafetyJudge (MELCHIOR)**: 破壊的シェルコマンド、機密漏洩、セキュリティ脅威の阻止。
  - **ConsistencyJudge (BALTHASAR)**: 過去の言明や確定事実（A-MEM/KnowledgeGraph）との論理矛盾・過信の検出。
  - **GoodhartJudge / ValuesGuard (CASPAR)**: VALUES.md（V1〜V7）適合性判定、および全指標単調改善による形骸化（Goodhart's Law）の監視。
- `execute_step` の回答生成フェーズで合議を実行。Block 判定時には Reflexion ガイダンスを注入し、最大リトライ枠内で自己修正を促す。

### 4. 非侵入型 REPL UX (dmn_inbox バッファリング)
- バックグラウンド DMN の発話が TTY 入力中に割り込んで表示を崩さないよう、`dmn_inbox` キューによるバッファリングを導入。
- ユーザーが次の入力を始めるプロンプト（`bonsai> `）を出力する直前の安全なタイミングでフラッシュし、`💡 [DMN]: ...` として控えめに提示。
- `BONSAI_DMN_NOTIFY=0` によるサイレントモードもサポート。

### 5. イベント駆動型外部知覚（SensorHub & 権限分離）
- `src/agent/sensors/` に完全同期イベント集約ハブ `SensorHub` を新設。
- `FileWatchSensor`（ファイル更新検知）、`IdleSensor`（離席・復帰検知）、`WindowChangedSensor`（アクティブアプリ切り替え）を統合。
- OS権限（macOS Accessibility）の事前チェックによるゼロクラッシュ起動、および `PrivacyFilter` による機微ファイル（.env, .ssh 等）や機微タイトルの事前遮断。

### 6. 記憶力学グラフの可視化層（Force-Directed Graph Viewer）
- `src/memory/export.rs` / `src/memory/html_viewer.rs` に外部 CDN 依存ゼロの単一自己完結型 HTML ビューア生成器を新設。
- A-MEM（`memories`, `memory_links`）およびナレッジグラフ（`knowledge_nodes`, `knowledge_edges`）をプライバシーサニタイズ（`SecretsFilter` & `PrivacyFilter`）付きで抽出。
- SVG + 2D 力学シミュレーション（パン・ズーム、ドラッグ、検索、詳細サイドバー）による直感的可視化。
- CLI コマンド（`bonsai --visualize`）、REPL スラッシュコマンド（`/graph`）、およびリアルタイム更新シンク（`VisualizationSink`）による即時レンダリング。

## Consequences

### Positive
- **自発的知能と知覚の確立**: ユーザーからの対話がない時間にも、失敗の反省や外部知覚の整理が進み、ナレッジベースが自律的に育つエコシステムが成立。
- **直感的な認知の可視化**: エージェントが何を記憶し、どのような連想リンクを形成しているかを即座にブラウザ上で安全に観察・検証可能。
- **堅牢な安全性とプライバシー**: MAGI パネルおよび PrivacyFilter により、小規模モデルの幻覚や機密情報の漏洩を足場側で確実に遮断。
- **卓越した UX**: FastPath の俊敏性と、DMN の知的で押し付けがましくないインサイト提示が両立。
- **完全同期・Clean Architecture 厳守**: 非同期ランタイムを持ち込まず、レイヤー順序（DEP-001）および行数制限（SIZE-001）を 100% 遵守。

### Verification
- 単体テスト: `cargo test --lib` (1,581 件全件 PASS)
- 包括的 E2E テスト: `cargo test --test e2e_extension_flow` (4 件全件 PASS)
- 実対話セッション実証: `cargo test --test interactive_session_demo` (PASS)
- 構造・レイヤー検証: `cargo test --test structural` (5 件全件 PASS)
- 静的解析・整形: `cargo clippy -- -D warnings` (0 warnings), `cargo fmt -- --check` (clean)
