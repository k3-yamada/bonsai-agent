# Vault Lint bail 分岐 test 追加 (項目 246 follow-up、critic F1)

**状態**: planning-only (2026-05-19 起票、項目 246 Phase 4 wiring 後の adversary review で起票)
**推奨度**: ★★ (Lab v22 paired 起動 blocking なし、ただし highest-stakes branch の test gap が silent regression risk)
**推定工数**: ~30 min (TDD strict 3 commit)
**起点**:
- 項目 246 Phase 4 wiring 完遂 (commits `ac25ac2` Red + `30a38d6` Green+Refactor + critic 対応 commit)
- `oh-my-claudecode:critic` adversary review で **FLAG F1**: strict + not_clean → `anyhow::bail!` 分岐 (`main.rs:621-633`) に test coverage ゼロ。silent regression で `bail!` を `Ok(())` にすり替えると CI passes、Lab paired runs に dirty Vault が混入する。

---

## §1. 問題定義

### 1.1 現状の test coverage
- `vault_lint.rs` の 8 tests = 4 検出軸 + 3 env getter + 1 clean state
- `audit.rs` の `test_audit_action_vault_lint_round_trip` = AuditAction enum シリアライズ
- `main.rs::handle_lab_mode` の pre-lab vault gate 自体は **test 不在**

### 1.2 critic finding F1 (verbatim)
> **F1**: `strict + bail` branch has zero test coverage
> - Highest-stakes branch in the wiring (aborts Lab paired runs).
> - Silent regression would convert `bail!` to `Ok(())`, allowing dirty Vault Lab runs.

### 1.3 risk
- 次の refactor で `if is_vault_lint_strict() && !report.is_clean()` の条件を逆転 / branch を消す変更が入ったとき CI が catch しない
- Lab v22 paired 起動の sanity gate という設計意図が code レベルで encode されていない

---

## §2. 設計

### 2.1 解決方針 — helper extraction + unit test
`main.rs::handle_lab_mode` の pre-lab vault gate ブロックを `vault_lint.rs` 内の helper 関数に抽出:

```rust
/// Lab cycle 起動前の Vault sanity gate (項目 246 Phase 4、main.rs から抽出).
///
/// 副作用: warn_log + audit_log emit. strict=true + not_clean で Err 返却.
///
/// Returns:
/// - Ok(VaultLintReport) — gate 通過 (clean、または warn-only mode で not_clean)
/// - Err(anyhow::Error) — strict gate FAIL (caller で bail へ転送)
pub fn run_vault_sanity_gate(
    vault_root: &Path,
    stale_days: i64,
    strict: bool,
    audit: Option<&AuditLog>,  // session_id 込みで caller が制御、None ならスキップ
) -> Result<VaultLintReport> { ... }
```

### 2.2 main.rs wiring 書換
```rust
if bonsai_agent::knowledge::vault_lint::is_vault_lint_lab_enabled() {
    let vault_root = dirs::data_dir().unwrap_or_else(...).join("bonsai-agent").join("vault");
    let audit = bonsai_agent::observability::audit::AuditLog::new(store.conn());
    let _report = bonsai_agent::knowledge::vault_lint::run_vault_sanity_gate(
        &vault_root,
        bonsai_agent::knowledge::vault_lint::vault_lint_stale_days(),
        bonsai_agent::knowledge::vault_lint::is_vault_lint_strict(),
        Some(&audit),
    )?;  // strict + not_clean なら自動 bail
}
```

### 2.3 TDD strict 3 phase

**Phase 1 (Red)** — 4 failing tests:
1. `t_run_vault_sanity_gate_clean_returns_ok`: tempdir 空 Vault + strict=true → Ok
2. `t_run_vault_sanity_gate_dirty_warn_only_returns_ok`: dirty Vault + strict=false → Ok + warn_log emit
3. `t_run_vault_sanity_gate_dirty_strict_returns_err`: dirty Vault + strict=true → **Err (核心 bail テスト)**
4. `t_run_vault_sanity_gate_emits_audit_log`: audit Some(&log) で audit_log row 1 件 INSERT 確認

**Phase 2 (Green)** — `run_vault_sanity_gate` 関数を vault_lint.rs に追加 + main.rs を新 helper 呼出に書換

**Phase 3 (Refactor)** — main.rs の旧 inline ブロック削除 (~30 行 reduction)、clippy/fmt clean

### 2.4 想定 line 範囲
- vault_lint.rs: +60 行 (helper fn + 4 tests)
- main.rs: -25 行 (inline ブロック → 1 行 helper 呼出)
- net: +35 行、test 1319 → 1323 (+4)

---

## §3. risks / mitigations

| # | Risk | Mitigation |
|---|------|-----------|
| R1 | helper の signature が複雑化 (audit Option、strict bool、stale_days i64) | wrapper `run_vault_sanity_gate_from_env(vault_root, store)` を別途用意し、main.rs はそちらを呼ぶ (test では細粒度版を直接 test) |
| R2 | dirty Vault tempdir 作成が複雑 (md ファイル 6 個用意 + timestamp 操作) | 既存 `vault_lint.rs` test suite の dirty Vault fixture (例: `t_vault_lint_detects_duplicate_entries`) を流用 |
| R3 | strict + not_clean Err 時の audit_log emit 順序 (現状: emit → bail) | helper も同順序を守る (test で「Err 後も audit_log row 1 件存在」を assert) |

---

## §4. Quick Start

```bash
cd /Users/keizo/bonsai-agent
git log -3 --oneline

# Phase 1 Red
$EDITOR src/knowledge/vault_lint.rs   # 4 failing test を mod tests 内に追加
cargo test --lib vault_lint           # 4 fail expected (helper fn 未実装)
git add -A && git commit -m "test(vault-lint): 項目 246 follow-up Phase 1 Red — bail branch coverage"

# Phase 2 Green
$EDITOR src/knowledge/vault_lint.rs   # pub fn run_vault_sanity_gate
$EDITOR src/main.rs                   # inline ブロック削除 + helper 呼出
cargo test --lib                      # 1319 → 1323 PASS
git commit -m "feat(vault-lint): 項目 246 follow-up Phase 2 Green — helper extraction + bail test"

# Phase 3 Refactor
cargo clippy --all-targets -- -D warnings
cargo fmt
git commit -m "refactor(vault-lint): 項目 246 follow-up Phase 3 — clippy/fmt polish"
```

---

## §5. 並行性 / 依存

### 完遂前提
- 項目 246 Phase 4 wiring 既に master HEAD に統合済 (`30a38d6` + critic F2/M1 fix commit)

### 並行可
- 項目 248 Phase 4 wiring (Dynamic Budget Compaction、別ファイル)
- 項目 249 残課題 (`BONSAI_LAB_TASK_LIMIT` 実機検証)

### 排他
- 本 follow-up Phase 2 で `main.rs::handle_lab_mode` を書換 → 並行で main.rs を触る他 plan (項目 247 Phase D 等) と conflict 可能性

---

## §6. metadata
- 起点 review artifact: `oh-my-claudecode:critic` agent output (本 session 14:30 頃完了)
- 起点 commits: `ac25ac2`, `30a38d6`, (critic F2/M1 fix commit)
- 関連 plan: `vault-lint-coverage-check.md` (項目 246 本体)、`lab-runtime-stabilization.md` (項目 249、main.rs 隣接)
- 想定 commit 範囲: 3 commit (Red + Green + Refactor、TDD strict 既存 pattern)
- **本 plan の項目化**: 項目 251 候補 (Phase 1-3 完遂時)
