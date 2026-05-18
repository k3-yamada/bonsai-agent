#!/usr/bin/env bash
# Lab v21 Smoke — Pearson r 検証代替 (項目 244 後の paired smoke、SMOKE_TASK_IDS 15 task)
#
# 起点:
#   - 項目 241 Lab v20 (core 32 task / 10 cycle) wall 19h 9m で完走するも Pearson r=0.0
#     (matched=0 deterministic で variance ゼロ、structural finding)
#   - 項目 242 G-7b で matched=8 復活実証、項目 243 G-7c で matched=12 / score 0.7613 達成
#   - Lab v21 paired (core 32 task) は cycle 1 で 5h+ slowdown により kill (project memory 662)
#   - 本 smoke paired は 15 task × k=3 × 10 cycle (~8h) で Pearson r 計算代替
#
# 目的: BONSAI_KG_FACTCHECK_ENABLED + BONSAI_FACTCHECK_ALL_TRAJECTORIES toggle で
#   ON/OFF paired Pearson r 解析。core 32 task より wall を 1/4 に縮め、cycle 1 slowdown
#   risk を回避。
#   - 5 paired (ON, OFF) cycle = 10 cycle、cycle 内 task list 同一 (SMOKE 15 deterministic)
#   - 各 cycle ~47 min (G-7c 実測)、計 ~8h
#
# ACCEPT 基準 (主条件 AND):
#   (a) Pearson r ≥ 0.3 (ON 5 cycle の matched/total vs failure_rate 相関 — matched 軸 variance)
#   (b) ON cycle 全 5 件で total >= 10 (15 task 中 ≥ 2/3 が factcheck emit)
#
#   副次観察 (informational only):
#   - paired t-test (Δscore mean / p-value、Lab v17 同形式) — factcheck 設計上 score 寄与なし想定
#   - matched/total 比 (G-7c で 12/15=0.80、paired 10 cycle で 0.70-0.85 想定)
#
# 前提:
#   - llama-server を別 terminal で先に起動済 (`-c 16384 --flash-attn on` 推奨)
#   - target/release/bonsai が build 済 (commit 2995085 以降 = 項目 244 ephemeral KG fix 含む)
#   - SMOKE_TASK_IDS は benchmark.rs で 15 task (success_fact 5 + halluc 3 + 既存 7)
#
# 使い方:
#   chmod +x scripts/lab_v21_smoke_paired.sh
#   nohup ./scripts/lab_v21_smoke_paired.sh ./lab-v21-smoke-logs > /tmp/lab_v21_smoke_run.log 2>&1 &
#   tail -f /tmp/lab_v21_smoke_run.log
#   # ~8h 後
#   python3 scripts/lab_v21_paired_ttest.py ./lab-v21-smoke-logs

set -euo pipefail

LOG_DIR="${1:-./lab-v21-smoke-logs}"
mkdir -p "$LOG_DIR"

BONSAI_BIN="${BONSAI_BIN:-./target/release/bonsai}"
if [[ ! -x "$BONSAI_BIN" ]]; then
    echo "ERROR: $BONSAI_BIN not found or not executable. Run 'cargo build --release' first." >&2
    exit 1
fi

# SMOKE 15 task tier (BONSAI_LAB_SMOKE=1 → smoke_tasks() = SMOKE_TASK_IDS 15 件)
export BONSAI_LAB_SMOKE=1

run_cycle() {
    local label="$1"
    local logfile="$LOG_DIR/${label}.log"
    local start
    start=$(date +%s)
    echo "=== [$(date '+%Y-%m-%d %H:%M:%S')] cycle ${label} START ===" | tee -a "$logfile"

    # ON / OFF 切替: factcheck enable + all_trajectories opt-in (項目 235) の 2 軸を一括 ON/OFF。
    # 項目 237 emit hook は env-gated でないため両 cycle で AssistantMessage event は emit される
    # (factcheck pass が ON 時のみ events を読み triple 抽出)。
    # 項目 244 ephemeral KG fix で seed-only scope lint = production KG 累積による false positive なし。
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
echo "Next: python3 scripts/lab_v21_paired_ttest.py $LOG_DIR"
