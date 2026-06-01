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
/// 編集追従 sync: 当該 filename タグの既存 ingest chunk を現ファイルの段落集合と
/// 突き合わせ、(a) 現ファイルに無い既存 chunk を purge (編集/削除された段落)、
/// (b) 既存に無い現 chunk を新規保存、(c) 不変 chunk は維持する。
///
/// これにより未変更ファイルの再 ingest は冪等 (0 新規・0 削除)、編集ファイルは
/// 旧版 chunk が残存せず最新状態を反映する。purge は `memories_ad` トリガで
/// FTS index も同期削除される。
fn ingest_file(store: &MemoryStore, path: &Path) -> Result<usize> {
    let content = std::fs::read_to_string(path)?;
    // ファイル名は per-file sync のキー。取得不能 (非 UTF-8 等) なら全ファイルが
    // 同一タグに退化し他ファイルを誤 purge し得るため bail する (ecc review MEDIUM)。
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("ingest: ファイル名を取得できません: {}", path.display()))?
        .to_string();
    let chunks = chunk_text(&content);

    // purge + add を 1 トランザクションで原子的に行う。途中エラー時は ROLLBACK し、
    // 「旧 chunk 削除済・新 chunk 未保存」の部分状態によるデータ損失を防ぐ
    // (ecc review HIGH)。
    store.conn().execute_batch("BEGIN")?;
    match sync_file_chunks(store, &filename, &chunks) {
        Ok(saved) => {
            store.conn().execute_batch("COMMIT")?;
            Ok(saved)
        }
        Err(e) => {
            let _ = store.conn().execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// 当該 filename の既存 ingest chunk を現ファイル段落と突き合わせ purge/add する。
/// 返り値は新規保存した chunk 数。呼出側 (`ingest_file`) のトランザクション内で実行される。
fn sync_file_chunks(store: &MemoryStore, filename: &str, chunks: &[String]) -> Result<usize> {
    use std::collections::HashSet;
    let current: HashSet<&str> = chunks.iter().map(String::as_str).collect();

    let existing = existing_ingest_chunks(store, filename)?;
    let existing_contents: HashSet<&str> = existing.iter().map(|(_, c)| c.as_str()).collect();

    // (a) purge: 現ファイルに存在しない既存 chunk を削除 (memories_ad トリガで FTS も同期)。
    for (id, c) in &existing {
        if !current.contains(c.as_str()) {
            store
                .conn()
                .execute("DELETE FROM memories WHERE id = ?1", rusqlite::params![id])?;
        }
    }
    // (b) add: 既存に無い現 chunk のみ保存 ((c) 不変 chunk は再保存しない = 冪等)。
    let tags = [filename.to_string()];
    let mut saved = 0;
    for chunk in chunks {
        if !existing_contents.contains(chunk.as_str()) {
            store.save_memory(chunk, "ingest", &tags)?;
            saved += 1;
        }
    }
    Ok(saved)
}

/// 指定 filename タグを持つ category='ingest' の既存 chunk (id, content) を返す。
/// tags は `save_memory` が `serde_json::to_string(&[filename])` で直列化した
/// JSON 配列文字列 (例 `["notes.md"]`)。同じ直列化で expected tag を再構築し
/// **完全一致** (`tags = ?`) で照合することで、LIKE substring の wildcard/JSON
/// アンカー破綻 (filename 中の `_`/`%`/`"` 等による他ファイル誤マッチ) を排除する
/// (ecc review CRITICAL)。
fn existing_ingest_chunks(store: &MemoryStore, filename: &str) -> Result<Vec<(i64, String)>> {
    let tag_json = serde_json::to_string(&[filename])?;
    let mut stmt = store
        .conn()
        .prepare("SELECT id, content FROM memories WHERE category = 'ingest' AND tags = ?1")?;
    let rows = stmt.query_map(rusqlite::params![tag_json], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn has_ingest_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| INGEST_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// root 配下に現存する ingest 対象ファイルの basename 集合を返す。
/// `ingest_path` と同じ traversal ルール (隠しdir 再帰除外・対象拡張子のみ)。
pub fn collect_ingest_filenames(root: &Path) -> Result<std::collections::HashSet<String>> {
    let mut set = std::collections::HashSet::new();
    collect_ingest_filenames_into(root, &mut set)?;
    Ok(set)
}

fn collect_ingest_filenames_into(
    path: &Path,
    set: &mut std::collections::HashSet<String>,
) -> Result<()> {
    if path.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(path)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| !n.starts_with('.'))
                    .unwrap_or(false)
            })
            .collect();
        entries.sort();
        for entry in entries {
            collect_ingest_filenames_into(&entry, set)?;
        }
    } else if path.is_file()
        && has_ingest_extension(path)
        && let Some(name) = path.file_name().and_then(|n| n.to_str())
    {
        // file 判定は `ingest_path` の file arm と対称 (隠しdir 除外 + 対象拡張子)。
        // 片方を変更したら他方も意識的に揃えること (現存判定の不一致は誤 purge を招く)。
        set.insert(name.to_string());
    }
    Ok(())
}

/// `existing` に含まれない filename タグを持つ ingest chunk を全削除し削除数を返す。
/// DB のみ操作 (filesystem 非依存でテスト容易)。purge は 1 トランザクションで原子化。
/// tag は `["filename"]` 形式 (ingest_file が付与)。parse 不能/空タグは安全側で温存する。
pub fn purge_orphans(
    store: &MemoryStore,
    existing: &std::collections::HashSet<String>,
) -> Result<usize> {
    // 全 ingest chunk の distinct tag を取得し、現存 filename に無いものを削除対象に。
    let orphan_tags: Vec<String> = {
        let mut stmt = store
            .conn()
            .prepare("SELECT DISTINCT tags FROM memories WHERE category = 'ingest'")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut tags = Vec::new();
        for r in rows {
            let tag_json = r?;
            // tag は ["filename"] 形式。parse 不能/空タグは安全側で温存 (削除しない)。
            match serde_json::from_str::<Vec<String>>(&tag_json) {
                Ok(arr) => {
                    if let Some(fname) = arr.first()
                        && !existing.contains(fname)
                    {
                        tags.push(tag_json);
                    }
                }
                Err(_) => continue,
            }
        }
        tags
    };
    if orphan_tags.is_empty() {
        return Ok(0);
    }
    // 削除を 1 トランザクションで原子化 (memories_ad トリガで FTS も同期削除)。
    store.conn().execute_batch("BEGIN")?;
    let result = (|| -> Result<usize> {
        let mut purged = 0;
        for tag_json in &orphan_tags {
            purged += store.conn().execute(
                "DELETE FROM memories WHERE category = 'ingest' AND tags = ?1",
                rusqlite::params![tag_json],
            )?;
        }
        Ok(purged)
    })();
    match result {
        Ok(purged) => {
            store.conn().execute_batch("COMMIT")?;
            Ok(purged)
        }
        Err(e) => {
            let _ = store.conn().execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// root を走査して現存ファイル集合を作り、削除済ファイルの孤児 ingest chunk を purge する。
/// `ingest_file` の編集追従 (既存ファイルの段落 sync) を補完し、ファイル削除にも追従させる。
///
/// 注意: タグは basename のみで保持するため、別 subdir に同名ファイル
/// (例 `2025/note.md` と `2026/note.md`) が存在すると 1 ファイル扱いになる。
/// その場合、片方を削除しても basename が現存集合に残るため orphan 掃除は抑止される
/// (安全側 = 誤削除しない方向の挙動)。
pub fn reconcile_ingested_files(store: &MemoryStore, root: &Path) -> Result<usize> {
    let existing = collect_ingest_filenames(root)?;
    purge_orphans(store, &existing)
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
    fn t_ingest_follows_edits_purges_stale() {
        // ファイル編集後の再 ingest で旧 chunk を purge し新 chunk へ置換する (編集追従)。
        // 旧実装は global content dedup のみで旧 chunk が残存 → Red。
        let store = MemoryStore::in_memory().unwrap();
        let dir = std::env::temp_dir();
        let fpath = dir.join(format!(
            "bonsai_ingest_edit_{}_{}.md",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&fpath, "keep para\n\nold para").unwrap();
        assert_eq!(ingest_path(&store, &fpath).unwrap(), 2, "初回 2 段落");

        // old para → new para に編集 (keep para は不変)。
        std::fs::write(&fpath, "keep para\n\nnew para").unwrap();
        let saved = ingest_path(&store, &fpath).unwrap();
        assert_eq!(saved, 1, "新規は new para の 1 件のみ");
        assert!(
            store.search_memories("old para", 5).unwrap().is_empty(),
            "旧 chunk は purge され想起されないべき"
        );
        assert!(
            !store.search_memories("new para", 5).unwrap().is_empty(),
            "新 chunk は想起できるべき"
        );
        assert!(
            !store.search_memories("keep para", 5).unwrap().is_empty(),
            "不変 chunk は維持されるべき"
        );
        assert_eq!(store.memory_count().unwrap(), 2, "総数は 2 (keep + new)");
        let _ = std::fs::remove_file(&fpath);
    }

    #[test]
    fn t_ingest_filename_like_wildcard_isolation() {
        // filename の `_` が LIKE wildcard として誤マッチし他ファイルの chunk を
        // purge しないこと (escape 検証)。a_.md 再 ingest が a1.md を巻き込まない。
        let store = MemoryStore::in_memory().unwrap();
        let base = std::env::temp_dir().join(format!(
            "bonsai_ingest_wild_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("a1.md"), "file one content").unwrap();
        std::fs::write(base.join("a_.md"), "file two content").unwrap();
        assert_eq!(ingest_path(&store, &base).unwrap(), 2, "2 ファイル取り込み");

        // a_.md のみ編集して再 ingest。
        std::fs::write(base.join("a_.md"), "file two edited").unwrap();
        ingest_path(&store, &base.join("a_.md")).unwrap();
        assert!(
            !store
                .search_memories("file one content", 5)
                .unwrap()
                .is_empty(),
            "a1.md の chunk は wildcard 誤マッチで purge されないべき"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn t_ingest_filename_with_quote_exact_match() {
        // filename に `"` を含んでも自身の chunk のみ同期する (exact JSON tag match)。
        // LIKE substring match では JSON アンカー (") が壊れ他ファイルを誤 purge し得る
        // (ecc review CRITICAL)。exact match なら serde の quote escape が両側で整合。
        let store = MemoryStore::in_memory().unwrap();
        let base = std::env::temp_dir().join(format!(
            "bonsai_ingest_quote_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("normal.md"), "normal file content").unwrap();
        std::fs::write(base.join("q\"x.md"), "quote file original").unwrap();
        assert_eq!(ingest_path(&store, &base).unwrap(), 2, "2 ファイル取り込み");

        // quote ファイルのみ編集して再 ingest。
        std::fs::write(base.join("q\"x.md"), "quote file edited").unwrap();
        ingest_path(&store, &base.join("q\"x.md")).unwrap();
        assert!(
            !store
                .search_memories("normal file content", 5)
                .unwrap()
                .is_empty(),
            "他ファイル (normal.md) の chunk は維持されるべき"
        );
        assert!(
            store
                .search_memories("quote file original", 5)
                .unwrap()
                .is_empty(),
            "編集前の chunk は purge されるべき"
        );
        assert!(
            !store
                .search_memories("quote file edited", 5)
                .unwrap()
                .is_empty(),
            "編集後の chunk は想起できるべき"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn t_purge_orphans_removes_deleted_file_chunks() {
        // existing に無い filename タグの ingest chunk のみ削除、非 ingest は温存 (DB のみ)。
        let store = MemoryStore::in_memory().unwrap();
        store
            .save_memory("alpha body", "ingest", &["a.md".to_string()])
            .unwrap();
        store
            .save_memory("beta body", "ingest", &["b.md".to_string()])
            .unwrap();
        store
            .save_memory("manual fact", "fact", &["note".to_string()])
            .unwrap();
        let existing: std::collections::HashSet<String> =
            ["a.md".to_string()].into_iter().collect();

        let purged = purge_orphans(&store, &existing).unwrap();
        assert_eq!(purged, 1, "孤児 b.md の 1 chunk のみ削除");
        assert!(
            !store.search_memories("alpha body", 5).unwrap().is_empty(),
            "現存 a.md は温存"
        );
        assert!(
            store.search_memories("beta body", 5).unwrap().is_empty(),
            "孤児 b.md は purge"
        );
        assert!(
            !store.search_memories("manual fact", 5).unwrap().is_empty(),
            "非 ingest は温存"
        );
    }

    #[test]
    fn t_reconcile_purges_orphans_after_file_deletion() {
        // ファイル削除後の reconcile で孤児 chunk を掃除する (編集追従の補完)。
        let store = MemoryStore::in_memory().unwrap();
        let base = std::env::temp_dir().join(format!(
            "bonsai_reconcile_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("a.md"), "alpha body").unwrap();
        std::fs::write(base.join("b.md"), "beta body").unwrap();
        assert_eq!(ingest_path(&store, &base).unwrap(), 2, "2 ファイル取り込み");

        // b.md をディスクから削除して reconcile。
        std::fs::remove_file(base.join("b.md")).unwrap();
        let purged = reconcile_ingested_files(&store, &base).unwrap();
        assert_eq!(purged, 1, "削除された b.md の chunk を掃除");
        assert!(
            !store.search_memories("alpha body", 5).unwrap().is_empty(),
            "現存 a.md は温存"
        );
        assert!(
            store.search_memories("beta body", 5).unwrap().is_empty(),
            "削除済 b.md は purge"
        );
        let _ = std::fs::remove_dir_all(&base);
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
