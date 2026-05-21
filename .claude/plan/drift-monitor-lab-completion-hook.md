# Drift Monitor Lab Completion Hook — Z-3 Phase 5 Integration plan

**状態**: planning-only (2026-05-21 起票、項目 259 候補)
**推奨度**: ★★ (Z-3 自然 phase 5、Codex Harness Step 8 完結 + Lab cycle 終了時 operator visibility 強化)
**推定工数**: plan (~380 LOC) + Phase 1-3 (TDD strict, ~1.5h) + Phase 4 smoke (~30 min) = total ~2-2.5h
**起点**:
- 項目 257 (Z-3 Phase 1-4 完遂、`scripts/drift/{dead_code.sh, docs_sync.py, outdated.sh, coverage.sh, run_lint.sh}` + `docs/quality/drift-YYYYMMDD.md` 出力 production-ready)
- Codex Harness 記事 (Zenn dragon1208/66547a030c0236) Step 8 「GC drift monitor」の最終 piece = 「Lab cycle と連動した drift signal の operator 視野誘導」
- 項目 246 / 251 / 254 で確立した「Lab cycle 起動前 vault_lint pre-gate」pattern (`run_vault_sanity_gate` in `main.rs::handle_lab_mode`) の sibling pattern を post-cycle 軸で実装
- Lab v22 完走判定 (~5h) 後、operator が log を確認するタイミングで drift report を自動 surface → drift 蓄積仮説 (Gemini CCG synthesis 由来) の機械検証 channel 確立

---

## §1. Motivation

### 1.1 Why now

Zenn Codex Harness 記事 Step 8 verbatim:
> **GC drift monitor**: 週次 Codex タスクで dead code / docs↔code drift / outdated deps / quality scores を自動検出し、Lab cycle 終了時に operator 視野に乗せる。

bonsai 現状 (項目 257 完遂後):
- **Step 7 (observability)**: 95% カバー (audit_log + log_event + Lab tsv + checkpoint history)
- **Step 8 (GC drift monitor)**: 85% カバー (項目 257 で 4 phase 実装済、但し **Lab cycle hook integration は未実装**)

未実装 gap:
1. `scripts/drift/run_lint.sh` は manual run (`bash scripts/drift/run_lint.sh`) のみ
2. Lab v22 完走 (~5h) 後の operator は `tail /tmp/lab_v22_run.log` で結果確認するが drift signal は別 channel
3. 仮説「Lab REJECT root cause = stale context / drift 蓄積」(Gemini synthesis、項目 257) を機械検証する trigger 不在
4. Karpathy LLM Wiki Lint パターンの 3 軸 (Schema/Concept/Lint) のうち **Lint 軸 = bonsai は KG (244) / Vault (246/251/254) / drift (257) の 6 重発展済、残るは「自動 fire trigger」軸のみ**

### 1.2 既存資産マップ

| 軸 | 現状 (項目) | 評価 |
|---|---|---|
| pre-Lab vault_lint gate | `main.rs::handle_lab_mode` (項目 246/251) | ✅ pattern 確立 |
| post-Lab drift hook | (なし) | ❌ **Phase 5 gap** |
| drift script orchestrator | `scripts/drift/run_lint.sh` (項目 257) | ✅ 実装済 |
| Lab v22 entry points | `scripts/lab_v22_aa_test.sh` / `lab_v22_paired.sh` (項目 247) | ✅ 入口 2 件特定 |
| Lab cycle exit code 維持 contract | A/A test + paired smoke 両方で `BONSAI_BIN` exit code が wall clock 判定の唯一 signal | ⚠️ 干渉 NG |
| audit_log emit | `AuditAction::VaultLint` (項目 254) | ⚠️ DriftLint variant 未追加 |

### 1.3 設計原則

- **Read-only Drift Linter** (項目 257 Gemini CCG synthesis): auto-fix 一切なし、production code touch ゼロ
- **Lab cycle 失敗影響なし** (advisory only): drift 検出は Lab cycle の ACCEPT/REJECT 判定に影響させない (strict mode は将来の opt-in `BONSAI_DRIFT_LINT_STRICT=1`)
- **Backward compat 100%**: `BONSAI_DRIFT_LINT_LAB=1` env unset で既存 Lab cycle 挙動完全保持
- **既存 hook 配置 pattern 流用**: 項目 246/251 vault_lint pre-gate の sibling として post-cycle 配置

---

## §2. Acceptance criteria

| ID | Criterion | 検証方法 |
|---|---|---|
| AC-1 | `BONSAI_DRIFT_LINT_LAB` env unset 時、Lab cycle wall clock + 出力 100% backward compat (drift_report 生成ゼロ) | G-D5-2 smoke |
| AC-2 | `BONSAI_DRIFT_LINT_LAB=1` 時、Lab cycle 終了直後に `docs/quality/drift-YYYYMMDD.md` 自動生成 | G-D5-1 smoke |
| AC-3 | drift_report 生成失敗 (e.g. nightly rustup 未 install) → Lab cycle exit code 維持 (graceful skip) | G-D5-3 smoke |
| AC-4 | drift detected → Lab cycle exit code 維持 (advisory only、ACCEPT/REJECT 判定不変) | G-D5-3 smoke |
| AC-5 | drift_report header に commit hash + cycle ID (e.g. `test_on_3`) 紐付け追記 | report 内容確認 |
| AC-6 | wall clock 影響 ≤ 60s (4 phase total、Lab 5h 完走に対し negligible) | G-D5-1 measurement |
| AC-7 | strict mode は本 plan の対象外 (将来 opt-in、本 plan では advisory only 確定) | plan §3 案検討 |
| AC-8 | Phase 4 smoke G-D5-1..3 全 PASS | smoke 実機 |
| AC-9 | 既存 cargo test --lib 1348 passed 維持 (production code touch ゼロなので退行不可能) | `cargo test --lib` |
| AC-10 | 既存 `cargo test --test structural` 4 PASS 維持 (Z-4 layer linter 不変) | `cargo test --test structural` |

---

## §3. 3 案比較

### 案 A: bash wrapper trap (lab_v22_*.sh の trailing trap)

```bash
# scripts/lab_v22_aa_test.sh / lab_v22_paired.sh 末尾に追加
on_lab_complete() {
    if [[ "${BONSAI_DRIFT_LINT_LAB:-0}" == "1" ]]; then
        bash "$(dirname "$0")/drift/run_lint.sh" || true  # advisory only
    fi
}
trap on_lab_complete EXIT
```

shell-level hook、Lab cycle Rust binary には全く触れず。

### 案 B: Rust main.rs post-cycle (`handle_lab_mode` 末尾)

```rust
// main.rs::handle_lab_mode の `let experiments = run_experiment_loop(...)?;` 後
if is_drift_lint_lab_enabled() {
    if let Err(e) = run_drift_lint_post_cycle() {
        log_event(LogLevel::Warn, "lab.drift_lint", &format!("advisory failed: {e}"));
    }
}
```

項目 246/251 vault_lint pre-gate の symmetric pattern、Rust 側 env-gated。

### 案 C: External systemd / launchd timer

`~/Library/LaunchAgents/bonsai.drift.plist` で Lab cycle 終了 file mtime watch。completely decoupled、cross-platform 困難。

### 5 軸比較

| 軸 | 案 A (bash trap) | 案 B (Rust main.rs) | 案 C (systemd) |
|---|---|---|---|
| 実装工数 | ★★★ (~50 LOC) | ★★ (~80 LOC + 4 test) | ★ (~150 LOC + 2 platform) |
| Lab 干渉リスク | ★★★ (Rust binary 不変) | ★★ (?演算子 bail リスク) | ★★★ (完全独立) |
| Rollback 容易性 | ★★★ (5 行削除) | ★★ (commit revert + import) | ★ (plist + sudo unload) |
| Observability | ★★ (shell stdout) | ★★★ (log_event + audit_log) | ★ (systemd journal 分離) |
| Cross-platform 性 | ★★★ (POSIX 標準) | ★★★ (Rust 同梱) | ★ (macOS/Linux 別) |
| pattern symmetry | ★ (非対称) | ★★★ (pre/post 同形) | ★ (完全非対称) |
| **総合** | **17/21 ★** | **18/21 ★** | **11/21 ★** |

**推奨案 = 案 A (bash wrapper trap)**

推奨理由:
1. Lab 干渉リスク最小 (Rust binary 不変 = 退行不可能、項目 252 で確立した原則)
2. rollback 5 行 (案 B は ~80 LOC + import + 4 test)
3. Z-3 Read-only Drift Linter 原則踏襲 (項目 257 「production code touch ゼロ」)
4. operator が `tail /tmp/lab_v22_run.log` 確認時に drift_report path も同 log に echo される (認知負荷最小)
5. symmetry コスト (案 B 18 ★ で唯一勝るが 30 LOC + 4 test + import 増) は ROI 低
6. 将来 `BONSAI_DRIFT_LINT_STRICT=1` でも案 A は `exit 1` 1 行追加で対応可

### Backup 採択 (案 B)

`AuditAction::DriftLint` audit_log permanent log 要件化したタイミングで案 B 移行可。本 plan では案 A 確定。

---

## §4. TDD strict 3 phase outline (推奨案 A)

### Phase 1: Red (~30 min)

**変更 file**:
- `scripts/lab_v22_aa_test.sh`: 末尾に `on_lab_complete` 関数 stub + `trap on_lab_complete EXIT` 追加 (`echo "[drift hook] not implemented"` のみ)
- `scripts/lab_v22_paired.sh`: 同 stub 追加
- `tests/drift_hook.sh` (新規、~80 LOC): integration test 3 case

**Red criteria** (3 test FAIL 期待):
1. `t_drift_hook_env_off_no_report`: env unset + dummy lab → drift report 不在 (stub print のみ、PASS)
2. `t_drift_hook_env_on_report_generated`: env=1 + dummy lab → drift report 存在 (FAIL: stub 不生成)
3. `t_drift_hook_lab_exit_code_preserved`: lab exit 0 → trap 後 exit 0 (FAIL: trap 未実装)

### Phase 2: Green (~45 min)

**`scripts/lab_v22_aa_test.sh` 内 `on_lab_complete()` 本実装**:

```bash
on_lab_complete() {
    local exit_code=$?
    if [[ "${BONSAI_DRIFT_LINT_LAB:-0}" == "1" ]]; then
        local last_cycle="${LOG_DIR}/test_off_5.log"
        local commit_hash=$(git rev-parse HEAD 2>/dev/null || echo unknown)
        echo "=== [drift_lint] Lab cycle complete, running drift_lint... ==="
        bash "$(dirname "$0")/drift/run_lint.sh" || true
        local report="docs/quality/drift-$(date +%Y%m%d).md"
        if [[ -f "$report" ]]; then
            {
                echo ""
                echo "## Lab Cycle Linkage"
                echo "- Triggered by: $(basename "$0")"
                echo "- Last cycle log: ${last_cycle}"
                echo "- Commit at trigger: \`${commit_hash}\`"
                echo "- Lab exit code (preserved): ${exit_code}"
            } >> "$report"
            echo "[drift_lint] report appended: ${report}"
        fi
    fi
    exit "$exit_code"  # AC-3/AC-4: Lab exit code 厳密維持
}
trap on_lab_complete EXIT
```

`scripts/lab_v22_paired.sh`: 同実装 (last_cycle 変数のみ paired 用)。

**Green criteria**: Phase 1 の 3 test 全 PASS。

### Phase 3: Refactor (~15 min)

- `scripts/drift/lab_hook.sh` 新設 (~30 LOC): `on_lab_complete()` 共通実装抽出
- `lab_v22_aa_test.sh` / `lab_v22_paired.sh`: `source "$(dirname "$0")/drift/lab_hook.sh"` + `trap on_lab_complete EXIT` 1 行 (DRY、項目 251 helper extraction pattern 流用)
- `scripts/drift/lab_hook.sh` 冒頭に header (役割 / env / contract / 将来拡張)
- `tests/drift_hook.sh`: source pattern 後の挙動確証 1 test 追加

**Refactor criteria**: 4 test 全 PASS、`shellcheck scripts/drift/lab_hook.sh` clean。

---

## §5. Phase 4 Smoke acceptance (G-D5-1..3)

### G-D5-1: drift_report 自動生成確証

setup:
```bash
cargo build --release
rm -f docs/quality/drift-$(date +%Y%m%d).md
export BONSAI_DRIFT_LINT_LAB=1
export BONSAI_LAB_SMOKE=1
export BONSAI_LAB_TEMP=0
```
run: `bash scripts/lab_v22_aa_test.sh ./test-lab-logs-g-d5-1`

check:
- ✅ `docs/quality/drift-$(date +%Y%m%d).md` 生成
- ✅ report 末尾 `## Lab Cycle Linkage` section + commit hash + cycle log path
- ✅ Lab wall clock 増加 ≤ 60s
- ✅ Lab exit code 0

期待時間: ~5h (Lab v22 SMOKE) + ~30s drift overhead

### G-D5-2: env unset で skip 確証 (AC-1)

setup:
```bash
unset BONSAI_DRIFT_LINT_LAB
rm -f docs/quality/drift-$(date +%Y%m%d).md
export BONSAI_LAB_SMOKE=1
```
run: `bash scripts/lab_v22_aa_test.sh ./test-lab-logs-g-d5-2`

check:
- ✅ drift report 不在
- ✅ Lab wall clock = baseline
- ✅ Lab exit code 0
- ✅ log に `[drift_lint]` 不在

### G-D5-3: drift 失敗時 advisory 確証 (AC-3/AC-4)

setup:
```bash
export BONSAI_DRIFT_LINT_LAB=1
mv scripts/drift/dead_code.sh scripts/drift/dead_code.sh.bak
echo -e '#!/usr/bin/env bash\nexit 99' > scripts/drift/dead_code.sh
chmod +x scripts/drift/dead_code.sh
```
run: `bash scripts/lab_v22_aa_test.sh ./test-lab-logs-g-d5-3 || echo "ABORTED"`

check:
- ✅ Lab exit code 0 (drift exit 99 非伝搬)
- ✅ log 末尾 `[drift_lint] report appended` 存在
- ✅ drift report 内 dead_code section に fail マーカー

teardown:
```bash
mv scripts/drift/dead_code.sh.bak scripts/drift/dead_code.sh
```

---

## §6. Rollback strategy

完全 rollback (3 commit 順次 revert):
```bash
git revert <phase3_commit>
git revert <phase2_commit>
git revert <phase1_commit>
cargo test --lib  # 1348 passed 維持
cargo test --test structural  # 4 PASS 維持
```

段階 rollback:

| 状況 | 対応 |
|---|---|
| G-D5-1 fail | Phase 2 のみ revert、Phase 1 stub 維持 (no-op、害なし) |
| Phase 3 shellcheck fail | Phase 3 のみ revert |
| 案 B 移行 (audit_log 要件化) | 全 revert、§3 案 B path で再起票 |
| 想定外 on 化 → 干渉観測 | env unset で stop、production CLI 影響ゼロ |

---

## §7. bonsai 既存資産との整合性

### 項目 246/251 vault_lint pre-gate との対称性

- pre-gate: `main.rs::handle_lab_mode` で `run_vault_sanity_gate(...)` 実行
- post-hook: `scripts/drift/lab_hook.sh` (案 A)
- 非対称: pre-gate は Rust + strict bail 可、post-hook は shell + advisory only
- 設計判断: pre-gate は Lab 起動前 cycle 浪費回避で bail 必要、post-hook は Lab 完走後の operator visibility で advisory 十分

### 項目 247 Lab v22 metric redesign との独立性

- Lab v22 metric は Lab cycle ログを post-processing
- 本 hook は drift signal を別 channel に出力
- 干渉なし

### 項目 257 Z-3 Phase 1-4 との連続性

- 項目 257 = manual `bash scripts/drift/run_lint.sh`
- 本 phase 5 = 自動 trigger、出力 path/format 完全継承
- `run_lint.sh` 内部 0 行変更 (caller 側で wrap)

### 項目 254 Vault 5 軸目との pattern flow

共通 pattern: 「既存資産への 1 軸 incremental 拡張、TDD strict 3 phase、production code touch 最小化」

---

## §8. Open questions / 将来 phase

### 8.1 案 B (Rust main.rs) への将来移行 trigger

要件化したら案 B 検討:
1. `AuditAction::DriftLint` で audit_log permanent log
2. drift 検出 → 自動 issue 起票 / Slack webhook
3. paired smoke 内の per-cycle drift snapshot

### 8.2 `BONSAI_DRIFT_LINT_STRICT=1` 将来追加

案 A で 1 行追加可:
```bash
if [[ "${BONSAI_DRIFT_LINT_STRICT:-0}" == "1" ]] && grep -q "⚠️ Drift detected" "$report"; then
    exit 1
fi
```

AC-7 で本 plan 対象外明示、別 phase で起票推奨。

### 8.3 cycle ID 自動推定の精度

`last_cycle="${LOG_DIR}/test_off_5.log"` ハードコード。将来 cycle 数変動なら `ls -t "${LOG_DIR}"/*.log | head -1` で最新自動推定に置換可。本 plan では aa_test/paired 共に 5-cycle 固定で hardcode 十分。

### 8.4 drift_report rotation

日次重複なし (項目 257) だが Lab 5h × 2 run / day では同一 report に 2 回 append。cycle linkage section 複数追記可は operator visibility 優先で是。

---

## §9. Implementation checklist

- [ ] Phase 1 Red: stub 追加、3 test FAIL 確証
- [ ] Phase 2 Green: `on_lab_complete()` 本実装、3 test PASS
- [ ] Phase 3 Refactor: `scripts/drift/lab_hook.sh` helper extract、4 test PASS + shellcheck clean
- [ ] Phase 4 Smoke G-D5-1: drift_report 自動生成 + cycle linkage section
- [ ] Phase 4 Smoke G-D5-2: env unset 完全 no-op
- [ ] Phase 4 Smoke G-D5-3: drift internal failure → Lab exit code 維持
- [ ] cargo test --lib 1348 passed 維持
- [ ] cargo test --test structural 4 PASS 維持
- [ ] CLAUDE.md「直近 5 項目」に 259 entry 追加
- [ ] memory/harness_patterns_archive.md に項目 259 verbatim 追記
- [ ] git commits: Phase 1 Red / Phase 2 Green / Phase 3 Refactor / Phase 4 Smoke evidence (4 commits 目安)

---

**plan path**: `/Users/keizo/bonsai-agent/.claude/plan/drift-monitor-lab-completion-hook.md`
**総行数**: 約 380 行
