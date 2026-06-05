//! 概念候補検出 (knowledge 層、純粋、LLM 非依存)。
//!
//! 知識基盤強化計画 Phase 1 (`.claude/plan/knowledge-base-concept-pages.md`)。
//! Karpathy LLM Wiki の「概念ページ＝合成成果物」を bonsai に移植する第一段。
//!
//! ここでは **複数 source を横断して共有テーマを持つ vault entry 群** を
//! 決定的にクラスタ化するだけで、LLM 合成 (横断的知見の生成) は agent 層が担う。
//! 層分離 ([[feedback_clean_architecture]]): 本 module は LLM backend に依存しない。

use crate::knowledge::extractor::StockEntry;
use std::collections::HashSet;

/// 概念候補: 共有テーマ (高頻度 term) を介して 2+ source を横断する entry 群。
///
/// Phase 1 の決定的出力。Phase 2 (agent 層) が member の raw entry を再読込し
/// LLM で概念ページへ合成する入力となる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConceptCandidate {
    /// クラスタの主題を表す正規化済みキー (共有する高頻度 term)。
    pub theme_key: String,
    /// メンバ entry の content key (先頭 60 字、`Vault::record_to_graph` と整合)。重複なし・昇順。
    pub member_entry_keys: Vec<String>,
    /// メンバが由来する distinct source 集合 (昇順、空 source は除外)。
    pub member_sources: Vec<String>,
    /// 候補の強さ = distinct source 数 × member entry 数 (決定的、乱立時の上位 N 選別に使用)。
    pub score: usize,
}

/// 概念候補検出の閾値設定。
#[derive(Debug, Clone)]
pub struct ConceptConfig {
    /// クラスタ成立に必要な distinct source 数の下限 (横断性の担保)。
    pub min_sources: usize,
    /// クラスタ成立に必要な member entry 数の下限。
    pub min_cluster_size: usize,
    /// 返却する候補の上限 (score 上位 N、概念の乱立防止)。
    pub max_candidates: usize,
}

impl Default for ConceptConfig {
    fn default() -> Self {
        Self {
            min_sources: 2,
            min_cluster_size: 2,
            max_candidates: 20,
        }
    }
}

/// entry の content から有意な term を抽出 (小文字化・非英数分割・len>=3・stopword 除去・entry 内重複排除)。
///
/// `memory::search::tokenize_for_graph` と同方針。日本語/絵文字混在は現状拾わないが、
/// Phase 1 の英語識別子・技術語クラスタリングには十分。
fn extract_terms(content: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "the", "and", "for", "are", "but", "not", "you", "all", "can", "had", "her", "was", "one",
        "our", "out", "day", "get", "has", "him", "his", "how", "man", "new", "now", "old", "see",
        "two", "way", "who", "did", "its", "let", "put", "say", "she", "too", "use", "with",
        "from", "this", "that", "have", "they", "what", "your", "when", "will", "there", "their",
        "would", "could", "should", "into", "about", "which", "were", "been",
    ];
    let stop: HashSet<&&str> = STOPWORDS.iter().collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut terms: Vec<String> = Vec::new();
    for raw in content.split(|c: char| !c.is_alphanumeric()) {
        let t = raw.to_ascii_lowercase();
        if t.len() < 3 || stop.contains(&t.as_str()) {
            continue;
        }
        if seen.insert(t.clone()) {
            terms.push(t);
        }
    }
    terms
}

/// entry の content key (先頭 60 字、`Vault::record_to_graph` と同一規則)。
fn content_key(content: &str) -> String {
    content.chars().take(60).collect()
}

/// 概念候補を決定的に検出する純粋関数。
///
/// アルゴリズム:
/// 1. 各 entry を term 列に分解し、`term -> (member entry 群, distinct source 群)` を集計。
/// 2. `sources >= min_sources` かつ `entries >= min_cluster_size` の term を候補化。
/// 3. score (= sources × entries) 降順 → theme_key 昇順で安定ソートし上位 `max_candidates` 件。
pub fn detect_concept_candidates(
    entries: &[StockEntry],
    config: &ConceptConfig,
) -> Vec<ConceptCandidate> {
    // STUB (Red): 未実装。Green で集計ロジックを入れる。
    let _ = (entries, config, extract_terms("") , content_key(""));
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::extractor::StockCategory;

    fn entry(content: &str, source: &str) -> StockEntry {
        StockEntry {
            category: StockCategory::Fact,
            content: content.to_string(),
            source: source.to_string(),
        }
    }

    #[test]
    fn t_three_sources_one_theme_yields_one_candidate() {
        let entries = vec![
            entry("rust ownership prevents data races", "session_a"),
            entry("rust borrow checker enforces lifetimes", "session_b"),
            entry("rust zero cost abstractions improve speed", "session_c"),
        ];
        let cands = detect_concept_candidates(&entries, &ConceptConfig::default());
        assert_eq!(cands.len(), 1, "3 source が rust を共有 → 1 候補: {cands:?}");
        let c = &cands[0];
        assert_eq!(c.theme_key, "rust");
        assert_eq!(c.member_sources, vec!["session_a", "session_b", "session_c"]);
        assert_eq!(c.member_entry_keys.len(), 3);
        assert_eq!(c.score, 9, "3 source × 3 entry = 9");
    }

    #[test]
    fn t_isolated_entries_yield_no_candidate() {
        let entries = vec![
            entry("rust ownership model", "session_a"),
            entry("python dynamic typing", "session_b"),
            entry("golang goroutine scheduler", "session_c"),
        ];
        let cands = detect_concept_candidates(&entries, &ConceptConfig::default());
        assert!(cands.is_empty(), "共有テーマなし → 候補なし: {cands:?}");
    }

    #[test]
    fn t_single_source_repeated_term_rejected() {
        // 同一 source 内で term 反復しても横断性 (>=2 source) を満たさない。
        let entries = vec![
            entry("rust ownership prevents races", "session_a"),
            entry("rust borrow checker strict", "session_a"),
        ];
        let cands = detect_concept_candidates(&entries, &ConceptConfig::default());
        assert!(cands.is_empty(), "単一 source → 候補なし: {cands:?}");
    }

    #[test]
    fn t_empty_source_does_not_count() {
        let entries = vec![
            entry("rust ownership prevents races", ""),
            entry("rust borrow checker strict", ""),
            entry("rust lifetimes annotate refs", "session_c"),
        ];
        let cands = detect_concept_candidates(&entries, &ConceptConfig::default());
        assert!(
            cands.is_empty(),
            "空 source は distinct source に数えない → 1 source のみ → 候補なし: {cands:?}"
        );
    }

    #[test]
    fn t_max_candidates_truncates_by_score() {
        let mut entries = Vec::new();
        // beta/gamma は 2 source × 2 entry = score 4、alpha は 3 source × 3 entry = score 9。
        for s in ["s1", "s2"] {
            entries.push(entry("beta shared concept here", s));
            entries.push(entry("gamma shared concept here", s));
        }
        for s in ["s1", "s2", "s3"] {
            entries.push(entry("alpha shared concept here", s));
        }
        let cfg = ConceptConfig {
            min_sources: 2,
            min_cluster_size: 2,
            max_candidates: 1,
        };
        let cands = detect_concept_candidates(&entries, &cfg);
        assert_eq!(cands.len(), 1, "max_candidates=1 で 1 件に制限");
        assert_eq!(cands[0].theme_key, "alpha", "score 最大の alpha が残る");
    }

    #[test]
    fn t_deterministic_order() {
        let entries = vec![
            entry("zeta shared topic", "s1"),
            entry("zeta shared topic", "s2"),
            entry("alpha shared topic", "s1"),
            entry("alpha shared topic", "s2"),
        ];
        let a = detect_concept_candidates(&entries, &ConceptConfig::default());
        let b = detect_concept_candidates(&entries, &ConceptConfig::default());
        assert_eq!(a, b, "同一入力で同一出力 (決定的)");
        // 全候補が同 score の場合 theme_key 昇順。
        let keys: Vec<&str> = a.iter().map(|c| c.theme_key.as_str()).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "同 score は theme_key 昇順: {keys:?}");
    }
}
