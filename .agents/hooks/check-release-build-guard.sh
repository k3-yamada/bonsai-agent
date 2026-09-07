#!/bin/bash
# PreToolUse hook for run_command: Guards against dangerous commands like `cargo build --release` during Lab runs

python3 -c '
import sys, json

try:
    data = json.load(sys.stdin)
    tool_call = data.get("toolCall", {})
    args = tool_call.get("args", {})
    cmd = args.get("CommandLine", "")

    # Lab 破壊リスクのある cargo build --release
    if "cargo build --release" in cmd or "cargo build -r" in cmd:
        # Lab や Smoke スクリプト実行時の一貫性破壊を警告
        out = {
            "decision": "ask",
            "reason": "【安全ガード】CLAUDE.md規律: Lab/Smoke稼働中の cargo build --release は target/release/bonsai を上書きし、実験の一貫性を破壊する危険があります。検証には cargo test --lib を使用してください。"
        }
    elif "git reset --hard" in cmd or "git checkout -- ." in cmd:
        out = {
            "decision": "ask",
            "reason": "【安全ガード】未コミットの変更を破壊的に巻き戻すコマンドが検出されました。Clippy警告を理由とする巻き戻しは禁止されています。"
        }
    else:
        out = {"decision": "allow"}

    print(json.dumps(out))
except Exception as e:
    # パース失敗時等はデフォルト allow
    print(json.dumps({"decision": "allow"}))
'
