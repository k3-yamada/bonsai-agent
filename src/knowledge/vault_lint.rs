//! Vault lint pass — Knowledge Vault 整合性チェック (項目 246、plan vault-lint-coverage-check.md).
//!
//! Karpathy LLM Wiki Lint パターン (Zenn 記事 tsurubee 著) を bonsai-agent の Vault
//! (`crate::knowledge::vault::Vault`) に拡張。項目 244 KG lint と同形の env-gated 補助 lint。
//!
//! 検出軸 (plan §3.1):
//! - duplicate_entries: 同 cat 内で content[0..50] prefix が >1 回出現
//! - stale_entries: timestamp が `stale_threshold_days` 日前より古い
//! - cross_category_leaks: 同 content[0..50] が異なる cat に分散
//! - incomplete_entries: content が空 / whitespace のみ / "TODO" のみ等
//! - orphan_entries: KG link 欠落 (Phase 1+2 では未実装、Phase 3+ で wiring)
//!
//! TDD strict Phase 1 Red: skeleton 実装で `VaultLintReport` 全フィールドが空を返す。
//! 5 unit test のうち 4 件が FAIL する想定 (clean=true は sanity gate として PASS)。

use crate::knowledge::vault::Vault;

/// Vault lint 結果 (plan §3.1 (d) factcheck sanity gate の Vault 軸).
///
/// 各フィールドは検出された違反 entry のリスト。空 = clean。
#[derive(Debug, Clone, Default)]
pub struct VaultLintReport {
    /// (category, content[0..50], count) で同 cat 内重複
    pub duplicate_entries: Vec<(String, String, usize)>,
    /// (category, timestamp_str, content_excerpt) で stale 検出
    pub stale_entries: Vec<(String, String, String)>,
    /// (content[0..50], categories) で cross-cat leak
    pub cross_category_leaks: Vec<(String, Vec<String>)>,
    /// (category, line_content) で空/不完全 entry
    pub incomplete_entries: Vec<(String, String)>,
    /// (category, content_excerpt) で KG link 欠落 (Phase 1+2 未実装)
    pub orphan_entries: Vec<(String, String)>,
}

impl VaultLintReport {
    /// 全フィールドが空なら true (lint clean、issue ゼロ).
    pub fn is_clean(&self) -> bool {
        self.duplicate_entries.is_empty()
            && self.stale_entries.is_empty()
            && self.cross_category_leaks.is_empty()
            && self.incomplete_entries.is_empty()
            && self.orphan_entries.is_empty()
    }

    /// warning log を `[INFO][lab.vault_lint]` prefix で出力 (Phase 3 で本実装、Phase 1 stub).
    pub fn warn_log(&self) {
        // Phase 1+2 stub. Phase 3 Refactor で実装予定。
    }
}

/// Lab 起動時の Vault sanity lint pass.
///
/// `stale_threshold_days`: 90 (default、`BONSAI_VAULT_LINT_STALE_DAYS` env で override 可)。
///
/// Phase 1 Red 実装: 全フィールド空を返す skeleton。Phase 2 Green で 4 軸 (duplicate /
/// stale / cross-cat / incomplete) を実装、orphan は Phase 3+ で KG wiring 経由実装。
pub fn lint_vault_for_lab(vault: &Vault, stale_threshold_days: i64) -> VaultLintReport {
    // Phase 1 Red: skeleton — 全フィールド空、is_clean() == true
    let _ = (vault, stale_threshold_days);
    VaultLintReport::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::extractor::{StockCategory, StockEntry};
    use tempfile::tempdir;

    fn make_entry(category: StockCategory, content: &str) -> StockEntry {
        StockEntry {
            category,
            content: content.to_string(),
            source: "test_source".to_string(),
        }
    }

    /// Phase 1 Red 期待: skeleton で空 → 検出 0 → FAIL (expected len>=1)
    #[test]
    fn t_vault_lint_detects_duplicate_entries() {
        let dir = tempdir().expect("tempdir");
        let vault = Vault::new(dir.path()).expect("vault new");
        let e1 = make_entry(StockCategory::Fact, "the same long content of fifty characters length here xx");
        vault.append(&e1).expect("append 1");
        // 重複 append (Vault 側 50 char prefix dedup で skip される可能性あるが、
        // lint は md ファイル ベースで判定するため、回避するために少し変えて append)
        let e2 = make_entry(StockCategory::Fact, "the same long content of fifty characters length here yy");
        vault.append(&e2).expect("append 2");
        let report = lint_vault_for_lab(&vault, 90);
        assert!(
            !report.duplicate_entries.is_empty(),
            "重複 entry を検出 (Phase 1 Red 期待 FAIL、Phase 2 Green で PASS)"
        );
    }

    /// Phase 1 Red 期待: skeleton で空 → 検出 0 → FAIL
    #[test]
    fn t_vault_lint_detects_stale_entries() {
        let dir = tempdir().expect("tempdir");
        let vault = Vault::new(dir.path()).expect("vault new");
        // 直接 md ファイルに 100 日前の timestamp で書込 (Vault::append 経由だと現在時刻になる)
        let stale_ts = chrono::Utc::now() - chrono::Duration::days(100);
        let stale_line = format!(
            "\n- [{}] some stale content from long ago\n",
            stale_ts.format("%Y-%m-%d %H:%M")
        );
        let facts_path = dir.path().join("facts.md");
        let existing = std::fs::read_to_string(&facts_path).unwrap_or_default();
        std::fs::write(&facts_path, existing + &stale_line).expect("write");
        let report = lint_vault_for_lab(&vault, 90);
        assert!(
            !report.stale_entries.is_empty(),
            "90 日以上前の entry を検出 (Phase 1 Red FAIL 期待)"
        );
    }

    /// Phase 1 Red 期待: skeleton で空 → 検出 0 → FAIL
    #[test]
    fn t_vault_lint_detects_cross_category_leaks() {
        let dir = tempdir().expect("tempdir");
        let vault = Vault::new(dir.path()).expect("vault new");
        // 同一 50 char prefix content を decisions と facts の 2 cat に append
        let common = "shared content that lands in two different categories for cross-cat leak test";
        let e1 = make_entry(StockCategory::Decision, common);
        let e2 = make_entry(StockCategory::Fact, common);
        vault.append(&e1).expect("append d");
        vault.append(&e2).expect("append f");
        let report = lint_vault_for_lab(&vault, 90);
        assert!(
            !report.cross_category_leaks.is_empty(),
            "cross-cat leak を検出 (Phase 1 Red FAIL 期待)"
        );
    }

    /// Phase 1 Red 期待: skeleton で空 → 検出 0 → FAIL
    #[test]
    fn t_vault_lint_detects_incomplete_entries() {
        let dir = tempdir().expect("tempdir");
        let _vault = Vault::new(dir.path()).expect("vault new");
        // Vault::append は content 短すぎる/空でも書き込むので、直接 md に "- [ts]  \n" を書込
        let facts_path = dir.path().join("facts.md");
        let existing = std::fs::read_to_string(&facts_path).unwrap_or_default();
        let incomplete_line = format!(
            "\n- [{}] \n- [{}] TODO\n",
            chrono::Utc::now().format("%Y-%m-%d %H:%M"),
            chrono::Utc::now().format("%Y-%m-%d %H:%M")
        );
        std::fs::write(&facts_path, existing + &incomplete_line).expect("write");
        let vault = Vault::new(dir.path()).expect("vault reopen");
        let report = lint_vault_for_lab(&vault, 90);
        assert!(
            !report.incomplete_entries.is_empty(),
            "空 / TODO のみの entry を検出 (Phase 1 Red FAIL 期待)"
        );
    }

    /// Phase 1 Red 期待: skeleton で空 → is_clean=true → PASS (sanity gate)
    #[test]
    fn t_vault_lint_clean_on_empty_vault() {
        let dir = tempdir().expect("tempdir");
        let vault = Vault::new(dir.path()).expect("vault new");
        let report = lint_vault_for_lab(&vault, 90);
        assert!(
            report.is_clean(),
            "新規 Vault (entry なし) は clean=true (Phase 1 でも PASS)"
        );
    }
}
