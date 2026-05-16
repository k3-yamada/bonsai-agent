#!/usr/bin/env bash
# Lab v20 — KG-Grounded Hallucination Check effectiveness paired evaluation
#           (Plan A 系列 Phase 5、項目 230 + 235 + 237 累積、plan §3 Phase 5)
#
# 目的: BONSAI_KG_FACTCHECK_ENABLED + BONSAI_FACTCHECK_ALL_TRAJECTORIES toggle で
#   ON/OFF paired Pearson r 解析用ログ収集。
#   - factcheck は pool 蓄積を伴わない (KG は別途蓄積、factcheck pass は read-only)
#   - 5 paired (ON, OFF) cycle = 10 cycle、cycle 内 task list 同一 (core 22 deterministic)
#   - 合計 10 cycle、各 ~60-90 min、計 ~10-15h
#
# ACCEPT 基準 (plan §2 主条件 AND):
#   (a) Pearson r ≥ 0.3 (ON 5 cycle の (conflict+unknown)/total vs failure_rate 相関)
#   (b) ON cycle 全 5 件で total >= 1 (factcheck pass が triple 抽出 + KG 検証で実発火)
#
#   副次観察 (informational only):
#   - paired t-test (Δscore mean / p-value、Lab v17 同) — factcheck 設計上 score 寄与なし
#
# 前提:
#   - llama-server を別 terminal で先に起動済 (`-c 16384 --flash-attn on` 推奨)
#   - target/release/bonsai が build 済 (`cargo build --release`、本 commit 795004c 以降必須 =
#     項目 237 emit hook 含む binary)
#   - core 22 task tier (BONSAI_BENCH_TIER=core)
#
# 使い方:
#   chmod +x scripts/lab_v20_paired.sh
#   nohup ./scripts/lab_v20_paired.sh ./lab-v20-logs > /tmp/lab_v20_run.log 2>&1 &
#   tail -f /tmp/lab_v20_run.log
#   # ~10-15h 後
#   python3 scripts/lab_v20_paired_ttest.py ./lab-v20-logs

set -euo pipefail

LOG_DIR="${1:-./lab-v20-logs}"
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

    # ON / OFF 切替: factcheck enable + all_trajectories opt-in (項目 235) の 2 軸を一括 ON/OFF。
    # 項目 237 emit hook は env-gated でないため両 cycle で AssistantMessage event は emit される
    # (factcheck pass が ON 時のみ events を読み triple 抽出)。
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
# 順序固定 (ON 先・OFF 後) の時間効果は reset_session_data_for_lab で events store が
# cycle 開始時に reset されるため event-level 影響は隔離される。
# factcheck は cycle 内 read-only pass (KG 蓄積なし) のため paired 性は保たれる。
for i in 1 2 3 4 5; do
    run_cycle "test_on_${i}"
    run_cycle "test_off_${i}"
done

echo "=== ALL CYCLES COMPLETE ==="
echo "Logs: $LOG_DIR"
echo "Next: python3 scripts/lab_v20_paired_ttest.py $LOG_DIR"
