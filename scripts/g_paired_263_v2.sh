#!/usr/bin/env bash
# Phase 2 Paired Re-evaluation — 項目 263 BUDGET ratio tune 真効果再評価
#
# plan: .claude/plan/lab-v22-paired-metric-mandatory.md §3 Phase 2 target #1
# 前提 plan: .claude/plan/max-context-tokens-reduction-force-prune.md (G-MCT2 ACCEPT 後実行)
#
# 目的: G-DB-R-3 +9.5% / G-DB-R-2 -0.32% 観測の真因が H-A4 noise vs 真効果どちらかを
#   paired 設計で確証。default 30/30/15/25 (kg-heavy、項目 263) vs
#   40/30/20/10 (旧 default、項目 248) の差を ratio override で実現。
#
# Design:
#   A cycle: BONSAI_DYNAMIC_BUDGET_RATIOS="0.40,0.30,0.20,0.10" (旧 default 等価)
#   B cycle: BONSAI_DYNAMIC_BUDGET_RATIOS="0.30,0.30,0.15,0.25" (項目 263 新 default)
#     (B は unset 時 default と等価だが explicit に env で明示しておく)
#   交互 ABABAB... = 5 paired cycle (10 cycle total)
#   各 cycle: 5 T6 lh_* × k=3 = 15 run、wall ~60-80 min
#
# ACCEPT 条件 (B - A、新 default が旧より優位か):
#   Δ score ≥ max(0.010, σ_noise × 2) かつ Wilcoxon p < 0.05 かつ Cohen's dz ≥ 0.3
#   σ_noise = A/A test Phase 1 で確立予定値
#
# 前提:
#   - MLX server 起動済
#   - Phase 4 Smoke G-MCT2 ACCEPT 確認後実行 (prune 発火経路確証済)
#   - σ_noise 値が既知なら post-processing で適用
#
# Wall: ~12h (10 cycle × ~70 min/cycle)
#
# Usage:
#   chmod +x scripts/g_paired_263_v2.sh
#   nohup ./scripts/g_paired_263_v2.sh > /tmp/g_paired_263_run.log 2>&1 &
#   tail -f /tmp/g_paired_263_run.log
#   # ~12h 後
#   python3 scripts/lab_v22_metric.py ./lab-paired-263-v2-logs --mode paired

set -euo pipefail

# Z-3 drift monitor post-cycle hook
# shellcheck source=scripts/drift/lab_hook.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/drift/lab_hook.sh"
trap on_lab_complete EXIT

LOG_DIR="${1:-./lab-paired-263-v2-logs}"
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
export BONSAI_T6_PROMPT_AUGMENT=1      # 項目 262 stack
export BONSAI_DYNAMIC_BUDGET=1         # 項目 263 + 261 stack
unset BONSAI_T6_MEMORY_AUG             # 項目 264 D-2 は OFF (本 paired は ratio diff のみ)

run_cycle() {
    local label="$1"
    local ratios="$2"
    local logfile="$LOG_DIR/${label}.log"
    local start
    start=$(date +%s)

    export BONSAI_DYNAMIC_BUDGET_RATIOS="$ratios"

    echo "=== [$(date '+%Y-%m-%d %H:%M:%S')] cycle ${label} START (RATIOS=${ratios}) ===" | tee -a "$logfile"
    "$BONSAI_BIN" --lab --lab-experiments 0 >>"$logfile" 2>&1
    local end
    end=$(date +%s)
    local dur=$((end - start))
    echo "=== [$(date '+%Y-%m-%d %H:%M:%S')] cycle ${label} END (duration=${dur}s) ===" | tee -a "$logfile"
}

# N paired = 2N cycle (ABAB...AB)、N は BONSAI_PAIRED_COUNT で override 可 (default 5)
# A = 旧 default 40/30/20/10、B = 新 default 30/30/15/25
PAIRED_COUNT="${BONSAI_PAIRED_COUNT:-5}"
echo "PAIRED_COUNT=${PAIRED_COUNT} (env BONSAI_PAIRED_COUNT で override 可、default 5)"
for i in $(seq 1 "$PAIRED_COUNT"); do
    run_cycle "cycle_a_${i}" "0.40,0.30,0.20,0.10"
    run_cycle "cycle_b_${i}" "0.30,0.30,0.15,0.25"
done

echo "=== ALL PAIRED CYCLES COMPLETE ==="
echo "Logs: $LOG_DIR"
echo ""
echo "Analysis:"
echo "  python3 scripts/lab_v22_metric.py $LOG_DIR --mode paired"
echo ""
echo "Next:"
echo "  - ACCEPT (B > A): 項目 263 ratio tune 真効果確証 → default 維持確定"
echo "  - REJECT or ≈0: H-A4 noise の可能性 → revert to 40/30/20/10 検討"
