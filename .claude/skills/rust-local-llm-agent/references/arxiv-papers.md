# 参考論文詳細

このスキルの設計原則の根拠となるarXiv論文のサマリー。

## 目次

1. [2段階生成パイプライン](#2段階生成パイプライン)
2. [動的ツール選択](#動的ツール選択)
3. [Reflexionパターン](#reflexionパターン)
4. [メモリアーキテクチャ](#メモリアーキテクチャ)
5. [コンテキスト管理](#コンテキスト管理)
6. [ツール権限・安全性](#ツール権限安全性)
7. [エラーリカバリ](#エラーリカバリ)
8. [1ビットLLM](#1ビットllm)
9. [小型モデルエージェント](#小型モデルエージェント)

---

## 2段階生成パイプライン

### Let Me Speak Freely? (2408.02442)
構造化フォーマット（JSON/XML等）を強制するとLLMの推論能力が低下する。
自由形式の方が正確。Chain-of-Thoughtは自由形式で生成させ、
最終出力のみJSON制約を適用する2段階方式が最適。

### DCCD: Draft-Conditioned Constrained Decoding (2603.03305)
セマンティック計画と構文強制を分離。まず自由形式でドラフトを生成し、
次に制約付きで整形する。従来のconstrained decodingより精度が高い。
小型モデルでの「projection tax」（構造化強制による品質低下）を回避。

---

## 動的ツール選択

### TinyAgent: Function Calling at the Edge (2409.00608)
1.1Bと7Bの小型モデルでGPT-4-Turboに匹敵するfunction calling性能を達成。
ツール検索メカニズムにより必要なツールのみをコンテキストに入れることで
トークン消費を削減。量子化でエッジデプロイ可能。

### Less is More: Optimizing Function Calling on Edge Devices (2411.15399)
利用可能なツール数を選択的に削減することで、ファインチューニング不要で
エッジデバイスでの関数呼び出し性能・実行時間・電力効率が改善。

### SLM Survey: Small Language Models for Agentic Systems (2510.03847)
推奨アーキテクチャ:
- スキーマファーストプロンプティング（ツールスキーマをプロンプト最上部に配置）
- 型安全な関数レジストリ
- 信頼度スコアリング（出力の確信度が低い場合にフォールバック）
- ガイドデコーディング（XGrammar, Outlines）による厳密なJSON Schema出力

### MCP Tool Descriptions Are Smelly (2602.14878)
ツール記述の品質が悪いと実行ステップ数が増え、タスク成功率が下がり、
トークンオーバーヘッドが増大する。記述の明確さとパラメータ説明の完全性が重要。

---

## Reflexionパターン

### Reflexion: Language Agents with Verbal Reinforcement Learning (2303.11366)
言語的フィードバックを通じてエージェントを強化。エピソード記憶バッファに
反省テキストを保持し、次の試行で改善された意思決定を実現。
HumanEvalでGPT-4を91%精度で達成。

### Agent-R: Training Language Model Agents to Reflect (2501.11425)
MCTSによる反復的自己学習で、リアルタイムのエラー回復と行動修正能力を獲得。
失敗軌跡からの自己批判データセットを自動生成。
小型モデルでも自己修正能力を後天的に獲得可能。

---

## メモリアーキテクチャ

### A-MEM: Agentic Memory for LLM Agents (2502.12110)
Zettelkasten方式に基づく動的インデキシングとリンキングで
相互接続知識ネットワークを構築。選択的top-k検索で
MemGPT比85-93%のトークン使用量削減を達成。
原子的ノート + 動的リンク + 選択的検索の組み合わせが最も効果的。

### MemGPT: Towards LLMs as Operating Systems (2310.08560)
OSの仮想メモリ管理に着想を得た階層型メモリの基本パターン。
高速メモリ（コンテキストウィンドウ）と低速メモリ（外部ストレージ）間の
データ移動を自律管理。

---

## コンテキスト管理

### ReadAgent: A Human-Inspired Reading Agent with Gist Memory (2402.09727)
長文書を「メモリエピソード」と「要旨メモリ」に分割し、
有効コンテキスト長を最大20倍に拡張。必要時にオリジナルを再取得する
2層メモリアーキテクチャ。

### ACON: Optimizing Context Compression for Long-horizon Agents (2510.00615)
失敗事例を分析してコンテキスト圧縮ガイドラインを最適化。
小型コンプレッサモデルに蒸留可能。
「何を残し何を捨てるか」のルール設計が重要。

### SUPO: Scaling LLM Multi-turn RL with Summarization (2510.06727)
要約ベースのコンテキスト管理で固定コンテキスト長の制限を超えてスケール。
`max_context_tokens`設定を超えた場合に中間ステップを自動要約。

---

## ツール権限・安全性

### Progent: Programmable Privilege Control for LLM Agents (2504.11703)
ツール呼び出しポリシーをDSLで定義し、きめ細かい権限制御を実現。
Unix的なパーミッションモデルをエージェントツール呼び出しに適用。

### AgentSpec: Customizable Runtime Enforcement (2503.18666)
ランタイム制約をDSLで定義し強制するフレームワーク。
宣言的な安全制約の定義と実行時強制が完全自律エージェントには不可欠。

---

## エラーリカバリ

### Why Do Multi-Agent LLM Systems Fail? (2503.13657)
14種類の固有失敗モードを特定。仕様問題、エージェント間コンフリクト、
タスク検証問題に分類。各失敗モードに対するガードレールが必要。

### Where LLM Agents Fail (2509.25370)
メモリ、リフレクション、プランニング、アクション、システムレベルの
各段階での失敗モードを分類。構造化エラーログ
（どの段階で何が失敗したか）とリトライ時の失敗情報コンテキスト注入が有効。

---

## 1ビットLLM

### BitNet b1.58: The Era of 1-bit LLMs (2402.17764)
全パラメータを三値{-1, 0, 1}に。FP16と同等のperplexityとタスク性能を
達成しつつ、レイテンシ・メモリ・スループット・エネルギー消費で大幅な優位性。

### ACBench: Can Compressed LLMs Truly Act? (2505.19433)
4-bit量子化ではツール使用/関数呼び出しは1-3%の低下に留まる。
ただしリアルワールドアプリケーション精度は10-15%低下。
1-bit/1.58-bitでのツール呼び出し精度の直接評価はまだ未踏。

### BitVLA: 1-bit Vision-Language-Action Models (2506.07530)
1-bitモデルがアクション生成でフルプレシジョン同等を達成。
モデルメモリ11倍削減、レイテンシ4.4倍削減。
エージェント的タスクにおける1-bit実用性の実証。

---

## 小型モデルエージェント

### Small Language Models for Efficient Agentic Tool Calling (2512.15943)
ターゲットファインチューニングにより小型LMがToolBenchで77.55%パス率を達成。
汎用大型モデルより、ツール呼び出し特化のファインチューニングが効果的。

### LATS: Language Agent Tree Search (2310.04406)
モンテカルロ木探索をLLMエージェントに統合。HumanEvalで92.7%のpass@1精度。
計算コストは高いがリソース制約下ではReActにフォールバックする適応型設計が理想的。
