use crate::knowledge::extractor::{StockCategory, StockEntry};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// ナレッジVault: mdファイルにストックを蓄積（Karpathyパターン）
pub struct Vault {
    root: PathBuf,
}

impl Vault {
    pub fn new(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root)?;
        for cat in &[
            "decisions",
            "facts",
            "preferences",
            "patterns",
            "insights",
            "todos",
        ] {
            let p = root.join(format!("{cat}.md"));
            if !p.exists() {
                std::fs::write(&p, format!("# {}\n\n", capitalize(cat)))?;
            }
        }
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    /// ストックエントリをmdファイルに追記
    pub fn append(&self, entry: &StockEntry) -> Result<()> {
        let path = self.root.join(format!("{}.md", entry.category.as_str()));
        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M");
        let line = format!(
            "\n- [{timestamp}] {}\n",
            entry
                .content
                .replace('\n', " ")
                .chars()
                .take(200)
                .collect::<String>()
        );
        let mut content = std::fs::read_to_string(&path).unwrap_or_default();
        // 重複チェック（同じ内容が既にあればスキップ）
        if content.contains(&entry.content.chars().take(50).collect::<String>()) {
            return Ok(());
        }
        content.push_str(&line);
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// ストックエントリをKnowledgeGraphに記録（LLM Wiki相互リンク）
    pub fn record_to_graph(
        &self,
        entry: &StockEntry,
        graph: &crate::memory::graph::KnowledgeGraph,
    ) -> Result<()> {
        let category = entry.category.as_str();
        let content_key = entry.content.chars().take(60).collect::<String>();

        // カテゴリノード（vault_category型）
        let cat_id = graph.add_node("vault_category", category)?;
        // エントリノード（vault_entry型）
        let entry_id = graph.add_node("vault_entry", &content_key)?;
        // カテゴリ→エントリのcontainsエッジ
        graph.add_edge(cat_id, entry_id, "contains", 1.0)?;

        // ソース情報があればソース→エントリのextracted_fromエッジ
        if !entry.source.is_empty() {
            let source_id = graph.add_node("source", &entry.source)?;
            graph.add_edge(source_id, entry_id, "extracted_from", 1.0)?;
        }
        Ok(())
    }

    /// 複数エントリをバッチ追記
    pub fn append_all(&self, entries: &[StockEntry]) -> Result<usize> {
        let mut count = 0;
        for entry in entries {
            if self.append(entry).is_ok() {
                count += 1;
            }
        }
        Ok(count)
    }

    /// 特定カテゴリのストックを読み込み
    pub fn read_category(&self, category: &StockCategory) -> Result<String> {
        let path = self.root.join(format!("{}.md", category.as_str()));
        Ok(std::fs::read_to_string(&path).unwrap_or_default())
    }

    /// 全カテゴリの概要を返す
    pub fn summary(&self) -> Result<String> {
        let mut out = String::from("# Knowledge Vault\n\n");
        for cat in &[
            "decisions",
            "facts",
            "preferences",
            "patterns",
            "insights",
            "todos",
        ] {
            let path = self.root.join(format!("{cat}.md"));
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            let lines = content.lines().filter(|l| l.starts_with("- [")).count();
            out.push_str(&format!("- **{cat}**: {lines} entries\n"));
        }
        Ok(out)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Rules（Decision/Pattern）を常時ロード用に返す（最新N件）
    pub fn read_rules(&self, max_per_category: usize) -> Result<Vec<String>> {
        use crate::knowledge::extractor::StockCategory;
        let mut rules = Vec::new();
        for cat in StockCategory::all() {
            if cat.is_rule()
                && let Ok(content) = self.read_category(cat)
            {
                let lines: Vec<&str> = content.lines().filter(|l| l.starts_with("- [")).collect();
                // 最新N件（末尾から取得）
                for line in lines.iter().rev().take(max_per_category) {
                    rules.push(line.to_string());
                }
            }
        }
        Ok(rules)
    }

    /// タスクコンテキストに関連するDocsカテゴリのみ返す
    pub fn read_docs_for_context(
        &self,
        task_context: &str,
        max_per_category: usize,
    ) -> Result<Vec<String>> {
        use crate::knowledge::extractor::StockCategory;
        let relevant_cats = StockCategory::docs_for_task_context(task_context);
        let mut docs = Vec::new();
        for cat in &relevant_cats {
            if let Ok(content) = self.read_category(cat) {
                let lines: Vec<&str> = content.lines().filter(|l| l.starts_with("- [")).collect();
                for line in lines.iter().rev().take(max_per_category) {
                    docs.push(line.to_string());
                }
            }
        }
        Ok(docs)
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::extractor::StockEntry;
    fn tmp_vault() -> Vault {
        let p = PathBuf::from(format!("/tmp/bonsai-vault-test-{}", uuid::Uuid::new_v4()));
        Vault::new(&p).unwrap()
    }
    #[test]
    fn t_create() {
        let v = tmp_vault();
        assert!(v.root().join("decisions.md").exists());
        std::fs::remove_dir_all(v.root()).ok();
    }
    #[test]
    fn t_append() {
        let v = tmp_vault();
        v.append(&StockEntry {
            category: StockCategory::Decision,
            content: "Rustを採用".into(),
            source: "s1".into(),
        })
        .unwrap();
        let c = v.read_category(&StockCategory::Decision).unwrap();
        assert!(c.contains("Rustを採用"));
        std::fs::remove_dir_all(v.root()).ok();
    }
    #[test]
    fn t_dedup() {
        let v = tmp_vault();
        let e = StockEntry {
            category: StockCategory::Fact,
            content: "1ビットLLMは1.28GB".into(),
            source: "s1".into(),
        };
        v.append(&e).unwrap();
        v.append(&e).unwrap();
        let c = v.read_category(&StockCategory::Fact).unwrap();
        assert_eq!(c.matches("1.28GB").count(), 1); // 重複なし
        std::fs::remove_dir_all(v.root()).ok();
    }
    #[test]
    fn t_summary() {
        let v = tmp_vault();
        v.append(&StockEntry {
            category: StockCategory::Todo,
            content: "テストを書く".into(),
            source: "s1".into(),
        })
        .unwrap();
        let s = v.summary().unwrap();
        assert!(s.contains("todos"));
        std::fs::remove_dir_all(v.root()).ok();
    }

    #[test]
    fn t_read_rules() {
        let v = tmp_vault();
        v.append(&StockEntry {
            category: StockCategory::Decision,
            content: "Rustを採用することにした".into(),
            source: "s1".into(),
        })
        .unwrap();
        v.append(&StockEntry {
            category: StockCategory::Pattern,
            content: "TDDパターンを使う".into(),
            source: "s1".into(),
        })
        .unwrap();
        v.append(&StockEntry {
            category: StockCategory::Fact,
            content: "1ビットLLMは1.28GBである".into(),
            source: "s1".into(),
        })
        .unwrap();
        let rules = v.read_rules(5).unwrap();
        assert!(
            rules.iter().any(|r| r.contains("Rust")),
            "Decision is rule: {rules:?}"
        );
        assert!(
            rules.iter().any(|r| r.contains("TDD")),
            "Pattern is rule: {rules:?}"
        );
        assert!(
            !rules.iter().any(|r| r.contains("1.28GB")),
            "Fact is NOT rule: {rules:?}"
        );
        std::fs::remove_dir_all(v.root()).ok();
    }

    #[test]
    fn t_read_docs_for_context() {
        let v = tmp_vault();
        v.append(&StockEntry {
            category: StockCategory::Insight,
            content: "ureqではSSLが動かないとわかった".into(),
            source: "s1".into(),
        })
        .unwrap();
        v.append(&StockEntry {
            category: StockCategory::Fact,
            content: "仕様として1ビットは制約がある".into(),
            source: "s1".into(),
        })
        .unwrap();
        // エラー関連コンテキスト → Insightが含まれる
        let docs = v
            .read_docs_for_context("エラーの原因を調べたい", 5)
            .unwrap();
        assert!(
            docs.iter().any(|d| d.contains("SSL")),
            "Insight included: {docs:?}"
        );
        // 仕様関連コンテキスト → Factが含まれる
        let docs2 = v.read_docs_for_context("仕様を確認したい", 5).unwrap();
        assert!(
            docs2.iter().any(|d| d.contains("1ビット")),
            "Fact included: {docs2:?}"
        );
        // 無関係コンテキスト → 空
        let docs3 = v.read_docs_for_context("hello world", 5).unwrap();
        assert!(docs3.is_empty(), "No docs: {docs3:?}");
        std::fs::remove_dir_all(v.root()).ok();
    }

    #[test]
    fn test_record_to_graph() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::new(dir.path()).unwrap();
        let store = crate::memory::store::MemoryStore::in_memory().unwrap();
        let graph = crate::memory::graph::KnowledgeGraph::new(store.conn());

        let entry = StockEntry {
            category: StockCategory::Decision,
            content: "Rustを採用した".to_string(),
            source: "session_001".to_string(),
        };
        vault.record_to_graph(&entry, &graph).unwrap();

        let neighbors = graph.neighbors("decisions", 1).unwrap();
        assert!(
            !neighbors.is_empty(),
            "カテゴリ→エントリのエッジが存在すべき"
        );
    }

    #[test]
    fn test_record_to_graph_without_source() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::new(dir.path()).unwrap();
        let store = crate::memory::store::MemoryStore::in_memory().unwrap();
        let graph = crate::memory::graph::KnowledgeGraph::new(store.conn());

        let entry = StockEntry {
            category: StockCategory::Fact,
            content: "Rustは安全な言語".to_string(),
            source: String::new(),
        };
        vault.record_to_graph(&entry, &graph).unwrap();

        let neighbors = graph.neighbors("facts", 1).unwrap();
        assert!(!neighbors.is_empty());
    }
}
