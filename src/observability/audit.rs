use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

/// 監査ログエントリ
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: i64,
    pub timestamp: String,
    pub session_id: Option<String>,
    pub action_type: String,
    pub action_data: String,
}

/// 監査ログの種類
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum AuditAction {
    LlmCall {
        prompt_tokens: usize,
        completion_tokens: usize,
        duration_ms: u64,
        model_id: String,
    },
    ToolCall {
        tool_name: String,
        args: String,
        success: bool,
        output_preview: String,
    },
    SecurityEvent {
        event_type: String,
        detail: String,
    },
    /// ステップ単位の成否記録（p^n診断用）
    StepOutcome {
        step_index: usize,
        outcome: String,
        duration_ms: u64,
        tools_used: Vec<String>,
        consecutive_failures: usize,
    },
}

/// 監査ログライター（append-only）
pub struct AuditLog<'a> {
    conn: &'a Connection,
}

impl<'a> AuditLog<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// 監査ログを記録
    pub fn log(&self, session_id: Option<&str>, action: &AuditAction) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let action_type = match action {
            AuditAction::LlmCall { .. } => "llm_call",
            AuditAction::ToolCall { .. } => "tool_call",
            AuditAction::SecurityEvent { .. } => "security_event",
            AuditAction::StepOutcome { .. } => "step_outcome",
        };
        let action_data = serde_json::to_string(action)?;

        self.conn.execute(
            "INSERT INTO audit_log (timestamp, session_id, action_type, action_data) VALUES (?1, ?2, ?3, ?4)",
            params![&now, session_id, action_type, &action_data],
        )?;
        Ok(())
    }

    /// 直近の監査ログを取得
    pub fn recent(&self, limit: usize) -> Result<Vec<AuditEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, timestamp, session_id, action_type, action_data
             FROM audit_log
             ORDER BY id DESC
             LIMIT ?1",
        )?;

        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(AuditEntry {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                session_id: row.get(2)?,
                action_type: row.get(3)?,
                action_data: row.get(4)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// 特定セッションの監査ログを取得
    pub fn for_session(&self, session_id: &str) -> Result<Vec<AuditEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, timestamp, session_id, action_type, action_data
             FROM audit_log
             WHERE session_id = ?1
             ORDER BY id",
        )?;

        let rows = stmt.query_map(params![session_id], |row| {
            Ok(AuditEntry {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                session_id: row.get(2)?,
                action_type: row.get(3)?,
                action_data: row.get(4)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// 監査ログの総件数
    pub fn count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM audit_log", [], |row| row.get(0))?;
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
    fn test_log_llm_call() {
        let store = test_store();
        let audit = AuditLog::new(store.conn());
        audit
            .log(
                Some("session-1"),
                &AuditAction::LlmCall {
                    prompt_tokens: 100,
                    completion_tokens: 50,
                    duration_ms: 1200,
                    model_id: "bonsai-8b".to_string(),
                },
            )
            .unwrap();
        assert_eq!(audit.count().unwrap(), 1);
    }

    #[test]
    fn test_log_tool_call() {
        let store = test_store();
        let audit = AuditLog::new(store.conn());
        audit
            .log(
                Some("session-1"),
                &AuditAction::ToolCall {
                    tool_name: "shell".to_string(),
                    args: r#"{"command":"ls"}"#.to_string(),
                    success: true,
                    output_preview: "file1.txt\nfile2.txt".to_string(),
                },
            )
            .unwrap();
    }

    #[test]
    fn test_log_security_event() {
        let store = test_store();
        let audit = AuditLog::new(store.conn());
        audit
            .log(
                None,
                &AuditAction::SecurityEvent {
                    event_type: "path_denied".to_string(),
                    detail: "~/.ssh/id_rsa へのアクセスをブロック".to_string(),
                },
            )
            .unwrap();
    }

    #[test]
    fn test_recent() {
        let store = test_store();
        let audit = AuditLog::new(store.conn());
        for i in 0..5 {
            audit
                .log(
                    Some(&format!("s-{i}")),
                    &AuditAction::ToolCall {
                        tool_name: "shell".to_string(),
                        args: format!("cmd-{i}"),
                        success: true,
                        output_preview: String::new(),
                    },
                )
                .unwrap();
        }
        let recent = audit.recent(3).unwrap();
        assert_eq!(recent.len(), 3);
        assert!(recent[0].id > recent[1].id);
    }

    #[test]
    fn test_for_session() {
        let store = test_store();
        let audit = AuditLog::new(store.conn());
        audit
            .log(
                Some("s1"),
                &AuditAction::ToolCall {
                    tool_name: "shell".into(),
                    args: "a".into(),
                    success: true,
                    output_preview: String::new(),
                },
            )
            .unwrap();
        audit
            .log(
                Some("s2"),
                &AuditAction::ToolCall {
                    tool_name: "shell".into(),
                    args: "b".into(),
                    success: true,
                    output_preview: String::new(),
                },
            )
            .unwrap();
        audit
            .log(
                Some("s1"),
                &AuditAction::ToolCall {
                    tool_name: "file_read".into(),
                    args: "c".into(),
                    success: true,
                    output_preview: String::new(),
                },
            )
            .unwrap();

        let s1_logs = audit.for_session("s1").unwrap();
        assert_eq!(s1_logs.len(), 2);
    }

    #[test]
    fn test_action_serialization() {
        let action = AuditAction::LlmCall {
            prompt_tokens: 10,
            completion_tokens: 5,
            duration_ms: 500,
            model_id: "test".to_string(),
        };
        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("LlmCall"));
        assert!(json.contains("prompt_tokens"));
    }

    #[test]
    fn test_empty_audit() {
        let store = test_store();
        let audit = AuditLog::new(store.conn());
        assert_eq!(audit.count().unwrap(), 0);
        assert!(audit.recent(10).unwrap().is_empty());
    }

    // --- StepOutcome テスト ---

    #[test]
    fn test_log_step_outcome_success() {
        let store = test_store();
        let audit = AuditLog::new(store.conn());
        audit
            .log(
                Some("session-1"),
                &AuditAction::StepOutcome {
                    step_index: 0,
                    outcome: "continue".to_string(),
                    duration_ms: 1500,
                    tools_used: vec!["shell".to_string(), "file_read".to_string()],
                    consecutive_failures: 0,
                },
            )
            .unwrap();
        assert_eq!(audit.count().unwrap(), 1);
        let entries = audit.recent(1).unwrap();
        assert_eq!(entries[0].action_type, "step_outcome");
    }

    #[test]
    fn test_log_step_outcome_aborted() {
        let store = test_store();
        let audit = AuditLog::new(store.conn());
        audit
            .log(
                Some("session-1"),
                &AuditAction::StepOutcome {
                    step_index: 3,
                    outcome: "aborted".to_string(),
                    duration_ms: 500,
                    tools_used: vec![],
                    consecutive_failures: 3,
                },
            )
            .unwrap();
        let entries = audit.recent(1).unwrap();
        let data: serde_json::Value = serde_json::from_str(&entries[0].action_data).unwrap();
        assert_eq!(data["step_index"], 3);
        assert_eq!(data["consecutive_failures"], 3);
    }

    #[test]
    fn test_step_outcome_duration_recorded() {
        let store = test_store();
        let audit = AuditLog::new(store.conn());
        audit
            .log(
                Some("s1"),
                &AuditAction::StepOutcome {
                    step_index: 0,
                    outcome: "final_answer".to_string(),
                    duration_ms: 2500,
                    tools_used: vec![],
                    consecutive_failures: 0,
                },
            )
            .unwrap();
        let entries = audit.recent(1).unwrap();
        let data: serde_json::Value = serde_json::from_str(&entries[0].action_data).unwrap();
        assert_eq!(data["duration_ms"], 2500);
    }

    #[test]
    fn test_step_outcome_consecutive_failures_tracked() {
        let store = test_store();
        let audit = AuditLog::new(store.conn());
        for i in 0..3 {
            audit
                .log(
                    Some("s1"),
                    &AuditAction::StepOutcome {
                        step_index: i,
                        outcome: "error".to_string(),
                        duration_ms: 100,
                        tools_used: vec![],
                        consecutive_failures: i + 1,
                    },
                )
                .unwrap();
        }
        let entries = audit.for_session("s1").unwrap();
        assert_eq!(entries.len(), 3);
        let last_data: serde_json::Value =
            serde_json::from_str(&entries[2].action_data).unwrap();
        assert_eq!(last_data["consecutive_failures"], 3);
    }
}
