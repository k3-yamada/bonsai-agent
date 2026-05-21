#!/usr/bin/env bash
# Z-3 Phase 5 drift hook integration test.
# Tests lab_v22_aa_test.sh / lab_v22_paired.sh の post-cycle drift_lint trigger 動作.
#
# Phase 1 Red: 3 test case。
#   - t_drift_hook_env_off_no_report  : BONSAI_DRIFT_LINT_LAB unset → report 未生成 (PASS)
#   - t_drift_hook_env_on_report_generated : BONSAI_DRIFT_LINT_LAB=1 → report 生成 (Phase 1 Red FAIL)
#   - t_drift_hook_lab_exit_code_preserved : trap が exit code を破壊しないこと (PASS)
#
# 設計上の注意:
#   - lab_v22_aa_test.sh は BONSAI_BIN 不在で exit 1 + stderr ERROR を出して即返る。
#     Lab cycle (~5h) は起動しない。
#   - BONSAI_BIN 不在起動なので stdout/stderr は捨てて良い。

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "$PROJECT_ROOT"

PASS=0
FAIL=0

assert() {
    local name="$1" expected="$2" actual="$3"
    if [[ "$expected" == "$actual" ]]; then
        echo "PASS: $name"
        PASS=$((PASS + 1))
    else
        echo "FAIL: $name (expected='$expected', got='$actual')"
        FAIL=$((FAIL + 1))
    fi
}

# Helper: drift report path for today.
drift_report_path() {
    local today
    today=$(date +%Y%m%d)
    echo "docs/quality/drift-${today}.md"
}

# Helper: run aa_test.sh with BONSAI_BIN forced to non-existent path so Lab cycle is never started.
# The script exits at the binary-existence check (exit 1), so wall time < 1s.
run_aa_stub() {
    BONSAI_BIN=/nonexistent/bonsai_test_stub \
        bash scripts/lab_v22_aa_test.sh /tmp/drift_hook_test_logs >/dev/null 2>&1 || true
}

# Test 1: env unset で drift report 不在 (stub は echo のみ、生成しない)
test_env_off_no_report() {
    local report
    report=$(drift_report_path)
    local backup="${report}.drift_hook_test_backup"

    # backup existing report if present
    [[ -f "$report" ]] && cp "$report" "$backup"
    rm -f "$report"

    unset BONSAI_DRIFT_LINT_LAB
    run_aa_stub

    local report_exists="no"
    [[ -f "$report" ]] && report_exists="yes"

    # restore
    rm -f "$report"
    [[ -f "$backup" ]] && mv "$backup" "$report"

    assert "t_drift_hook_env_off_no_report" "no" "$report_exists"
}

# Test 2: env on で drift report 生成 (Phase 1 Red = FAIL 期待、stub は生成しない)
test_env_on_report_generated() {
    local report
    report=$(drift_report_path)
    local backup="${report}.drift_hook_test_backup"

    [[ -f "$report" ]] && cp "$report" "$backup"
    rm -f "$report"

    export BONSAI_DRIFT_LINT_LAB=1
    run_aa_stub
    unset BONSAI_DRIFT_LINT_LAB

    local report_exists="no"
    [[ -f "$report" ]] && report_exists="yes"

    rm -f "$report"
    [[ -f "$backup" ]] && mv "$backup" "$report"

    # Phase 1 Red: stub は生成しないので report_exists="no"、expected="yes" → FAIL
    assert "t_drift_hook_env_on_report_generated" "yes" "$report_exists"
}

# Test 3: Lab cycle exit code 維持 (trap が exit code を破壊しないこと)
# BONSAI_BIN 不在なので exit code = 1 が期待値。
# on_lab_complete stub は `exit "$_ec"` で同じ code を再 exit するので 1 が返るはず。
test_lab_exit_code_preserved() {
    BONSAI_BIN=/nonexistent/bonsai_test_stub \
        bash scripts/lab_v22_aa_test.sh /tmp/drift_hook_test_logs >/dev/null 2>&1
    local exit_code=$?

    # trap が exit code を破壊しなければ 1 (binary not found) が返る
    assert "t_drift_hook_lab_exit_code_preserved" "1" "$exit_code"
}

# Test 4: Lab Cycle Linkage section が追記される
test_cycle_linkage_appended() {
    local today
    today=$(date +%Y%m%d)
    local report="docs/quality/drift-${today}.md"
    local backup="${report}.drift_hook_test_backup"

    [[ -f "$report" ]] && cp "$report" "$backup"
    rm -f "$report"

    export BONSAI_DRIFT_LINT_LAB=1
    BONSAI_BIN=/nonexistent/path bash scripts/lab_v22_aa_test.sh /tmp/drift_hook_test_logs >/dev/null 2>&1 || true
    unset BONSAI_DRIFT_LINT_LAB

    local has_linkage="no"
    [[ -f "$report" ]] && grep -q "## Lab Cycle Linkage" "$report" && has_linkage="yes"

    rm -f "$report"
    [[ -f "$backup" ]] && mv "$backup" "$report"

    assert "t_drift_hook_cycle_linkage_appended" "yes" "$has_linkage"
}

# Test 5: shared helper source pattern が機能する確証
test_helper_source_pattern() {
    # lab_hook.sh 存在 + on_lab_complete 定義含むこと
    local helper="scripts/drift/lab_hook.sh"
    local has_helper="no" has_func="no"
    [[ -f "$helper" ]] && has_helper="yes"
    [[ "$has_helper" == "yes" ]] && grep -q "^on_lab_complete()" "$helper" && has_func="yes"
    # caller (aa_test.sh) が source pattern 採用済
    local has_source="no"
    grep -q 'source.*drift/lab_hook.sh' scripts/lab_v22_aa_test.sh && has_source="yes"
    assert "t_drift_hook_helper_source_pattern" "yes:yes:yes" "${has_helper}:${has_func}:${has_source}"
}

echo "=== Z-3 Phase 5 drift_hook test (Phase 3 Refactor) ==="
test_env_off_no_report
test_env_on_report_generated
test_lab_exit_code_preserved
test_cycle_linkage_appended
test_helper_source_pattern
echo ""
echo "Pass: $PASS, Fail: $FAIL"
[[ $FAIL -eq 0 ]] && exit 0 || exit 1
