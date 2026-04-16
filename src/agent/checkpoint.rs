use anyhow::Result;
use rusqlite::{Connection, params};
use std::process::Command;

/// チェックポイント1件（メタデータ）
#[derive(Debug, Clone)]
pub struct Checkpoint {
    pub id: i64,
    pub description: String,
    pub git_ref: Option<String>,
    pub timestamp: String,
    /// ロールバック実行時刻（DB由来エントリのみ。インメモリでは常にNone）
    pub rolled_back_at: Option<String>,
}

/// チェックポイントマネージャ
///
/// `new()` でインメモリのみ、`with_persistence(conn, session_id)` で SQLite 永続化。
/// 永続化モードでは process 再起動後も `load_persisted()` で復元可能。
pub struct CheckpointManager<'a> {
    cps: Vec<Checkpoint>,
    ctr_inmem: i64, // インメモリモード用カウンタ（負の値で永続IDと衝突回避）
    conn: Option<&'a Connection>,
    session_id: Option<String>,
}

impl<'a> CheckpointManager<'a> {
    /// インメモリのみのマネージャ（プロセス終了で消失）
    pub fn new() -> Self {
        Self {
            cps: Vec::new(),
            ctr_inmem: -1, // -1 から減少（DB自動採番1+とは衝突しない）
            conn: None,
            session_id: None,
        }
    }

    /// 永続化モード: 全 create/rollback を SQLite に記録
    pub fn with_persistence(conn: &'a Connection, session_id: Option<String>) -> Self {
        Self {
            cps: Vec::new(),
            ctr_inmem: -1,
            conn: Some(conn),
            session_id,
        }
    }

    /// DB から既存チェックポイント履歴をロード（プロセス再起動後の復元用）
    pub fn load_persisted(
        conn: &'a Connection,
        session_id: Option<&str>,
    ) -> Result<Vec<Checkpoint>> {
        let mut stmt = match session_id {
            Some(_) => conn.prepare(
                "SELECT id, description, git_ref, timestamp, rolled_back_at
                 FROM checkpoints WHERE session_id = ?1 ORDER BY id",
            )?,
            None => conn.prepare(
                "SELECT id, description, git_ref, timestamp, rolled_back_at
                 FROM checkpoints ORDER BY id",
            )?,
        };
        let map_row = |row: &rusqlite::Row<'_>| {
            Ok(Checkpoint {
                id: row.get(0)?,
                description: row.get(1)?,
                git_ref: row.get(2)?,
                timestamp: row.get(3)?,
                rolled_back_at: row.get(4)?,
            })
        };
        let rows: Vec<Checkpoint> = match session_id {
            Some(sid) => stmt
                .query_map(params![sid], map_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?,
            None => stmt
                .query_map([], map_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?,
        };
        Ok(rows)
    }

    /// チェックポイントを作成し、git stash と DB（設定時）に記録
    pub fn create(&mut self, desc: &str) -> Result<i64> {
        let now = chrono::Utc::now().to_rfc3339();
        let git_ref = if is_git() {
            let o = Command::new("git")
                .args([
                    "stash",
                    "push",
                    "-m",
                    &format!("bonsai-cp-{desc}"),
                    "--include-untracked",
                ])
                .output()?;
            if o.status.success() && !String::from_utf8_lossy(&o.stdout).contains("No local changes")
            {
                Some(format!("stash@{{{}}}", self.cps.len()))
            } else {
                None
            }
        } else {
            None
        };

        let id = if let Some(conn) = self.conn {
            conn.execute(
                "INSERT INTO checkpoints (session_id, description, git_ref, timestamp)
                 VALUES (?1, ?2, ?3, ?4)",
                params![self.session_id.as_deref(), desc, git_ref.as_deref(), &now],
            )?;
            conn.last_insert_rowid()
        } else {
            let id = self.ctr_inmem;
            self.ctr_inmem -= 1;
            id
        };

        self.cps.push(Checkpoint {
            id,
            description: desc.into(),
            git_ref,
            timestamp: now,
            rolled_back_at: None,
        });
        Ok(id)
    }

    /// 指定IDのチェックポイントへロールバック
    pub fn rollback(&self, id: i64) -> Result<bool> {
        let cp = self
            .cps
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| anyhow::anyhow!("CP {id} not found"))?;
        let success = if let Some(r) = &cp.git_ref {
            let _ = Command::new("git").args(["checkout", "."]).output();
            Command::new("git")
                .args(["stash", "apply", r])
                .output()?
                .status
                .success()
        } else if is_git() {
            let _ = Command::new("git").args(["checkout", "."]).output();
            true
        } else {
            false
        };
        // DB 永続化モードならロールバック時刻を記録
        if let Some(conn) = self.conn {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE checkpoints SET rolled_back_at = ?1 WHERE id = ?2",
                params![&now, id],
            )?;
        }
        Ok(success)
    }

    pub fn rollback_last(&self) -> Result<bool> {
        self.cps
            .last()
            .map(|c| self.rollback(c.id))
            .unwrap_or_else(|| Err(anyhow::anyhow!("no cp")))
    }

    pub fn list(&self) -> &[Checkpoint] {
        &self.cps
    }

    pub fn count(&self) -> usize {
        self.cps.len()
    }
}

impl Default for CheckpointManager<'_> {
    fn default() -> Self {
        Self::new()
    }
}

fn is_git() -> bool {
    Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::MemoryStore;

    #[test]
    fn t_create() {
        let mut m = CheckpointManager::new();
        let id = m.create("t").unwrap();
        assert!(id < 0, "インメモリIDは負");
        assert_eq!(m.count(), 1);
    }

    #[test]
    fn t_multi() {
        let mut m = CheckpointManager::new();
        m.create("a").unwrap();
        m.create("b").unwrap();
        assert_eq!(m.count(), 2);
    }

    #[test]
    fn t_git() {
        assert!(is_git());
    }

    #[test]
    fn t_rb_err() {
        assert!(CheckpointManager::new().rollback(99).is_err());
    }

    #[test]
    fn t_rb_last() {
        assert!(CheckpointManager::new().rollback_last().is_err());
    }

    #[test]
    fn t_persist_create() {
        let store = MemoryStore::in_memory().unwrap();
        let mut m = CheckpointManager::with_persistence(store.conn(), Some("s1".to_string()));
        let id = m.create("desc-A").unwrap();
        assert!(id > 0, "永続IDは正の自動採番");
        assert_eq!(m.count(), 1);

        // DB から復元できる
        let loaded = CheckpointManager::load_persisted(store.conn(), Some("s1")).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].description, "desc-A");
        assert!(loaded[0].rolled_back_at.is_none());
    }

    #[test]
    fn t_persist_session_filter() {
        let store = MemoryStore::in_memory().unwrap();
        let mut m_a = CheckpointManager::with_persistence(store.conn(), Some("s-A".to_string()));
        m_a.create("for-A").unwrap();
        let mut m_b = CheckpointManager::with_persistence(store.conn(), Some("s-B".to_string()));
        m_b.create("for-B").unwrap();
        m_b.create("for-B-2").unwrap();

        let a = CheckpointManager::load_persisted(store.conn(), Some("s-A")).unwrap();
        let b = CheckpointManager::load_persisted(store.conn(), Some("s-B")).unwrap();
        let all = CheckpointManager::load_persisted(store.conn(), None).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 2);
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn t_persist_rollback_marks_timestamp() {
        let store = MemoryStore::in_memory().unwrap();
        let mut m = CheckpointManager::with_persistence(store.conn(), Some("s".to_string()));
        let id = m.create("cp").unwrap();
        let _ = m.rollback(id); // git_ref が None でも実行は走り DB 更新
        let loaded = CheckpointManager::load_persisted(store.conn(), Some("s")).unwrap();
        assert!(
            loaded[0].rolled_back_at.is_some(),
            "ロールバック後はタイムスタンプ記録"
        );
    }

    #[test]
    fn t_persist_no_session_id() {
        let store = MemoryStore::in_memory().unwrap();
        let mut m = CheckpointManager::with_persistence(store.conn(), None);
        m.create("no-session").unwrap();
        let loaded = CheckpointManager::load_persisted(store.conn(), None).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].description, "no-session");
    }
}
