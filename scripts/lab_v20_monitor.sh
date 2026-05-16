#!/usr/bin/env bash
# Lab v20 進捗 ad-hoc snapshot utility (項目 238、本 session 2026-05-16)
#
# 目的:
#   - lab-v20-logs/test_{on,off}_{1..5}.log の size + 完走 cycle 数表示
#   - bonsai PID 稼働確認 (ps -ef | grep)
#   - 完走 cycle 数 + 残 cycle 数の概算 + ETA
#   - ON cycle の [INFO][lab.factcheck] total / conflicting / unknown を逐次表示
#
# 使い方:
#   ./scripts/lab_v20_monitor.sh                 # 即時 snapshot
#   watch -n 60 ./scripts/lab_v20_monitor.sh     # 60 秒毎更新
#
# 完走後の解析:
#   python3 scripts/lab_v20_paired_ttest.py ./lab-v20-logs

set -euo pipefail

LOG_DIR="${1:-./lab-v20-logs}"

if [[ ! -d "$LOG_DIR" ]]; then
    echo "ERROR: log dir not found: $LOG_DIR" >&2
    exit 1
fi

echo "=== [Lab v20 KG-FactCheck Snapshot $(date '+%Y-%m-%d %H:%M:%S')] ==="
echo

# 1) bonsai プロセス確認
echo "## bonsai プロセス"
BONSAI_PIDS=$(ps -ef | grep "target/release/bonsai --lab" | grep -v grep | awk '{print $2}' || true)
if [[ -z "$BONSAI_PIDS" ]]; then
    echo "  ⚠ bonsai --lab プロセス不在 (Lab 完走 or crash)"
else
    echo "  ✓ PID(s): $BONSAI_PIDS 稼働中"
fi
echo

# 2) cycle 進捗
echo "## cycle 進捗"
COMPLETED=0
IN_PROGRESS=""
for i in 1 2 3 4 5; do
    for kind in on off; do
        LABEL="test_${kind}_${i}"
        LOGFILE="${LOG_DIR}/${LABEL}.log"
        if [[ ! -f "$LOGFILE" ]]; then
            continue
        fi
        SIZE=$(stat -f%z "$LOGFILE" 2>/dev/null || echo 0)
        if grep -q "END (duration=" "$LOGFILE" 2>/dev/null; then
            DUR=$(grep "END (duration=" "$LOGFILE" | tail -1 | grep -oE 'duration=[0-9]+' | head -1 | cut -d= -f2)
            # ON cycle なら factcheck 結果も表示
            FC_INFO=""
            if [[ "$kind" == "on" ]]; then
                FC_LINE=$(grep "lab.factcheck.*FactCheck post-Lab" "$LOGFILE" | tail -1 || true)
                if [[ -n "$FC_LINE" ]]; then
                    TOTAL=$(echo "$FC_LINE" | grep -oE 'total=[0-9]+' | head -1 | cut -d= -f2)
                    CONF=$(echo "$FC_LINE" | grep -oE 'conflicting=[0-9]+' | head -1 | cut -d= -f2)
                    UNK=$(echo "$FC_LINE" | grep -oE 'unknown=[0-9]+' | head -1 | cut -d= -f2)
                    FC_INFO=" [fact: total=${TOTAL} conf=${CONF} unk=${UNK}]"
                fi
            fi
            echo "  ✓ ${LABEL}: ${SIZE} bytes, ${DUR}s${FC_INFO}"
            COMPLETED=$((COMPLETED + 1))
        else
            echo "  ⏳ ${LABEL}: ${SIZE} bytes (進行中)"
            IN_PROGRESS="${LABEL}"
        fi
    done
done
echo
echo "  完走: ${COMPLETED}/10 cycle (warm-up なし、5 paired ON/OFF)"

# 3) 最新 cycle の tail 3 行
if [[ -n "$IN_PROGRESS" ]]; then
    echo
    echo "## 最新 cycle (${IN_PROGRESS}) tail 3 行"
    tail -3 "${LOG_DIR}/${IN_PROGRESS}.log" 2>/dev/null | sed 's/^/  /'
fi

# 4) 経過時間 + 残 cycle 推定
if [[ -f "${LOG_DIR}/test_on_1.log" ]]; then
    FIRST_START=$(grep -oE "[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2}" "${LOG_DIR}/test_on_1.log" | head -1 || true)
    if [[ -n "$FIRST_START" ]]; then
        echo
        echo "## 経過時間"
        START_EPOCH=$(date -j -f "%Y-%m-%d %H:%M:%S" "$FIRST_START" "+%s" 2>/dev/null || echo 0)
        NOW_EPOCH=$(date "+%s")
        ELAPSED=$((NOW_EPOCH - START_EPOCH))
        ELAPSED_H=$((ELAPSED / 3600))
        ELAPSED_M=$(((ELAPSED % 3600) / 60))
        echo "  開始: $FIRST_START"
        echo "  経過: ${ELAPSED_H}h ${ELAPSED_M}m (${ELAPSED}s)"
        if [[ $COMPLETED -gt 0 ]]; then
            AVG_PER_CYCLE=$((ELAPSED / COMPLETED))
            REMAINING=$((10 - COMPLETED))
            ETA_SEC=$((AVG_PER_CYCLE * REMAINING))
            ETA_H=$((ETA_SEC / 3600))
            ETA_M=$(((ETA_SEC % 3600) / 60))
            echo "  残 cycle 平均推定: ${AVG_PER_CYCLE}s/cycle、ETA ${ETA_H}h ${ETA_M}m"
        else
            echo "  残 cycle 推定: 平均値算出待ち (cycle 1 完走後再計算)"
        fi
    fi
fi

echo

# 5) 完走検出 + 次ステップ案内
if [[ $COMPLETED -ge 10 ]]; then
    echo "## ✅ Lab v20 完走 (10/10 cycle)"
    echo "  次ステップ (Pearson r ACCEPT 判定):"
    echo "    python3 scripts/lab_v20_paired_ttest.py ./lab-v20-logs"

    # macOS desktop notification (env `BONSAI_MONITOR_NOTIFY=1` で ON)
    if [[ "${BONSAI_MONITOR_NOTIFY:-0}" == "1" ]]; then
        if command -v osascript &>/dev/null; then
            osascript -e 'display notification "Lab v20 全 10 cycle 完走、Pearson r 解析起動可" with title "Bonsai Lab v20 Complete" sound name "Glass"' 2>/dev/null || true
        fi
    fi
elif [[ -z "$BONSAI_PIDS" ]] && [[ $COMPLETED -lt 10 ]]; then
    echo "## ⚠️ bonsai プロセス不在 + 未完走 ($COMPLETED/10) = crash 疑い"
    echo "  /tmp/lab_v20_run.log を tail で確認推奨:"
    echo "    tail -50 /tmp/lab_v20_run.log"
    if [[ "${BONSAI_MONITOR_NOTIFY:-0}" == "1" ]]; then
        if command -v osascript &>/dev/null; then
            osascript -e "display notification \"Lab v20 crash 疑い ($COMPLETED/10 cycle で停止)\" with title \"Bonsai Lab v20 Alert\" sound name \"Basso\"" 2>/dev/null || true
        fi
    fi
fi

echo "=== End of snapshot ==="
