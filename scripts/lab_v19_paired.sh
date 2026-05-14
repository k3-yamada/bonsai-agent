#!/usr/bin/env bash
# Lab v19 — Frontier Benchmark effectiveness paired evaluation (項目 229 plan §3 Phase 5)
#
# 目的: BONSAI_FRONTIER_{ENABLED,INJECT_ENABLED} toggle で ON/OFF paired t-test 用ログ収集。
#   - frontier は pool 蓄積を伴わない (cycle 内独立) ため warm-up 不要
#   - Test Phase: 5 paired (ON, OFF) cycle = 10 cycle、cycle 内 task list 同一 (core 22 deterministic)
#   - 合計 10 cycle、各 ~75-90 min (v17 比 +15% bucket aggregation + T6 inject runs)、計 ~12-15h
#
# ACCEPT 基準 (plan §3 Phase 5 + Lab v17 と同 conservative pattern):
#   (a) mean Δscore ≥ +0.015 AND p < 0.10 → ACCEPT (production default ON)
#   (b) OR bucket [8K, 16K)+ で OFF baseline 比 score variance ≥ +50% 拡大
#       (第 6 軸 = context-length axis baseline 確立、ttest 別計測)
#
# 前提:
#   - llama-server を別 terminal で先に起動済 (`-c 16384 --flash-attn on` 推奨、PID 26629 で稼働中)
#   - target/release/bonsai が build 済 (`cargo build --release`)
#   - core 22 task tier (BONSAI_BENCH_TIER=core)
#
# 使い方:
#   chmod +x scripts/lab_v19_paired.sh
#   nohup ./scripts/lab_v19_paired.sh ./lab-v19-logs > /tmp/lab_v19_run.log 2>&1 &
#   tail -f /tmp/lab_v19_run.log
#   # ~12-15h 後
#   python3 scripts/lab_v19_paired_ttest.py ./lab-v19-logs

set -euo pipefail

LOG_DIR="${1:-./lab-v19-logs}"
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

    # ON / OFF 切替: frontier bucketing + inject の 2 軸を一括 ON/OFF
    if [[ "$label" == *"_on_"* ]]; then
        BONSAI_FRONTIER_ENABLED=1 BONSAI_FRONTIER_INJECT_ENABLED=1 \
            "$BONSAI_BIN" --lab --lab-experiments 0 \
            >>"$logfile" 2>&1
    else
        unset BONSAI_FRONTIER_ENABLED
        unset BONSAI_FRONTIER_INJECT_ENABLED
        "$BONSAI_BIN" --lab --lab-experiments 0 \
            >>"$logfile" 2>&1
    fi

    local end
    end=$(date +%s)
    local dur=$((end - start))
    echo "=== [$(date '+%Y-%m-%d %H:%M:%S')] cycle ${label} END (duration=${dur}s) ===" | tee -a "$logfile"
}

# Test Phase: 5 paired (alternating ON, OFF for each i)
# 順序固定 (ON 先・OFF 後) の時間効果は reset_session_data_for_lab で events store が
# cycle 開始時に reset されるため event-level 影響は隔離される。
# frontier metric は cycle 内独立計測 (pool 蓄積なし) のため paired 性は保たれる。
for i in 1 2 3 4 5; do
    run_cycle "test_on_${i}"
    run_cycle "test_off_${i}"
done

echo "=== ALL CYCLES COMPLETE ==="
echo "Logs: $LOG_DIR"
echo "Next: python3 scripts/lab_v19_paired_ttest.py $LOG_DIR"
