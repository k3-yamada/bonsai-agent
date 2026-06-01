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
            // 隠しエントリ (.obsidian/.git/.claude 等の設定 dir、.DS_Store 等) は
            // 個人知識ではないため再帰取り込みから除外する。
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| !n.starts_with('.'))
                    .unwrap_or(false)
            })
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

/// 単一ファイルを取り込む。返り値は**新規**保存した chunk 数。
///
/// 冪等性: content が完全一致する既存メモリがあれば保存をスキップする。
/// これにより `--ingest` の再実行で recall が重複汚染されない。
/// 注意: ファイル編集で段落内容が変わった場合、旧版 chunk は残存する
/// (content 一致 dedup のため)。完全な置換が必要なら別途 purge が必要。
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
        if content_exists(store, chunk)? {
            continue;
        }
        store.save_memory(chunk, "ingest", std::slice::from_ref(&filename))?;
        saved += 1;
    }
    Ok(saved)
}

/// content が完全一致する既存メモリの有無を返す (ingest 冪等化用)。
fn content_exists(store: &MemoryStore, content: &str) -> Result<bool> {
    let n: i64 = store.conn().query_row(
        "SELECT COUNT(*) FROM memories WHERE content = ?1",
        rusqlite::params![content],
        |row| row.get(0),
    )?;
    Ok(n > 0)
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
    fn t_ingest_skips_hidden_dirs() {
        // 隠しディレクトリ (.obsidian/.git/.claude 等の設定 dir) は再帰取り込みしない。
        let store = MemoryStore::in_memory().unwrap();
        let base = std::env::temp_dir().join(format!(
            "bonsai_ingest_hidden_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join(".hidden")).unwrap();
        std::fs::write(base.join("visible.md"), "visible knowledge").unwrap();
        std::fs::write(
            base.join(".hidden").join("config.md"),
            "hidden config noise",
        )
        .unwrap();

        let n = ingest_path(&store, &base).unwrap();
        assert_eq!(n, 1, "隠しdir内は取り込まず visible.md の 1 件のみ");
        assert!(
            store.search_memories("config", 5).unwrap().is_empty(),
            "隠しdir内の content は保存されないべき"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn t_ingest_idempotent_skips_duplicate_chunks() {
        // 同一ファイルの再 ingest は重複 chunk を保存しない (content 完全一致 dedup)。
        let store = MemoryStore::in_memory().unwrap();
        let dir = std::env::temp_dir();
        let fpath = dir.join(format!(
            "bonsai_ingest_idem_{}_{}.md",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&fpath, "alpha note\n\nbeta note").unwrap();

        let first = ingest_path(&store, &fpath).unwrap();
        assert_eq!(first, 2, "初回は 2 段落保存");
        let second = ingest_path(&store, &fpath).unwrap();
        assert_eq!(second, 0, "再 ingest は重複を保存しない (新規 0 件)");
        assert_eq!(store.memory_count().unwrap(), 2, "総数は 2 のまま");
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
