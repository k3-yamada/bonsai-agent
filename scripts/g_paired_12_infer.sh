#!/usr/bin/env bash
# Issue #12 Phase 2 — 推論パラメータ (temperature / repeat_penalty) paired 検証
#
# 起点: scripts/g_paired_263_v2.sh (項目 263 BUDGET ratio paired re-evaluation) をベースに、
#   MiniCPM5-2B baseline 確立後の推論パラメータチューニング用に作り直したもの。
#
# 変更点 (vs g_paired_263_v2.sh):
#   - `BONSAI_T6_PROMPT_AUGMENT=1` / `BONSAI_DYNAMIC_BUDGET=1` の export を完全削除。
#     項目 268 (BUDGET ratio tune) / 269 (T6 PROMPT_AUGMENT) は paired evidence で
#     REJECT 確定・default OFF 済みの機能であり、本 Lab に混入させてはいけない
#     (CLAUDE.md 直近 5 項目 269 参照)。念のため起動時に明示 unset もしておく。
#   - `BONSAI_LAB_MLX_ONLY` / `BONSAI_LAB_MLX_WARMUP` は一切設定しない
#     (backend 方針: llama-server / GGUF、全パラメータ有効)。
#   - 引数で候補 A / B を選択し、control (temp=0.6, repeat_penalty=1.05 = MiniCPM5-2B
#     既定値) との paired 比較を行う。
#   - 各 cycle ログ冒頭に有効な env (BONSAI_LAB_TEMP / BONSAI_LAB_REPEAT_PENALTY /
#     BONSAI_MODEL_ID / BONSAI_LAB_SMOKE / BONSAI_BENCH_LADDER) を echo し事後検証可能にする。
#   - ログファイル名は `scripts/lab_v22_metric.py::collect_cycles()` の fallback 規約
#     (`cycle_a_{i}.log` = OFF/baseline, `cycle_b_{i}.log` = ON/variant、
#     delta = on(cycle_b) − off(cycle_a)) に厳密対応させる:
#       control (baseline)  → cycle_a_${i}.log
#       candidate (比較対象) → cycle_b_${i}.log
#     どちらの cycle かはファイル名ではなくログ内容 (ROLE= 行) で識別する。
#
# Design:
#   candidate_a: temp=0.3, repeat_penalty=1.1
#   candidate_b: temp=1.0, repeat_penalty=1.05 (repeat_penalty は既定値のまま)
#   control    : temp=0.6, repeat_penalty=1.05 (常に MiniCPM5-2B 既定値)
#   交互 control/candidate ×N paired (2N cycle)。各 cycle: BONSAI_LAB_SMOKE=1 (15 task pool)。
#
# ACCEPT 判定 (上司承認済み、fullモード): p ≤ 0.05 かつ Cohen's dz ≥ 0.40
#
# 前提:
#   - llama-server 起動済 (GGUF、全パラメータ有効)
#   - target/release/bonsai が build 済
#
# Usage:
#   chmod +x scripts/g_paired_12_infer.sh
#   nohup ./scripts/g_paired_12_infer.sh candidate_a ./lab-paired-12-a-logs \
#       > /tmp/g_paired_12_a_run.log 2>&1 &
#   tail -f /tmp/g_paired_12_a_run.log
#   python3 scripts/lab_v22_metric.py ./lab-paired-12-a-logs --mode paired

set -euo pipefail

# Z-3 drift monitor post-cycle hook
# shellcheck source=scripts/drift/lab_hook.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/drift/lab_hook.sh"
trap on_lab_complete EXIT

CANDIDATE="${1:?Usage: $0 <candidate_a|candidate_b> [log_dir]}"
case "$CANDIDATE" in
    candidate_a)
        CAND_TEMP="0.3"
        CAND_RP="1.1"
        ;;
    candidate_b)
        CAND_TEMP="1.0"
        CAND_RP="1.05"
        ;;
    *)
        echo "ERROR: 不明な候補 '$CANDIDATE' (candidate_a か candidate_b を指定)" >&2
        exit 1
        ;;
esac

# control cycle は常に MiniCPM5-2B 既定値 (temp=0.6, repeat_penalty=1.05)。
CONTROL_TEMP="0.6"
CONTROL_RP="1.05"

LOG_DIR="${2:-./lab-paired-12-${CANDIDATE}-logs}"
mkdir -p "$LOG_DIR"

BONSAI_BIN="${BONSAI_BIN:-./target/release/bonsai}"
if [[ ! -x "$BONSAI_BIN" ]]; then
    echo "ERROR: $BONSAI_BIN not found." >&2
    echo "       Run 'cargo build --release' first." >&2
    exit 1
fi

# 共通 env stack (backend: llama-server、BONSAI_LAB_MLX_ONLY / BONSAI_LAB_MLX_WARMUP は
# 一切設定しない)。
export BONSAI_LAB_LONG_SSE=1
export BONSAI_LAB_SMOKE=1              # 15 task pool (Phase 2 paired 検証用)

# 項目 268/269 paired REJECT 確定・default OFF 済み機能の混入防止 (念のため明示 unset)。
unset BONSAI_T6_PROMPT_AUGMENT
unset BONSAI_DYNAMIC_BUDGET
unset BONSAI_T6_MEMORY_AUG
unset BONSAI_LAB_MLX_ONLY
unset BONSAI_LAB_MLX_WARMUP

# ファイル名ラベル (file_label) は lab_v22_metric.py::collect_cycles() の fallback 規約
# (`cycle_a_{i}.log` = OFF/baseline, `cycle_b_{i}.log` = ON/variant) に厳密対応させる。
# 符号規約: delta = on(cycle_b) − off(cycle_a) のため、
#   control (baseline)  → file_label="a" (cycle_a_${i}.log)
#   candidate (比較対象) → file_label="b" (cycle_b_${i}.log)
# role_desc は人間可読なラベル (ログ内容側にのみ残し、事後にどちらの cycle かを
# ファイル名に依らず検証できるようにする)。
run_cycle() {
    local file_label="$1"
    local role_desc="$2"
    local temp="$3"
    local rp="$4"
    local i="$5"
    local logfile="$LOG_DIR/cycle_${file_label}_${i}.log"
    local start
    start=$(date +%s)

    export BONSAI_LAB_TEMP="$temp"
    export BONSAI_LAB_REPEAT_PENALTY="$rp"

    {
        echo "=== [$(date '+%Y-%m-%d %H:%M:%S')] cycle ${role_desc} (file_label=${file_label}, pair=${i}) START ==="
        echo "    ROLE=${role_desc}"
        echo "    BONSAI_LAB_TEMP=${BONSAI_LAB_TEMP}"
        echo "    BONSAI_LAB_REPEAT_PENALTY=${BONSAI_LAB_REPEAT_PENALTY}"
        echo "    BONSAI_MODEL_ID=${BONSAI_MODEL_ID:-<unset>}"
        echo "    BONSAI_LAB_SMOKE=${BONSAI_LAB_SMOKE:-<unset>}"
        echo "    BONSAI_BENCH_LADDER=${BONSAI_BENCH_LADDER:-<unset>}"
    } | tee -a "$logfile"

    "$BONSAI_BIN" --lab --lab-experiments 0 >>"$logfile" 2>&1

    local end
    end=$(date +%s)
    local dur=$((end - start))
    echo "=== [$(date '+%Y-%m-%d %H:%M:%S')] cycle ${role_desc} (file_label=${file_label}, pair=${i}) END (duration=${dur}s) ===" | tee -a "$logfile"
}

# N paired = 2N cycle (control/candidate 交互)、N は BONSAI_PAIRED_COUNT で override 可 (default 5)
PAIRED_COUNT="${BONSAI_PAIRED_COUNT:-5}"
echo "CANDIDATE=${CANDIDATE} (temp=${CAND_TEMP}, repeat_penalty=${CAND_RP}) vs" \
    "CONTROL (temp=${CONTROL_TEMP}, repeat_penalty=${CONTROL_RP})"
echo "PAIRED_COUNT=${PAIRED_COUNT} (env BONSAI_PAIRED_COUNT で override 可、default 5)"
echo "log naming: cycle_a_{i}.log=control(baseline) / cycle_b_{i}.log=${CANDIDATE}" \
    "(lab_v22_metric.py collect_cycles() 規約準拠、delta=on(b)-off(a))"
for i in $(seq 1 "$PAIRED_COUNT"); do
    run_cycle "a" "control" "$CONTROL_TEMP" "$CONTROL_RP" "$i"
    run_cycle "b" "$CANDIDATE" "$CAND_TEMP" "$CAND_RP" "$i"
done

echo "=== ALL PAIRED CYCLES COMPLETE ==="
echo "Logs: $LOG_DIR"
echo ""
echo "Analysis:"
echo "  python3 scripts/lab_v22_metric.py $LOG_DIR --mode paired"
echo ""
echo "ACCEPT 判定 (fullモード): p <= 0.05 かつ Cohen's dz >= 0.40"
