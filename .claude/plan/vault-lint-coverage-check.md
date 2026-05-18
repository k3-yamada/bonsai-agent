# Vault Lint Coverage Check — Knowledge Vault 整合性 lint pass (項目 245 候補)

**状態**: planning-only (2026-05-18 起票)
**推奨度**: ★★ (LLM Wiki Lint パターン Vault 軸拡張、項目 244 の自然な続編)
**推定工数**: ~3-4h plan + Phase 1-3 (TDD strict) + ~30 min Phase 4 smoke
**起点**:
- 項目 244 KG lint 完遂 (commit `2995085` までで Phase 1-4 配線 + G-8a v2 PASS)
- Karpathy LLM Wiki Lint パターン (Zenn 記事 tsurubee 著) の 4 軸 (Schema/Concept/Log/Lint) を
  bonsai-agent 三領域 (KG / Vault / Memory) へ展開する 2 段目
- `src/knowledge/vault.rs` は append-only md 蓄積で proactive 整合性 check なし

---

## §1. 問題定義

### 1.1 Vault の現状 (append-only / 6 category)
| category | path | dedup 方式 |
|----------|------|------------|
| decisions / facts / preferences / patterns / insights / todos | `<root>/<cat>.md` | content[0..50] prefix substring match (`vault.rs:46`) |

`Vault::append()` は:
- timestamp + content[0..200] を `\n- [...] ...\n` で追記
- content[0..50] が既存ファイル内に substring match なら skip (緩い dedup)
- `record_to_graph()` で KG に vault_category / vault_entry / contains / extracted_from を投入

### 1.2 Vault 未検出の 5 軸

| # | 問題軸 | 既実装 | 未実装 |
|---|--------|--------|--------|
| 1 | duplicate (near-dup) | 50 文字 prefix substring | 50 文字以降の variant が独立 entry 化 |
| 2 | stale entry | なし | 90 日以上更新なし entry の visibility ゼロ |
| 3 | orphan entry (source 欠落) | なし | `entry.source.is_empty()` で KG link 欠落 |
| 4 | cross-category leak | なし | 同一 content[0..50] が異なる cat に分散 |
| 5 | incomplete entry | なし | content 空 / whitespace-only / "TODO" のみ等 |

### 1.3 LLM Wiki Lint 適用 (KG lint との対応)
| KG lint (項目 244) | Vault lint (本 plan) | 動機 |
|--------------------|----------------------|------|
| conflicting triples | cross-category leak | 同一 fact が複数 category で矛盾扱い |
| orphan nodes | orphan entry (no source) | trace 不能な entry は KG 連携で価値半減 |
| uncovered seed triples | stale entry (>90d) | 使われない entry の visibility |
| case variant nodes | duplicate (near-dup) | 表記揺れの volume 検出 |

---

## §2. 設計 — 3 案比較 (推奨 = 案 B、項目 244 と同形)

| 案 | 内容 | 採否候補 |
|---|---|---|
| A | standalone `cargo test --include-ignored vault_lint` のみ、Lab 起動時実行なし | ★ 最小 scope |
| **B** | Lab smoke startup に `lint_vault_for_lab() -> VaultLintReport` 注入、warning のみで Lab 続行 + `BONSAI_VAULT_LINT_STRICT=1` で gate 化 | ★★★ 推奨 |
| C | Lab smoke startup に strict gate、FAIL で abort | ★ 開発時厳しすぎ |

### 2.1 案 B (推奨)

**新規追加**:

1. `src/knowledge/vault.rs` (or 新規 `src/knowledge/vault_lint.rs`) に:
   ```rust
   pub struct VaultLintReport {
       pub duplicate_entries: Vec<(String /* cat */, String /* content[0..50] */, usize /* count */)>,
       pub stale_entries: Vec<(String /* cat */, String /* timestamp */, String /* content */)>,
       pub orphan_entries: Vec<(String /* cat */, String /* content */)>,
       pub cross_category_leaks: Vec<(String /* content[0..50] */, Vec<String /* cat */>)>,
       pub incomplete_entries: Vec<(String /* cat */, String /* line */)>,
   }
   impl VaultLintReport {
       pub fn is_clean(&self) -> bool { ... }
       pub fn warn_log(&self);  // [INFO][lab.vault_lint] prefix
   }
   pub fn lint_vault_for_lab(vault: &Vault, stale_threshold_days: i64) -> VaultLintReport;
   ```

2. `src/agent/experiment.rs::run_factcheck_pass_lab` の lint 配線箇所 (項目 244 KG lint 隣接) に `lint_vault_for_lab` 呼出を追加:
   ```rust
   if std::env::var("BONSAI_VAULT_LINT_ENABLED").is_ok() {
       let report = lint_vault_for_lab(&vault, 90);
       report.warn_log();
       if is_vault_lint_strict() && !report.is_clean() {
           anyhow::bail!("Vault lint FAIL — see [INFO][lab.vault_lint] log");
       }
   }
   ```

3. env gate:
   - `BONSAI_VAULT_LINT_ENABLED=1` で lint 実行 (default OFF、KG lint と独立)
   - `BONSAI_VAULT_LINT_STRICT=1` で FAIL abort (項目 244 `BONSAI_KG_LINT_STRICT` と同パターン)
   - `BONSAI_VAULT_LINT_STALE_DAYS` で stale 閾値変更 (default 90)

**Pros**:
- KG lint と同形 (env name / log prefix / warn_log)、運用 protocol 統一
- vault file 数固定 (6 cat) で I/O cost <1 sec、Lab startup 阻害なし
- entry source field の活用 (record_to_graph と整合)

**Cons**:
- timestamp parsing (chrono format `%Y-%m-%d %H:%M`) で stale 判定、frontmatter のない md は parse error 経路必要
- cross-category leak 判定で全 cat ファイル read = 6 file open (small overhead)

### 2.2 案 A (棄却): standalone test
- KG lint と同 reasoning で棄却 (Lab 起動前 protocol 統合性失う)

### 2.3 案 C (棄却): strict gate default
- 既存 vault は legacy entry 多数で lint clean は非現実的、開発 friction 大

---

## §3. 実装 — TDD strict 5 phase

### Phase 1 (Red) — 5 failing test

1. `t_vault_lint_detects_duplicate_entries`: 同一 content を同 cat に 2 回 append (50 char prefix 同) → duplicate_entries.len() >= 1
2. `t_vault_lint_detects_stale_entries`: timestamp が 100 日前の entry を inject → stale_entries.len() >= 1
3. `t_vault_lint_detects_orphan_entries`: KG record_to_graph せず append のみ → orphan_entries で source 空判定
4. `t_vault_lint_detects_cross_category_leaks`: 同 content[0..50] を decisions / facts 両方に append → cross_category_leaks.len() == 1
5. `t_vault_lint_clean_on_empty_vault`: 新規 Vault で is_clean() == true

### Phase 2 (Green)

- `src/knowledge/vault_lint.rs` 新規 (~120 行): VaultLintReport / lint_vault_for_lab + helper
- `src/knowledge/mod.rs` に `pub mod vault_lint;` 追加
- 全 5 test PASS、1294 → 1299 passed (+5)

### Phase 3 (Refactor)

- `warn_log` を `[INFO][lab.vault_lint]` prefix で出力 (項目 244 `[INFO][lab.lint]` と整合)
- docstring に項目 245 起源 + LLM Wiki Lint パターン参照
- env getter `is_vault_lint_strict()` / `vault_lint_stale_days()` (SSOT 抽出)
- clippy/fmt clean

### Phase 4 (Smoke G-9a/b/c)

| Gate | env | 期待 |
|------|-----|------|
| G-9a | `BONSAI_VAULT_LINT_ENABLED=1` | Lab 起動時 `[INFO][lab.vault_lint]` 出力、is_clean=true で warn なし (legacy vault 想定では false 想定なので **production vault は別途 cleanup 必要**) |
| G-9b | legacy vault on disk + `BONSAI_VAULT_LINT_ENABLED=1` | warn_log に各軸 count 出力、Lab 続行 |
| G-9c | G-9b 同条件 + `BONSAI_VAULT_LINT_STRICT=1` | Lab abort (exit code 非ゼロ)、`Vault lint FAIL` |

### Phase 5 (本番運用、別 plan / 項目 246?)

- Vault cleanup script (stale entry archive、duplicate merge)
- 整合性確保後 `BONSAI_VAULT_LINT_STRICT=1` 常時 ON 化検討

---

## §4. risks / mitigations

| # | Risk | Mitigation |
|---|------|-----------|
| R1 | legacy vault に大量の lint finding が出て warning が noise 化 | Phase 4 G-9b で実 vault に対する初回 baseline を採取、件数を memory に記録 |
| R2 | timestamp parse 失敗で panic | `chrono::DateTime::parse_from_str` の Result を `unwrap_or` で skip + parse_failed カウンタ |
| R3 | KG link 経路 (record_to_graph) は append() と独立、orphan 判定が緩い | Phase 1 で `entry.source.is_empty()` を orphan criterion に固定 (KG state 非依存) |
| R4 | stale 閾値の 90 日が arbitrary | `BONSAI_VAULT_LINT_STALE_DAYS` env で調整可能、default = 90 = LLM Wiki 慣例値 |
| R5 | duplicate 判定で 50 char prefix の collision がノイズ化 | Phase 3 で hash-based dedup へ昇格検討 (Phase 2 は string prefix で MVP) |

---

## §5. 期待効果

### Vault 品質の継続的 visibility
6 cat × N entry の long-tail で stale / duplicate / orphan の trend を `audit_log` 記録、
長期的な vault health 観測可能。LLM Wiki パターン整合。

### 項目 244 で確立した env-gate pattern の再利用
`BONSAI_KG_LINT_*` と同形の `BONSAI_VAULT_LINT_*` env 3 種を導入、設計原則を 1 軸ずつ展開。
項目 246 (Memory lint) への拡張も同 pattern で進められる base 確立。

### Lab paired 起動前 cleanup ループ短縮
G-9b の warning 件数で vault cleanup の優先 cat 特定、cleanup 後 G-9c で gate 確証→
production deployment まで 1-2 cycle short-cut。

---

## §6. 起票候補項目

- **項目 245** = 本 plan の Phase 1-3 完遂 (lint_vault_for_lab + G-9a/b/c)
- 項目 246 (将来) = Memory lint (`memory/store.rs` A-MEM 矛盾 experience + stale skill 検出)
- 項目 247 (将来) = vault cleanup script (lint finding 自動修復)

---

## §7. 依存 / 並行性

### 完遂前提
- 項目 244 KG lint Phase 1-4 完遂 ✅ (commit `2995085` / `5108e44`)
- Smoke 15-task paired 5-cycle 完走 (~8h) — Vault lint も同 lint 軸で運用 protocol 統合性が必要

### 並行可
- Smoke paired 稼働中に Phase 1-3 (新 file `vault_lint.rs` 独立) は並行可
- experiment.rs 配線 (Phase 4) は smoke paired 完走後に start 推奨

### 排他
- vault.rs 同時編集回避 (production code 変更点が `pub mod vault_lint;` 追加のみで minimal)

---

## §8. ロールバック戦略

- `vault_lint.rs` は新規 file、削除で完全 rollback
- env-gated default OFF (`BONSAI_VAULT_LINT_ENABLED` 設定時のみ実行)
- 完全 rollback = `git revert <commit>` で 1-2 commit reversal

---

## §9. Quick Start

```bash
cd /Users/keizo/bonsai-agent
git log -3 --oneline

# Phase 1 Red
$EDITOR src/knowledge/vault_lint.rs  # VaultLintReport + lint_vault_for_lab() 仕様
$EDITOR src/knowledge/mod.rs         # pub mod vault_lint;
cargo test --lib --quiet vault_lint 2>&1 | tail -10  # 5 FAIL

# Phase 2 Green
cargo test --lib  # 1294 → 1299 passed (+5)
cargo clippy -- -D warnings
cargo fmt -- --check

# Phase 3 Refactor + commit
git add -A && git commit -m "feat(vault): 項目 245 Vault lint pass (LLM Wiki Lint 軸拡張)"

# Phase 4 Smoke G-9a/b/c
cargo build --release  # binary 更新
BONSAI_VAULT_LINT_ENABLED=1 ./target/release/bonsai --lab --lab-experiments 0 \
    2>&1 | grep -E "lab.vault_lint"  # G-9a
# (G-9b/c は legacy vault state に依存、別 sub-section で詳細記載)
```

---

## §10. metadata

- 起点 commit: `5108e44` (項目 244 final docs)
- 関連 plan: `kg-lint-coverage-check.md` (項目 244、parent pattern)
- 関連 memory: `MEMORY.md` の LLM Wiki Lint パターン (Karpathy 経由、Zenn 記事 tsurubee 著)
- 想定 commit 範囲: 3-4 commits (Phase 1 / Phase 2 / Phase 3 / Phase 4 配線)
- 想定 line 範囲: +200 行 / -10 行 (vault_lint.rs 新規 + mod.rs + experiment.rs 配線)
