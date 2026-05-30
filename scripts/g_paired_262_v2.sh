#!/usr/bin/env bash
# Phase 2 Paired Re-evaluation — 項目 262 T6 PROMPT_AUGMENT 真効果再評価
#
# plan: .claude/plan/lab-v22-paired-metric-mandatory.md §3 Phase 2 target #3
# 前提 plan: .claude/plan/max-context-tokens-reduction-force-prune.md (G-MCT2 ACCEPT 後実行)
#
# 目的: G-T6-2 +14.4% strong ACCEPT 観測の真因が unpaired/historical noise vs 真効果
#   どちらかを paired 設計で確証。AUGMENT directive (3 段 step-by-step+restate+revise)
#   が単独で T6 LongHorizonPlanning にどの程度寄与するかを再測定。
#
# Design:
#   A cycle: BONSAI_T6_PROMPT_AUGMENT unset (baseline = G-T6-1 等価、unset)
#   B cycle: BONSAI_T6_PROMPT_AUGMENT=1 (variant = G-T6-2 等価、+14.4% claim)
#   交互 ABABAB... = 5 paired cycle (10 cycle total)
#   各 cycle: 5 T6 lh_* × k=3 = 15 run、wall ~60-80 min
#
# ACCEPT 条件 (B - A、AUGMENT が unset より優位か):
#   Δ score ≥ max(0.010, σ_noise × 2) かつ Wilcoxon p < 0.05 かつ Cohen's dz ≥ 0.3
#   期待 = +0.05 ~ +0.10 (項目 262 historical 0.7671 → 0.8778 = +14.4% を paired で再現)
#
# 前提:
#   - MLX server 起動済
#   - Phase 4 Smoke G-MCT2 ACCEPT 確認後実行
#   - σ_noise 値が既知なら post-processing で適用
#
# Wall: ~12h (10 cycle × ~70 min/cycle)
#
# Usage:
#   chmod +x scripts/g_paired_262_v2.sh
#   nohup ./scripts/g_paired_262_v2.sh > /tmp/g_paired_262_run.log 2>&1 &
#   tail -f /tmp/g_paired_262_run.log
#   # ~12h 後
#   python3 scripts/lab_v22_metric.py ./lab-paired-262-v2-logs --mode paired

set -euo pipefail

# Z-3 drift monitor post-cycle hook
# shellcheck source=scripts/drift/lab_hook.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/drift/lab_hook.sh"
trap on_lab_complete EXIT

LOG_DIR="${1:-./lab-paired-262-v2-logs}"
mkdir -p "$LOG_DIR"

BONSAI_BIN="${BONSAI_BIN:-./target/release/bonsai}"
if [[ ! -x "$BONSAI_BIN" ]]; then
    echo "ERROR: $BONSAI_BIN not found." >&2
    echo "       Run 'cargo build --release' first." >&2
    exit 1
fi

# 共通 env stack
export BONSAI_LAB_LONG_SSE=1
export BONSAI_LAB_MLX_ONLY=1
export BONSAI_LAB_MLX_WARMUP=1
export BONSAI_LAB_TEMP=0
export BONSAI_LAB_TASK_LIMIT=5
export BONSAI_LAB_SMOKE=1              # 項目 265 max_context=6000 自動
export BONSAI_DYNAMIC_BUDGET=1         # 項目 263 + 261 stack
unset BONSAI_T6_MEMORY_AUG             # 項目 264 D-2 は OFF (本 paired は AUGMENT diff のみ)
unset BONSAI_DYNAMIC_BUDGET_RATIOS     # default 30/30/15/25 を使用 (項目 263 維持)

run_cycle() {
    local label="$1"
    local aug_value="$2"
    local logfile="$LOG_DIR/${label}.log"
    local start
    start=$(date +%s)

    if [[ "$aug_value" == "1" ]]; then
        export BONSAI_T6_PROMPT_AUGMENT=1
    else
        unset BONSAI_T6_PROMPT_AUGMENT
    fi

    echo "=== [$(date '+%Y-%m-%d %H:%M:%S')] cycle ${label} START (PROMPT_AUGMENT=${aug_value}) ===" | tee -a "$logfile"
    "$BONSAI_BIN" --lab --lab-experiments 0 >>"$logfile" 2>&1
    local end
    end=$(date +%s)
    local dur=$((end - start))
    echo "=== [$(date '+%Y-%m-%d %H:%M:%S')] cycle ${label} END (duration=${dur}s) ===" | tee -a "$logfile"
}

# 5 paired = 10 cycle (ABAB...AB)
for i in 1 2 3 4 5; do
    run_cycle "cycle_a_${i}" "0"
    run_cycle "cycle_b_${i}" "1"
done

echo "=== ALL PAIRED CYCLES COMPLETE ==="
echo "Logs: $LOG_DIR"
echo ""
echo "Analysis:"
echo "  python3 scripts/lab_v22_metric.py $LOG_DIR --mode paired"
echo ""
echo "Next:"
echo "  - ACCEPT (B > A): 項目 262 AUGMENT 真効果再確証 → default 切替検討"
echo "  - REJECT or ≈0: H-A4 noise の可能性 → BONSAI_T6_PROMPT_AUGMENT 維持 OFF"
