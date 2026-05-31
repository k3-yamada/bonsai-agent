# ADR-004: デフォルト化済み変異 (Lab ACCEPT → 恒久適用)

## Status: Accepted (2026-05-31、各変異は個別 Lab session で ACCEPT)

## Context

ハーネス変異は Lab 実機実験で評価し、ACCEPT されたもののみ恒久適用 (default ON / コードに固定) する。
2026-04〜05 の Lab v6〜v22 を通じて、多数の候補変異のうち統計的に有意な改善を示したものは少数だった ("Scaffolding > Model" ADR-002 を裏付ける一方、効果の選別が厳しいことも示す)。

恒久適用された 4 変異 (定量証拠付き):

| 項目 | 変異 | 証拠 |
|------|------|------|
| 10 | 計画強制ルール (タスク冒頭で plan を出力させる) | Lab v6.2 唯一の ACCEPT |
| 47 | ツール使用前 `<think>` で意図記述 | +0.032 実証 |
| 50 | フォールバック戦略 (失敗時の代替手段提示) | +0.001 実証 |
| 136 | 回答前ファイル内容確認 | Lab v9 +0.0157 実証 |

## Decision

**上記 4 変異を default 適用とし、CLAUDE.md「デフォルト化済み変異」section で SSOT 管理する。**

1. これらは system prompt / agent loop に固定され、env gate なしで常時有効。
2. 新変異の default 化は Lab ACCEPT (理想は ADR-003 の paired evidence) を経てのみ。
3. default 化済み変異の除去・改変は同等の Lab evidence を要する (silent regression 防止)。

## Consequences

**Positive**:
- 実証済みの信頼性向上が全 session に効く。
- "Scaffolding > Model" の具体的成果物 (4 変異全て harness 側、モデル fine-tune なし)。

**Negative / Trade-off**:
- 項目 50 (+0.001) は効果が極小で、noise との境界。当時は unpaired 評価だった点に留意 (ADR-003 の paired 規律は後発)。
- default 変異の累積で system prompt が長くなり、1bit context budget を圧迫するリスク → compaction (ADR 候補) で緩和。

## Related

- ADR-002 (Scaffolding > Model — 本変異群はその実証)
- ADR-003 (Paired evidence — 今後の default 化判断基準)
- CLAUDE.md「デフォルト化済み変異」section (項目 10/47/50/136)
- harness_patterns_archive.md (各項目 verbatim)
