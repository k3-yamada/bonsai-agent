# ADR-007: KG Fact-Check による Anti-Hallucination (Plan A 系列)

## Status: Accepted (2026-05-31)

## Context

1 ビット量子化モデル (Bonsai-8B) は精度 floor が低く、事実の fabrication (hallucination) が起きやすい。
agent の回答に含まれる主張を検証する仕組みとして、Knowledge Graph (KG) を用いた fact-check を Plan A 系列 (項目 230/234/235/237/239) で実装した。

経緯:
- **項目 230**: KG fact-check Plan A の基本 wiring (`factcheck.rs`、回答主張を KG fact と照合)。
- **項目 234**: 真因 finding (failed-only trajectory ↔ hallucination SUCCESS の排他)。
- **項目 235**: Trajectory Scope Expansion (env-gated `BONSAI_FACTCHECK_ALL_TRAJECTORIES=1`)。
- **項目 237**: AssistantMessage event emit hook → **conflicting=3 = Bonsai-8B fabricate 検出初成功**。
- **項目 239**: Pattern 1 regex dash fix。

検証軸: `total` (検証対象主張数) / `matched` (KG 一致) / `conflicting` (KG と矛盾 = fabrication 疑い) / `unknown` (KG に情報なし)。
Lab v20 finding (項目 241): `matched=0` 時に `(conflicting+unknown)/total=1.0` が deterministic になる structural property。conflicting=3 deterministic は Plan A の真効力として安定確証。

## Decision

**KG fact-check を anti-hallucination の検証層として採用し、env-gated で運用する。**

1. 回答主張を KG fact と照合し、`conflicting` (矛盾) を fabrication signal として検出。
2. ephemeral KG (`MemoryStore::in_memory()`) で seed-only scope に限定し false positive (conflicting=1995 等) を回避 (項目 244 KG lint と整合)。
3. `BONSAI_FACTCHECK_ALL_TRAJECTORIES` / `BONSAI_KG_LINT_STRICT` 等 env で scope/厳格度を制御。
4. audit log に FactCheck action を emit し、検証の可観測性を確保。

## Consequences

**Positive**:
- 1bit モデルの fabrication を構造的に検出 (conflicting=3 実証)、回答信頼性向上 (ADR-002 scaffolding の安全層)。
- ephemeral KG で seed scope 限定、production memory を汚染しない。

**Negative / Trade-off**:
- KG seed の網羅性に依存 (`unknown` が多いと検証カバレッジ低)。
- `matched=0` structural property により Lab score 軸での効果測定が難しい (項目 241 で天井検出)。検証は conflicting deterministic で代替。

## Related

- ADR-002 (Scaffolding > Model — fact-check は安全層 scaffolding)
- CLAUDE.md Safety / Filter / Anti-Halluc カテゴリ (項目 230/234/235/237/239)
- KG lint (項目 244)、Lab v20 (項目 241 structural finding)
- harness_patterns_archive.md (Plan A 系列 verbatim)
