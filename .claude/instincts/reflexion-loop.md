---
id: bonsai-reflexion-loop
trigger: "エージェントのエラーハンドリングやリトライを書くとき"
confidence: 0.85
domain: agent-architecture
source: arxiv-2303.11366-2501.11425
---

# Reflexion: 失敗から学ぶ自己反省ループ

## アクション
ツール実行が失敗した場合、単純リトライではなく:
1. 失敗モードを分類（ParseError / ToolExecError / LoopDetected）
2. エラー情報をコンテキストに追加
3. LLMに反省プロンプトを送り、修正された行動を生成
4. 最大3回までリトライ、LoopDetectedは即打ち切り

## 根拠
言語的フィードバックによる自己強化がエージェントの成功率を大幅に改善する（arxiv:2303.11366）。
MCTSベースの自己学習で小型モデルでも自己修正能力を獲得可能（arxiv:2501.11425）。
