#!/usr/bin/env bash
# Phase 4 Smoke G-MCT2 — 項目 265 max_context_tokens reduction 効果検証
#
# plan: .claude/plan/max-context-tokens-reduction-force-prune.md §7
# 関連 commits: 945b18e (Phase 1 Red) / a8a0128 (Phase 2 Green) / a877878 (Phase 3 wiring)
#
# 目的: BONSAI_LAB_SMOKE=1 で自動 max_context=6000 (項目 265) 適用、
#   compact_level1/2 強制発火を実機確証。項目 263 ratio tune (BONSAI_DYNAMIC_BUDGET=1) +
#   項目 262 AUGMENT directive (BONSAI_T6_PROMPT_AUGMENT=1) と stack。
#
# ACCEPT 条件 (plan §4.1):
#   (a) `[prev:` marker count ≥ 5 (5 task × k=3 = 15 run の 80%+ 発火率)
#   (b) `compaction.budget` emit と prune marker の time window 整合
#   (c) cargo test --lib 1377 passed retention (退行ゼロ)
#
# 前提:
#   - MLX server 起動済 (`./scripts/start-mlx-server.sh`、port 8000)
#   - target/release/bonsai が a877878 以降 commit で build 済
#     (`cargo build --release` を本 script 前に実行)
#
# Wall: ~80 min (T=0 deterministic、SMOKE=1 で task=5 × k=3 = 15 run)
#
# Usage:
#   chmod +x scripts/g_mct2_smoke.sh
#   ./scripts/g_mct2_smoke.sh
#   # ~80 min 後
#   grep -c "\[prev:" lab-265-smoke-logs/g_mct2_smoke.log

set -euo pipefail

# Z-3 Phase 5: drift monitor post-cycle hook (項目 260)
# shellcheck source=scripts/drift/lab_hook.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/drift/lab_hook.sh"
trap on_lab_complete EXIT

LOG_DIR="${1:-./lab-265-smoke-logs}"
mkdir -p "$LOG_DIR"

BONSAI_BIN="${BONSAI_BIN:-./target/release/bonsai}"
if [[ ! -x "$BONSAI_BIN" ]]; then
    echo "ERROR: $BONSAI_BIN not found or not executable." >&2
    echo "       Run 'cargo build --release' first." >&2
    exit 1
fi

# 項目 249 F1/F2: MLX primary + long SSE timeout
export BONSAI_LAB_LONG_SSE=1
export BONSAI_LAB_MLX_ONLY=1

# 項目 252 F4: MLX pre-warm (cycle 初動の prompt cache 安定化)
export BONSAI_LAB_MLX_WARMUP=1

# 項目 247 Phase C: temperature=0 で sampling noise 排除
export BONSAI_LAB_TEMP=0

# 項目 249 F3: task pool 削減
export BONSAI_LAB_TASK_LIMIT=5

# 項目 265 主要: smoke mode = 自動 max_context=6000 (level1=4500/level2=5400/level3=6000)
export BONSAI_LAB_SMOKE=1

# 項目 262 stack: T6 system prompt augment (+14.4% strong ACCEPT 済み)
export BONSAI_T6_PROMPT_AUGMENT=1

# 項目 263 + 261 stack: Dynamic Budget + axis-priority prune (30/30/15/25 kg-heavy)
export BONSAI_DYNAMIC_BUDGET=1

LOGFILE="$LOG_DIR/g_mct2_smoke.log"
echo "=== [$(date '+%Y-%m-%d %H:%M:%S')] G-MCT2 START ===" | tee -a "$LOGFILE"
echo "binary: $BONSAI_BIN" | tee -a "$LOGFILE"
echo "binary mtime: $(stat -f '%Sm' "$BONSAI_BIN" 2>/dev/null || stat -c '%y' "$BONSAI_BIN")" | tee -a "$LOGFILE"
echo "env:" | tee -a "$LOGFILE"
env | grep -E '^BONSAI_(LAB|T6|DYNAMIC)' | sort | tee -a "$LOGFILE"
echo "---" | tee -a "$LOGFILE"

START=$(date +%s)
"$BONSAI_BIN" --lab --lab-experiments 0 >>"$LOGFILE" 2>&1
END=$(date +%s)
DUR=$((END - START))
echo "=== [$(date '+%Y-%m-%d %H:%M:%S')] G-MCT2 END (duration=${DUR}s) ===" | tee -a "$LOGFILE"

# ACCEPT 判定 emit
PREV_COUNT=$(grep -c "\[prev:" "$LOGFILE" || true)
BUDGET_EMIT=$(grep -c "compaction.budget" "$LOGFILE" || true)
SUMMARIZED=$(grep -c "\[summarized\]" "$LOGFILE" || true)

echo "" | tee -a "$LOGFILE"
echo "=== ACCEPT 判定 ===" | tee -a "$LOGFILE"
echo "(a) [prev: marker count = $PREV_COUNT (≥ 5 で ACCEPT)" | tee -a "$LOGFILE"
echo "    compaction.budget emit = $BUDGET_EMIT" | tee -a "$LOGFILE"
echo "    [summarized] marker = $SUMMARIZED" | tee -a "$LOGFILE"
echo "" | tee -a "$LOGFILE"
if [[ "$PREV_COUNT" -ge 5 ]]; then
    echo "RESULT: ACCEPT (prune 発火確証、項目 265 効果実証)" | tee -a "$LOGFILE"
else
    echo "RESULT: REJECT or INVESTIGATE (prune 不発火、要 root cause 解析)" | tee -a "$LOGFILE"
fi
echo "" | tee -a "$LOGFILE"
echo "Next:" | tee -a "$LOGFILE"
echo "  - ACCEPT 後: A/A test (Phase 1 lab-v22-paired-metric-mandatory plan、~5h)" | tee -a "$LOGFILE"
echo "  - 副次: G-MCT3 = BONSAI_DYNAMIC_BUDGET=1 vs unset paired で axis-priority prune 効果可視化" | tee -a "$LOGFILE"
