#!/usr/bin/env bash
# Z-3 Lightweight Drift Linter — orchestrator.
# Runs Phase 1 (dead code) + Phase 2 (docs↔code sync) sequentially.
# Output: docs/quality/drift-YYYYMMDD.md (single report file per day).
# Read-only: NO auto-fix.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "$PROJECT_ROOT"

DATE=$(date +%Y%m%d)
REPORT="docs/quality/drift-${DATE}.md"
COMMIT=$(git rev-parse HEAD 2>/dev/null || echo "unknown")
BRANCH=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "unknown")

mkdir -p docs/quality

# Header overwrite (single report per day、複数 run でも header 1 回).
cat > "$REPORT" <<EOF
# bonsai-agent Drift Report (${DATE})

> Z-3 (Zenn Codex Harness Step 8) Lightweight Drift Linter 出力.
> Read-only 検出のみ、auto-fix なし. 確認後の修正は manual.

- Commit: \`${COMMIT}\`
- Branch: \`${BRANCH}\`
- Generated: $(date '+%Y-%m-%d %H:%M:%S %Z')

---
EOF

# Track exit status.
overall_status=0

# Phase 1: dead code.
echo "=== Phase 1: dead code ==="
if bash "${SCRIPT_DIR}/dead_code.sh"; then
    echo "Phase 1: OK"
else
    overall_status=1
    echo "Phase 1: FAIL"
fi

# Phase 2: docs ↔ code sync.
echo "=== Phase 2: docs ↔ code sync ==="
if python3 "${SCRIPT_DIR}/docs_sync.py"; then
    echo "Phase 2: OK"
else
    overall_status=1
    echo "Phase 2: drift detected"
fi

# Phase 3: cargo outdated (root deps only).
echo "=== Phase 3: cargo outdated ==="
if bash "${SCRIPT_DIR}/outdated.sh"; then
    echo "Phase 3: OK"
else
    overall_status=1
    echo "Phase 3: FAIL"
fi

# Phase 4: cargo llvm-cov coverage (summary-only).
echo "=== Phase 4: cargo llvm-cov ==="
if bash "${SCRIPT_DIR}/coverage.sh"; then
    echo "Phase 4: OK"
else
    overall_status=1
    echo "Phase 4: FAIL"
fi

# Phase 5: curation tooling 自己テスト (claudemd_archive regex + drift recent_items check).
# regression test を drift cycle に組込み、手動実行依存を解消 (ADR-001 enforcement loop を閉じる).
echo "=== Phase 5: curation tooling self-test ==="
if [[ -f "${PROJECT_ROOT}/scripts/tests/test_claudemd_tooling.py" ]]; then
    if python3 -m unittest scripts.tests.test_claudemd_tooling >/dev/null 2>&1; then
        echo "Phase 5: OK"
        {
            echo ""
            echo "## Curation Tooling Self-Test"
            echo ""
            echo "✅ scripts/tests/test_claudemd_tooling.py 全 test PASS (claudemd_archive regex + drift recent_items check)."
        } >> "$REPORT"
    else
        overall_status=1
        echo "Phase 5: FAIL"
        {
            echo ""
            echo "## Curation Tooling Self-Test"
            echo ""
            echo "⚠️ **scripts/tests/test_claudemd_tooling.py FAIL**. 修正方法: \`python3 -m unittest scripts.tests.test_claudemd_tooling\` で詳細確認、claudemd_archive.py ITEM_START_RE / docs_sync.py check_recent_items_section_count の regression を疑う. 参照: docs/decisions/ADR-001-claudemd-size-governance.md"
        } >> "$REPORT"
    fi
else
    echo "Phase 5: SKIP (test file not found)"
fi

# Summary footer.
{
    echo ""
    echo "---"
    echo ""
    echo "## Summary"
    echo ""
    if [[ $overall_status -eq 0 ]]; then
        echo "✅ All drift checks passed."
    else
        echo "⚠️ Drift detected — see sections above for details. 修正方法: 各 section の panic message に従う. 参照: docs/architecture/module-layer-rules.md"
    fi
    echo ""
    echo "Report: \`${REPORT}\`"
} >> "$REPORT"

echo ""
echo "=== Drift Linter complete (status=${overall_status}) ==="
echo "Report: ${REPORT}"
exit $overall_status
