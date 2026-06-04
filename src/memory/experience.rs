use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::domain::event::{Event, TrajectoryCandidate};

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

/// AgentHER HSL: subgoal 達成判定方式
///
/// 案 A: ToolCallEnd.success==true を sub-achievement とみなす
/// 案 B: 副作用既知ホワイトリスト (file_write/multi_edit/git_commit) のみ
/// デフォルト: ToolEndSuccessOrSideEffect (recall 重視)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubgoalJudgeMethod {
    ToolEndSuccess,
    SideEffectOnly,
    #[default]
    ToolEndSuccessOrSideEffect,
}

/// AgentHER HSL: 失敗 trajectory から達成済 subgoal を抽出した記録
///
/// `trajectory` は session 全体の tool 列、`achieved_subgoals[i]` は対応する
/// `trajectory[subgoal_indices[i]]` 位置で達成された subgoal の記述。
/// invariant: `achieved_subgoals.len() == subgoal_indices.len()` かつ
/// 全 i で `subgoal_indices[i] < trajectory.len()`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HindsightRelabel {
    pub original_goal: String,
    pub achieved_subgoals: Vec<String>,
    pub subgoal_indices: Vec<usize>,
    pub trajectory: Vec<String>,
    pub tool_success_rate: f64,
    pub session_id: String,
    pub total_steps: usize,
}

impl HindsightRelabel {
    /// achieved_subgoals[subgoal_index] に対応する prefix を tool_sequence とする
    /// TrajectoryCandidate を生成 (SkillStore::promote_from_trajectory への adapter)
    pub fn into_relabeled_candidate(&self, subgoal_index: usize) -> Option<TrajectoryCandidate> {
        if subgoal_index >= self.achieved_subgoals.len() {
            return None;
        }
        let traj_idx = self.subgoal_indices[subgoal_index];
        if traj_idx >= self.trajectory.len() {
            return None;
        }
        let tool_sequence: Vec<String> = self.trajectory[..=traj_idx].to_vec();
        if tool_sequence.is_empty() {
            return None;
        }
        let task_description = format!(
            "{} (subgoal: {})",
            self.original_goal, self.achieved_subgoals[subgoal_index]
        );
        Some(TrajectoryCandidate {
            session_id: self.session_id.clone(),
            task_description,
            tool_sequence,
            // この prefix までは成功扱い (HSL relabel 意味論)
            tool_success_rate: 1.0,
            total_steps: traj_idx + 1,
            duration_ms: 0,
        })
    }
}

/// SubgoalJudgeMethod に基づく subgoal 達成判定
fn is_subgoal_achieved(method: SubgoalJudgeMethod, tool_name: &str, success: bool) -> bool {
    if !success {
        return false;
    }
    match method {
        SubgoalJudgeMethod::ToolEndSuccess => true,
        SubgoalJudgeMethod::SideEffectOnly => {
            matches!(tool_name, "file_write" | "multi_edit" | "git_commit")
        }
        // 案 A 包含 (recall 重視のセマンティック分離、実質 ToolEndSuccess と同一)
        SubgoalJudgeMethod::ToolEndSuccessOrSideEffect => true,
    }
}

/// AgentHER HSL: Event 列から HindsightRelabel を mining
///
/// フィルタ: SessionEnd 必須、ToolCallStart >= 2 件、>= 1 subgoal 達成。
/// 単一 session の events を期待 (multi-session 分割は呼出側責務)。
pub fn extract_hindsight_relabels(
    events: &[Event],
    method: SubgoalJudgeMethod,
) -> Vec<HindsightRelabel> {
    if events.is_empty() {
        return Vec::new();
    }

    // SessionEnd 必須 (項目 162 整合)
    if !events.iter().any(|e| e.event_type == "session_end") {
        return Vec::new();
    }

    let session_id = events[0].session_id.clone();

    // original_goal を user_message から抽出
    let original_goal = events
        .iter()
        .find(|e| e.event_type == "user_message")
        .and_then(|e| serde_json::from_str::<serde_json::Value>(&e.event_data).ok())
        .and_then(|v| v.get("content").and_then(|c| c.as_str()).map(String::from))
        .unwrap_or_default();

    let mut trajectory: Vec<String> = Vec::new();
    let mut achieved_subgoals: Vec<String> = Vec::new();
    let mut subgoal_indices: Vec<usize> = Vec::new();
    let mut tool_end_total = 0usize;
    let mut tool_end_success = 0usize;
    let mut current_tool_idx: Option<usize> = None;
    let mut last_tool_name: Option<String> = None;

    for ev in events {
        match ev.event_type.as_str() {
            "tool_call_start" => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&ev.event_data)
                    && let Some(name) = v.get("tool").and_then(|t| t.as_str())
                {
                    current_tool_idx = Some(trajectory.len());
                    trajectory.push(name.to_string());
                    last_tool_name = Some(name.to_string());
                }
            }
            "tool_call_end" => {
                tool_end_total += 1;
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&ev.event_data) {
                    let success = v.get("success").and_then(|s| s.as_bool()).unwrap_or(false);
                    if success {
                        tool_end_success += 1;
                    }
                    let tool_name = v
                        .get("tool")
                        .and_then(|t| t.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| last_tool_name.clone())
                        .unwrap_or_default();
                    if is_subgoal_achieved(method, &tool_name, success)
                        && let Some(idx) = current_tool_idx
                    {
                        achieved_subgoals.push(format!("{tool_name} 成功 (step {idx})"));
                        subgoal_indices.push(idx);
                    }
                }
                current_tool_idx = None;
            }
            _ => {}
        }
    }

    // min_steps filter (trajectory < 2 → 除外)
    if trajectory.len() < 2 {
        return Vec::new();
    }

    // >= 1 subgoal achievement 必要 (false-positive 防止)
    if achieved_subgoals.is_empty() {
        return Vec::new();
    }

    let tool_success_rate = if tool_end_total == 0 {
        0.0
    } else {
        tool_end_success as f64 / tool_end_total as f64
    };

    let total_steps = trajectory.len();

    vec![HindsightRelabel {
        original_goal,
        achieved_subgoals,
        subgoal_indices,
        trajectory,
        tool_success_rate,
        session_id,
        total_steps,
    }]
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

    /// 期限切れの経験を削除
    pub fn purge_expired(&self) -> Result<usize> {
        let now = chrono::Utc::now().to_rfc3339();
        let deleted = self.conn.execute(
            "DELETE FROM experiences WHERE expires_at IS NOT NULL AND expires_at < ?1",
            params![&now],
        )?;
        Ok(deleted)
    }

    /// 経験にTTLを設定（日数）
    pub fn set_ttl(&self, id: i64, ttl_days: i64) -> Result<()> {
        let expires = chrono::Utc::now() + chrono::Duration::days(ttl_days);
        self.conn.execute(
            "UPDATE experiences SET expires_at = ?1 WHERE id = ?2",
            params![&expires.to_rfc3339(), id],
        )?;
        Ok(())
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

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
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

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
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

    /// AgentHER ECHO: HindsightRelabel から「失敗だが部分達成」を Insight として記録
    ///
    /// 1 trajectory につき 1 レコード:
    /// - task_context = original_goal
    /// - action       = trajectory.join(" -> ")
    /// - outcome      = 部分達成: {achieved_subgoals.join(", ")}
    /// - lesson       = "失敗 trajectory から hindsight 抽出: 主目標未達だが N 個の subgoal は達成、再利用候補"
    /// - tool_name    = trajectory 末尾 tool
    pub fn record_hindsight_insight(&self, relabel: &HindsightRelabel) -> Result<i64> {
        let action = relabel.trajectory.join(" -> ");
        let outcome = format!("部分達成: {}", relabel.achieved_subgoals.join(", "));
        let lesson = format!(
            "失敗 trajectory から hindsight 抽出: 主目標未達だが {} 個の subgoal は達成、再利用候補",
            relabel.achieved_subgoals.len()
        );
        let tool_name = relabel.trajectory.last().map(|s| s.as_str());
        self.record(&RecordParams {
            exp_type: ExperienceType::Insight,
            task_context: &relabel.original_goal,
            action: &action,
            outcome: &outcome,
            lesson: Some(&lesson),
            tool_name,
            error_type: None,
            error_detail: None,
        })
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
        exp.record(&success_params("list files", "shell: ls"))
            .unwrap();
        exp.record(&success_params("create directory", "shell: mkdir test"))
            .unwrap();

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
    fn t_purge_expired_removes_old() {
        let store = test_conn();
        let exp = ExperienceStore::new(store.conn());
        let id = exp
            .record(&success_params("old task", "old action"))
            .unwrap();
        // 過去の日時を直接設定
        store
            .conn()
            .execute(
                "UPDATE experiences SET expires_at = '2020-01-01T00:00:00Z' WHERE id = ?1",
                params![id],
            )
            .unwrap();
        let deleted = exp.purge_expired().unwrap();
        assert_eq!(deleted, 1);
    }

    #[test]
    fn t_set_ttl_updates_expires() {
        let store = test_conn();
        let exp = ExperienceStore::new(store.conn());
        let id = exp
            .record(&success_params("ttl task", "ttl action"))
            .unwrap();
        exp.set_ttl(id, 30).unwrap();
        let expires: Option<String> = store
            .conn()
            .query_row(
                "SELECT expires_at FROM experiences WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(expires.is_some());
    }

    #[test]
    fn t_purge_keeps_valid() {
        let store = test_conn();
        let exp = ExperienceStore::new(store.conn());
        let id = exp.record(&success_params("valid", "valid")).unwrap();
        // 未来の日時を設定
        store
            .conn()
            .execute(
                "UPDATE experiences SET expires_at = '2099-01-01T00:00:00Z' WHERE id = ?1",
                params![id],
            )
            .unwrap();
        let deleted = exp.purge_expired().unwrap();
        assert_eq!(deleted, 0);
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
        })
        .unwrap();

        assert_eq!(exp.success_count("shell").unwrap(), 2);
    }

    // ============ AgentHER HSL/ECHO テスト (Phase 1 Red) ============

    fn make_event(id: i64, session_id: &str, event_type: &str, data: &str) -> Event {
        Event {
            id,
            session_id: session_id.to_string(),
            event_type: event_type.to_string(),
            event_data: data.to_string(),
            step_index: None,
            created_at: "2026-05-07T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn t_extract_hsl_basic_success_subgoal() {
        // file_write 成功 + shell 失敗 = 1 trajectory に 1 subgoal
        let events = vec![
            make_event(1, "s1", "session_start", "{}"),
            make_event(2, "s1", "user_message", r#"{"content":"FizzBuzz実装"}"#),
            make_event(3, "s1", "tool_call_start", r#"{"tool":"file_write"}"#),
            make_event(
                4,
                "s1",
                "tool_call_end",
                r#"{"tool":"file_write","success":true}"#,
            ),
            make_event(5, "s1", "tool_call_start", r#"{"tool":"shell"}"#),
            make_event(
                6,
                "s1",
                "tool_call_end",
                r#"{"tool":"shell","success":false}"#,
            ),
            make_event(7, "s1", "session_end", "{}"),
        ];
        let result =
            extract_hindsight_relabels(&events, SubgoalJudgeMethod::ToolEndSuccessOrSideEffect);
        assert_eq!(result.len(), 1, "1 trajectory から 1 HindsightRelabel");
        let r = &result[0];
        assert_eq!(r.original_goal, "FizzBuzz実装");
        assert_eq!(
            r.achieved_subgoals.len(),
            1,
            "file_write 成功 1 件のみ subgoal"
        );
        assert!(r.achieved_subgoals[0].contains("file_write"));
        assert_eq!(
            r.subgoal_indices,
            vec![0],
            "trajectory index 0 = file_write"
        );
        assert_eq!(r.trajectory, vec!["file_write", "shell"]);
        assert!((r.tool_success_rate - 0.5).abs() < 1e-9);
        assert_eq!(r.session_id, "s1");
        assert_eq!(r.total_steps, 2);
    }

    #[test]
    fn t_extract_hsl_filters_all_failures() {
        // 全 ToolCallEnd success=false → 0 件 (false-positive 防止)
        let events = vec![
            make_event(1, "s1", "session_start", "{}"),
            make_event(2, "s1", "user_message", r#"{"content":"全失敗タスク"}"#),
            make_event(3, "s1", "tool_call_start", r#"{"tool":"shell"}"#),
            make_event(
                4,
                "s1",
                "tool_call_end",
                r#"{"tool":"shell","success":false}"#,
            ),
            make_event(5, "s1", "tool_call_start", r#"{"tool":"file_write"}"#),
            make_event(
                6,
                "s1",
                "tool_call_end",
                r#"{"tool":"file_write","success":false}"#,
            ),
            make_event(7, "s1", "session_end", "{}"),
        ];
        let result =
            extract_hindsight_relabels(&events, SubgoalJudgeMethod::ToolEndSuccessOrSideEffect);
        assert!(result.is_empty(), "全 success=false なら relabel 0 件");
    }

    #[test]
    fn t_extract_hsl_side_effect_method() {
        // SideEffectOnly: shell exit 0 を除外、file_write のみ subgoal
        let events = vec![
            make_event(1, "s1", "session_start", "{}"),
            make_event(2, "s1", "user_message", r#"{"content":"混合タスク"}"#),
            make_event(3, "s1", "tool_call_start", r#"{"tool":"shell"}"#),
            make_event(
                4,
                "s1",
                "tool_call_end",
                r#"{"tool":"shell","success":true}"#,
            ),
            make_event(5, "s1", "tool_call_start", r#"{"tool":"file_write"}"#),
            make_event(
                6,
                "s1",
                "tool_call_end",
                r#"{"tool":"file_write","success":true}"#,
            ),
            make_event(7, "s1", "session_end", "{}"),
        ];
        let result = extract_hindsight_relabels(&events, SubgoalJudgeMethod::SideEffectOnly);
        assert_eq!(result.len(), 1);
        let r = &result[0];
        assert_eq!(
            r.achieved_subgoals.len(),
            1,
            "SideEffectOnly では shell を除外、file_write のみ"
        );
        assert_eq!(
            r.subgoal_indices,
            vec![1],
            "trajectory index 1 = file_write"
        );
        assert!(r.achieved_subgoals[0].contains("file_write"));
    }

    #[test]
    fn t_extract_hsl_session_end_required() {
        // SessionEnd 不在 trajectory は除外 (項目 162 整合)
        let events = vec![
            make_event(1, "s1", "session_start", "{}"),
            make_event(2, "s1", "user_message", r#"{"content":"未完了タスク"}"#),
            make_event(3, "s1", "tool_call_start", r#"{"tool":"file_write"}"#),
            make_event(
                4,
                "s1",
                "tool_call_end",
                r#"{"tool":"file_write","success":true}"#,
            ),
            make_event(5, "s1", "tool_call_start", r#"{"tool":"file_write"}"#),
            make_event(
                6,
                "s1",
                "tool_call_end",
                r#"{"tool":"file_write","success":true}"#,
            ),
            // SessionEnd なし
        ];
        let result =
            extract_hindsight_relabels(&events, SubgoalJudgeMethod::ToolEndSuccessOrSideEffect);
        assert!(result.is_empty(), "SessionEnd 不在は除外");
    }

    #[test]
    fn t_extract_hsl_min_steps_filter() {
        // tool_call_start < 2 (min_steps) は除外
        let events = vec![
            make_event(1, "s1", "session_start", "{}"),
            make_event(2, "s1", "user_message", r#"{"content":"短すぎ"}"#),
            make_event(3, "s1", "tool_call_start", r#"{"tool":"file_write"}"#),
            make_event(
                4,
                "s1",
                "tool_call_end",
                r#"{"tool":"file_write","success":true}"#,
            ),
            make_event(5, "s1", "session_end", "{}"),
        ];
        let result =
            extract_hindsight_relabels(&events, SubgoalJudgeMethod::ToolEndSuccessOrSideEffect);
        assert!(result.is_empty(), "tool_call < 2 (min_steps) は除外");
    }

    #[test]
    fn t_record_hindsight_insight_creates_experience() {
        // ExperienceStore に type=insight で 1 レコード追加
        let store = test_conn();
        let exp = ExperienceStore::new(store.conn());
        let relabel = HindsightRelabel {
            original_goal: "FizzBuzz実装".into(),
            achieved_subgoals: vec!["file_write 成功".into()],
            subgoal_indices: vec![0],
            trajectory: vec!["file_write".into(), "shell".into()],
            tool_success_rate: 0.5,
            session_id: "s1".into(),
            total_steps: 2,
        };
        let id = exp.record_hindsight_insight(&relabel).unwrap();
        assert!(id > 0);

        let count: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM experiences WHERE type = 'insight' AND task_context LIKE '%FizzBuzz%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "type=insight で 1 レコード追加");
    }
}
