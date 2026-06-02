# ADR-009: 知識デーモン recall/ingest 層の設計判断

## Status: Accepted (2026-06-02)

## Context

ローカル Obsidian vault (~9000 chunk 規模) を 1bit Bonsai-8B から検索・想起するための
知識デーモン (recall/ingest 層) を、2026-06-01〜06-02 の複数セッションで feature-complete まで
構築した (CLAUDE.md 項目 271 系列、handoff 06-01b〜06-02b)。

本 ADR は、その過程で下した一連の設計判断 — **何を採用し、何を「本規模では不採用」とし、
何を rationale 付きで deferred したか** — を恒久記録する。判断は handoff に散在していたため集約する。

検索基盤は既存の Layer 1 keyword / Layer 2 semantic / Layer 2.5 graph / RRF hybrid / Layer 3
chunk read (ADR-005 / arag_alignment.md) に乗る。本層の主眼は **1bit モデルが実際に消費する
recall 出力の品質** であり、「Scaffolding > Model」(ADR-002) に従いハーネス側で精度を底上げする。

## Decision

### 採用した機構

- **CJK bigram トークン化** (項目 271): 日本語の助詞膠着 (「の使い方」等) で recall 0 件化していた
  真因を根治。実 vault で OLD 0 件 → NEW 739 件。
- **ASCII case-insensitive recall** + **snake_case `_` を ASCII token 化** (`agent_loop` を
  `["agent","loop"]` に分割せず literal 照合) + **LIKE escape** (`_` の LIKE ワイルドカード化による
  全表走査 / IDF 汚染を防止)。本コードベースは snake_case モジュール名が頻出するため実害があった。
- **多 token overlap の IDF 重み付け ranking** (handoff 06-01c)。
- **ingest 編集追従 sync** (exact JSON tag で再 ingest 時に更新) + **削除孤児掃除**
  (`--ingest-prune`)。
- **recall 出典 provenance** (ingest 由来 chunk に filename tag) + **snippet 短縮 + content dedup**。
- **CRLF 正規化** (`chunk_text`): `\r\n\r\n` (Windows / 同期 vault) を段落分割できず
  ファイル全体が 1 巨大 chunk 化していたバグを修正。
- **`BONSAI_DB_PATH` env override** (ADR 起票時点で追加): DB パス解決を
  `env > data_dir > CWD` の 3 段優先順位に。live 検証で production DB を汚染せず隔離 DB に
  逃がせる (`HOME=/tmp` ハックの代替、testability 改善)。`config::resolve_db_path` は I/O なしの
  純粋関数として TDD で実装。

### 本規模では不採用とした機構

- **trigram FTS / full-path tag / vector RRF の追加**: ~9000 chunk・SQLite in-proc・M2 16GB の
  現規模では既存の keyword + IDF + bigram で実用十分な recall を達成しており、追加の複雑性・
  footprint に見合う ROI がない。ccg (codex + gemini + collision audit) の戦略判断で確認。
  桁違いの規模増 or 実測 recall 不足が出た時点で再評価する。
- **recall snippet の match 語ハイライト (【】強調)**: 不採用。理由 = (1) 出力フォーマットを変える
  破壊的変更で「content 完全一致」を前提とする既存テスト群を壊す、(2) **1bit モデルが消費する
  入力**にマーカーを注入する behavior change であり、ADR-003 (paired evidence) 上、未検証で
  blind ship できない。実施するなら Lab paired smoke での効果検証が前提。

## Consequences

**Positive**:
- 助詞・snake_case・CRLF という実 vault で頻出する 3 つの recall 品質ゼロ要因を根治。
- 検索基盤の複雑性を増やさず (既存 5 経路 + bigram/IDF) 現規模の実用品質を確保。
- `BONSAI_DB_PATH` により以降のライブ検証が production 非汚染で安全に行える。
- 「ベンチ改善案でも 1bit 入力への未検証 behavior change は ADR-003 で gate」する規律を recall
  出力にも適用した事例。

**Negative / Trade-off**:
- 純粋ベクトル ANN / trigram の検索能力は持たない。大規模化時は再評価が必要。
- 以下を意図的に deferred とした (現規模で許容、rationale 付き):
  - **`ingest.rs::unchecked_transaction()`**: 外側 tx 活性時に unsound。CLI(ingest) と
    agent-loop(save_session) は同一 store で interleave せず低確率 + 意図的 RAII 設計。
  - **dup-content HashSet dedup**: 同一ファイル内の重複段落が 1 件化 → 影響軽微。
  - **IDF N+1 SQL** (token あたり 1 COUNT): token cap 32 + 本規模で許容。

## Related

- ADR-002 (Scaffolding > Model — recall 出力品質をハーネス側で底上げ)
- ADR-003 (Paired Evidence — snippet highlight 不採用の根拠)
- ADR-005 (sqlite-vec REJECT — 検索層 footprint 判断の先行事例)
- CLAUDE.md 項目 271 系列 / recall・ingest 系コミット
- memory: session_2026_06_01b〜06_02b handoff、arag_alignment.md (検索層 5 経路)
