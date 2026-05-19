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
use chrono::{NaiveDateTime, TimeZone};
use std::collections::{BTreeMap, BTreeSet};

/// vault.rs:34 と同形式の timestamp ("%Y-%m-%d %H:%M")。
const TIMESTAMP_FMT: &str = "%Y-%m-%d %H:%M";

/// vault.rs の append() で書き込まれる全 6 カテゴリ。
const CATEGORIES: &[&str] = &[
    "decisions",
    "facts",
    "preferences",
    "patterns",
    "insights",
    "todos",
];

/// 重複 / cross-cat 判定に使う content prefix 長 (vault.rs::append の dedup 50 文字と整合)。
const PREFIX_LEN: usize = 50;

/// incomplete 検出の単語パターン (trim 後の content が完全一致した場合に incomplete 認定)。
const INCOMPLETE_MARKERS: &[&str] = &["TODO", "FIXME", "WIP", "..."];

/// vault md の 1 行 (`- [YYYY-MM-DD HH:MM] content`) を parse。
fn parse_vault_line(line: &str) -> Option<(chrono::DateTime<chrono::Utc>, String, String)> {
    let trimmed = line.trim_start();
    let stripped = trimmed.strip_prefix("- [")?;
    let end_bracket = stripped.find(']')?;
    let timestamp_str = &stripped[..end_bracket];
    let naive = NaiveDateTime::parse_from_str(timestamp_str, TIMESTAMP_FMT).ok()?;
    let timestamp = chrono::Utc.from_utc_datetime(&naive);
    let content = stripped[end_bracket + 1..].trim().to_string();
    Some((timestamp, timestamp_str.to_string(), content))
}

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

    /// warning log を `[INFO][lab.vault_lint]` prefix で出力 (項目 244 KG lint と整合).
    ///
    /// 各軸の count + clean=true/false を tracing 経由ではなく `log_event` で出力可能だが、
    /// 本 plan §3.1 ではシンプルに eprintln + prefix を採用 (運用 protocol で grep 容易)。
    pub fn warn_log(&self) {
        eprintln!(
            "[INFO][lab.vault_lint] duplicate={} stale={} cross_cat={} incomplete={} orphan={} clean={}",
            self.duplicate_entries.len(),
            self.stale_entries.len(),
            self.cross_category_leaks.len(),
            self.incomplete_entries.len(),
            self.orphan_entries.len(),
            self.is_clean()
        );
    }
}

/// `BONSAI_VAULT_LINT_STRICT=1` で strict gate mode (FAIL 時 abort) を opt-in 化.
///
/// 既定 OFF (warning のみ、Lab 続行)。production CI / paired Lab 起動前で strict gate 推奨。
pub fn is_vault_lint_strict() -> bool {
    matches!(
        std::env::var("BONSAI_VAULT_LINT_STRICT").as_deref(),
        Ok("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

/// 項目 246 Phase 4 用 Lab wiring gate. `BONSAI_VAULT_LINT_LAB=1` で Lab cycle 起動前 sanity pass を発火.
///
/// Why: `is_vault_lint_strict` と独立した gate (strict は「失敗時 bail」、本 gate は「pass を発火するか」).
/// default OFF で production CLI / 通常 lab 起動は影響なし、paired smoke 起動前のみ opt-in.
pub fn is_vault_lint_lab_enabled() -> bool {
    matches!(
        std::env::var("BONSAI_VAULT_LINT_LAB").as_deref(),
        Ok("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

/// `BONSAI_VAULT_LINT_STALE_DAYS` env で stale 閾値を override (default 90).
///
/// 範囲外 (0 以下 or 365 超) は default fallback、parse 失敗も同様。
pub fn vault_lint_stale_days() -> i64 {
    const DEFAULT: i64 = 90;
    std::env::var("BONSAI_VAULT_LINT_STALE_DAYS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|d| (1..=365).contains(d))
        .unwrap_or(DEFAULT)
}

/// Lab 起動時の Vault sanity lint pass.
///
/// `stale_threshold_days`: 90 (default、`BONSAI_VAULT_LINT_STALE_DAYS` env で override 可)。
///
/// Phase 2 Green 実装: 4 軸 (duplicate / stale / cross-cat / incomplete) を検出。
/// orphan_entries は Phase 3+ で KG wiring 経由実装 (本 phase では常に空)。
pub fn lint_vault_for_lab(vault: &Vault, stale_threshold_days: i64) -> VaultLintReport {
    let mut report = VaultLintReport::default();
    let now = chrono::Utc::now();
    let stale_threshold = chrono::Duration::days(stale_threshold_days);

    // prefix → set of categories (cross-cat 判定用)
    let mut prefix_to_cats: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for cat in CATEGORIES {
        let path = vault.root().join(format!("{cat}.md"));
        let content = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        // 同 cat 内 prefix → count (duplicate 判定用)
        let mut prefix_counts_in_cat: BTreeMap<String, usize> = BTreeMap::new();

        for line in content.lines() {
            let Some((timestamp, timestamp_str, parsed_content)) = parse_vault_line(line) else {
                continue;
            };

            // incomplete: 空 / whitespace / INCOMPLETE_MARKERS 完全一致
            let trimmed = parsed_content.trim();
            if trimmed.is_empty()
                || INCOMPLETE_MARKERS.contains(&trimmed)
                || INCOMPLETE_MARKERS
                    .iter()
                    .any(|m| trimmed.eq_ignore_ascii_case(m))
            {
                report
                    .incomplete_entries
                    .push((cat.to_string(), line.trim().to_string()));
                continue;
            }

            // stale: timestamp が threshold より古い
            let age = now - timestamp;
            if age > stale_threshold {
                let excerpt: String = parsed_content.chars().take(60).collect();
                report
                    .stale_entries
                    .push((cat.to_string(), timestamp_str, excerpt));
            }

            // duplicate / cross-cat 用 prefix 集計
            let prefix: String = parsed_content.chars().take(PREFIX_LEN).collect();
            *prefix_counts_in_cat.entry(prefix.clone()).or_insert(0) += 1;
            prefix_to_cats
                .entry(prefix)
                .or_default()
                .insert(cat.to_string());
        }

        // duplicate: 同 cat 内で count >= 2
        for (prefix, count) in prefix_counts_in_cat {
            if count >= 2 {
                report
                    .duplicate_entries
                    .push((cat.to_string(), prefix, count));
            }
        }
    }

    // cross_category_leaks: prefix が 2 cat 以上
    for (prefix, cats) in prefix_to_cats {
        if cats.len() >= 2 {
            report
                .cross_category_leaks
                .push((prefix, cats.into_iter().collect()));
        }
    }

    report
}

/// 項目 246 Phase 4 用 cross-test env mutex (BONSAI_VAULT_LINT_LAB set/unset を直列化).
///
/// strict / stale_days と独立 lock (テスト並行性を最大化、ただし同一 env 名は直列化必須).
/// clippy `items_after_test_module` 回避のため tests module の前に配置.
#[cfg(test)]
pub(crate) static VAULT_LINT_LAB_ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

    /// Phase 2 Green 期待: 重複 prefix を検出 → PASS
    /// Note: `Vault::append` は 50 char prefix dedup を行うため、test fixture は直接 md に書込んで
    ///       past data や bug で混入する重複シナリオを模擬する。
    #[test]
    fn t_vault_lint_detects_duplicate_entries() {
        let dir = tempdir().expect("tempdir");
        let vault = Vault::new(dir.path()).expect("vault new");
        // 直接 facts.md に同 prefix の重複 entry を書込 (append() dedup bypass)
        let facts_path = dir.path().join("facts.md");
        let existing = std::fs::read_to_string(&facts_path).unwrap_or_default();
        let now_ts = chrono::Utc::now().format("%Y-%m-%d %H:%M");
        let dup = format!(
            "\n- [{}] the same long content of fifty characters length here xx\n- [{}] the same long content of fifty characters length here yy\n",
            now_ts, now_ts
        );
        std::fs::write(&facts_path, existing + &dup).expect("write");
        let report = lint_vault_for_lab(&vault, 90);
        assert!(
            !report.duplicate_entries.is_empty(),
            "重複 entry を検出 (Phase 2 Green PASS、同 prefix 50 char で count>=2)"
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
        let common =
            "shared content that lands in two different categories for cross-cat leak test";
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

    /// Phase 3 env getter test: BONSAI_VAULT_LINT_STALE_DAYS 動作確証
    /// (env mutex は config::LAB_TEMP_ENV_TEST_LOCK と独立、本 test 内のみ env 触る)
    #[test]
    fn t_vault_lint_stale_days_default_90() {
        // env unset を保証 (前 test 残留対策)
        unsafe { std::env::remove_var("BONSAI_VAULT_LINT_STALE_DAYS") };
        assert_eq!(vault_lint_stale_days(), 90, "env unset で default 90");
    }

    /// Phase 3 env getter test: BONSAI_VAULT_LINT_STRICT 動作確証
    #[test]
    fn t_vault_lint_strict_default_off() {
        unsafe { std::env::remove_var("BONSAI_VAULT_LINT_STRICT") };
        assert!(!is_vault_lint_strict(), "env unset で strict OFF");
    }

    /// 項目 246 Phase 4 Red: Lab wiring 用 env gate `BONSAI_VAULT_LINT_LAB` の default OFF 動作確証.
    ///
    /// Why: production CLI / 通常起動で Vault lint pass が暴発しないことを担保。
    /// 既存 `is_vault_lint_strict` と独立 (strict は「失敗時 bail」、本 gate は「pass を発火するか」)。
    #[test]
    fn t_is_vault_lint_lab_enabled_default_off() {
        let _g = VAULT_LINT_LAB_ENV_TEST_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("BONSAI_VAULT_LINT_LAB") };
        assert!(
            !is_vault_lint_lab_enabled(),
            "env unset で Lab wiring gate OFF (production no-op)"
        );
    }

    /// 項目 246 Phase 4 Red: env=1 で Lab wiring gate ON 動作確証.
    ///
    /// Why: paired Lab cycle で sanity gate 起動を opt-in 化する明示 env protocol を固定する。
    #[test]
    fn t_is_vault_lint_lab_enabled_when_set() {
        let _g = VAULT_LINT_LAB_ENV_TEST_LOCK.lock().unwrap();
        unsafe { std::env::set_var("BONSAI_VAULT_LINT_LAB", "1") };
        let result = is_vault_lint_lab_enabled();
        unsafe { std::env::remove_var("BONSAI_VAULT_LINT_LAB") };
        assert!(result, "BONSAI_VAULT_LINT_LAB=1 で Lab wiring gate ON");
    }
}

