use anyhow::Result;
use std::path::{Path, PathBuf};

/// ADR（Architecture Decision Record）エントリ
/// Replan/Advisor介入時の意思決定理由を構造化記録
pub struct AdrEntry {
    /// 判断のタイトル（例: "Replan: ループ検出により再計画"）
    pub title: String,
    /// 判断の背景・コンテキスト
    pub context: String,
    /// 採用した判断
    pub decision: String,
    /// 判断の根拠
    pub rationale: String,
}

/// ADRをVaultのadr/サブディレクトリに蓄積するライター
pub struct AdrWriter {
    adr_dir: PathBuf,
}

impl AdrWriter {
    /// adr_dirを作成し、AdrWriterを返す
    pub fn new(vault_root: &Path) -> Result<Self> {
        let adr_dir = vault_root.join("adr");
        std::fs::create_dir_all(&adr_dir)?;
        Ok(Self { adr_dir })
    }

    /// ADRエントリをMarkdownファイルとして記録
    /// ファイル名: NNNN-<sanitized_title>.md（連番）
    pub fn record(&self, entry: &AdrEntry) -> Result<PathBuf> {
        let seq = self.count()? + 1;
        let sanitized = sanitize_filename(&entry.title);
        let filename = format!("{seq:04}-{sanitized}.md");
        let path = self.adr_dir.join(&filename);

        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M UTC");
        let content = format!(
            "# {title}\n\n\
             **Date**: {timestamp}\n\n\
             ## Context\n\n\
             {context}\n\n\
             ## Decision\n\n\
             {decision}\n\n\
             ## Rationale\n\n\
             {rationale}\n",
            title = entry.title,
            context = entry.context,
            decision = entry.decision,
            rationale = entry.rationale,
        );
        std::fs::write(&path, content)?;
        Ok(path)
    }

    /// 記録済みADR数を返す
    pub fn count(&self) -> Result<usize> {
        let count = std::fs::read_dir(&self.adr_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
            .count();
        Ok(count)
    }
}

/// ファイル名に使えない文字を除去し、スペースをハイフンに変換
fn sanitize_filename(title: &str) -> String {
    title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else if c == ' ' || c == ':' || c == '/' {
                '-'
            } else {
                // マルチバイト文字はそのまま保持
                if c.is_alphanumeric() { c } else { '-' }
            }
        })
        .collect::<String>()
        .replace("--", "-")
        .trim_matches('-')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(title: &str) -> AdrEntry {
        AdrEntry {
            title: title.to_string(),
            context: "ループ検出で停滞を検知".to_string(),
            decision: "再計画を実行".to_string(),
            rationale: "同一ツール3回連続失敗のため".to_string(),
        }
    }

    #[test]
    fn t_adr_record_creates_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let writer = AdrWriter::new(tmp.path()).unwrap();
        let entry = make_entry("Replan: ループ検出");
        let path = writer.record(&entry).unwrap();
        assert!(path.exists(), "ADRファイルが作成されること");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("# Replan: ループ検出"),
            "タイトルが含まれること"
        );
        assert!(
            content.contains("ループ検出で停滞を検知"),
            "コンテキストが含まれること"
        );
        assert!(content.contains("再計画を実行"), "判断が含まれること");
        assert!(
            content.contains("同一ツール3回連続失敗のため"),
            "根拠が含まれること"
        );
    }

    #[test]
    fn t_adr_record_appends() {
        let tmp = tempfile::TempDir::new().unwrap();
        let writer = AdrWriter::new(tmp.path()).unwrap();
        let p1 = writer.record(&make_entry("ADR-1")).unwrap();
        let p2 = writer.record(&make_entry("ADR-2")).unwrap();
        assert_ne!(p1, p2, "異なるファイルに記録されること");
        assert!(p1.exists());
        assert!(p2.exists());
    }

    #[test]
    fn t_adr_count() {
        let tmp = tempfile::TempDir::new().unwrap();
        let writer = AdrWriter::new(tmp.path()).unwrap();
        assert_eq!(writer.count().unwrap(), 0);
        writer.record(&make_entry("ADR-1")).unwrap();
        assert_eq!(writer.count().unwrap(), 1);
        writer.record(&make_entry("ADR-2")).unwrap();
        assert_eq!(writer.count().unwrap(), 2);
    }
}
