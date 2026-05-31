# ADR-002: 「Scaffolding > Model」設計原則

## Status: Accepted (2026-05-31、起源は project 創設期)

## Context

bonsai-agent は Bonsai-8B (1 ビット量子化 Qwen3-8B、1.28GB) を Mac M2 16GB 上で動作させる自律型エージェント。
1 ビット量子化モデルは精度 floor が構造的に低く (AgentFloor 6-tier profile: T1=0.68 / T2=0.52 / T6=0.47)、モデル単体の改善余地は限定的。

一方、エージェントの実タスク成功率は p^n 問題 (ステップ蓄積による失敗確率の指数的増大) に支配される。
n ステップのタスクで各ステップ成功率 p なら全体成功率は p^n。p を 0.9 → 0.95 に上げるより、n を減らす / 失敗を検出し回復する harness を厚くする方が ROI が高い。

Lab 実機実験の累積 evidence:
- Lab ACCEPT された変異の大半は harness 側 (項目 10 計画強制 / 47 think directive / 50 fallback / 136 ファイル確認) で、モデル側 fine-tune ではない。
- モデル能力を前提にした機能 (項目 262/263/264 の augmentation 系) は paired evidence でしばしば REJECT (ADR-003 参照)。

## Decision

**「Scaffolding > Model」を全設計判断の第一原則とする。**

1. 改善は **harness (scaffolding) 側で信頼性を底上げ**することを優先し、モデル能力の向上を前提にしない。
2. 具体的 scaffolding 軸: LoopDetector / StallDetector / Continue Sites / 計画強制 / fallback chain / fact-check / compaction / checkpoint / advisor-critic。
3. 新機能は「1 ビットモデルでも壊れない構造的保証」を持つこと。モデルの賢さに依存する機能は慎重に評価 (paired evidence 必須、ADR-003)。
4. p^n 対策 = ステップ数削減 + 失敗検出/回復の二軸。

## Consequences

**Positive**:
- 1 ビットモデルの精度 floor に律速されない信頼性向上経路を確保。
- harness 改善は paired Lab で再現性高く ACCEPT されやすい (項目 10/47/50/136 が実証)。
- モデル交換 (将来のより強力な 1bit/ternary モデル) 時も scaffolding 資産が残る。

**Negative / Trade-off**:
- harness 複雑化のリスク (項目 256 SIZE-001 で benchmark.rs 4476 行等の巨大 file 検出) → Z-4 layer linter で抑制。
- モデル能力で解ける問題まで harness で過剰対処する可能性 → Lab paired evidence で費用対効果を検証。

## Related

- ADR-003 (Paired evidence — scaffolding 効果の検証規律)
- CLAUDE.md プロジェクト概要 + デフォルト化済み変異 (項目 10/47/50/136)
- AgentFloor 6-tier profile (項目 223/224)
- harness_patterns_archive.md (項目 1-268、scaffolding 機構の verbatim 記録)
