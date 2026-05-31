# ADR-005: sqlite-vec ベクトル検索の採用検討 → REJECT → wiring 撤去

## Status: Accepted (2026-05-31。決定内容 = 「採用しない」)

## Context

memory / knowledge 検索の高速化候補として sqlite-vec (SQLite 拡張によるベクトル類似検索) を評価した (項目 220-222)。

評価経緯:
- **項目 220 Step A**: sqlite-vec の基本 wiring を採用、backfill / recall ベンチで良好な数値 (G-4.4 182ms/10K backfill、G-4.5 recall=1.0)。
- **項目 220 Step B (Milvus Lite)**: REJECT。
- **項目 221 Lab REJECT**: 実機 Lab paired smoke で G-4.2 = score Δ=-0.0031 (PASS 相当の中立) だが **RSS +99.9 MB NG**。原因 = embedder の OnceLock 常駐 (architectural constant、起動ごとに固定コスト)。M2 16GB 環境では memory footprint が許容外。
- **項目 222 wiring removal**: `index_memory_if_enabled` を dead-code 化し撤去 (+8/-334 行)。

既存の検索層 (Layer 1 keyword / Layer 2 semantic / Layer 2.5 graph / RRF hybrid / Layer 3 chunk read) で実用上十分な recall (LongMemEval-S R@5=0.91、agentmemory graph fusion R@10=98.6) を達成しており、sqlite-vec の追加 ROI が footprint コストに見合わなかった。

## Decision

**sqlite-vec は採用しない。wiring は撤去済。再導入は M2 16GB の RSS 制約を解決する設計 (embedder の遅延ロード / 共有 / 外部プロセス化) を伴う場合のみ検討する。**

## Consequences

**Positive**:
- M2 16GB の memory footprint を保全 (OnceLock embedder +99.9 MB を回避)。
- 検索層は既存 5 経路で R@5=0.91 / R@10=98.6 を維持、複雑性増を回避。
- 「ベンチ数値が良くても実機 RSS 制約で REJECT」= ADR-003 の「実機 evidence 優先」を補強する事例。

**Negative / Trade-off**:
- 純粋なベクトル ANN 速度は得られない (大規模 memory 時の semantic 検索 latency は keyword+RRF で代替)。
- 将来 memory が桁違いに増えた場合は再評価が必要 (embedder 設計改善前提)。

## Related

- ADR-003 (実機 evidence 優先 — 本件は RSS 制約での REJECT 事例)
- CLAUDE.md sqlite-vec 系列 (項目 220/221/222)
- memory: 検索層は Layer 1/2/2.5/3 + RRF (arag_alignment.md)、LongMemEval-S (項目 227)、graph fusion (項目 228)
- harness_patterns_archive.md (項目 220-222 verbatim)
