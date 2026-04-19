use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

/// イベントの種類
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EventType {
    SessionStart,
    UserMessage,
    AssistantMessage,
    ToolCallStart,
    ToolCallEnd,
    PlanGenerated,
    StepCompleted,
    SessionEnd,
}

impl EventType {
    pub fn as_str(&self) -> &str {
        match self {
            Self::SessionStart => "session_start",
            Self::UserMessage => "user_message",
            Self::AssistantMessage => "assistant_message",
            Self::ToolCallStart => "tool_call_start",
            Self::ToolCallEnd => "tool_call_end",
            Self::PlanGenerated => "plan_generated",
            Self::StepCompleted => "step_completed",
            Self::SessionEnd => "session_end",
        }
    }
}

/// 不変イベントレコード
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: i64,
    pub session_id: String,
    pub event_type: String,
    pub event_data: String,
    pub step_index: Option<usize>,
    pub created_at: String,
}

/// Event Sourcing ストア（append-only）
pub struct EventStore<'a> {
    conn: &'a Connection,
}

impl<'a> EventStore<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// イベントを記録（append-only）
    pub fn append(
        &self,
        session_id: &str,
        event_type: &EventType,
        event_data: &str,
        step_index: Option<usize>,
    ) -> Result<i64> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO events (session_id, event_type, event_data, step_index, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session_id,
                event_type.as_str(),
                event_data,
                step_index.map(|s| s as i64),
                &now
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// セッションのイベントを時系列で取得（リプレイ用）
    pub fn replay(&self, session_id: &str) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, event_type, event_data, step_index, created_at
             FROM events WHERE session_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            Ok(Event {
                id: row.get(0)?,
                session_id: row.get(1)?,
                event_type: row.get(2)?,
                event_data: row.get(3)?,
                step_index: row.get::<_, Option<i64>>(4)?.map(|v| v as usize),
                created_at: row.get(5)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// イベント種別ごとの件数を取得（分析用）
    pub fn count_by_type(&self, session_id: &str) -> Result<Vec<(String, usize)>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_type, COUNT(*) FROM events
             WHERE session_id = ?1 GROUP BY event_type ORDER BY COUNT(*) DESC",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// 全セッションのイベント総数
    pub fn total_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))?;
        Ok(count as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::MemoryStore;

    fn test_store() -> MemoryStore {
        MemoryStore::in_memory().unwrap()
    }

    #[test]
    fn test_append_and_replay() {
        let store = test_store();
        let es = EventStore::new(store.conn());
        es.append("s1", &EventType::SessionStart, "{}", None)
            .unwrap();
        es.append(
            "s1",
            &EventType::UserMessage,
            r#"{"content":"hello"}"#,
            Some(0),
        )
        .unwrap();
        es.append(
            "s1",
            &EventType::ToolCallStart,
            r#"{"tool":"shell"}"#,
            Some(0),
        )
        .unwrap();

        let events = es.replay("s1").unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event_type, "session_start");
        assert_eq!(events[1].step_index, Some(0));
    }

    #[test]
    fn test_count_by_type() {
        let store = test_store();
        let es = EventStore::new(store.conn());
        es.append("s1", &EventType::ToolCallStart, "{}", Some(0))
            .unwrap();
        es.append("s1", &EventType::ToolCallStart, "{}", Some(1))
            .unwrap();
        es.append("s1", &EventType::ToolCallEnd, "{}", Some(0))
            .unwrap();

        let counts = es.count_by_type("s1").unwrap();
        let tc_count = counts
            .iter()
            .find(|(t, _)| t == "tool_call_start")
            .map(|(_, c)| *c);
        assert_eq!(tc_count, Some(2));
    }

    #[test]
    fn test_total_count() {
        let store = test_store();
        let es = EventStore::new(store.conn());
        assert_eq!(es.total_count().unwrap(), 0);
        es.append("s1", &EventType::SessionStart, "{}", None)
            .unwrap();
        assert_eq!(es.total_count().unwrap(), 1);
    }

    #[test]
    fn test_replay_empty_session() {
        let store = test_store();
        let es = EventStore::new(store.conn());
        let events = es.replay("nonexistent").unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_event_type_as_str() {
        assert_eq!(EventType::SessionStart.as_str(), "session_start");
        assert_eq!(EventType::ToolCallEnd.as_str(), "tool_call_end");
        assert_eq!(EventType::PlanGenerated.as_str(), "plan_generated");
    }
}
