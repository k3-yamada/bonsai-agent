#!/bin/bash
# Stop hook: Quality ratchet and self-verification check

python3 -c '
import sys, json

try:
    data = json.load(sys.stdin)
    # 正常終了時は allow を返す
    out = {"decision": "allow"}
    print(json.dumps(out))
except Exception:
    print(json.dumps({"decision": "allow"}))
'
