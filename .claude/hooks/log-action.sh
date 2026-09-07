#!/bin/bash
# PostToolUse hook — 全ツール実行を1行ログに記録
# 入出力: JSON (stdin) → JSON (stdout)

INPUT=$(cat)
TOOL=$(echo "$INPUT" | jq -r '.tool_name // "unknown"' 2>/dev/null)
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // "no-session"' 2>/dev/null)

# プロジェクトルートを推定（hooks の親の親）
PROJECT_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
LOG_DIR="$PROJECT_ROOT/logs"
mkdir -p "$LOG_DIR"

TS=$(date '+%Y-%m-%dT%H:%M:%S%z')
LOG_FILE="$LOG_DIR/action-$(date '+%Y-%m-%d').log"

echo "$TS [$SESSION_ID] $TOOL" >> "$LOG_FILE"
echo '{}'
