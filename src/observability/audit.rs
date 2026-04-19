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
    /// アドバイザー呼出（コスト追跡・観測性）
    AdvisorCall {
        /// "verification" or "replan"
        role: String,
        /// "remote" (外部API) or "local" (組込みプロンプト)
        source: String,
        /// 投入プロンプト長（文字数、近似トークン量）
        prompt_len: usize,
        /// HTTP呼出時間（ms、ローカルは0）
        duration_ms: u64,
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
            AuditAction::AdvisorCall { .. } => "advisor_call",
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

    /// Advisorコスト集計を計算
    ///
    /// `session_id` を指定すると該当セッションのみ集計、None なら全体集計。
    /// 戻り値は role/source 別カウント、平均prompt_len、平均HTTP duration_ms。
    pub fn advisor_stats(&self, session_id: Option<&str>) -> Result<AdvisorStats> {
        let sql = match session_id {
            Some(_) => {
                "SELECT action_data FROM audit_log
                 WHERE action_type = 'advisor_call' AND session_id = ?1"
            }
            None => "SELECT action_data FROM audit_log WHERE action_type = 'advisor_call'",
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows: Vec<String> = match session_id {
            Some(sid) => stmt
                .query_map(params![sid], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?,
            None => stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?,
        };

        let mut stats = AdvisorStats::default();
        let mut prompt_len_sum: u64 = 0;
        let mut duration_sum: u64 = 0;
        let mut remote_duration_count: u64 = 0;

        for data in &rows {
            let v: serde_json::Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => continue,
            };
            stats.total_calls += 1;
            match v["role"].as_str().unwrap_or("") {
                "verification" => stats.verification_calls += 1,
                "replan" => stats.replan_calls += 1,
                _ => {}
            }
            let source = v["source"].as_str().unwrap_or("");
            match source {
                "remote" => stats.remote_calls += 1,
                "local" => stats.local_calls += 1,
                _ => {}
            }
            prompt_len_sum += v["prompt_len"].as_u64().unwrap_or(0);
            let d = v["duration_ms"].as_u64().unwrap_or(0);
            if source == "remote" && d > 0 {
                duration_sum += d;
                remote_duration_count += 1;
            }
        }

        if stats.total_calls > 0 {
            stats.avg_prompt_len = prompt_len_sum / stats.total_calls as u64;
        }
        if remote_duration_count > 0 {
            stats.avg_remote_duration_ms = duration_sum / remote_duration_count;
        }
        Ok(stats)
    }
}

/// Advisor呼出統計（コスト追跡・観測性）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AdvisorStats {
    /// 総呼出数
    pub total_calls: usize,
    /// role: verification の回数
    pub verification_calls: usize,
    /// role: replan の回数
    pub replan_calls: usize,
    /// source: remote の回数（実HTTP呼出）
    pub remote_calls: usize,
    /// source: local の回数（フォールバック含む）
    pub local_calls: usize,
    /// 平均プロンプト長（文字数、近似トークン量）
    pub avg_prompt_len: u64,
    /// remote呼出の平均所要時間（ms、HTTP往復）
    pub avg_remote_duration_ms: u64,
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
    fn test_advisor_stats_empty() {
        let store = test_store();
        let audit = AuditLog::new(store.conn());
        let stats = audit.advisor_stats(None).unwrap();
        assert_eq!(stats.total_calls, 0);
        assert_eq!(stats.avg_prompt_len, 0);
    }

    #[test]
    fn test_advisor_stats_aggregates_correctly() {
        let store = test_store();
        let audit = AuditLog::new(store.conn());
        // verification x2 (remote, local) + replan x1 (remote)
        audit
            .log(
                Some("s-1"),
                &AuditAction::AdvisorCall {
                    role: "verification".to_string(),
                    source: "remote".to_string(),
                    prompt_len: 100,
                    duration_ms: 200,
                },
            )
            .unwrap();
        audit
            .log(
                Some("s-1"),
                &AuditAction::AdvisorCall {
                    role: "verification".to_string(),
                    source: "local".to_string(),
                    prompt_len: 50,
                    duration_ms: 0,
                },
            )
            .unwrap();
        audit
            .log(
                Some("s-1"),
                &AuditAction::AdvisorCall {
                    role: "replan".to_string(),
                    source: "remote".to_string(),
                    prompt_len: 150,
                    duration_ms: 400,
                },
            )
            .unwrap();

        let stats = audit.advisor_stats(None).unwrap();
        assert_eq!(stats.total_calls, 3);
        assert_eq!(stats.verification_calls, 2);
        assert_eq!(stats.replan_calls, 1);
        assert_eq!(stats.remote_calls, 2);
        assert_eq!(stats.local_calls, 1);
        assert_eq!(stats.avg_prompt_len, 100); // (100+50+150)/3
        assert_eq!(stats.avg_remote_duration_ms, 300); // (200+400)/2 — local除外
    }

    #[test]
    fn test_advisor_stats_filters_by_session() {
        let store = test_store();
        let audit = AuditLog::new(store.conn());
        audit
            .log(
                Some("s-A"),
                &AuditAction::AdvisorCall {
                    role: "verification".to_string(),
                    source: "remote".to_string(),
                    prompt_len: 100,
                    duration_ms: 100,
                },
            )
            .unwrap();
        audit
            .log(
                Some("s-B"),
                &AuditAction::AdvisorCall {
                    role: "replan".to_string(),
                    source: "remote".to_string(),
                    prompt_len: 200,
                    duration_ms: 200,
                },
            )
            .unwrap();
        let stats_a = audit.advisor_stats(Some("s-A")).unwrap();
        assert_eq!(stats_a.total_calls, 1);
        assert_eq!(stats_a.verification_calls, 1);
        assert_eq!(stats_a.replan_calls, 0);
        let stats_b = audit.advisor_stats(Some("s-B")).unwrap();
        assert_eq!(stats_b.total_calls, 1);
        assert_eq!(stats_b.replan_calls, 1);
    }

    #[test]
    fn test_advisor_stats_ignores_other_action_types() {
        let store = test_store();
        let audit = AuditLog::new(store.conn());
        audit
            .log(
                Some("s"),
                &AuditAction::LlmCall {
                    prompt_tokens: 100,
                    completion_tokens: 50,
                    duration_ms: 1000,
                    model_id: "test".to_string(),
                },
            )
            .unwrap();
        let stats = audit.advisor_stats(None).unwrap();
        assert_eq!(stats.total_calls, 0, "LlmCallはadvisor統計に含めない");
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
        let last_data: serde_json::Value = serde_json::from_str(&entries[2].action_data).unwrap();
        assert_eq!(last_data["consecutive_failures"], 3);
    }
}
