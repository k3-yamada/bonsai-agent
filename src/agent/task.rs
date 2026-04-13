use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

/// タスクの状態
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Pending,
    InProgress,
    WaitingForHuman,
    Completed,
    Failed,
}

impl TaskState {
    fn as_str(&self) -> &str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::WaitingForHuman => "waiting",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "in_progress" => Self::InProgress,
            "waiting" => Self::WaitingForHuman,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

/// ステップの記録
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepRecord {
    pub description: String,
    pub outcome: String,
    pub timestamp: String,
}

/// タスク
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub goal: String,
    pub state: TaskState,
    pub parent_id: Option<String>,
    pub step_current: i64,
    pub step_log: Vec<StepRecord>,
    pub context: Option<String>,
    pub error_info: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// タスクマネージャー
pub struct TaskManager<'a> {
    conn: &'a Connection,
}

impl<'a> TaskManager<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// タスクを作成
    pub fn create(&self, goal: &str, parent_id: Option<&str>) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();

        self.conn.execute(
            "INSERT INTO tasks (id, goal, state, parent_id, step_current, step_log, created_at, updated_at)
             VALUES (?1, ?2, 'pending', ?3, 0, '[]', ?4, ?4)",
            params![&id, goal, parent_id, &now],
        )?;
        Ok(id)
    }

    /// タスクを取得
    pub fn get(&self, task_id: &str) -> Result<Option<Task>> {
        let result = self.conn.query_row(
            "SELECT id, goal, state, parent_id, step_current, step_log, context, error_info, created_at, updated_at
             FROM tasks WHERE id = ?1",
            params![task_id],
            |row| {
                let step_log_json: String = row.get(5)?;
                let step_log: Vec<StepRecord> = serde_json::from_str(&step_log_json).unwrap_or_default();
                Ok(Task {
                    id: row.get(0)?,
                    goal: row.get(1)?,
                    state: TaskState::from_str(&row.get::<_, String>(2)?),
                    parent_id: row.get(3)?,
                    step_current: row.get(4)?,
                    step_log,
                    context: row.get(6)?,
                    error_info: row.get(7)?,
                    created_at: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            },
        );

        match result {
            Ok(t) => Ok(Some(t)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// 状態を更新
    pub fn update_state(&self, task_id: &str, state: TaskState) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE tasks SET state = ?1, updated_at = ?2 WHERE id = ?3",
            params![state.as_str(), &now, task_id],
        )?;
        Ok(())
    }

    /// ステップを記録
    pub fn add_step(&self, task_id: &str, description: &str, outcome: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let task = self
            .get(task_id)?
            .ok_or_else(|| anyhow::anyhow!("タスクが見つかりません"))?;

        let mut steps = task.step_log;
        steps.push(StepRecord {
            description: description.to_string(),
            outcome: outcome.to_string(),
            timestamp: now.clone(),
        });
        let step_json = serde_json::to_string(&steps)?;

        self.conn.execute(
            "UPDATE tasks SET step_log = ?1, step_current = ?2, updated_at = ?3 WHERE id = ?4",
            params![&step_json, steps.len() as i64, &now, task_id],
        )?;
        Ok(())
    }

    /// コンテキストを保存（中断時用）
    pub fn save_context(&self, task_id: &str, context: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE tasks SET context = ?1, updated_at = ?2 WHERE id = ?3",
            params![context, &now, task_id],
        )?;
        Ok(())
    }

    /// エラー情報を保存
    pub fn set_error(&self, task_id: &str, error: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE tasks SET error_info = ?1, state = 'failed', updated_at = ?2 WHERE id = ?3",
            params![error, &now, task_id],
        )?;
        Ok(())
    }

    /// 未完了タスク一覧
    pub fn list_incomplete(&self) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, goal, state, parent_id, step_current, step_log, context, error_info, created_at, updated_at
             FROM tasks
             WHERE state IN ('pending', 'in_progress', 'waiting')
             ORDER BY updated_at DESC",
        )?;

        let rows = stmt.query_map([], |row| {
            let step_log_json: String = row.get(5)?;
            let step_log: Vec<StepRecord> =
                serde_json::from_str(&step_log_json).unwrap_or_default();
            Ok(Task {
                id: row.get(0)?,
                goal: row.get(1)?,
                state: TaskState::from_str(&row.get::<_, String>(2)?),
                parent_id: row.get(3)?,
                step_current: row.get(4)?,
                step_log,
                context: row.get(6)?,
                error_info: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// サブタスクを取得
    pub fn subtasks(&self, parent_id: &str) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, goal, state, parent_id, step_current, step_log, context, error_info, created_at, updated_at
             FROM tasks WHERE parent_id = ?1 ORDER BY created_at",
        )?;

        let rows = stmt.query_map(params![parent_id], |row| {
            let step_log_json: String = row.get(5)?;
            let step_log: Vec<StepRecord> =
                serde_json::from_str(&step_log_json).unwrap_or_default();
            Ok(Task {
                id: row.get(0)?,
                goal: row.get(1)?,
                state: TaskState::from_str(&row.get::<_, String>(2)?),
                parent_id: row.get(3)?,
                step_current: row.get(4)?,
                step_log,
                context: row.get(6)?,
                error_info: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
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
    fn test_create_task() {
        let store = test_store();
        let mgr = TaskManager::new(store.conn());
        let id = mgr.create("ファイル一覧を表示", None).unwrap();
        assert!(!id.is_empty());
    }

    #[test]
    fn test_get_task() {
        let store = test_store();
        let mgr = TaskManager::new(store.conn());
        let id = mgr.create("テスト", None).unwrap();
        let task = mgr.get(&id).unwrap().unwrap();
        assert_eq!(task.goal, "テスト");
        assert_eq!(task.state, TaskState::Pending);
    }

    #[test]
    fn test_update_state() {
        let store = test_store();
        let mgr = TaskManager::new(store.conn());
        let id = mgr.create("テスト", None).unwrap();
        mgr.update_state(&id, TaskState::InProgress).unwrap();
        let task = mgr.get(&id).unwrap().unwrap();
        assert_eq!(task.state, TaskState::InProgress);
    }

    #[test]
    fn test_add_step() {
        let store = test_store();
        let mgr = TaskManager::new(store.conn());
        let id = mgr.create("テスト", None).unwrap();
        mgr.add_step(&id, "ls実行", "成功").unwrap();
        mgr.add_step(&id, "ファイル読み取り", "成功").unwrap();

        let task = mgr.get(&id).unwrap().unwrap();
        assert_eq!(task.step_log.len(), 2);
        assert_eq!(task.step_current, 2);
    }

    #[test]
    fn test_save_context() {
        let store = test_store();
        let mgr = TaskManager::new(store.conn());
        let id = mgr.create("テスト", None).unwrap();
        mgr.save_context(&id, "中断時のコンテキスト").unwrap();

        let task = mgr.get(&id).unwrap().unwrap();
        assert_eq!(task.context, Some("中断時のコンテキスト".to_string()));
    }

    #[test]
    fn test_set_error() {
        let store = test_store();
        let mgr = TaskManager::new(store.conn());
        let id = mgr.create("テスト", None).unwrap();
        mgr.set_error(&id, "タイムアウト").unwrap();

        let task = mgr.get(&id).unwrap().unwrap();
        assert_eq!(task.state, TaskState::Failed);
        assert_eq!(task.error_info, Some("タイムアウト".to_string()));
    }

    #[test]
    fn test_list_incomplete() {
        let store = test_store();
        let mgr = TaskManager::new(store.conn());
        mgr.create("pending", None).unwrap();
        let id2 = mgr.create("done", None).unwrap();
        mgr.update_state(&id2, TaskState::Completed).unwrap();
        mgr.create("in_progress", None).unwrap();

        let incomplete = mgr.list_incomplete().unwrap();
        assert_eq!(incomplete.len(), 2);
    }

    #[test]
    fn test_subtasks() {
        let store = test_store();
        let mgr = TaskManager::new(store.conn());
        let parent = mgr.create("親タスク", None).unwrap();
        mgr.create("子タスク1", Some(&parent)).unwrap();
        mgr.create("子タスク2", Some(&parent)).unwrap();

        let subs = mgr.subtasks(&parent).unwrap();
        assert_eq!(subs.len(), 2);
    }

    #[test]
    fn test_task_not_found() {
        let store = test_store();
        let mgr = TaskManager::new(store.conn());
        assert!(mgr.get("nonexistent").unwrap().is_none());
    }

    #[test]
    fn test_task_state_serialization() {
        let json = serde_json::to_string(&TaskState::InProgress).unwrap();
        assert_eq!(json, "\"in_progress\"");
        let state: TaskState = serde_json::from_str("\"completed\"").unwrap();
        assert_eq!(state, TaskState::Completed);
    }
}
