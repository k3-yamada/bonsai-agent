#!/bin/bash
# Stop hook — セッション終了マーカーをログに書き込む
# 入出力: JSON (stdin) → JSON (stdout)

INPUT=$(cat)
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // "no-session"' 2>/dev/null)

PROJECT_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
LOG_DIR="$PROJECT_ROOT/logs"
mkdir -p "$LOG_DIR"

TS=$(date '+%Y-%m-%dT%H:%M:%S%z')
echo "$TS [$SESSION_ID] SESSION_END" >> "$LOG_DIR/sessions.log"
echo '{}'
