#!/usr/bin/env bash
# Smoke / Paired log progress checker — operator が任意時点で smoke 進捗を 1 行確認.
#
# 対応 smoke runner:
#   - g_mct2_smoke.sh (項目 265 max_context reduction)
#   - g_paired_262_v2.sh / g_paired_263_v2.sh / g_paired_265_v2.sh (Phase 2 paired)
#   - lab_v22_aa_test.sh (A/A test)
#   - その他 lab-*-logs/ 系列
#
# Usage:
#   ./scripts/smoke_status.sh                              # auto-detect 最新 log
#   ./scripts/smoke_status.sh lab-265-smoke-logs/g_mct2_smoke.log
#   ./scripts/smoke_status.sh lab-paired-265-v2-logs/      # directory も可
#
# 出力: 経過時間 + marker counts + ACCEPT 予測 + 最終 3 行.

set -euo pipefail

TARGET="${1:-}"

# auto-detect 最新 smoke log if not specified
if [[ -z "$TARGET" ]]; then
    # 直近 24h で modify された lab-*-logs/ ディレクトリ内最新 .log を選択
    TARGET=$(find . -path "./target" -prune -o -name "*.log" -mmin -1440 -type f -print 2>/dev/null \
        | grep -E "lab-.*-logs" | head -1)
    if [[ -z "$TARGET" ]]; then
        echo "ERROR: auto-detect 失敗、引数で smoke log path 指定してください" >&2
        echo "       例: $0 lab-265-smoke-logs/g_mct2_smoke.log" >&2
        exit 1
    fi
    echo "auto-detected: $TARGET"
fi

# directory ならその中の最新 .log を採用
if [[ -d "$TARGET" ]]; then
    TARGET=$(find "$TARGET" -maxdepth 1 -name "*.log" -type f -print | sort | tail -1)
fi

if [[ ! -f "$TARGET" ]]; then
    echo "ERROR: $TARGET not found." >&2
    exit 1
fi

echo "=== Smoke Status: $TARGET ==="

# 経過時間 (smoke runner PID の推測、macOS pgrep ERE alternation 不対応のため
# pattern 毎に separate invocation)
for pattern in "g_mct2_smoke" "g_paired_" "lab_v22_aa_test"; do
    # set -e tolerance: pgrep returns 1 if no match, prevent early exit
    PIDS=$(pgrep -f "$pattern" 2>/dev/null || true)
    for pid in $PIDS; do
        ps -p "$pid" -o pid,etime,command 2>/dev/null || true
    done
done

echo ""
echo "Markers (smoke 進行の reliable signal):"
COMP_BUDGET=$(grep -c "compaction.budget" "$TARGET" || true)
PREV=$(grep -c "\[prev:" "$TARGET" || true)
SUMMARIZED=$(grep -c "\[summarized\]" "$TARGET" || true)
CTX_GUARD=$(grep -c "context_guard" "$TARGET" || true)
BASELINE=$(grep -c "ベースライン計測中" "$TARGET" || true)
TIMEOUT=$(grep -c "WARN.*タイムアウト" "$TARGET" || true)
COMPLETE_MARKER=$(grep -c "完了:" "$TARGET" || true)
RESULT_LINE=$(grep -E "^RESULT:" "$TARGET" || true)

printf "  %-25s %s\n" "compaction.budget emit:" "$COMP_BUDGET"
printf "  %-25s %s\n" "[prev: marker:" "$PREV"
printf "  %-25s %s\n" "[summarized] marker:" "$SUMMARIZED"
printf "  %-25s %s\n" "context_guard warn:" "$CTX_GUARD"
printf "  %-25s %s\n" "ベースライン start:" "$BASELINE"
printf "  %-25s %s\n" "タイムアウト warn:" "$TIMEOUT"
printf "  %-25s %s\n" "実験完了 marker:" "$COMPLETE_MARKER"

echo ""
if [[ -n "$RESULT_LINE" ]]; then
    echo "RESULT: $RESULT_LINE"
else
    echo "RESULT: (smoke 進行中、終了 marker 未検出)"
fi

echo ""
echo "ACCEPT 予測 (G-MCT2 条件 a):"
if [[ "$PREV" -ge 5 ]]; then
    echo "  [PASS] [prev: count = $PREV >= 5 -> ACCEPT 予測"
elif [[ "$PREV" -gt 0 ]]; then
    echo "  [WAIT] [prev: count = $PREV (まだ >= 5 未達、smoke 継続待ち)"
else
    echo "  [WAIT] [prev: count = 0 (まだ level1 threshold 未達、context 蓄積待ち)"
fi

echo ""
echo "Last 3 lines:"
tail -3 "$TARGET"
