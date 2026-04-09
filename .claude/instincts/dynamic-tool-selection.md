---
id: bonsai-dynamic-tool-selection
trigger: "ツールをシステムプロンプトに注入するとき"
confidence: 0.9
domain: agent-architecture
source: arxiv-2409.00608-2411.15399
---

# 動的ツール選択: 全ツールを注入しない

## アクション
`ToolRegistry::select_relevant(query, max=5)`で、ユーザー入力に関連するツールのみを
システムプロンプトに注入する。全ツールを入れない。

## 根拠
小型モデル（8B以下）では全ツールをプロンプトに入れると精度が低下する（arxiv:2409.00608）。
ツール数を選択的に削減することで、ファインチューニング不要で性能が改善する（arxiv:2411.15399）。
