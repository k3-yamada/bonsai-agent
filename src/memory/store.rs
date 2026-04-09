use anyhow::Result;
use rusqlite::{params, Connection};

use crate::db::migrate;

/// SQLite統合ストア。A-MEMメモリ、セッション、経験、プロファイルを一元管理。
pub struct MemoryStore {
    conn: Connection,
}

impl MemoryStore {
    /// ファイルベースのDBを開く
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        let mut store = Self { conn };
        store.initialize()?;
        Ok(store)
    }

    /// インメモリDB（テスト用）
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let mut store = Self { conn };
        store.initialize()?;
        Ok(store)
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
        let result = self.conn.query_row(
            "SELECT MAX(version) FROM schema_version",
            [],
            |row| row.get::<_, u32>(0),
        );
        match result {
            Ok(v) => Ok(v),
            Err(_) => Ok(0),
        }
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    // --- メモリ CRUD ---

    /// メモリを保存
    pub fn save_memory(
        &self,
        content: &str,
        category: &str,
        tags: &[String],
    ) -> Result<i64> {
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
        let mut stmt = self.conn.prepare(
            "SELECT m.id, m.content, m.category, m.tags, m.access_count, m.created_at
             FROM memories_fts f
             JOIN memories m ON f.rowid = m.id
             WHERE memories_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![query, limit as i64], |row| {
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
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
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

    /// セッションを保存
    pub fn save_session(
        &self,
        session: &crate::agent::conversation::Session,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT OR REPLACE INTO sessions (id, created_at, updated_at, summary) VALUES (?1, ?2, ?3, ?4)",
            params![
                &session.id,
                session.created_at.to_rfc3339(),
                &now,
                &session.summary,
            ],
        )?;

        // 既存メッセージを削除して再挿入（簡易実装）
        self.conn.execute(
            "DELETE FROM messages WHERE session_id = ?1",
            params![&session.id],
        )?;

        for msg in &session.messages {
            let role = match msg.role {
                crate::agent::conversation::Role::System => "system",
                crate::agent::conversation::Role::User => "user",
                crate::agent::conversation::Role::Assistant => "assistant",
                crate::agent::conversation::Role::Tool => "tool",
            };
            self.conn.execute(
                "INSERT INTO messages (session_id, role, content, tool_call_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![&session.id, role, &msg.content, &msg.tool_call_id, &now],
            )?;
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

        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// セッションを読み込み
    pub fn load_session(&self, session_id: &str) -> Result<Option<crate::agent::conversation::Session>> {
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
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM memories",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
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
            .save_memory("Rust is a fast programming language", "fact", &["rust".to_string()])
            .unwrap();
        store
            .save_memory("Python is a scripting language", "fact", &["python".to_string()])
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
        assert_eq!(
            store.get_profile("lang").unwrap(),
            Some("en".to_string())
        );
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
}
