---
id: bonsai-amem-memory
trigger: "メモリやセッション永続化を実装するとき"
confidence: 0.85
domain: agent-architecture
source: arxiv-2502.12110
---

# A-MEM式メモリ: 原子的ノート + 動的リンク

## アクション
メモリを原子的エントリ（content + tags JSON配列）として保存し、
`memory_links`テーブルでノート間の関係（related_to, derived_from, contradicts）を管理する。
検索はSQLite FTS5全文検索 + タグベースグラフ探索の組み合わせ。
フラットなkey-valueストアにしない。

## 根拠
Zettelkasten方式の動的インデキシングとリンキングで、MemGPT比85-93%のトークン使用量削減を
達成（arxiv:2502.12110）。原子的ノート + 選択的top-k検索がトークン効率と検索品質を両立。
