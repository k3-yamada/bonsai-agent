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
}
