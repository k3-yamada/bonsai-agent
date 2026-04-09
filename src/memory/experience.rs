use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

/// 経験の種類
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExperienceType {
    Success,
    Failure,
    Insight,
}

impl ExperienceType {
    fn as_str(&self) -> &str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Insight => "insight",
        }
    }
}

/// 経験レコード
#[derive(Debug, Clone)]
pub struct Experience {
    pub id: i64,
    pub exp_type: ExperienceType,
    pub task_context: String,
    pub action: String,
    pub outcome: String,
    pub lesson: Option<String>,
    pub tool_name: Option<String>,
    pub error_type: Option<String>,
    pub error_detail: Option<String>,
    pub reuse_count: i64,
    pub created_at: String,
}

/// 経験記録の入力パラメータ
pub struct RecordParams<'a> {
    pub exp_type: ExperienceType,
    pub task_context: &'a str,
    pub action: &'a str,
    pub outcome: &'a str,
    pub lesson: Option<&'a str>,
    pub tool_name: Option<&'a str>,
    pub error_type: Option<&'a str>,
    pub error_detail: Option<&'a str>,
}

/// 経験メモリの操作
pub struct ExperienceStore<'a> {
    conn: &'a Connection,
}

impl<'a> ExperienceStore<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// 経験を記録
    pub fn record(&self, params: &RecordParams<'_>) -> Result<i64> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO experiences (type, task_context, action, outcome, lesson, tool_name, error_type, error_detail, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                params.exp_type.as_str(),
                params.task_context,
                params.action,
                params.outcome,
                params.lesson,
                params.tool_name,
                params.error_type,
                params.error_detail,
                &now,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 類似タスクの過去経験を検索（task_contextのキーワードマッチ）
    pub fn find_similar(&self, context: &str, limit: usize) -> Result<Vec<Experience>> {
        // 簡易的なLIKE検索。将来はベクトル検索に置き換え。
        let pattern = format!("%{}%", context.split_whitespace().next().unwrap_or(""));
        let mut stmt = self.conn.prepare(
            "SELECT id, type, task_context, action, outcome, lesson, tool_name, error_type, error_detail, reuse_count, created_at
             FROM experiences
             WHERE task_context LIKE ?1
             ORDER BY created_at DESC
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![&pattern, limit as i64], |row| {
            let type_str: String = row.get(1)?;
            let exp_type = match type_str.as_str() {
                "success" => ExperienceType::Success,
                "failure" => ExperienceType::Failure,
                _ => ExperienceType::Insight,
            };
            Ok(Experience {
                id: row.get(0)?,
                exp_type,
                task_context: row.get(2)?,
                action: row.get(3)?,
                outcome: row.get(4)?,
                lesson: row.get(5)?,
                tool_name: row.get(6)?,
                error_type: row.get(7)?,
                error_detail: row.get(8)?,
                reuse_count: row.get(9)?,
                created_at: row.get(10)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// 特定ツールの失敗パターンを集計
    pub fn failure_patterns(&self, tool_name: &str) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT error_detail, COUNT(*) as cnt
             FROM experiences
             WHERE type = 'failure' AND tool_name = ?1 AND error_detail IS NOT NULL
             GROUP BY error_detail
             ORDER BY cnt DESC",
        )?;

        let rows = stmt.query_map(params![tool_name], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// 成功経験の数をカウント
    pub fn success_count(&self, tool_name: &str) -> Result<i64> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM experiences WHERE type = 'success' AND tool_name = ?1",
            params![tool_name],
            |row| row.get(0),
        )?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::MemoryStore;

    fn test_conn() -> MemoryStore {
        MemoryStore::in_memory().unwrap()
    }

    fn success_params<'a>(ctx: &'a str, action: &'a str) -> RecordParams<'a> {
        RecordParams {
            exp_type: ExperienceType::Success,
            task_context: ctx,
            action,
            outcome: "OK",
            lesson: None,
            tool_name: Some("shell"),
            error_type: None,
            error_detail: None,
        }
    }

    #[test]
    fn test_record_success() {
        let store = test_conn();
        let exp = ExperienceStore::new(store.conn());
        let id = exp
            .record(&RecordParams {
                exp_type: ExperienceType::Success,
                task_context: "list files",
                action: "shell: ls -la",
                outcome: "files listed",
                lesson: None,
                tool_name: Some("shell"),
                error_type: None,
                error_detail: None,
            })
            .unwrap();
        assert!(id > 0);
    }

    #[test]
    fn test_record_failure() {
        let store = test_conn();
        let exp = ExperienceStore::new(store.conn());
        exp.record(&RecordParams {
            exp_type: ExperienceType::Failure,
            task_context: "delete file",
            action: "shell: rm important.txt",
            outcome: "Permission denied",
            lesson: Some("insufficient permissions"),
            tool_name: Some("shell"),
            error_type: Some("ToolExecError"),
            error_detail: Some("PermissionDenied"),
        })
        .unwrap();
    }

    #[test]
    fn test_find_similar() {
        let store = test_conn();
        let exp = ExperienceStore::new(store.conn());
        exp.record(&success_params("list files", "shell: ls")).unwrap();
        exp.record(&success_params("create directory", "shell: mkdir test")).unwrap();

        let results = exp.find_similar("list", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].exp_type, ExperienceType::Success);
    }

    #[test]
    fn test_failure_patterns() {
        let store = test_conn();
        let exp = ExperienceStore::new(store.conn());

        for _ in 0..3 {
            exp.record(&RecordParams {
                exp_type: ExperienceType::Failure,
                task_context: "API call",
                action: "shell: curl api.example.com",
                outcome: "timeout",
                lesson: None,
                tool_name: Some("shell"),
                error_type: Some("ToolExecError"),
                error_detail: Some("Timeout"),
            })
            .unwrap();
        }
        exp.record(&RecordParams {
            exp_type: ExperienceType::Failure,
            task_context: "run command",
            action: "shell: nonexistent",
            outcome: "not found",
            lesson: None,
            tool_name: Some("shell"),
            error_type: Some("ToolExecError"),
            error_detail: Some("CommandNotFound"),
        })
        .unwrap();

        let patterns = exp.failure_patterns("shell").unwrap();
        assert_eq!(patterns.len(), 2);
        assert_eq!(patterns[0].0, "Timeout");
        assert_eq!(patterns[0].1, 3);
    }

    #[test]
    fn test_success_count() {
        let store = test_conn();
        let exp = ExperienceStore::new(store.conn());
        exp.record(&success_params("t1", "a1")).unwrap();
        exp.record(&success_params("t2", "a2")).unwrap();
        exp.record(&RecordParams {
            exp_type: ExperienceType::Failure,
            task_context: "t3",
            action: "a3",
            outcome: "err",
            lesson: None,
            tool_name: Some("shell"),
            error_type: None,
            error_detail: None,
        }).unwrap();

        assert_eq!(exp.success_count("shell").unwrap(), 2);
    }
}
