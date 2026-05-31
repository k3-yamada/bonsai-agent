# ADR-001: CLAUDE.md Size Governance & docs/ Single Source of Truth

## Status: Accepted (2026-05-31)

## Context

CLAUDE.md は Claude Code session 起動時に毎回 context へ auto-load される convention file。
プロジェクトのハーネスパターンは項目番号付き (1-268+) で蓄積し、各項目が TDD strict phase / 実機 finding / paired evidence など詳細な記録を持つ。

過去の肥大化 pattern (実測):
- 2026-05-07: 項目 1-201 を archive 分離 (初回)
- 2026-05-10: 202-219 追加
- 2026-05-16: 220-239 追加 + 案 C 手動再整理で 82 KB → 13.5 KB (-83%)
- 2026-05-16 (項目 255): Zenn Codex Harness Step 1+2 適用、docs/ knowledge base 整備 + CLAUDE.md 202→88 行
- 2026-05-31: 再び 116 行 / 53 KB へ肥大 (「直近 5 項目」section に 21 項目 247-268 蓄積)

約 2 ヶ月毎に手動 archive が発生する運用負債。auto-load される性質上、肥大は全 session の入力 token コストに直結する。

## Decision

CLAUDE.md を「索引 + デフォルト化済み変異 + 直近 N 項目の 1 行サマリー」のみに限定し、詳細記録は外部 SSOT に分離する。

1. **目標サイズ**: ≤ 100 行 / ~14 KB (Zenn dragon1208 Codex Harness 推奨の 100 行原則)。
2. **詳細の SSOT 分離**:
   - ハーネスパターン項目 verbatim → `~/.claude/projects/-Users-keizo-bonsai-agent/memory/harness_patterns_archive.md` (project root 外部、Claude Code session memory)
   - アーキテクチャ / Lab 履歴 / runbook → `docs/architecture/`, `docs/quality/`, `docs/execution/`
3. **「直近 N 項目」FIFO 規則**: N+1 項目目追加時は最古 1 件を archive へ flush、各 entry は 1 行 (200-400 字、改行禁止)。
4. **Mechanical enforcement** (`scripts/drift/docs_sync.py`):
   - 100 行 gate + 項目 0 件検出で FAIL (format drift catch)
   - CLAUDE.md 言及項目 ↔ archive cross-ref で欠落 FAIL
   - 「### 直近 N 項目」header N ↔ section 実数の整合 FAIL (Z-3 第 3 軸)
5. **Auto-flush tool**: `scripts/claudemd_archive.py --mode {check,dry-run,apply}` で N+1 検出 → 最古を archive append + CLAUDE.md rewrite。
6. **運用ルール SSOT**: `docs/maintenance/claudemd-curation.md`。

## Consequences

**Positive**:
- auto-load token 大幅削減 (2026-05-31 実測: 53 KB → 8.9 KB、-83%)。全 session の入力コスト恒久低減。
- mechanical enforcement により再肥大を CI で機械的に catch、手動 audit 不要化。
- Codex (AGENTS.md) / Claude Code (CLAUDE.md) 両 IDE foundation、docs/ が人間にも navigable。

**Negative / Trade-off**:
- 詳細参照に 1 hop 追加 (項目番号 → archive lookup)。ただし archive verbatim 完備で情報損失ゼロ。
- FIFO flush の運用規律が必要 (人間 or auto-flush tool 依存)。enforcement で違反は検出可能。
- archive と CLAUDE.md の二重管理 (cross-ref drift リスク) → docs_sync.py で sync 検証。

**Rejected alternatives**:
- 案 B (50 行 aggressive、カテゴリ索引も外部化): 索引の即時可視性を失うため不採用。
- 案 C (archive 移行のみ、構造変更なし): 蓄積防止策がなく再発するため不採用。

## Related

- `.claude/plan/claudemd-size-reduction-item-255-recreate.md` — 2026-05-31 実行手順
- `.claude/plan/claudemd-archive-policy.md` — auto-flush tool 設計 (項目 240)
- `.claude/plan/agents-md-docs-knowledge-base.md` — Z-1 docs/ 整備 (項目 255)
- `docs/maintenance/claudemd-curation.md` — 運用ルール SSOT
- `scripts/drift/docs_sync.py` — mechanical enforcement (項目 257-260, Z-3)
- CLAUDE.md 項目 255 (Z-1), 257 (Z-3 drift linter)
- harness_patterns_archive.md (項目 verbatim SSOT)
