#!/usr/bin/env bash
# Z-3 Phase 5: Drift Monitor Lab Completion Hook (shared helper)
#
# 役割:
#   Lab v22 wrapper script (lab_v22_aa_test.sh / lab_v22_paired.sh) の post-cycle で
#   drift_lint を自動 trigger し、drift report に Lab cycle linkage を追記する.
#
# 環境変数:
#   BONSAI_DRIFT_LINT_LAB=1   # opt-in (default off)、unset で完全 no-op
#
# Contract:
#   - advisory only: drift exit code を Lab cycle exit code に伝搬させない
#   - 元 Lab exit code を exit "$exit_code" で厳密維持 (AC-3/AC-4)
#   - env unset で 100% backward compat (drift_report 生成ゼロ)
#
# 将来拡張:
#   BONSAI_DRIFT_LINT_STRICT=1   # 検出時 exit 1 で Lab 失敗化 (本 phase scope 外)
#
# 使い方 (caller 側):
#   source "$(dirname "$0")/drift/lab_hook.sh"
#   # mkdir -p "$LOG_DIR" の直後、BONSAI_BIN check の前に trap 登録:
#   trap on_lab_complete EXIT
#
# 参照: docs/architecture/module-layer-rules.md
# 起点: .claude/plan/drift-monitor-lab-completion-hook.md §4 Phase 3 Refactor

on_lab_complete() {
    local exit_code=$?
    if [[ "${BONSAI_DRIFT_LINT_LAB:-0}" == "1" ]]; then
        # caller script の dir を絶対 path で取得.
        local caller_script="${BASH_SOURCE[1]:-${BASH_SOURCE[0]}}"
        local script_dir
        script_dir="$(cd "$(dirname "${caller_script}")" && pwd)"
        local last_cycle="${LOG_DIR:-unknown}/test_off_5.log"
        local commit_hash
        commit_hash=$(git rev-parse HEAD 2>/dev/null || echo unknown)
        echo "=== [drift_lint] Lab cycle complete, running drift_lint... ==="
        # advisory only: drift exit code を Lab exit code に伝搬させない
        bash "${script_dir}/drift/run_lint.sh" || true
        local report
        report="$(cd "${script_dir}/.." && pwd)/docs/quality/drift-$(date +%Y%m%d).md"
        if [[ -f "$report" ]]; then
            {
                echo ""
                echo "## Lab Cycle Linkage"
                echo "- Triggered by: $(basename "${caller_script}")"
                echo "- Last cycle log: \`${last_cycle}\`"
                echo "- Commit at trigger: \`${commit_hash}\`"
                echo "- Lab exit code (preserved): ${exit_code}"
            } >> "$report"
            echo "[drift_lint] report appended: ${report}"
        fi
    fi
    exit "$exit_code"  # AC-3/AC-4: 元の exit code を厳密に維持
}
