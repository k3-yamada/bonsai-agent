#!/usr/bin/env bash
# Z-3 Phase 4: cargo llvm-cov coverage wrapper.
# Uses cargo llvm-cov if available, else graceful skip.
# Output: appends to docs/quality/drift-YYYYMMDD.md.
# Read-only: NO auto-modify scores.md (parsing 複雑、future iteration で対応).

set -uo pipefail

# critic HIGH #3 pattern: standalone 実行時の PWD 依存解消、PROJECT_ROOT cd.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "$PROJECT_ROOT"

DATE=$(date +%Y%m%d)
REPORT="docs/quality/drift-${DATE}.md"
SECTION="## Coverage (cargo llvm-cov)"

mkdir -p docs/quality

if [[ ! -f "$REPORT" ]]; then
    cat > "$REPORT" <<EOF
# bonsai-agent Drift Report (${DATE})

> Z-3 (Zenn Codex Harness Step 8) Lightweight Drift Linter 出力.
> Read-only 検出のみ、auto-fix なし. 確認後の修正は manual.

EOF
fi

{
    echo ""
    echo "${SECTION}"
    echo ""
    echo "Generated: $(date '+%Y-%m-%d %H:%M:%S %Z')"
    echo ""
} >> "$REPORT"

# cargo + cargo-llvm-cov 存在確認 (graceful skip).
if ! command -v cargo >/dev/null 2>&1; then
    echo "**SKIP**: cargo not installed in PATH." >> "$REPORT"
    echo "SKIP: cargo not in PATH"
    exit 0
fi

if ! cargo llvm-cov --version >/dev/null 2>&1; then
    {
        echo "**SKIP**: \`cargo llvm-cov\` not available."
        echo "  修正方法: \`cargo install cargo-llvm-cov --locked\` でインストール、本 script 再実行."
        echo "  参照: docs/architecture/module-layer-rules.md"
    } >> "$REPORT"
    echo "SKIP: cargo-llvm-cov not installed"
    exit 0
fi

# 時間制約あり: summary-only mode (full lib test 経由、~30-60s)
echo "Running cargo llvm-cov --workspace --summary-only --lib ... (~30-60s)"
COV_OUTPUT=$(cargo llvm-cov --workspace --summary-only --lib 2>&1 || true)

# 簡易結果分類: 'TOTAL' 行で overall coverage を抽出.
if echo "$COV_OUTPUT" | grep -q "TOTAL"; then
    {
        echo "✅ **Coverage summary generated.**"
        echo ""
        echo "\`\`\`"
        echo "$COV_OUTPUT" | tail -30
        echo "\`\`\`"
        echo ""
        echo "📌 **TODO**: docs/quality/scores.md への module 別 coverage upsert は future iteration で対応 (parsing 複雑、本 phase は raw output append のみ). 参照: docs/architecture/module-layer-rules.md"
    } >> "$REPORT"
    echo "PASS: coverage summary captured"
else
    {
        echo "INFO: cargo llvm-cov returned non-standard output (test failure or config error)."
        echo ""
        echo "\`\`\`"
        echo "$COV_OUTPUT" | tail -20
        echo "\`\`\`"
    } >> "$REPORT"
    echo "INFO: see $REPORT"
fi
