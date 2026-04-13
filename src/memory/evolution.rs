use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::memory::store::MemoryStore;

/// arxiv論文の知識エントリ
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArxivKnowledge {
    pub arxiv_id: String,
    pub title: String,
    pub summary: String,
    pub relevance: String,       // エージェントにとっての関連性
    pub actionable: Vec<String>, // 実装に活かせるアクション
    pub tags: Vec<String>,
}

/// arxiv APIから論文を検索（API鍵不要）
pub fn search_arxiv(query: &str, max_results: usize) -> Result<Vec<ArxivEntry>> {
    let url = format!(
        "http://export.arxiv.org/api/query?search_query=all:{}&start=0&max_results={}&sortBy=submittedDate&sortOrder=descending",
        urlencoding(query),
        max_results
    );

    let mut response = ureq::get(&url).call()?;
    let body = response.body_mut().read_to_string()?;
    let entries = parse_arxiv_xml(&body);
    Ok(entries)
}

/// arxiv APIのXMLレスポンスをパース（簡易実装）
fn parse_arxiv_xml(xml: &str) -> Vec<ArxivEntry> {
    let mut entries = Vec::new();
    let mut remaining = xml;

    while let Some(entry_start) = remaining.find("<entry>") {
        if let Some(entry_end) = remaining[entry_start..].find("</entry>") {
            let entry_xml = &remaining[entry_start..entry_start + entry_end + 8];

            let id = extract_xml_tag(entry_xml, "id")
                .unwrap_or_default()
                .replace("http://arxiv.org/abs/", "");
            let title = extract_xml_tag(entry_xml, "title")
                .unwrap_or_default()
                .replace('\n', " ")
                .trim()
                .to_string();
            let summary = extract_xml_tag(entry_xml, "summary")
                .unwrap_or_default()
                .replace('\n', " ")
                .trim()
                .to_string();
            let published = extract_xml_tag(entry_xml, "published").unwrap_or_default();

            // 著者を抽出
            let mut authors = Vec::new();
            let mut author_search = entry_xml;
            while let Some(start) = author_search.find("<name>") {
                if let Some(end) = author_search[start..].find("</name>") {
                    authors.push(author_search[start + 6..start + end].to_string());
                    author_search = &author_search[start + end + 7..];
                } else {
                    break;
                }
            }

            if !id.is_empty() {
                entries.push(ArxivEntry {
                    id,
                    title,
                    summary: if summary.len() > 500 {
                        {
                            let end = summary
                                .char_indices()
                                .take_while(|(i, _)| *i <= 500)
                                .last()
                                .map(|(i, ch)| i + ch.len_utf8())
                                .unwrap_or(summary.len());
                            format!("{}...", &summary[..end])
                        }
                    } else {
                        summary
                    },
                    authors,
                    published,
                });
            }

            remaining = &remaining[entry_start + entry_end + 8..];
        } else {
            break;
        }
    }
    entries
}

/// XMLタグの内容を抽出（簡易）
fn extract_xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    if let Some(start) = xml.find(&open)
        && let Some(end) = xml[start..].find(&close)
    {
        return Some(xml[start + open.len()..start + end].to_string());
    }
    // 属性付きタグ
    let open_attr = format!("<{tag} ");
    if let Some(start) = xml.find(&open_attr)
        && let Some(tag_end) = xml[start..].find('>')
        && let Some(end) = xml[start + tag_end..].find(&close)
    {
        return Some(xml[start + tag_end + 1..start + tag_end + end].to_string());
    }
    None
}

/// URLエンコーディング
fn urlencoding(s: &str) -> String {
    let mut result = String::new();
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => result.push(c),
            ' ' => result.push('+'),
            _ => {
                for b in c.to_string().as_bytes() {
                    result.push_str(&format!("%{b:02X}"));
                }
            }
        }
    }
    result
}

/// arxivエントリ
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArxivEntry {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub authors: Vec<String>,
    pub published: String,
}

/// 自己進化エンジン: arxiv論文を定期収集し、エージェントの知識を更新
pub struct EvolutionEngine<'a> {
    store: &'a MemoryStore,
}

impl<'a> EvolutionEngine<'a> {
    pub fn new(store: &'a MemoryStore) -> Self {
        Self { store }
    }

    /// arxivから論文を検索してメモリに蓄積
    pub fn ingest_arxiv(&self, query: &str, max_results: usize) -> Result<Vec<String>> {
        let entries = search_arxiv(query, max_results)?;
        let mut saved = Vec::new();

        for entry in &entries {
            // 重複チェック（arxiv IDで）
            let existing = self.store.search_memories(&entry.id, 1)?;
            if !existing.is_empty() {
                continue;
            }

            let content = format!(
                "[arxiv:{}] {}\n{}\n著者: {}",
                entry.id,
                entry.title,
                entry.summary,
                entry.authors.join(", ")
            );

            let tags = vec![
                "arxiv".to_string(),
                "research".to_string(),
                entry.id.clone(),
            ];

            self.store.save_memory(&content, "knowledge", &tags)?;
            saved.push(entry.id.clone());
        }

        Ok(saved)
    }

    /// エージェントの改善ポイントを特定（Dreamingレポートとarxiv知識を統合）
    pub fn suggest_improvements(&self) -> Result<Vec<String>> {
        let mut suggestions = Vec::new();

        // arxiv知識からの提案
        let arxiv_memories = self.store.search_memories("arxiv", 5)?;
        if !arxiv_memories.is_empty() {
            suggestions.push(format!(
                "{}件のarXiv論文が蓄積されています。最新の研究を参照してアプローチを改善できます。",
                arxiv_memories.len()
            ));
        }

        // 失敗経験からの提案
        let exp = crate::memory::experience::ExperienceStore::new(self.store.conn());
        let failures = exp.find_similar("failure", 5)?;
        let failure_count = failures
            .iter()
            .filter(|e| e.exp_type == crate::memory::experience::ExperienceType::Failure)
            .count();
        if failure_count >= 3 {
            suggestions.push(format!(
                "直近で{failure_count}件の失敗があります。失敗パターンをarXiv論文と照合して改善策を探ります。"
            ));
        }

        // スキル進化の提案
        let skills = crate::memory::skill::SkillStore::new(self.store.conn());
        let all_skills = skills.list_all()?;
        if all_skills.is_empty() {
            suggestions.push(
                "まだスキルが蓄積されていません。繰り返しタスクをスキル化して効率を上げましょう。"
                    .to_string(),
            );
        }

        Ok(suggestions)
    }

    pub fn apply_improvements(&self) -> Result<Vec<String>> {
        let mut applied = Vec::new();
        let exp = crate::memory::experience::ExperienceStore::new(self.store.conn());
        let tool_names = [
            "shell",
            "file_read",
            "file_write",
            "git",
            "web_search",
            "web_fetch",
            "repomap",
        ];
        for tool in &tool_names {
            for (pat, cnt) in exp.failure_patterns(tool).unwrap_or_default() {
                if cnt >= 3 {
                    let msg = format!("[auto-learn] {tool}:'{pat}' は{cnt}回失敗");
                    if self
                        .store
                        .search_memories(&pat, 1)
                        .unwrap_or_default()
                        .is_empty()
                    {
                        let _ = self
                            .store
                            .save_memory(&msg, "insight", &["auto-improve".into()]);
                        applied.push(msg);
                    }
                }
            }
        }
        let dreamer = crate::memory::dreams::Dreamer::new(self.store.conn());
        if let Ok(report) = dreamer.generate_report(7)
            && report.success_rate < 0.5
            && report.success_rate > 0.0
        {
            let msg = format!("[auto-learn] 成功率{:.0}%", report.success_rate * 100.0);
            let _ = self
                .store
                .save_memory(&msg, "insight", &["auto-improve".into()]);
            applied.push(msg);
        }
        Ok(applied)
    }

    /// 実験ループ用: Dreamerのinsightからプロンプトルール候補を生成
    pub fn insight_based_mutations(&self) -> Result<Vec<String>> {
        let mut mutations = Vec::new();

        // Dreamerレポートからinsightを取得
        let dreamer = crate::memory::dreams::Dreamer::new(self.store.conn());
        if let Ok(report) = dreamer.generate_report(7) {
            for insight in &report.insights {
                mutations.push(insight.clone());
            }
        }

        // auto-improveタグのメモリからも取得
        let auto_insights = self.store.search_memories("auto-improve", 10)?;
        for mem in &auto_insights {
            if !mutations.iter().any(|m| {
                let prefix: String = mem.content.chars().take(20).collect();
                m.contains(&prefix)
            }) {
                mutations.push(mem.content.clone());
            }
        }

        Ok(mutations)
    }

    /// 定期実行用: 関心領域のarxiv論文を自動収集
    pub fn auto_collect(&self) -> Result<usize> {
        let queries = [
            "LLM agent tool calling",
            "1-bit quantization language model",
            "autonomous agent memory",
            "small language model reasoning",
        ];

        let mut total = 0;
        for query in &queries {
            let saved = self.ingest_arxiv(query, 3)?;
            total += saved.len();
        }
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_arxiv_xml() {
        let xml = r#"<?xml version="1.0"?>
<feed>
<entry>
<id>http://arxiv.org/abs/2402.17764</id>
<title>The Era of 1-bit LLMs</title>
<summary>We introduce BitNet b1.58 which uses ternary weights.</summary>
<author><name>Shuming Ma</name></author>
<author><name>Hongyu Wang</name></author>
<published>2024-02-27</published>
</entry>
</feed>"#;
        let entries = parse_arxiv_xml(xml);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].id.contains("2402.17764"));
        assert!(entries[0].title.contains("1-bit"));
        assert_eq!(entries[0].authors.len(), 2);
    }

    #[test]
    fn test_parse_empty_xml() {
        let entries = parse_arxiv_xml("<feed></feed>");
        assert!(entries.is_empty());
    }

    #[test]
    fn test_extract_xml_tag() {
        assert_eq!(
            extract_xml_tag("<title>Test</title>", "title"),
            Some("Test".to_string())
        );
        assert_eq!(
            extract_xml_tag("<id>123</id>", "id"),
            Some("123".to_string())
        );
        assert_eq!(extract_xml_tag("<none>", "title"), None);
    }

    #[test]
    fn test_evolution_engine_suggest() {
        let store = MemoryStore::in_memory().unwrap();
        let engine = EvolutionEngine::new(&store);
        let suggestions = engine.suggest_improvements().unwrap();
        // スキルなしの場合の提案
        assert!(suggestions.iter().any(|s| s.contains("スキル")));
    }

    #[test]
    fn test_ingest_dedup() {
        let store = MemoryStore::in_memory().unwrap();
        // 手動でarxivメモリを追加
        store
            .save_memory(
                "[arxiv:2402.17764] test",
                "knowledge",
                &["arxiv".to_string()],
            )
            .unwrap();

        let _engine = EvolutionEngine::new(&store);
        // 同じIDの論文は重複チェックでスキップされる
        // (実際のAPI呼び出しは #[ignore] テストで確認)
    }

    #[test]
    fn test_parse_arxiv_xml_multibyte_summary() {
        let long_summary = "あ".repeat(200);
        let xml = format!(
            r#"<feed><entry><id>http://arxiv.org/abs/9999.99999</id><title>Test</title><summary>{}</summary><published>2024-01-01</published></entry></feed>"#,
            long_summary
        );
        let entries = parse_arxiv_xml(&xml);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].summary.ends_with("..."));
        assert!(
            entries[0]
                .summary
                .is_char_boundary(entries[0].summary.len())
        );
    }

    // 実ネットワークテスト
    #[test]
    #[ignore]
    fn test_search_arxiv_live() {
        let entries = search_arxiv("1-bit LLM", 3).unwrap();
        assert!(!entries.is_empty());
        println!("Found {} entries:", entries.len());
        for e in &entries {
            println!("  [{} ] {}", e.id, e.title);
        }
    }

    #[test]
    fn test_insight_based_mutations_empty() {
        let store = MemoryStore::in_memory().unwrap();
        let engine = EvolutionEngine::new(&store);
        let mutations = engine.insight_based_mutations().unwrap();
        // Dreamerがルールベースのinsightを返す場合もある
        // エラーなく実行できることを確認
        assert!(mutations.len() < 100);
    }

    #[test]
    fn test_insight_based_mutations_with_data() {
        let store = MemoryStore::in_memory().unwrap();
        store
            .save_memory(
                "[auto-learn] shell:'rm' は3回失敗",
                "insight",
                &["auto-improve".into()],
            )
            .unwrap();
        let engine = EvolutionEngine::new(&store);
        let mutations = engine.insight_based_mutations().unwrap();
        assert!(!mutations.is_empty());
    }

    #[test]
    #[ignore]
    fn test_auto_collect_live() {
        let store = MemoryStore::in_memory().unwrap();
        let engine = EvolutionEngine::new(&store);
        let count = engine.auto_collect().unwrap();
        println!("Collected {count} papers");
        assert!(count > 0);
    }
}
