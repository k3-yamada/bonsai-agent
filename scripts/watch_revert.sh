#!/bin/bash
# ファイル変更監視スクリプト — 巻き戻し原因特定用
# 変更検知時にタイムスタンプ+lsof(書込プロセス特定)+git diffを記録

LOG="/tmp/bonsai-revert-watch.log"
WATCH_FILES=(
  "src/agent/parse.rs"
  "src/agent/mod.rs"
  "src/agent/tool_selection_bench.rs"
)

cd /Users/keizo/bonsai-agent || exit 1

echo "=== watch_revert.sh started at $(date) ===" >> "$LOG"
echo "Watching: ${WATCH_FILES[*]}" >> "$LOG"
echo "PID: $$" >> "$LOG"
echo "---" >> "$LOG"

fswatch -r --event=Updated --event=Renamed --event=Removed \
  "${WATCH_FILES[@]}" | while read -r changed_file; do
  TS=$(date '+%Y-%m-%d %H:%M:%S.%N' 2>/dev/null || date '+%Y-%m-%d %H:%M:%S')
  echo "" >> "$LOG"
  echo "[$TS] CHANGE DETECTED: $changed_file" >> "$LOG"

  # 変更したプロセスを特定（macOS lsof）
  echo "  lsof:" >> "$LOG"
  lsof "$changed_file" 2>/dev/null | tail -5 >> "$LOG" 2>&1

  # 直近のファイルアクセスプロセス（macOS fs_usage代替）
  echo "  last_modifier: $(stat -f '%Su %Sm' "$changed_file" 2>/dev/null)" >> "$LOG"

  # git diff（変更内容）
  echo "  git_diff:" >> "$LOG"
  git diff --stat -- "$changed_file" >> "$LOG" 2>&1

  # ファイルの先頭行（存在確認）
  echo "  head:" >> "$LOG"
  head -3 "$changed_file" >> "$LOG" 2>&1

  echo "  ---" >> "$LOG"
done
