#!/usr/bin/env bash
# Z-3 Phase 1: dead code drift detection.
# Uses cargo +nightly udeps if available, else graceful skip.
# Output: appends to docs/quality/drift-YYYYMMDD.md.
# Read-only: NO auto-fix (Gemini CCG synthesis 推奨 = Read-Only Drift Linter).

set -uo pipefail

# critic HIGH #3 fix: standalone 実行時の PWD 依存解消、必ず PROJECT_ROOT に cd.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "$PROJECT_ROOT"

DATE=$(date +%Y%m%d)
REPORT="docs/quality/drift-${DATE}.md"
SECTION="## Dead Code (cargo +nightly udeps)"

mkdir -p docs/quality

# Initialize report if missing.
if [[ ! -f "$REPORT" ]]; then
    cat > "$REPORT" <<EOF
# bonsai-agent Drift Report (${DATE})

> Z-3 (Zenn Codex Harness Step 8) Lightweight Drift Linter 出力.
> Read-only 検出のみ、auto-fix なし. 確認後の修正は manual.

EOF
fi

# Section header.
{
    echo ""
    echo "${SECTION}"
    echo ""
    echo "Generated: $(date '+%Y-%m-%d %H:%M:%S %Z')"
    echo ""
} >> "$REPORT"

# cargo +nightly udeps run (graceful skip if nightly not installed).
if ! command -v cargo >/dev/null 2>&1; then
    echo "**SKIP**: cargo not installed in PATH." >> "$REPORT"
    echo "SKIP: cargo not in PATH"
    exit 0
fi

if ! rustup toolchain list 2>/dev/null | grep -q nightly; then
    echo "**SKIP**: Rust nightly toolchain not installed." >> "$REPORT"
    echo "  修正方法: \`rustup toolchain install nightly\` で nightly 追加後、本 script 再実行." >> "$REPORT"
    echo "SKIP: nightly toolchain not installed"
    exit 0
fi

# udeps subcommand 存在確認 (graceful skip).
if ! cargo +nightly udeps --version >/dev/null 2>&1; then
    echo "**SKIP**: \`cargo +nightly udeps\` not available." >> "$REPORT"
    echo "  修正方法: \`cargo +nightly install cargo-udeps --locked\` でインストール、本 script 再実行." >> "$REPORT"
    echo "SKIP: cargo-udeps not installed"
    exit 0
fi

# Actual run.
echo "Running cargo +nightly udeps --workspace ..."
UDEPS_OUTPUT=$(cargo +nightly udeps --workspace 2>&1 || true)

# Detect "All deps seem to be used" message.
if echo "$UDEPS_OUTPUT" | grep -q "All deps seem to be used"; then
    {
        echo "✅ **No unused dependencies detected.**"
        echo ""
        echo "\`\`\`"
        echo "$UDEPS_OUTPUT" | tail -5
        echo "\`\`\`"
    } >> "$REPORT"
    echo "PASS: 0 unused deps"
else
    {
        echo "⚠️ **Unused dependencies or warnings detected.** 修正方法: 参照 docs/architecture/module-layer-rules.md + Cargo.toml の該当 dep 削除検討."
        echo ""
        echo "\`\`\`"
        echo "$UDEPS_OUTPUT" | tail -50
        echo "\`\`\`"
    } >> "$REPORT"
    echo "DETECTED: unused deps or warnings (see $REPORT)"
fi
