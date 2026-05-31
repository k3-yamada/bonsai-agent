# ADR-008: Context Compaction 戦略 (multi-level prune + smoke/env override)

## Status: Accepted (2026-05-31)

## Context

Bonsai-8B は context window が限られ、長いタスクで会話履歴が膨張すると 1bit モデルの attention が散漫化し品質が落ちる (p^n 問題、ADR-002)。
context 管理を compaction で行う (項目 6/12/41/46/78/81/82/158/159/178/187/265)。

主要機構:
- **multi-level compaction**: level1 (buffer prune) / level2 (summary 圧縮)。`max_context_tokens` を閾値に発火。
- **prune protect**: 直近 N message を保護し、古い履歴のみ prune。
- **smoke/env override (項目 265)**: `BONSAI_LAB_MAX_CTX` env > `BONSAI_LAB_SMOKE=1` (→6000) > default 14000 の 3 段優先。
- **dynamic budget (項目 248/261/263、env-gated)**: 4 軸 (buffer/summary/entities/kg) 別 budget allocation。**ただし paired REJECT (ADR-003、項目 268)、env default OFF**。

決定的 finding (項目 265 G-MCT2): production smoke の k=3 baseline は各 iteration が独立 session で context reset され、4500 tokens 未満で完了するため level1 が発火しない。即ち smoke 構造では prune が起きず、compaction の Lab 評価は構造的に困難。

## Decision

**multi-level compaction + prune protect を core 機構として採用。dynamic budget は env-gated で default OFF (paired REJECT)、axis-priority prune infrastructure は max_context 縮小で実発火させる future phase の base として保持。**

1. **level1/level2 + prune protect** を production default として運用。
2. **max_context_tokens** は default 14000、smoke では env override で縮小可 (項目 265)。
3. **dynamic budget (4 軸 allocation)** は `BONSAI_DYNAMIC_BUDGET` env-gated、**default OFF** (ADR-003 paired REJECT、項目 268 dz=-0.86)。infrastructure (Phase 5 axis-priority prune ~180 LOC + 8 test) は削除せず future base として保持。
4. compaction の Lab 評価は smoke 構造 (独立 session reset) で prune 不発火のため、max_context 縮小 or 長 session task で実発火させてから行う。

## Consequences

**Positive**:
- 長タスクで context budget を管理し 1bit attention 散漫化を抑制。
- smoke/env override で Lab 評価時の prune 強制発火手段を確保。

**Negative / Trade-off**:
- dynamic budget は paired で効果実証できず default OFF (投資が future base 化に留まる)。
- smoke 構造的に prune 不発火のため、compaction 改善の Lab 評価コストが高い (max_context 縮小 or 長 session 設計が前提)。

## Related

- ADR-002 (Scaffolding > Model — compaction は p^n 対策の core scaffolding)
- ADR-003 (Paired evidence — dynamic budget REJECT の根拠)
- CLAUDE.md Compaction / Context カテゴリ (項目 6/12/.../265)、項目 268 (BUDGET paired REJECT)
- memory: item_268_destructive_path_profile_2026_05_31.md
- harness_patterns_archive.md (項目 248/261/263/265 verbatim)
