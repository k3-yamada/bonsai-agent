---
trigger: always_on
description: "「Scaffolding > Model」および ADR-003 Paired Evidence の設計思想ガイド"
---

# Scaffolding > Model & Paired Evidence 原則

## 1. 「Scaffolding > Model」思想

- **Bonsai-8B（1-bit 量子化モデル）の特性**:
  モデル自体の重み・推論能力はコンパクト（1.28GB）であり、複雑な自己修正や長大な文脈保持を単体で行うには物理的な限界があります。
- **ハーネス（Scaffolding）の役割**:
  - ステップ累積による失敗確率の指数的増大（\(p^n\) 問題）を外部から制御・遮断する。
  - ループ検出器（LoopDetector）、停滞検知器（StallDetector）、計画強制ルール、AI+Tool ペア保護、コンテキスト圧縮（Compaction）などの外部装置で補う。
  - 「モデルに賢くさせようとする」のではなく、「モデルが失敗し得ない足場を築く」ことを最優先する。

## 2. ADR-003: Paired Evidence 規律

- **Unpaired 評価の罠**:
  単一の実行やベースライン比較で「+10% 改善」が出たとしても、それはプロンプトや乱数シード、ハードウェアジッターに起因するノイズであることが極めて多い（過去に多数の変異が paired 再評価で覆された実績あり）。
- **厳格な統計的検証**:
  - 同一タスク・同一条件下での Paired 比較（A/B テスト）を実施。
  - Cohen's dz や Wilcoxon 符号順位検定による有意性検証を経て初めて本流（default ON）に組み込む。
  - 不用意な新規プロンプト拡張やメモリ注入を「良さそう」という直感だけで永続化しない。

## 3. Goodhart's Law & VALUES.md の能動的監視

- システムが自己改善・最適化を進める中で、代理指標（メトリクス）のみが改善し、本来の価値（VALUES.md V1〜V7）から逸脱することを防ぐ。
- `MetricConsistencyChecker` や将来の多重監視（MAGI型）を用いて、変質と形骸化を継続的に検知する。
