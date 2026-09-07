#!/bin/bash
# PostToolUse hook for file editing tools: reminders and clean state

python3 -c '
import sys, json

try:
    _ = json.load(sys.stdin)
except Exception:
    pass

# PostToolUse expects an empty JSON object
print(json.dumps({}))
'
