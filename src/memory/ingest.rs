//! ファイル取り込み (ingest): ローカルのメモ/文書を memory に投入する。
//!
//! ①パーソナル知識デーモン Phase 2。`remember` がユーザー発話を1件保存するのに対し、
//! ingest は既存のファイル群 (Markdown/テキストのメモ) を chunk して一括投入し、
//! recall で想起できる知識量を供給する value multiplier。

use crate::memory::store::MemoryStore;
use anyhow::Result;
use std::path::Path;

/// 取り込み対象とする拡張子。
const INGEST_EXTENSIONS: &[&str] = &["md", "txt", "markdown", "text"];

/// テキストを段落 (空行区切り) 単位の chunk に分割する。
/// 各 chunk は trim され、空 chunk は除外される。
pub fn chunk_text(content: &str) -> Vec<String> {
    content
        .split("\n\n")
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
        .collect()
}

/// パス (ファイル or ディレクトリ) を memory に取り込む。返り値は保存した chunk 数。
///
/// - ファイル: 対象拡張子なら chunk して各段落を save_memory。
/// - ディレクトリ: 直下の対象拡張子ファイルを再帰的に取り込む。
///
/// 各 chunk は category="ingest"、tag にファイル名を付与する。
pub fn ingest_path(store: &MemoryStore, path: &Path) -> Result<usize> {
    if path.is_dir() {
        let mut total = 0;
        let mut entries: Vec<_> = std::fs::read_dir(path)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        entries.sort();
        for entry in entries {
            total += ingest_path(store, &entry)?;
        }
        Ok(total)
    } else if path.is_file() && has_ingest_extension(path) {
        ingest_file(store, path)
    } else {
        Ok(0)
    }
}

/// 単一ファイルを取り込む。
fn ingest_file(store: &MemoryStore, path: &Path) -> Result<usize> {
    let content = std::fs::read_to_string(path)?;
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();
    let chunks = chunk_text(&content);
    let mut saved = 0;
    for chunk in &chunks {
        store.save_memory(chunk, "ingest", std::slice::from_ref(&filename))?;
        saved += 1;
    }
    Ok(saved)
}

fn has_ingest_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| INGEST_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_chunk_text_splits_paragraphs() {
        let chunks = chunk_text("first para\n\nsecond para\n\nthird");
        assert_eq!(chunks, vec!["first para", "second para", "third"]);
    }

    #[test]
    fn t_chunk_text_trims_and_drops_empty() {
        let chunks = chunk_text("  a  \n\n\n\n  \n\n b ");
        assert_eq!(chunks, vec!["a", "b"]);
    }

    #[test]
    fn t_chunk_text_empty_input() {
        assert!(chunk_text("   \n\n  ").is_empty());
    }

    #[test]
    fn t_has_ingest_extension() {
        assert!(has_ingest_extension(Path::new("notes.md")));
        assert!(has_ingest_extension(Path::new("a.TXT")));
        assert!(!has_ingest_extension(Path::new("photo.png")));
        assert!(!has_ingest_extension(Path::new("noext")));
    }

    #[test]
    fn t_ingest_file_saves_chunks_and_recallable() {
        let store = MemoryStore::in_memory().unwrap();
        let dir = std::env::temp_dir();
        let fpath = dir.join(format!("bonsai_ingest_test_{}.md", std::process::id()));
        std::fs::write(&fpath, "Rust is fast\n\nコーヒーが好き").unwrap();

        let n = ingest_path(&store, &fpath).unwrap();
        assert_eq!(n, 2, "2 段落が保存されるべき");

        // FTS5 で取り込んだ内容が想起できる
        let hits = store.search_memories("Rust", 5).unwrap();
        assert!(
            hits.iter().any(|m| m.content.contains("Rust is fast")),
            "ingest した chunk が検索できるべき"
        );
        let _ = std::fs::remove_file(&fpath);
    }

    #[test]
    fn t_ingest_dir_recurses_target_files_only() {
        let store = MemoryStore::in_memory().unwrap();
        let base = std::env::temp_dir().join(format!("bonsai_ingest_dir_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("a.md"), "alpha note").unwrap();
        std::fs::write(base.join("b.txt"), "beta note").unwrap();
        std::fs::write(base.join("c.png"), "not text").unwrap();

        let n = ingest_path(&store, &base).unwrap();
        assert_eq!(n, 2, "対象拡張子 2 ファイルのみ取り込む (.png 除外)");
        let _ = std::fs::remove_dir_all(&base);
    }
}
