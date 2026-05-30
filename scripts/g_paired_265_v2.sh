#!/usr/bin/env bash
# Phase 2 Paired Re-evaluation — 項目 264 案 D-2 (MEMORY_AUG) 真効果再評価
#
# plan: .claude/plan/lab-v22-paired-metric-mandatory.md §3 Phase 2 target #2
# 前提 plan: .claude/plan/max-context-tokens-reduction-force-prune.md (G-MCT2 ACCEPT 後実行)
#
# 目的: G-T6-D-2 -19.1% 観測の真因が H-A4 noise vs triple stack destructive どちらか
#   を paired 設計 (同 binary + 同 MLX session + 同 task set) で σ_noise 比較。
#
# Design:
#   A cycle: BONSAI_T6_MEMORY_AUG=0 (baseline = G-T6-D-1 等価、項目 264 D-1)
#   B cycle: BONSAI_T6_MEMORY_AUG=1 (variant = G-T6-D-2 等価、項目 264 D-2)
#   交互 ABABAB... = 5 paired cycle (10 cycle total)
#   各 cycle: 5 T6 lh_* × k=3 = 15 run、wall ~60-80 min (max_context=6000 で短縮期待)
#
# ACCEPT 条件 (paired t-test):
#   Δ score (B - A) ≥ max(0.010, σ_noise × 2) かつ Wilcoxon p < 0.05 かつ Cohen's dz ≥ 0.3
#   σ_noise = lab-v22-paired-metric-mandatory plan Phase 1 A/A test で確立予定値
#
# 前提:
#   - MLX server 起動済 (`./scripts/start-mlx-server.sh`)
#   - target/release/bonsai が a877878+02fb56a 以降で build 済
#   - Phase 4 Smoke G-MCT2 ACCEPT 確認後実行 (prune 発火経路確証済)
#   - σ_noise 値が判明していれば metric phase で適用可能 (未確立で運用も可)
#
# Wall: ~12h (10 cycle × ~70 min/cycle、要 MLX server 連続稼働)
#
# Usage:
#   chmod +x scripts/g_paired_265_v2.sh
#   nohup ./scripts/g_paired_265_v2.sh > /tmp/g_paired_265_run.log 2>&1 &
#   tail -f /tmp/g_paired_265_run.log
#   # ~12h 後
#   python3 scripts/lab_v22_metric.py ./lab-paired-265-v2-logs --mode paired --noise-floor 0.030

set -euo pipefail

# Z-3 drift monitor post-cycle hook
# shellcheck source=scripts/drift/lab_hook.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/drift/lab_hook.sh"
trap on_lab_complete EXIT

LOG_DIR="${1:-./lab-paired-265-v2-logs}"
mkdir -p "$LOG_DIR"

BONSAI_BIN="${BONSAI_BIN:-./target/release/bonsai}"
if [[ ! -x "$BONSAI_BIN" ]]; then
    echo "ERROR: $BONSAI_BIN not found." >&2
    echo "       Run 'cargo build --release' first." >&2
    exit 1
fi

# 共通 env stack (paired 両 cycle で同一)
export BONSAI_LAB_LONG_SSE=1
export BONSAI_LAB_MLX_ONLY=1
export BONSAI_LAB_MLX_WARMUP=1
export BONSAI_LAB_TEMP=0
export BONSAI_LAB_TASK_LIMIT=5
export BONSAI_LAB_SMOKE=1              # 項目 265 max_context=6000 自動
export BONSAI_T6_PROMPT_AUGMENT=1      # 項目 262 stack
export BONSAI_DYNAMIC_BUDGET=1         # 項目 263 + 261 stack

run_cycle() {
    local label="$1"   # cycle_a_1 / cycle_b_1 / ...
    local aug_value="$2"
    local logfile="$LOG_DIR/${label}.log"
    local start
    start=$(date +%s)

    if [[ "$aug_value" == "1" ]]; then
        export BONSAI_T6_MEMORY_AUG=1
    else
        unset BONSAI_T6_MEMORY_AUG
    fi

    echo "=== [$(date '+%Y-%m-%d %H:%M:%S')] cycle ${label} START (MEMORY_AUG=${aug_value}) ===" | tee -a "$logfile"
    "$BONSAI_BIN" --lab --lab-experiments 0 >>"$logfile" 2>&1
    local end
    end=$(date +%s)
    local dur=$((end - start))
    echo "=== [$(date '+%Y-%m-%d %H:%M:%S')] cycle ${label} END (duration=${dur}s) ===" | tee -a "$logfile"
}

# N paired = 2N cycle (ABAB...AB)、N は BONSAI_PAIRED_COUNT で override 可 (default 5)
PAIRED_COUNT="${BONSAI_PAIRED_COUNT:-5}"
echo "PAIRED_COUNT=${PAIRED_COUNT} (env BONSAI_PAIRED_COUNT で override 可、default 5)"
for i in $(seq 1 "$PAIRED_COUNT"); do
    run_cycle "cycle_a_${i}" "0"
    run_cycle "cycle_b_${i}" "1"
done

echo "=== ALL PAIRED CYCLES COMPLETE ==="
echo "Logs: $LOG_DIR"
echo ""
echo "Analysis:"
echo "  python3 scripts/lab_v22_metric.py $LOG_DIR --mode paired"
echo "  (σ_noise が既知なら --noise-floor N で ACCEPT 閾値統合)"
echo ""
echo "Next:"
echo "  - ACCEPT (Δ + Wilcoxon + dz 三軸満たす): 項目 264 案 D-2 真効果再確証 → env default 切替検討"
echo "  - REJECT: H-A4 measurement noise (前 finding 通り) を確定 → infrastructure 削除 plan (case B)"
