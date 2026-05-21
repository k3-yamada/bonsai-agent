#!/usr/bin/env bash
# Z-3 Phase 3: cargo outdated wrapper.
# Uses cargo outdated if available, else graceful skip.
# Output: appends to docs/quality/drift-YYYYMMDD.md.
# Read-only: NO auto-update (Lightweight Drift Linter 原則).

set -uo pipefail

DATE=$(date +%Y%m%d)
REPORT="docs/quality/drift-${DATE}.md"
SECTION="## Outdated Dependencies (cargo outdated)"

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

# cargo outdated subcommand 存在確認 (graceful skip).
if ! command -v cargo >/dev/null 2>&1; then
    echo "**SKIP**: cargo not installed in PATH." >> "$REPORT"
    echo "SKIP: cargo not in PATH"
    exit 0
fi

if ! cargo outdated --version >/dev/null 2>&1; then
    {
        echo "**SKIP**: \`cargo outdated\` not available."
        echo "  修正方法: \`cargo install cargo-outdated --locked\` でインストール、本 script 再実行."
        echo "  参照: docs/architecture/module-layer-rules.md"
    } >> "$REPORT"
    echo "SKIP: cargo-outdated not installed"
    exit 0
fi

# Actual run (root deps only、transitive deps は GitHub Dependabot 等別 tool が担当).
echo "Running cargo outdated --workspace --root-deps-only ..."
OUTDATED_OUTPUT=$(cargo outdated --workspace --root-deps-only 2>&1 || true)

# Detect "All dependencies are up to date" message.
if echo "$OUTDATED_OUTPUT" | grep -qi "all dependencies are up to date"; then
    {
        echo "✅ **All root dependencies up to date.**"
        echo ""
        echo "\`\`\`"
        echo "$OUTDATED_OUTPUT" | tail -5
        echo "\`\`\`"
    } >> "$REPORT"
    echo "PASS: 0 outdated root deps"
elif echo "$OUTDATED_OUTPUT" | grep -qiE "^Name\s|^---"; then
    # Table output detected: major / minor / patch bump available.
    {
        echo "⚠️ **Outdated dependencies detected.** 修正方法: Cargo.toml の version pin 更新 + cargo build/test 全 PASS 確証、必要なら sub-plan 起票. 参照: docs/architecture/module-layer-rules.md"
        echo ""
        echo "\`\`\`"
        echo "$OUTDATED_OUTPUT" | tail -30
        echo "\`\`\`"
    } >> "$REPORT"
    echo "DETECTED: outdated deps (see $REPORT)"
else
    # Other output (config error / network error etc) — log as INFO.
    {
        echo "INFO: cargo outdated returned non-standard output."
        echo ""
        echo "\`\`\`"
        echo "$OUTDATED_OUTPUT" | tail -20
        echo "\`\`\`"
    } >> "$REPORT"
    echo "INFO: see $REPORT"
fi
