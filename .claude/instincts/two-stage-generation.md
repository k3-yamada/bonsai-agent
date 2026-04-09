---
id: bonsai-two-stage-generation
trigger: "LLM出力のパース処理を書くとき"
confidence: 0.9
domain: agent-architecture
source: arxiv-2408.02442-2603.03305
---

# 2段階生成: 思考と構造化出力を分離する

## アクション
`<think>`タグ内の自由形式推論と`<tool_call>`タグ内のJSON出力を分離してパースする。
構造化出力の強制（GBNF文法等）は`<tool_call>`部分のみに適用し、思考部分には制約をかけない。

## 根拠
構造化フォーマットを強制するとLLMの推論能力が低下する（arxiv:2408.02442）。
思考フェーズは自由形式、出力フェーズのみ制約付きにするDCCD方式（arxiv:2603.03305）が最適。
