#!/usr/bin/env bash
# Lab v17 — ERL Heuristics Pool effectiveness paired evaluation (項目 214/215)
#
# 目的: BONSAI_ERL_DISABLED toggle を用いた ON/OFF paired t-test 用ログ収集。
#   - Warm-up Phase: 2 cycle ON で HeuristicStore に pool 蓄積
#   - Test Phase:    5 paired (ON, OFF) cycle = 10 cycle、cycle 内 task list 同一 (core 22 deterministic)
#   - 合計 12 cycle、各 ~60-90 min、計 ~12-18h
#
# 前提:
#   - llama-server を別 terminal で先に起動済 (`-c 12288 --flash-attn on` 推奨)
#   - target/release/bonsai が build 済 (`cargo build --release`)
#   - core 22 task tier (BONSAI_BENCH_TIER=core)
#
# 使い方:
#   chmod +x scripts/lab_v17_paired.sh
#   nohup ./scripts/lab_v17_paired.sh ./lab-v17-logs > /tmp/lab_v17_run.log 2>&1 &
#   tail -f /tmp/lab_v17_run.log
#   # ~12-18h 後
#   python3 scripts/lab_v17_paired_ttest.py ./lab-v17-logs

set -euo pipefail

LOG_DIR="${1:-./lab-v17-logs}"
mkdir -p "$LOG_DIR"

BONSAI_BIN="${BONSAI_BIN:-./target/release/bonsai}"
if [[ ! -x "$BONSAI_BIN" ]]; then
    echo "ERROR: $BONSAI_BIN not found or not executable. Run 'cargo build --release' first." >&2
    exit 1
fi

export BONSAI_BENCH_TIER="${BONSAI_BENCH_TIER:-core}"

run_cycle() {
    local label="$1"
    local logfile="$LOG_DIR/${label}.log"
    local start
    start=$(date +%s)
    echo "=== [$(date '+%Y-%m-%d %H:%M:%S')] cycle ${label} START ===" | tee -a "$logfile"

    # ON / OFF 切替
    if [[ "$label" == *"_off_"* ]]; then
        BONSAI_ERL_DISABLED=1 "$BONSAI_BIN" --lab --lab-experiments 0 \
            >>"$logfile" 2>&1
    else
        unset BONSAI_ERL_DISABLED
        "$BONSAI_BIN" --lab --lab-experiments 0 \
            >>"$logfile" 2>&1
    fi

    local end
    end=$(date +%s)
    local dur=$((end - start))
    echo "=== [$(date '+%Y-%m-%d %H:%M:%S')] cycle ${label} END (duration=${dur}s) ===" | tee -a "$logfile"
}

# Warm-up Phase: 2 cycle ON (空 pool → 蓄積)
for i in 1 2; do
    run_cycle "warmup_${i}"
done

# Test Phase: 5 paired (alternating ON, OFF for each i)
# 順序固定 (ON 先・OFF 後) の時間効果は reset_session_data_for_lab で events store が
# cycle 開始時に reset されるため event-level 影響は隔離される。
# heuristics persist は ON cycle にのみ蓄積され OFF cycle では inject_heuristics 短絡で
# 参照されないため、paired 性は保たれる。
for i in 1 2 3 4 5; do
    run_cycle "test_on_${i}"
    run_cycle "test_off_${i}"
done

echo "=== ALL CYCLES COMPLETE ==="
echo "Logs: $LOG_DIR"
echo "Next: python3 scripts/lab_v17_paired_ttest.py $LOG_DIR"
