#!/usr/bin/env bash
# Lab v22 A/A Test — Phase A: Noise Floor 計測 (項目 247、plan §4 Phase A)
#
# 起点:
#   - Lab v21 smoke REJECT (Pearson r=0、structural metric finding 再現)
#   - CCG synthesis (Codex + Gemini): A/A test で σ_Δ 測定し ACCEPT 閾値根拠確定が必須
#
# 目的: 同一 OFF 設定で paired 5 cycle (10 cycle total) を回し、Δscore = cycle_a - cycle_b
#   の sd σ を noise_floor として採取。Phase D/E の ACCEPT (a) 閾値 = max(0.010, σ × 2) の基準。
#
# 出力: `${LOG_DIR}/test_on_${i}.log` と `test_off_${i}.log` (両側 OFF、命名は paired analyzer
#   互換維持のため `on`/`off` を使うが env は同一)。
#
# 前提:
#   - MLX server 起動済 (`./scripts/start-mlx-server.sh`、port 8000、本 session 整備済)
#   - target/release/bonsai が build 済 (本 session の項目 247 Phase C 後の binary、
#     `BONSAI_LAB_TEMP` env 対応版)
#   - `BONSAI_LAB_TEMP=0` で greedy/deterministic 化 (Gemini 提案)
#
# Wall: ~5h (T=0 で cycle 30 min 想定 × 10 cycle、現行 v21 の 47 min より速い見込み)
#
# 使い方:
#   ./target/release/bonsai がない場合は先に `cargo build --release`
#   chmod +x scripts/lab_v22_aa_test.sh
#   nohup ./scripts/lab_v22_aa_test.sh ./lab-v22-aa-logs > /tmp/lab_v22_aa_run.log 2>&1 &
#   tail -f /tmp/lab_v22_aa_run.log
#   # ~5h 後
#   python3 scripts/lab_v22_metric.py ./lab-v22-aa-logs --mode aa
#   # 出力 sd(Δ) = σ を Phase D/E に --noise-floor で渡す

set -euo pipefail

LOG_DIR="${1:-./lab-v22-aa-logs}"
mkdir -p "$LOG_DIR"

# Z-3 Phase 5: drift monitor post-cycle hook (Phase 2 Green).
# BONSAI_DRIFT_LINT_LAB=1 で drift_lint を Lab cycle 終了後に自動 trigger.
# advisory only: drift exit code を Lab cycle exit code に伝搬させない.
# NOTE: trap は BONSAI_BIN 検査より前に登録することで、早期 exit 時も hook が動作する.
on_lab_complete() {
    local exit_code=$?
    if [[ "${BONSAI_DRIFT_LINT_LAB:-0}" == "1" ]]; then
        local script_dir
        script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
        local last_cycle="${LOG_DIR}/test_off_5.log"
        local commit_hash
        commit_hash=$(git rev-parse HEAD 2>/dev/null || echo unknown)
        echo "=== [drift_lint] Lab cycle complete, running drift_lint... ==="
        # advisory only: drift exit code を Lab exit code に伝搬させない
        bash "${script_dir}/drift/run_lint.sh" || true
        local report
        report="$(cd "${script_dir}/.." && pwd)/docs/quality/drift-$(date +%Y%m%d).md"
        if [[ -f "$report" ]]; then
            {
                echo ""
                echo "## Lab Cycle Linkage"
                echo "- Triggered by: $(basename "${BASH_SOURCE[0]}")"
                echo "- Last cycle log: \`${last_cycle}\`"
                echo "- Commit at trigger: \`${commit_hash}\`"
                echo "- Lab exit code (preserved): ${exit_code}"
            } >> "$report"
            echo "[drift_lint] report appended: ${report}"
        fi
    fi
    exit "$exit_code"  # AC-3/AC-4: 元の exit code を厳密に維持
}
trap on_lab_complete EXIT

BONSAI_BIN="${BONSAI_BIN:-./target/release/bonsai}"
if [[ ! -x "$BONSAI_BIN" ]]; then
    echo "ERROR: $BONSAI_BIN not found or not executable. Run 'cargo build --release' first." >&2
    exit 1
fi

# SMOKE 15 task tier (BONSAI_LAB_SMOKE=1 → smoke_tasks() = SMOKE_TASK_IDS 15 件)
export BONSAI_LAB_SMOKE=1

# Phase C: T=0 greedy 化で sampling noise 排除 (Gemini 提案、項目 247 Phase C)
export BONSAI_LAB_TEMP=0

run_cycle() {
    local label="$1"
    local logfile="$LOG_DIR/${label}.log"
    local start
    start=$(date +%s)
    echo "=== [$(date '+%Y-%m-%d %H:%M:%S')] cycle ${label} START (A/A、両側 OFF、T=0) ===" | tee -a "$logfile"

    # A/A test: 両側とも factcheck OFF (noise floor 測定のため env unset)。
    # ファイル名は `on`/`off` を使うが、env は両側同一 = OFF×OFF。
    # paired analyzer は ファイル名で pair 化するので、同名 pair でも noise floor 測定可。
    unset BONSAI_KG_FACTCHECK_ENABLED
    unset BONSAI_FACTCHECK_ALL_TRAJECTORIES
    "$BONSAI_BIN" --lab --lab-experiments 0 \
        >>"$logfile" 2>&1

    local end
    end=$(date +%s)
    local dur=$((end - start))
    echo "=== [$(date '+%Y-%m-%d %H:%M:%S')] cycle ${label} END (duration=${dur}s) ===" | tee -a "$logfile"
}

# A/A Test Phase: 5 paired (両側 OFF、env 同一)、cycle ごとに seed/state が変動
# (Bonsai は cycle 開始時に reset_session_data_for_lab で events store reset、
# task list は SMOKE 15 task 固定)。T=0 で sampling noise はゼロ、それでも残る Δ は
# 純粋な「実行非決定性 (load/timeing/scheduler/MLX 内部状態)」由来。
for i in 1 2 3 4 5; do
    run_cycle "test_on_${i}"
    run_cycle "test_off_${i}"
done

echo "=== ALL A/A CYCLES COMPLETE ==="
echo "Logs: $LOG_DIR"
echo "Next: python3 scripts/lab_v22_metric.py $LOG_DIR --mode aa"
echo "  → sd(Δ) = σ_noise が出力される。Phase D/E で --noise-floor σ を指定。"
