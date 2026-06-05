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

/// 候補のメンバ entry を全件から復元する純粋関数 (Phase 2 の raw 再読込用)。
///
/// content key (先頭 60 字) が候補の `member_entry_keys` に含まれる entry を返す。
/// agent 層はこの **生 content** を LLM 合成に渡し、要約の要約 (再帰的要約劣化) を避ける。
pub fn member_entries<'a>(
    candidate: &ConceptCandidate,
    entries: &'a [StockEntry],
) -> Vec<&'a StockEntry> {
    let keyset: HashSet<&str> = candidate
        .member_entry_keys
        .iter()
        .map(|s| s.as_str())
        .collect();
    entries
        .iter()
        .filter(|e| keyset.contains(content_key(&e.content).as_str()))
        .collect()
}

/// 合成済み概念ページ (Phase 2 の出力)。
///
/// `body` は agent 層が LLM に **member raw entry を再読込** させて合成した本文
/// (概要 / 横断的知見 / 未解決の問い、inline `[[source]]` 出典)。
/// 永続化前は status="draft"。LLM 非依存の純粋データ型として knowledge 層に置く。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConceptPage {
    /// 概念の主題キー (`ConceptCandidate::theme_key` 由来)。
    pub theme_key: String,
    /// 出典 source 集合 (frontmatter 用、昇順想定)。
    pub sources: Vec<String>,
    /// LLM 合成本文。
    pub body: String,
    /// ページ状態 ("draft" = 合成直後の未レビュー)。
    pub status: String,
}

/// theme_key をファイル名 slug 化 (英数のみ残し、その他は `-`、連続 `-` 圧縮、小文字)。
/// path traversal 防止 (`/`・`.` を含めない)。
pub fn theme_slug(theme_key: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;
    for c in theme_key.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    let trimmed = slug.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "concept".to_string()
    } else {
        trimmed
    }
}

/// 概念ページを markdown 文字列にレンダリング (frontmatter + 本文、決定的)。
pub fn render_concept_markdown(page: &ConceptPage, updated_at: &str) -> String {
    let sources = page.sources.join(", ");
    format!(
        "---\ntheme: {}\nsources: [{}]\nupdated_at: {}\nstatus: {}\n---\n\n# Concept: {}\n\n{}\n",
        page.theme_key, sources, updated_at, page.status, page.theme_key, page.body
    )
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
    use std::collections::BTreeMap;

    // term -> (member entry 数, dedup content key 集合, distinct source 集合)。
    // BTreeMap/BTreeSet で挿入順非依存の決定的反復を保証。
    #[derive(Default)]
    struct Agg {
        entry_count: usize,
        keys: std::collections::BTreeSet<String>,
        sources: std::collections::BTreeSet<String>,
    }
    let mut by_term: BTreeMap<String, Agg> = BTreeMap::new();

    for e in entries {
        let key = content_key(&e.content);
        for term in extract_terms(&e.content) {
            let agg = by_term.entry(term).or_default();
            agg.entry_count += 1;
            agg.keys.insert(key.clone());
            if !e.source.is_empty() {
                agg.sources.insert(e.source.clone());
            }
        }
    }

    let mut candidates: Vec<ConceptCandidate> = by_term
        .into_iter()
        .filter(|(_, agg)| {
            agg.sources.len() >= config.min_sources && agg.entry_count >= config.min_cluster_size
        })
        .map(|(theme_key, agg)| {
            let score = agg.sources.len() * agg.entry_count;
            ConceptCandidate {
                theme_key,
                member_entry_keys: agg.keys.into_iter().collect(),
                member_sources: agg.sources.into_iter().collect(),
                score,
            }
        })
        .collect();

    // score 降順 → theme_key 昇順 (安定・決定的)。
    candidates.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.theme_key.cmp(&b.theme_key))
    });
    candidates.truncate(config.max_candidates);
    candidates
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
        assert_eq!(
            cands.len(),
            1,
            "3 source が rust を共有 → 1 候補: {cands:?}"
        );
        let c = &cands[0];
        assert_eq!(c.theme_key, "rust");
        assert_eq!(
            c.member_sources,
            vec!["session_a", "session_b", "session_c"]
        );
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
        // 各 entry は theme 語 + 固有語のみ (テーマ間で偶発共有 term を作らない)。
        // beta は 2 source × 2 entry = score 4、alpha は 3 source × 3 entry = score 9。
        let entries = vec![
            entry("beta apple", "s1"),
            entry("beta banana", "s2"),
            entry("alpha cat", "s1"),
            entry("alpha dog", "s2"),
            entry("alpha echo", "s3"),
        ];
        let cfg = ConceptConfig {
            min_sources: 2,
            min_cluster_size: 2,
            max_candidates: 1,
        };
        let cands = detect_concept_candidates(&entries, &cfg);
        assert_eq!(cands.len(), 1, "max_candidates=1 で 1 件に制限");
        assert_eq!(cands[0].theme_key, "alpha", "score 最大の alpha が残る");
        assert_eq!(cands[0].score, 9, "alpha = 3 source × 3 entry");
    }

    #[test]
    fn t_deterministic_order() {
        // alpha/zeta どちらも 2 source × 2 entry = score 4。固有語で偶発共有を避ける。
        let entries = vec![
            entry("zeta cat", "s1"),
            entry("zeta dog", "s2"),
            entry("alpha apple", "s1"),
            entry("alpha banana", "s2"),
        ];
        let a = detect_concept_candidates(&entries, &ConceptConfig::default());
        let b = detect_concept_candidates(&entries, &ConceptConfig::default());
        assert_eq!(a, b, "同一入力で同一出力 (決定的)");
        let keys: Vec<&str> = a.iter().map(|c| c.theme_key.as_str()).collect();
        assert_eq!(
            keys,
            vec!["alpha", "zeta"],
            "同 score は theme_key 昇順: {keys:?}"
        );
    }

    #[test]
    fn t_member_entries_recovers_raw_content() {
        let entries = vec![
            entry("rust ownership prevents data races", "session_a"),
            entry("rust borrow checker enforces lifetimes", "session_b"),
            entry("python dynamic typing unrelated", "session_c"),
        ];
        let cands = detect_concept_candidates(&entries, &ConceptConfig::default());
        assert_eq!(cands.len(), 1);
        let members = member_entries(&cands[0], &entries);
        assert_eq!(members.len(), 2, "rust 2 entry が復元される");
        assert!(members.iter().all(|e| e.content.contains("rust")));
        // python entry は除外
        assert!(!members.iter().any(|e| e.content.contains("python")));
    }

    #[test]
    fn t_theme_slug_sanitizes() {
        assert_eq!(theme_slug("Rust"), "rust");
        assert_eq!(theme_slug("memory/graph.rs"), "memory-graph-rs");
        assert_eq!(theme_slug("  spaced  word  "), "spaced-word");
        assert_eq!(theme_slug("../etc/passwd"), "etc-passwd");
        assert_eq!(theme_slug("!!!"), "concept", "空 slug は fallback");
    }

    #[test]
    fn t_render_concept_markdown_has_frontmatter_and_body() {
        let page = ConceptPage {
            theme_key: "rust".into(),
            sources: vec!["session_a".into(), "session_b".into()],
            body: "概要: rust は所有権で安全。横断的知見は [[session_a]] と [[session_b]] に由来。"
                .into(),
            status: "draft".into(),
        };
        let md = render_concept_markdown(&page, "2026-06-05 10:00");
        assert!(md.starts_with("---\n"), "frontmatter 開始: {md}");
        assert!(md.contains("theme: rust"));
        assert!(md.contains("sources: [session_a, session_b]"));
        assert!(md.contains("updated_at: 2026-06-05 10:00"));
        assert!(md.contains("status: draft"));
        assert!(md.contains("# Concept: rust"));
        assert!(md.contains("[[session_a]]"), "inline 出典保持");
    }
}
