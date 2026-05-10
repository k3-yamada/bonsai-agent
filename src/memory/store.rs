use anyhow::Result;
use rusqlite::{Connection, params};

use crate::db::migrate;

#[cfg(feature = "embeddings")]
use crate::runtime::embedder::{DEFAULT_EMBEDDING_DIM, Embedder};
#[cfg(feature = "embeddings")]
use anyhow::bail;

/// vec0 SQL extension を process global に 1 度だけ自動ロード。
/// rusqlite の Connection::open は新規接続のため、auto_extension を
/// 事前登録しておくことで、以後すべての Connection で vec0 が使える。
#[cfg(feature = "embeddings")]
fn init_vec_extension() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // SAFETY: sqlite3_auto_extension は SQLite global registry への登録で
        // 副作用は他 Connection のロードのみ。sqlite3_vec_init はライブラリ
        // 提供の C 関数で、SQLite extension entrypoint signature 準拠。
        // transmute target は libsqlite3-sys (rusqlite ffi) が要求する型に合わせる。
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(
                    *mut rusqlite::ffi::sqlite3,
                    *mut *mut std::os::raw::c_char,
                    *const rusqlite::ffi::sqlite3_api_routines,
                ) -> std::os::raw::c_int,
            >(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
    });
}

/// SQLite統合ストア。A-MEMメモリ、セッション、経験、プロファイルを一元管理。
pub struct MemoryStore {
    conn: Connection,
    /// ファイルベースDBのパス（in-memoryはNone）。
    /// rusqlite Connectionは!Syncなため、並列実行時に別スレッドで
    /// 同一ファイルへ新しいConnectionを開くために保持する。
    path: Option<String>,
}

impl MemoryStore {
    /// ファイルベースのDBを開く
    pub fn open(path: &str) -> Result<Self> {
        // vec0 auto_extension は Connection::open より前に登録 (process 1 回限り)。
        #[cfg(feature = "embeddings")]
        init_vec_extension();
        let conn = Connection::open(path)?;
        let mut store = Self {
            conn,
            path: Some(path.to_string()),
        };
        store.initialize()?;
        Ok(store)
    }

    /// インメモリDB（テスト用）
    pub fn in_memory() -> Result<Self> {
        #[cfg(feature = "embeddings")]
        init_vec_extension();
        let conn = Connection::open_in_memory()?;
        let mut store = Self { conn, path: None };
        store.initialize()?;
        Ok(store)
    }

    /// ファイルパス（in-memoryならNone）
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// 並列実行用のConnectionクローン。
    /// file-backedならパスから新しいConnectionを開き、in-memoryならNoneを返す
    /// （in-memoryはプロセス内で共有できないため並列化不能）。
    /// SQLiteはWAL/rollbackジャーナルで複数Connectionから同時アクセス安全。
    pub fn try_clone_for_thread(&self) -> Option<Result<Self>> {
        self.path.as_ref().map(|p| Self::open(p))
    }

    /// スキーマの初期化/マイグレーション
    fn initialize(&mut self) -> Result<()> {
        let current = self.get_schema_version()?;
        let plan = migrate::plan_migrations(current);

        for version in &plan.migrations_to_apply {
            if let Some(sql) = migrate::get_migration_sql(*version) {
                self.conn.execute_batch(sql)?;
                self.conn.execute(
                    "INSERT OR REPLACE INTO schema_version (version) VALUES (?1)",
                    params![version],
                )?;
            }
        }
        Ok(())
    }

    fn get_schema_version(&self) -> Result<u32> {
        // schema_versionテーブルが存在しない場合は0
        let result = self
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get::<_, u32>(0)
            });
        match result {
            Ok(v) => Ok(v),
            Err(_) => Ok(0),
        }
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Lab cycle 専用のセッションデータリセット（ベンチマーク k 回実行 / task 切替で使用）
    ///
    /// **WARNING**: `messages` / `sessions` / `memories` を全 DELETE する破壊的操作。
    /// `events` / `experiences` / `skills` / `knowledge_graph` 等は保護される。
    /// Option A 移行 (項目 205) で persistent store に対しても呼ばれるようになったため、
    /// 名前で「Lab 限定の意図」を明示し誤用を防ぐ。bonsai-agent は実時間で
    /// persistent の messages/sessions/memories を使わない (Lab cycle 中のみ書込) ため
    /// 安全。将来 persistent.messages/sessions/memories を実時間活用する機能を追加する
    /// 場合は、本 method を呼ばず別 path で run-isolation を実装すること。
    pub fn reset_session_data_for_lab(&self) -> Result<()> {
        self.conn
            .execute_batch("DELETE FROM messages; DELETE FROM sessions; DELETE FROM memories;")?;
        Ok(())
    }

    /// 全テーブルの期限切れレコードを一括パージ
    pub fn purge_all_expired(&self) -> Result<usize> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut total = 0;
        total += self.conn.execute(
            "DELETE FROM memories WHERE expires_at IS NOT NULL AND expires_at < ?1",
            params![&now],
        )?;
        total += self.conn.execute(
            "DELETE FROM experiences WHERE expires_at IS NOT NULL AND expires_at < ?1",
            params![&now],
        )?;
        total += self.conn.execute(
            "DELETE FROM skills WHERE expires_at IS NOT NULL AND expires_at < ?1",
            params![&now],
        )?;
        Ok(total)
    }

    // --- メモリ CRUD ---

    /// メモリを保存
    pub fn save_memory(&self, content: &str, category: &str, tags: &[String]) -> Result<i64> {
        let tags_json = serde_json::to_string(tags)?;
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO memories (content, category, tags, created_at, accessed_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![content, category, &tags_json, &now, &now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// FTS5でメモリを検索
    pub fn search_memories(&self, query: &str, limit: usize) -> Result<Vec<MemoryRecord>> {
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let safe_query = format!("\"{}\"", query.replace('"', ""));
        let mut stmt = self.conn.prepare(
            "SELECT m.id, m.content, m.category, m.tags, m.access_count, m.created_at
             FROM memories_fts f
             JOIN memories m ON f.rowid = m.id
             WHERE memories_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![&safe_query, limit as i64], |row| {
            Ok(MemoryRecord {
                id: row.get(0)?,
                content: row.get(1)?,
                category: row.get(2)?,
                tags: row.get(3)?,
                access_count: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }

        // アクセスカウントを更新
        for r in &results {
            self.conn.execute(
                "UPDATE memories SET access_count = access_count + 1, accessed_at = ?1 WHERE id = ?2",
                params![chrono::Utc::now().to_rfc3339(), r.id],
            )?;
        }

        Ok(results)
    }

    /// 全メモリを取得（ベクトル検索のスキャン用）
    pub fn all_memories(&self) -> Result<Vec<MemoryRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content, category, tags, access_count, created_at FROM memories ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(MemoryRecord {
                id: row.get(0)?,
                content: row.get(1)?,
                category: row.get(2)?,
                tags: row.get(3)?,
                access_count: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// メモリ間リンクを作成
    pub fn link_memories(&self, source_id: i64, target_id: i64, relation: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO memory_links (source_id, target_id, relation) VALUES (?1, ?2, ?3)",
            params![source_id, target_id, relation],
        )?;
        Ok(())
    }

    // --- セッション ---

    /// セッションを保存（単一トランザクションでバッチ実行）
    pub fn save_session(&self, session: &crate::agent::conversation::Session) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();

        // 単一トランザクションで全操作を実行（N+2 fsync → 1 fsync）
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = self.save_session_inner(session, &now);
        if result.is_ok() {
            self.conn.execute_batch("COMMIT")?;
        } else {
            let _ = self.conn.execute_batch("ROLLBACK");
        }
        result
    }

    fn save_session_inner(
        &self,
        session: &crate::agent::conversation::Session,
        now: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO sessions (id, created_at, updated_at, summary) VALUES (?1, ?2, ?3, ?4)",
            params![
                &session.id,
                session.created_at.to_rfc3339(),
                now,
                &session.summary,
            ],
        )?;

        self.conn.execute(
            "DELETE FROM messages WHERE session_id = ?1",
            params![&session.id],
        )?;

        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO messages (session_id, role, content, tool_call_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for msg in &session.messages {
            let role = match msg.role {
                crate::agent::conversation::Role::System => "system",
                crate::agent::conversation::Role::User => "user",
                crate::agent::conversation::Role::Assistant => "assistant",
                crate::agent::conversation::Role::Tool => "tool",
            };
            stmt.execute(params![
                &session.id,
                role,
                &msg.content,
                &msg.tool_call_id,
                now
            ])?;
        }
        Ok(())
    }

    /// セッション一覧を取得（最新順）
    pub fn list_sessions(&self, limit: usize) -> Result<Vec<SessionSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.created_at, s.summary,
                    (SELECT content FROM messages WHERE session_id = s.id AND role = 'user' ORDER BY id LIMIT 1)
             FROM sessions s
             ORDER BY s.updated_at DESC
             LIMIT ?1",
        )?;

        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(SessionSummary {
                id: row.get(0)?,
                created_at: row.get(1)?,
                summary: row.get(2)?,
                first_user_message: row.get(3)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// セッションを読み込み
    pub fn load_session(
        &self,
        session_id: &str,
    ) -> Result<Option<crate::agent::conversation::Session>> {
        use crate::agent::conversation::{Message, Role, Session};

        let session_row = self.conn.query_row(
            "SELECT id, created_at, summary FROM sessions WHERE id = ?1",
            params![session_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        );

        let (id, created_at_str, summary) = match session_row {
            Ok(r) => r,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now());

        let mut stmt = self.conn.prepare(
            "SELECT role, content, tool_call_id FROM messages WHERE session_id = ?1 ORDER BY id",
        )?;

        let messages: Vec<Message> = stmt
            .query_map(params![&id], |row| {
                let role_str: String = row.get(0)?;
                let content: String = row.get(1)?;
                let tool_call_id: Option<String> = row.get(2)?;
                let role = match role_str.as_str() {
                    "system" => Role::System,
                    "user" => Role::User,
                    "assistant" => Role::Assistant,
                    _ => Role::Tool,
                };
                Ok(Message {
                    role,
                    content,
                    attachments: Vec::new(),
                    tool_call_id,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(Some(Session {
            id,
            messages,
            created_at,
            updated_at: chrono::Utc::now(),
            summary,
        }))
    }

    // --- ユーザープロファイル ---

    pub fn set_profile(&self, key: &str, value: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT OR REPLACE INTO user_profile (key, value, updated_at) VALUES (?1, ?2, ?3)",
            params![key, value, &now],
        )?;
        Ok(())
    }

    pub fn get_profile(&self, key: &str) -> Result<Option<String>> {
        let result = self.conn.query_row(
            "SELECT value FROM user_profile WHERE key = ?1",
            params![key],
            |row| row.get::<_, String>(0),
        );
        match result {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    // --- メモリメンテナンス ---

    /// 指定日数以上アクセスされていないメモリの数を返す
    pub fn count_stale_memories(&self, days: i64) -> Result<usize> {
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE accessed_at < ?1",
            params![&cutoff],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// 古い未使用メモリを削除
    pub fn purge_stale_memories(&self, days: i64, max_delete: usize) -> Result<usize> {
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();
        let deleted = self.conn.execute(
            "DELETE FROM memories WHERE id IN (
                SELECT id FROM memories
                WHERE accessed_at < ?1 AND access_count = 0
                ORDER BY accessed_at ASC
                LIMIT ?2
            )",
            params![&cutoff, max_delete as i64],
        )?;
        Ok(deleted)
    }

    /// メモリ総数
    pub fn memory_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    // --- sqlite-vec vec0 virtual table (plan T-1.1〜T-1.7、Phase 2 Green 実装) ---
    // V13 migration は initialize() 内で適用済 (open/in_memory 経由)。
    // すべて #[cfg(feature = "embeddings")] 配下、default build (= production)
    // では embeddings feature が default on のため可視。

    /// vec_memories の eager backfill。既に backfill 済 (count > 0) ならスキップ。
    /// 既存 memories 全件を embedder で 256d ベクトル化し vec_memories に投入。
    #[cfg(feature = "embeddings")]
    pub fn ensure_vec_table(&self, embedder: &dyn Embedder) -> Result<()> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM vec_memories", [], |row| row.get(0))?;
        if count > 0 {
            return Ok(());
        }
        let memories = self.all_memories()?;
        if memories.is_empty() {
            return Ok(());
        }
        let texts: Vec<&str> = memories.iter().map(|m| m.content.as_str()).collect();
        let embeddings = embedder.embed(&texts)?;
        let total = memories.len();
        for (i, (mem, emb)) in memories.iter().zip(embeddings.iter()).enumerate() {
            self.insert_memory_embedding(mem.id, emb)?;
            if (i + 1) % 100 == 0 {
                eprintln!("[ensure_vec_table] backfilled {}/{} memories", i + 1, total);
            }
        }
        Ok(())
    }

    /// 256d embedding を vec_memories に保存。次元不一致は Err (256d 厳格)。
    /// embedding は f32 little-endian で BLOB 化して vec0 に渡す。
    #[cfg(feature = "embeddings")]
    pub fn insert_memory_embedding(&self, memory_id: i64, embedding: &[f32]) -> Result<()> {
        if embedding.len() != DEFAULT_EMBEDDING_DIM {
            bail!(
                "embedding 次元不一致: expected {}, got {}",
                DEFAULT_EMBEDDING_DIM,
                embedding.len()
            );
        }
        let bytes: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
        self.conn.execute(
            "INSERT INTO vec_memories(memory_id, embedding) VALUES (?1, ?2)",
            params![memory_id, bytes],
        )?;
        Ok(())
    }

    /// vec0 KNN クエリ。距離昇順で最大 limit 件 (memory_id, distance) を返却。
    /// query は 256d 必須、それ以外は Err。
    #[cfg(feature = "embeddings")]
    pub fn vec_knn(&self, query: &[f32], limit: usize) -> Result<Vec<(i64, f32)>> {
        if query.len() != DEFAULT_EMBEDDING_DIM {
            bail!(
                "query 次元不一致: expected {}, got {}",
                DEFAULT_EMBEDDING_DIM,
                query.len()
            );
        }
        let bytes: Vec<u8> = query.iter().flat_map(|f| f.to_le_bytes()).collect();
        let mut stmt = self.conn.prepare(
            "SELECT memory_id, distance FROM vec_memories WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![bytes, limit as i64], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f32>(1)?))
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// IDs 指定の batch fetch (vector_search の N+1 回避用、plan G-2.4)。
    /// 空配列なら空 Vec、それ以外は IN clause で 1 クエリ取得。
    pub fn get_memories_by_ids(&self, ids: &[i64]) -> Result<Vec<MemoryRecord>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "SELECT id, content, category, tags, access_count, created_at FROM memories WHERE id IN ({placeholders})"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |row| {
            Ok(MemoryRecord {
                id: row.get(0)?,
                content: row.get(1)?,
                category: row.get(2)?,
                tags: row.get(3)?,
                access_count: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }
}

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: String,
    pub created_at: String,
    pub summary: Option<String>,
    pub first_user_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MemoryRecord {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub tags: String,
    pub access_count: i64,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::MIGRATIONS;

    fn test_store() -> MemoryStore {
        MemoryStore::in_memory().expect("インメモリDB作成に失敗")
    }

    #[test]
    fn test_initialize() {
        let store = test_store();
        let version = store.get_schema_version().unwrap();
        assert_eq!(version, MIGRATIONS.len() as u32);
    }

    #[test]
    fn test_save_and_search_memory() {
        let store = test_store();
        store
            .save_memory(
                "Rust is a fast programming language",
                "fact",
                &["rust".to_string()],
            )
            .unwrap();
        store
            .save_memory(
                "Python is a scripting language",
                "fact",
                &["python".to_string()],
            )
            .unwrap();

        let results = store.search_memories("Rust", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("Rust"));
    }

    #[test]
    fn test_search_updates_access_count() {
        let store = test_store();
        let id = store
            .save_memory("searchable keyword here", "fact", &[])
            .unwrap();

        // FTS5はトークナイザに依存するため英字でテスト
        store.search_memories("searchable", 10).unwrap();
        store.search_memories("searchable", 10).unwrap();

        let count: i64 = store
            .conn()
            .query_row(
                "SELECT access_count FROM memories WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_link_memories() {
        let store = test_store();
        let id1 = store.save_memory("A", "fact", &[]).unwrap();
        let id2 = store.save_memory("B", "fact", &[]).unwrap();

        store.link_memories(id1, id2, "related_to").unwrap();

        let count: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM memory_links WHERE source_id = ?1",
                params![id1],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_profile_set_get() {
        let store = test_store();
        store.set_profile("user_name", "keizo").unwrap();
        assert_eq!(
            store.get_profile("user_name").unwrap(),
            Some("keizo".to_string())
        );
    }

    #[test]
    fn test_profile_not_found() {
        let store = test_store();
        assert_eq!(store.get_profile("nonexistent").unwrap(), None);
    }

    #[test]
    fn test_profile_overwrite() {
        let store = test_store();
        store.set_profile("lang", "ja").unwrap();
        store.set_profile("lang", "en").unwrap();
        assert_eq!(store.get_profile("lang").unwrap(), Some("en".to_string()));
    }

    #[test]
    fn t_purge_all_expired() {
        let store = test_store();
        store.save_memory("test", "fact", &[]).unwrap();
        store
            .conn()
            .execute(
                "UPDATE memories SET expires_at = '2020-01-01T00:00:00Z' WHERE content = 'test'",
                [],
            )
            .unwrap();
        let deleted = store.purge_all_expired().unwrap();
        assert_eq!(deleted, 1);
    }

    #[test]
    fn t_purge_all_expired_no_expired() {
        let store = test_store();
        store.save_memory("fresh", "fact", &[]).unwrap();
        let deleted = store.purge_all_expired().unwrap();
        assert_eq!(deleted, 0);
    }

    #[test]
    fn test_memory_count() {
        let store = test_store();
        assert_eq!(store.memory_count().unwrap(), 0);
        store.save_memory("A", "fact", &[]).unwrap();
        store.save_memory("B", "skill", &[]).unwrap();
        assert_eq!(store.memory_count().unwrap(), 2);
    }

    #[test]
    fn test_save_session_transactional() {
        // セッション保存がトランザクション内で実行され、差分更新されることを検証
        use crate::agent::conversation::{Message, Session};
        let store = test_store();
        let mut session = Session::new();
        session.messages.push(Message::user("msg1"));
        session.messages.push(Message::user("msg2"));

        // 初回保存
        store.save_session(&session).unwrap();
        let loaded = store.load_session(&session.id).unwrap().unwrap();
        assert_eq!(loaded.messages.len(), 2);

        // メッセージ追加後の再保存で正しく更新される
        session.messages.push(Message::user("msg3"));
        store.save_session(&session).unwrap();
        let loaded = store.load_session(&session.id).unwrap().unwrap();
        assert_eq!(loaded.messages.len(), 3);
        assert_eq!(loaded.messages[2].content, "msg3");
    }

    #[test]
    fn test_save_session_idempotent() {
        // 同一セッションを複数回保存しても重複しない
        use crate::agent::conversation::{Message, Session};
        let store = test_store();
        let mut session = Session::new();
        session.messages.push(Message::user("hello"));

        store.save_session(&session).unwrap();
        store.save_session(&session).unwrap();
        store.save_session(&session).unwrap();

        let count: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
                params![&session.id],
                |row| row.get(0),
            )
            .unwrap();
        // 3回保存しても1メッセージのまま
        assert_eq!(count, 1);
    }

    #[test]
    fn test_purge_stale() {
        let store = test_store();
        // 手動で古いメモリを挿入
        store
            .conn()
            .execute(
                "INSERT INTO memories (content, category, tags, created_at, accessed_at, access_count) VALUES (?1, ?2, '[]', ?3, ?3, 0)",
                params!["古いメモリ", "fact", "2020-01-01T00:00:00Z"],
            )
            .unwrap();

        let count = store.count_stale_memories(1).unwrap();
        assert_eq!(count, 1);

        let deleted = store.purge_stale_memories(1, 100).unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(store.memory_count().unwrap(), 0);
    }

    #[test]
    fn test_reset_session_data_for_lab() {
        let store = test_store();

        // メモリを挿入
        store
            .save_memory("test memory", "fact", &["test".to_string()])
            .unwrap();
        assert!(store.memory_count().unwrap() > 0);

        // リセットでデータがクリアされる
        store.reset_session_data_for_lab().unwrap();
        assert_eq!(store.memory_count().unwrap(), 0);

        // リセット後もスキーマは正常（新規データ保存可能）
        store
            .save_memory("after reset", "fact", &["test".to_string()])
            .unwrap();
        assert_eq!(store.memory_count().unwrap(), 1);
    }

    // --- Phase 1 Red: sqlite-vec vec0 (plan T-1.1〜T-1.7) ---
    // すべて #[cfg(feature = "embeddings")] 配下 (T-1.4 は SCHEMA_VERSION 比較で非 gate)。
    // Phase 1 Red 段階では todo!() panic または assert fail で Red 確証。
    // Phase 2 Green で全 PASS 化、T-1.6 (既存 6 search test) は signature 不変で
    // pass 維持を gate (本 module 外、search.rs:168-219 で検証)。

    #[cfg(feature = "embeddings")]
    #[test]
    fn t_1_1_vec_memories_virtual_table_exists_after_ensure() {
        use crate::runtime::embedder::SimpleEmbedder;
        let store = test_store();
        let embedder = SimpleEmbedder::default();
        store.ensure_vec_table(&embedder).unwrap();
        let count: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='vec_memories'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "vec_memories virtual table が ensure 後に存在すべき"
        );
    }

    #[cfg(feature = "embeddings")]
    #[test]
    fn t_1_2_insert_memory_embedding_persists_256d() {
        use crate::runtime::embedder::{DEFAULT_EMBEDDING_DIM, SimpleEmbedder};
        let store = test_store();
        let embedder = SimpleEmbedder::default();
        store.ensure_vec_table(&embedder).unwrap();
        let mem_id = store.save_memory("test", "fact", &[]).unwrap();
        let emb = vec![0.1f32; DEFAULT_EMBEDDING_DIM];
        store.insert_memory_embedding(mem_id, &emb).unwrap();
        let count: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM vec_memories WHERE memory_id = ?1",
                params![mem_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "256d embedding が vec_memories に保存されるべき");
    }

    #[cfg(feature = "embeddings")]
    #[test]
    fn t_1_3_vec_knn_returns_top_k_distance_order() {
        use crate::runtime::embedder::{DEFAULT_EMBEDDING_DIM, SimpleEmbedder};
        let store = test_store();
        let embedder = SimpleEmbedder::default();
        store.ensure_vec_table(&embedder).unwrap();
        // 5 件 insert: i=0 が query にもっとも近接 (first dim のみ変化)。
        for i in 0..5 {
            let mem_id = store.save_memory(&format!("doc {i}"), "fact", &[]).unwrap();
            let mut emb = vec![0.0f32; DEFAULT_EMBEDDING_DIM];
            emb[0] = (i as f32) * 0.1;
            store.insert_memory_embedding(mem_id, &emb).unwrap();
        }
        let query = vec![0.0f32; DEFAULT_EMBEDDING_DIM];
        let results = store.vec_knn(&query, 3).unwrap();
        assert_eq!(results.len(), 3, "top-3 件返却すべき");
        // distance は昇順 (単調非減少)。
        for w in results.windows(2) {
            assert!(
                w[0].1 <= w[1].1,
                "distance 昇順違反: {} > {}",
                w[0].1,
                w[1].1
            );
        }
    }

    #[test]
    fn t_1_4_schema_version_is_v13_for_vec_memories() {
        use crate::db::schema::SCHEMA_VERSION;
        assert_eq!(
            SCHEMA_VERSION, 13,
            "V13 migration が vec_memories virtual table を追加するため SCHEMA_VERSION=13 になるべき (Phase 2 Green で適用)"
        );
    }

    #[cfg(feature = "embeddings")]
    #[test]
    fn t_1_5_ensure_vec_table_eager_backfill_existing_memories() {
        use crate::runtime::embedder::SimpleEmbedder;
        let store = test_store();
        // ensure 前に 3 件 memories を投入。
        for i in 0..3 {
            store.save_memory(&format!("mem {i}"), "fact", &[]).unwrap();
        }
        let embedder = SimpleEmbedder::default();
        store.ensure_vec_table(&embedder).unwrap();
        let count: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM vec_memories", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count, 3,
            "eager backfill が既存 memories 全件を vec_memories に投入すべき"
        );
    }

    #[cfg(feature = "embeddings")]
    #[test]
    fn t_1_7_insert_memory_embedding_rejects_non_256d() {
        use crate::runtime::embedder::SimpleEmbedder;
        let store = test_store();
        let embedder = SimpleEmbedder::default();
        store.ensure_vec_table(&embedder).unwrap();
        let mem_id = store.save_memory("test", "fact", &[]).unwrap();
        let bad_emb = vec![0.0f32; 128];
        let result = store.insert_memory_embedding(mem_id, &bad_emb);
        assert!(
            result.is_err(),
            "256d 以外の embedding は insert 拒否すべき (128d 入力)"
        );
    }
}
