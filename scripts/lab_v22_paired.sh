#!/usr/bin/env bash
# Lab v22 Paired — Phase D: 10-cycle smoke pilot (項目 247、plan §4 Phase D)
#
# 起点:
#   - Lab v21 smoke REJECT (Pearson r structural metric finding 再現)
#   - Phase A (A/A test) で noise_floor σ_Δ 計測済の前提
#   - 本 script は Phase B (新 metric) + Phase C (BONSAI_LAB_TEMP) を使って Δscore 主軸評価
#
# 目的: BONSAI_KG_FACTCHECK_ENABLED ON/OFF paired 5 cycle (10 cycle total) を T=0 で実施、
#   matched/total + Wilcoxon + Cohen's dz + factcheck 補助ゲート の新基準で ACCEPT/REJECT 判定。
#
# 出力: `${LOG_DIR}/test_{on,off}_{1..5}.log`
#
# ACCEPT 基準 (.claude/plan/lab-v22-metric-redesign.md §3.1):
#   (a) mean(Δscore) >= max(+0.010, noise_floor × 2)
#   (b) Wilcoxon one-sided p <= 0.10 (smoke) / 0.05 (full lab)
#   (c) paired Cohen's dz >= 0.30 (smoke) / 0.40 (full)
#   (d) factcheck sanity: matched/total>=0.78 AND unknown/total<=0.05 AND total>=8
#
# 前提:
#   - MLX server 起動済 (port 8000、本 session 整備済 `start-mlx-server.sh`)
#   - target/release/bonsai が build 済 (項目 247 Phase C 後 = `BONSAI_LAB_TEMP` 対応版)
#   - Phase A 完了済で noise_floor σ を取得済
#
# Wall: ~5h (T=0 で cycle 30 min 想定 × 10 cycle)
#
# 使い方:
#   chmod +x scripts/lab_v22_paired.sh
#   nohup ./scripts/lab_v22_paired.sh ./lab-v22-logs > /tmp/lab_v22_run.log 2>&1 &
#   tail -f /tmp/lab_v22_run.log
#   # ~5h 後
#   python3 scripts/lab_v22_metric.py ./lab-v22-logs --mode smoke --noise-floor <σ>

set -euo pipefail

LOG_DIR="${1:-./lab-v22-logs}"
mkdir -p "$LOG_DIR"

# Z-3 Phase 5: drift monitor post-cycle hook (Phase 3 Refactor).
# shared helper (scripts/drift/lab_hook.sh) を source し on_lab_complete を trap 登録.
# BONSAI_DRIFT_LINT_LAB=1 で drift_lint を Lab cycle 終了後に自動 trigger.
# NOTE: trap は BONSAI_BIN 検査より前に登録することで、早期 exit 時も hook が動作する.
# shellcheck source=scripts/drift/lab_hook.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/drift/lab_hook.sh"
trap on_lab_complete EXIT

BONSAI_BIN="${BONSAI_BIN:-./target/release/bonsai}"
if [[ ! -x "$BONSAI_BIN" ]]; then
    echo "ERROR: $BONSAI_BIN not found or not executable. Run 'cargo build --release' first." >&2
    exit 1
fi

# SMOKE 15 task tier (BONSAI_LAB_SMOKE=1 → smoke_tasks() = 15 件)
export BONSAI_LAB_SMOKE=1

# Phase C: T=0 greedy 化で sampling noise 排除
export BONSAI_LAB_TEMP=0

# 項目 263 follow-up: Lab v22 paired の wall time 暴走 (18h51m / unbounded 60 task) 防止.
# default 5 task で cycle ≤ ~30 min × 10 cycle ≈ 5h target に bound.
# operator が override したい場合は事前に `export BONSAI_LAB_TASK_LIMIT=N` で上書き可.
: "${BONSAI_LAB_TASK_LIMIT:=5}"
export BONSAI_LAB_TASK_LIMIT

run_cycle() {
    local label="$1"
    local logfile="$LOG_DIR/${label}.log"
    local start
    start=$(date +%s)
    echo "=== [$(date '+%Y-%m-%d %H:%M:%S')] cycle ${label} START (T=0) ===" | tee -a "$logfile"

    # ON / OFF 切替: factcheck enable + all_trajectories opt-in (項目 235) の 2 軸を一括 ON/OFF
    if [[ "$label" == *"_on_"* ]]; then
        BONSAI_KG_FACTCHECK_ENABLED=1 BONSAI_FACTCHECK_ALL_TRAJECTORIES=1 \
            "$BONSAI_BIN" --lab --lab-experiments 0 \
            >>"$logfile" 2>&1
    else
        unset BONSAI_KG_FACTCHECK_ENABLED
        unset BONSAI_FACTCHECK_ALL_TRAJECTORIES
        "$BONSAI_BIN" --lab --lab-experiments 0 \
            >>"$logfile" 2>&1
    fi

    local end
    end=$(date +%s)
    local dur=$((end - start))
    echo "=== [$(date '+%Y-%m-%d %H:%M:%S')] cycle ${label} END (duration=${dur}s) ===" | tee -a "$logfile"
}

# Test Phase: 5 paired (alternating ON, OFF for each i)
# T=0 で cycle 内 noise はゼロ、Δ は ON 軸の真効力 + residual noise (Phase A の σ) のみ。
# Phase A で σ を測ったので、ACCEPT (a) 閾値 = max(0.010, σ × 2) を Phase B analyzer で適用。
for i in 1 2 3 4 5; do
    run_cycle "test_on_${i}"
    run_cycle "test_off_${i}"
done

echo "=== ALL CYCLES COMPLETE ==="
echo "Logs: $LOG_DIR"
echo "Next: python3 scripts/lab_v22_metric.py $LOG_DIR --mode smoke --noise-floor <σ>"
echo "  ※ σ は Phase A (lab_v22_aa_test.sh) で測定済の値を渡す"
