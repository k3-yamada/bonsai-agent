# ADR-003: Lab 判定は Paired Evidence を必須とする (unpaired ACCEPT を信頼しない)

## Status: Accepted (2026-05-31)

## Context

bonsai-agent の機能改善は Lab 実機実験 (Bonsai-8B + MLX server) で ACCEPT/REJECT 判定する。
MLX 2-bit 推論は run-to-run の latency / score 変動が大きい (thermal throttling / scheduler jitter / DRAM bandwidth 競合)。

2026-05 の項目 261-268 で、unpaired (単一 cycle) smoke の ACCEPT が paired re-eval で次々と覆った:

| 項目 | 機能 | unpaired smoke | paired re-eval | 判定 |
|------|------|----------------|----------------|------|
| 263 | Dynamic Budget ratio tune | G-DB-R-3 **+9.5%** ACCEPT | mean Δ=-0.0683 / Cohen's dz=-0.86 | **REJECT** (項目 268) |
| 264 | T6 MEMORY_AUG | G-T6 系 mixed | mean Δ=-0.1384 / Cohen's dz=-10.60 | **REJECT** (項目 266) |

決定的 finding (項目 265 G-MCT2): production smoke の k=3 baseline は各 iteration が独立 session で context reset され、4500 tokens 未満で完了するため実 compaction prune が一切発火しない。
即ち unpaired single-cycle の +9.5% などは **cherry-picked measurement noise** であり、機能の真効果ではなかった。
paired ABAB...AB 設計は within-pair で MLX latency 変動を cancel out し、真の Δ を分離する。

(項目 268 destructive path profile: -6.83% の大部分は noise、code path overhead <1% を read-only 解析で確認 = paired でも残る noise の存在を裏付け。)

## Decision

**Lab 由来の ACCEPT 判定で production roll-out / env default ON を決める前に、paired re-evaluation を必須とする。**

1. **unpaired single-cycle smoke は探索 (screening) 専用**。production 判断の根拠にしない。
2. **paired ABAB...AB (推奨 5 pairs 以上)** で mean Δ + Wilcoxon p + Cohen's dz を算出。
3. **ACCEPT 条件** (`scripts/lab_v22_metric.py --mode paired`): mean Δ ≥ +0.010 / Wilcoxon p ≤ 0.10 / Cohen's dz ≥ +0.30 を全て満たす。
4. **σ_noise floor 確立**: A/A test (env=0 vs env=0、同一 binary) で noise floor を測定し、Δ が floor を超えるか判定。
5. **paired で REJECT された機能は速やかに env default OFF 化、infrastructure は paired-evidence-driven cleanup (項目 267 pattern) で撤去 or future-phase base として明示保持**。

## Consequences

**Positive**:
- noise-driven false ACCEPT を排除、production 信頼性向上 ("Scaffolding > Model" ADR-002 の品質保証層)。
- 項目 267 で確立した「paired REJECT → case B 削除」cleanup pattern により codebase 認知負荷を継続的に低減。
- 判定の統計的再現性 (Cohen's dz / Wilcoxon) で議論が evidence-based 化。

**Negative / Trade-off**:
- paired re-eval は wall time が高い (5 pairs × ~77 min/cycle = ~13h、MLX server 必須)。screening との二段階で総コスト増。
- MLX server 環境依存 (user 環境) のため CI 自動化困難、手動運用。
- 一部の真に良い機能も noise で paired ACCEPT 条件を満たさず見送られるリスク (false REJECT) → 複数 pair + A/A floor で緩和。

## Related

- ADR-002 (Scaffolding > Model — 本検証規律が支える品質原則)
- CLAUDE.md 項目 263 (ratio tune) / 266 (MEMORY_AUG REJECT) / 268 (BUDGET REJECT) / 265 (G-MCT2 構造的 finding)
- `.claude/plan/lab-v22-paired-metric-mandatory.md` (A/A → paired 手順)
- `scripts/lab_v22_metric.py` (paired metric: Wilcoxon + Cohen's dz)
- `scripts/g_paired_*_v2.sh` (paired runner)
- memory: item_268_destructive_path_profile_2026_05_31.md (noise 支配の read-only 裏付け)
