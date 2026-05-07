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

    /// 全セッションIDを取得（SessionStartイベントを持つもの）
    pub fn list_sessions(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT session_id FROM events WHERE event_type = 'session_start' ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// 成功軌跡を抽出（スキル昇格候補）
    ///
    /// フィルタ条件:
    /// - `min_tool_success_rate` 以上（ToolCallEnd の `success` フィールド比率）
    /// - `min_steps` 以上（ToolCallStart の件数）
    /// - SessionEnd イベントが存在すること
    ///
    /// 期待される event_data フォーマット:
    /// - UserMessage: `{"content": "タスク記述"}`
    /// - ToolCallStart: `{"tool": "shell"}` (tool名のみ必須)
    /// - ToolCallEnd:   `{"tool": "shell", "success": true}` (successのみ必須)
    ///
    /// 本 method は `extract_successful_trajectories_since_id(0, ...)` の薄いラッパ
    /// (handoff 05-07g Phase 5 で scoping 機構導入時に delegate 化、互換維持)。
    pub fn extract_successful_trajectories(
        &self,
        min_tool_success_rate: f64,
        min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>> {
        self.extract_successful_trajectories_since_id(0, min_tool_success_rate, min_steps)
    }

    /// 成功軌跡抽出 (Lab cycle scoping 対応、handoff 05-07g Phase 5)。
    ///
    /// `since_event_id` より大きい id を持つ SessionStart event の session のみを
    /// 対象とする。`extract_failed_trajectories_since_id` の対称、判定は `>=`。
    /// `since_event_id=0` で全期間 = `extract_successful_trajectories` と等価。
    pub fn extract_successful_trajectories_since_id(
        &self,
        since_event_id: i64,
        min_tool_success_rate: f64,
        min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT session_id FROM events
             WHERE event_type = 'session_start' AND id > ?1
             ORDER BY id",
        )?;
        let session_ids: Vec<String> = stmt
            .query_map(params![since_event_id], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut candidates = Vec::new();
        for sid in session_ids {
            if let Some(c) = self.build_trajectory(&sid)?
                && c.total_steps >= min_steps
                && c.tool_success_rate >= min_tool_success_rate
            {
                candidates.push(c);
            }
        }
        Ok(candidates)
    }

    /// 現時点での `events` テーブルの `MAX(id)` を返す (events 空なら 0)。
    ///
    /// Lab cycle 開始時に snapshot して `extract_*_trajectories_since_id(snapshot, ..)`
    /// の引数として渡せば、当該 cycle 内に記録された events だけを AgentHER pass の
    /// 対象にできる (handoff 05-07g Phase 5 / 項目 203 scoping)。
    /// SQL レベルのエラーは Result で伝播 (cold-start で events 空でも `COALESCE` で 0)。
    pub fn current_max_id(&self) -> Result<i64> {
        let id: i64 =
            self.conn
                .query_row("SELECT COALESCE(MAX(id), 0) FROM events", [], |row| {
                    row.get(0)
                })?;
        Ok(id)
    }

    /// AgentHER: 失敗 trajectory を抽出 (success_rate < max_tool_success_rate)
    ///
    /// `extract_successful_trajectories` の対称、SessionEnd 必須・min_steps 適用。
    /// 戻り値は同じ TrajectoryCandidate (互換最優先、success/failure 判別は呼出側責務)。
    /// 本 method は `extract_failed_trajectories_since_id(0, ...)` の薄いラッパ
    /// (handoff 05-07g の Phase 5 で scoping 機構導入時に delegate 化、互換維持)。
    pub fn extract_failed_trajectories(
        &self,
        max_tool_success_rate: f64,
        min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>> {
        self.extract_failed_trajectories_since_id(0, max_tool_success_rate, min_steps)
    }

    /// 失敗 trajectory 抽出 (Lab cycle scoping 対応、handoff 05-07g Phase 5)。
    ///
    /// `since_event_id` より大きい id を持つ SessionStart event の session のみを
    /// 対象とする。`run_experiment_loop` 開始時に `MAX(id)` を snapshot しておけば、
    /// 当該 cycle 内の events だけが AgentHER pass の対象になり、過去 cycle の
    /// 累積汚染を回避できる。`since_event_id=0` で全期間 = `extract_failed_trajectories`
    /// と等価。
    pub fn extract_failed_trajectories_since_id(
        &self,
        since_event_id: i64,
        max_tool_success_rate: f64,
        min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT session_id FROM events
             WHERE event_type = 'session_start' AND id > ?1
             ORDER BY id",
        )?;
        let session_ids: Vec<String> = stmt
            .query_map(params![since_event_id], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut candidates = Vec::new();
        for sid in session_ids {
            if let Some(c) = self.build_trajectory(&sid)?
                && c.total_steps >= min_steps
                && c.tool_success_rate < max_tool_success_rate
            {
                candidates.push(c);
            }
        }
        Ok(candidates)
    }

    fn build_trajectory(&self, session_id: &str) -> Result<Option<TrajectoryCandidate>> {
        let events = self.replay(session_id)?;
        if events.is_empty() {
            return Ok(None);
        }

        let has_session_end = events.iter().any(|e| e.event_type == "session_end");
        if !has_session_end {
            return Ok(None);
        }

        let task_description = events
            .iter()
            .find(|e| e.event_type == "user_message")
            .and_then(|e| serde_json::from_str::<serde_json::Value>(&e.event_data).ok())
            .and_then(|v| v.get("content").and_then(|c| c.as_str()).map(String::from))
            .unwrap_or_default();

        let mut tool_sequence = Vec::new();
        let mut tool_end_total = 0usize;
        let mut tool_end_success = 0usize;

        for ev in &events {
            match ev.event_type.as_str() {
                "tool_call_start" => {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&ev.event_data)
                        && let Some(name) = v.get("tool").and_then(|t| t.as_str())
                    {
                        tool_sequence.push(name.to_string());
                    }
                }
                "tool_call_end" => {
                    tool_end_total += 1;
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&ev.event_data)
                        && v.get("success").and_then(|s| s.as_bool()).unwrap_or(false)
                    {
                        tool_end_success += 1;
                    }
                }
                _ => {}
            }
        }

        let tool_success_rate = if tool_end_total == 0 {
            0.0
        } else {
            tool_end_success as f64 / tool_end_total as f64
        };

        let duration_ms = match (events.first(), events.last()) {
            (Some(start), Some(end)) => compute_duration_ms(&start.created_at, &end.created_at),
            _ => 0,
        };

        Ok(Some(TrajectoryCandidate {
            session_id: session_id.to_string(),
            task_description,
            tool_sequence,
            tool_success_rate,
            total_steps: events
                .iter()
                .filter(|e| e.event_type == "tool_call_start")
                .count(),
            duration_ms,
        }))
    }
}

/// 成功軌跡候補（スキル昇格元）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrajectoryCandidate {
    pub session_id: String,
    pub task_description: String,
    pub tool_sequence: Vec<String>,
    pub tool_success_rate: f64,
    pub total_steps: usize,
    pub duration_ms: u64,
}

impl TrajectoryCandidate {
    /// スキル昇格用の安定キー（tool_sequence join）
    pub fn tool_chain_key(&self) -> String {
        self.tool_sequence.join(" -> ")
    }
}

fn compute_duration_ms(start: &str, end: &str) -> u64 {
    let parse = |s: &str| chrono::DateTime::parse_from_rfc3339(s).ok();
    match (parse(start), parse(end)) {
        (Some(s), Some(e)) => (e - s).num_milliseconds().max(0) as u64,
        _ => 0,
    }
}

/// Event 永続化抽象 (Clean Architecture / Repository pattern、項目 209)。
///
/// SQLite/in-memory 詳細から callers を分離し、`&dyn EventRepository` で
/// AgentHER / ERL / Self-Verify の test 容易性を改善する。
///
/// 実装:
/// - `EventStore<'a>` (本番、SQLite-backed、`&'a Connection` 保持)
/// - `MockEventRepository` (test、`Vec<Event>` ベース、SQLite 不要)
///
/// 既存 inherent method は無変更で残置 (21 callsite 後方互換)、本 trait は
/// 委譲 impl のみ提供 (gradual migration、breaking change なし)。
pub trait EventRepository {
    /// イベントを追加し、付与された id を返す。
    fn append(
        &self,
        session_id: &str,
        event_type: &EventType,
        event_data: &str,
        step_index: Option<usize>,
    ) -> Result<i64>;

    /// session_id 内の全 event を id 昇順で返す (リプレイ用)。
    fn replay(&self, session_id: &str) -> Result<Vec<Event>>;

    /// session_id 内の event 種別ごとの件数 ((event_type, count) tuple Vec)。
    fn count_by_type(&self, session_id: &str) -> Result<Vec<(String, usize)>>;

    /// 全 session を通した event 総数。
    fn total_count(&self) -> Result<usize>;

    /// SessionStart event を持つ session_id 一覧 (distinct、id 昇順)。
    fn list_sessions(&self) -> Result<Vec<String>>;

    /// 成功軌跡を抽出 (スキル昇格候補、AgentHER HSL)。
    fn extract_successful_trajectories(
        &self,
        min_tool_success_rate: f64,
        min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>>;

    /// since_event_id より新しい event のみから成功 trajectory 抽出 (Lab cycle scoping、項目 162/203)。
    fn extract_successful_trajectories_since_id(
        &self,
        since_event_id: i64,
        min_tool_success_rate: f64,
        min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>>;

    /// 失敗軌跡を抽出 (AgentHER HSL relabel 候補)。
    fn extract_failed_trajectories(
        &self,
        max_tool_success_rate: f64,
        min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>>;

    /// since_event_id より新しい event のみから失敗 trajectory 抽出 (Lab cycle scoping)。
    fn extract_failed_trajectories_since_id(
        &self,
        since_event_id: i64,
        max_tool_success_rate: f64,
        min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>>;

    /// 現時点の events.id MAX (Lab cycle 開始時 snapshot 用、項目 206)。
    fn current_max_id(&self) -> Result<i64>;
}

impl<'a> EventRepository for EventStore<'a> {
    fn append(
        &self,
        _session_id: &str,
        _event_type: &EventType,
        _event_data: &str,
        _step_index: Option<usize>,
    ) -> Result<i64> {
        todo!("Phase 2 Green で inherent method 委譲")
    }

    fn replay(&self, _session_id: &str) -> Result<Vec<Event>> {
        todo!("Phase 2 Green で inherent method 委譲")
    }

    fn count_by_type(&self, _session_id: &str) -> Result<Vec<(String, usize)>> {
        todo!("Phase 2 Green で inherent method 委譲")
    }

    fn total_count(&self) -> Result<usize> {
        todo!("Phase 2 Green で inherent method 委譲")
    }

    fn list_sessions(&self) -> Result<Vec<String>> {
        todo!("Phase 2 Green で inherent method 委譲")
    }

    fn extract_successful_trajectories(
        &self,
        _min_tool_success_rate: f64,
        _min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>> {
        todo!("Phase 2 Green で inherent method 委譲")
    }

    fn extract_successful_trajectories_since_id(
        &self,
        _since_event_id: i64,
        _min_tool_success_rate: f64,
        _min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>> {
        todo!("Phase 2 Green で inherent method 委譲")
    }

    fn extract_failed_trajectories(
        &self,
        _max_tool_success_rate: f64,
        _min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>> {
        todo!("Phase 2 Green で inherent method 委譲")
    }

    fn extract_failed_trajectories_since_id(
        &self,
        _since_event_id: i64,
        _max_tool_success_rate: f64,
        _min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>> {
        todo!("Phase 2 Green で inherent method 委譲")
    }

    fn current_max_id(&self) -> Result<i64> {
        todo!("Phase 2 Green で inherent method 委譲")
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

    /// 軌跡抽出テスト用: session_idに成功軌跡イベントを投入
    fn seed_successful_trajectory(es: &EventStore, session_id: &str, n_tools: usize) {
        es.append(session_id, &EventType::SessionStart, "{}", None)
            .unwrap();
        es.append(
            session_id,
            &EventType::UserMessage,
            r#"{"content":"ファイル一覧を取得して"}"#,
            Some(0),
        )
        .unwrap();
        for i in 0..n_tools {
            es.append(
                session_id,
                &EventType::ToolCallStart,
                r#"{"tool":"shell"}"#,
                Some(i),
            )
            .unwrap();
            es.append(
                session_id,
                &EventType::ToolCallEnd,
                r#"{"tool":"shell","success":true}"#,
                Some(i),
            )
            .unwrap();
        }
        es.append(session_id, &EventType::SessionEnd, "{}", None)
            .unwrap();
    }

    #[test]
    fn test_list_sessions() {
        let store = test_store();
        let es = EventStore::new(store.conn());
        es.append("s1", &EventType::SessionStart, "{}", None)
            .unwrap();
        es.append("s2", &EventType::SessionStart, "{}", None)
            .unwrap();
        es.append("s3", &EventType::UserMessage, "{}", None)
            .unwrap(); // SessionStart なし → 除外
        let sessions = es.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions.contains(&"s1".to_string()));
        assert!(sessions.contains(&"s2".to_string()));
        assert!(!sessions.contains(&"s3".to_string()));
    }

    #[test]
    fn test_extract_successful_trajectories_basic() {
        let store = test_store();
        let es = EventStore::new(store.conn());
        seed_successful_trajectory(&es, "s1", 3);

        let cs = es.extract_successful_trajectories(0.8, 2).unwrap();
        assert_eq!(cs.len(), 1);
        let c = &cs[0];
        assert_eq!(c.session_id, "s1");
        assert_eq!(c.task_description, "ファイル一覧を取得して");
        assert_eq!(c.tool_sequence, vec!["shell", "shell", "shell"]);
        assert!((c.tool_success_rate - 1.0).abs() < f64::EPSILON);
        assert_eq!(c.total_steps, 3);
    }

    #[test]
    fn test_extract_trajectories_filters_low_success_rate() {
        let store = test_store();
        let es = EventStore::new(store.conn());
        es.append("s1", &EventType::SessionStart, "{}", None)
            .unwrap();
        es.append(
            "s1",
            &EventType::ToolCallStart,
            r#"{"tool":"shell"}"#,
            Some(0),
        )
        .unwrap();
        es.append(
            "s1",
            &EventType::ToolCallEnd,
            r#"{"tool":"shell","success":false}"#,
            Some(0),
        )
        .unwrap();
        es.append(
            "s1",
            &EventType::ToolCallStart,
            r#"{"tool":"shell"}"#,
            Some(1),
        )
        .unwrap();
        es.append(
            "s1",
            &EventType::ToolCallEnd,
            r#"{"tool":"shell","success":true}"#,
            Some(1),
        )
        .unwrap();
        es.append("s1", &EventType::SessionEnd, "{}", None).unwrap();

        let cs = es.extract_successful_trajectories(0.8, 1).unwrap();
        assert!(cs.is_empty(), "success_rate=0.5 は閾値0.8未満で除外");

        let cs_loose = es.extract_successful_trajectories(0.5, 1).unwrap();
        assert_eq!(cs_loose.len(), 1);
    }

    #[test]
    fn test_extract_trajectories_filters_min_steps() {
        let store = test_store();
        let es = EventStore::new(store.conn());
        seed_successful_trajectory(&es, "s1", 1);
        let cs = es.extract_successful_trajectories(0.0, 3).unwrap();
        assert!(cs.is_empty(), "1ステップは min_steps=3 未満で除外");
    }

    #[test]
    fn test_extract_trajectories_requires_session_end() {
        let store = test_store();
        let es = EventStore::new(store.conn());
        es.append("s1", &EventType::SessionStart, "{}", None)
            .unwrap();
        es.append(
            "s1",
            &EventType::ToolCallStart,
            r#"{"tool":"shell"}"#,
            Some(0),
        )
        .unwrap();
        es.append(
            "s1",
            &EventType::ToolCallEnd,
            r#"{"tool":"shell","success":true}"#,
            Some(0),
        )
        .unwrap();
        // SessionEnd なし
        let cs = es.extract_successful_trajectories(0.0, 0).unwrap();
        assert!(cs.is_empty(), "未完了セッションは除外");
    }

    #[test]
    fn test_trajectory_candidate_tool_chain_key() {
        let c = TrajectoryCandidate {
            session_id: "s".into(),
            task_description: "t".into(),
            tool_sequence: vec!["a".into(), "b".into(), "a".into()],
            tool_success_rate: 1.0,
            total_steps: 3,
            duration_ms: 0,
        };
        assert_eq!(c.tool_chain_key(), "a -> b -> a");
    }

    #[test]
    fn test_compute_duration_ms() {
        let d = compute_duration_ms("2026-04-24T10:00:00Z", "2026-04-24T10:00:02.500Z");
        assert_eq!(d, 2500);
        // 不正フォーマットは 0
        assert_eq!(compute_duration_ms("bad", "2026-04-24T10:00:00Z"), 0);
    }

    #[test]
    fn test_event_type_as_str() {
        assert_eq!(EventType::SessionStart.as_str(), "session_start");
        assert_eq!(EventType::ToolCallEnd.as_str(), "tool_call_end");
        assert_eq!(EventType::PlanGenerated.as_str(), "plan_generated");
    }

    #[test]
    fn test_current_max_id_empty_returns_zero() {
        // events 空 (cold-start) で COALESCE(MAX(id), 0) が 0 を返す
        let store = test_store();
        let es = EventStore::new(store.conn());
        assert_eq!(es.current_max_id().unwrap(), 0);
    }

    #[test]
    fn test_current_max_id_returns_max_after_appends() {
        // 2 events append 後に MAX(id) >= 2 を返す (AUTOINCREMENT 開始 1)
        let store = test_store();
        let es = EventStore::new(store.conn());
        es.append("s1", &EventType::SessionStart, "{}", None)
            .unwrap();
        es.append("s1", &EventType::SessionEnd, "{}", None).unwrap();
        let max = es.current_max_id().unwrap();
        assert!(max >= 2, "expected MAX(id) >= 2 after 2 appends, got {max}");
    }

    // === EventRepository trait test (Phase 1 Red、項目 209) ===
    //
    // 5 test:
    //   1. EventStore<'_> が EventRepository を impl していることの compile-time 確認
    //   2. Mock の append → replay round-trip
    //   3. Mock の extract_failed_trajectories で score filter
    //   4. Mock の current_max_id (cold-start で 0)
    //   5. trait object (`&dyn EventRepository`) 経由の polymorphism (SQLite + Mock 両方)

    use crate::memory::mocks::MockEventRepository;

    #[test]
    fn test_event_store_impls_event_repository() {
        // compile-time guarantee: EventStore<'_> が EventRepository を満たす
        let store = test_store();
        let es = EventStore::new(store.conn());
        let _repo: &dyn EventRepository = &es;
    }

    #[test]
    fn test_mock_event_repository_append_and_replay() {
        // Mock 単独で SQLite なしで append → replay が動作する
        let mock = MockEventRepository::new();
        let id1 = mock.append("s1", &EventType::SessionStart, "{}", None).unwrap();
        let id2 = mock
            .append(
                "s1",
                &EventType::UserMessage,
                r#"{"content":"hi"}"#,
                Some(0),
            )
            .unwrap();
        assert!(id1 >= 1, "first id should be >= 1, got {id1}");
        assert!(id2 > id1, "second id should be > first, got {id2} vs {id1}");

        let events = mock.replay("s1").unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "session_start");
        assert_eq!(events[1].event_type, "user_message");
        assert_eq!(events[1].step_index, Some(0));
    }

    #[test]
    fn test_mock_event_repository_extract_failed_filters_by_score() {
        // 1 session で 2 失敗 + 1 成功 (success_rate=1/3 ≈ 0.333) を seed、
        // max=0.8 で extract_failed → 1 件取得 (failure score 閾値以下)
        let mock = MockEventRepository::new();
        mock.append("s1", &EventType::SessionStart, "{}", None)
            .unwrap();
        mock.append("s1", &EventType::UserMessage, r#"{"content":"t"}"#, None)
            .unwrap();
        for (i, success) in [false, false, true].iter().enumerate() {
            mock.append(
                "s1",
                &EventType::ToolCallStart,
                r#"{"tool":"shell"}"#,
                Some(i),
            )
            .unwrap();
            let payload = format!(r#"{{"tool":"shell","success":{success}}}"#);
            mock.append("s1", &EventType::ToolCallEnd, &payload, Some(i))
                .unwrap();
        }
        mock.append("s1", &EventType::SessionEnd, "{}", None)
            .unwrap();

        let failed = mock.extract_failed_trajectories(0.8, 2).unwrap();
        assert_eq!(failed.len(), 1, "1 session should match max_success_rate=0.8");
        assert_eq!(failed[0].session_id, "s1");
        assert!(
            (failed[0].tool_success_rate - 1.0 / 3.0).abs() < 0.01,
            "expected ~0.333, got {}",
            failed[0].tool_success_rate
        );
    }

    #[test]
    fn test_mock_event_repository_current_max_id_empty_returns_zero() {
        // cold-start で MAX(id) = 0 (EventStore SQLite 実装と等価挙動)
        let mock = MockEventRepository::new();
        assert_eq!(mock.current_max_id().unwrap(), 0);
    }

    #[test]
    fn test_event_repository_trait_object_can_be_passed() {
        // `&dyn EventRepository` で SQLite-backed と Mock 両方を受けられる
        fn helper(repo: &dyn EventRepository) -> usize {
            repo.total_count().unwrap()
        }

        // SQLite-backed (concrete EventStore)
        let store = test_store();
        let es = EventStore::new(store.conn());
        es.append("s1", &EventType::SessionStart, "{}", None)
            .unwrap();
        assert_eq!(helper(&es), 1, "SQLite-backed total_count");

        // Mock-backed
        let mock = MockEventRepository::new();
        mock.append("s1", &EventType::SessionStart, "{}", None)
            .unwrap();
        assert_eq!(helper(&mock), 1, "Mock-backed total_count");
    }

    // === scoping test (agenther-event-flow-fix Phase 5 / Option A 移行で生存) ===
    // `extract_failed_trajectories_since_id` は AgentHER post-Lab pass で
    // `run_hindsight_pass(store, lab_start_event_id)` から使用される。`export_to` は
    // Option A 移行 (agenther-option-a-migration.md) で削除済み。

    #[test]
    fn test_extract_failed_trajectories_since_id_scoping() {
        // 1 cycle 目: failed session を 1 件 seed
        let store = test_store();
        let es = EventStore::new(store.conn());
        es.append("old1", &EventType::SessionStart, "{}", None)
            .unwrap();
        es.append(
            "old1",
            &EventType::UserMessage,
            r#"{"content":"古いタスク"}"#,
            None,
        )
        .unwrap();
        es.append(
            "old1",
            &EventType::ToolCallStart,
            r#"{"tool":"shell"}"#,
            Some(0),
        )
        .unwrap();
        es.append(
            "old1",
            &EventType::ToolCallEnd,
            r#"{"tool":"shell","success":false}"#,
            Some(0),
        )
        .unwrap();
        es.append(
            "old1",
            &EventType::ToolCallStart,
            r#"{"tool":"shell"}"#,
            Some(1),
        )
        .unwrap();
        es.append(
            "old1",
            &EventType::ToolCallEnd,
            r#"{"tool":"shell","success":false}"#,
            Some(1),
        )
        .unwrap();
        es.append("old1", &EventType::SessionEnd, "{}", None)
            .unwrap();

        // snapshot 取得 (Lab cycle 開始の境界)
        let snapshot_id: i64 = store
            .conn()
            .query_row("SELECT COALESCE(MAX(id), 0) FROM events", [], |row| {
                row.get(0)
            })
            .unwrap();

        // 2 cycle 目: 同じく failed session を 1 件 seed
        es.append("new1", &EventType::SessionStart, "{}", None)
            .unwrap();
        es.append(
            "new1",
            &EventType::UserMessage,
            r#"{"content":"新しいタスク"}"#,
            None,
        )
        .unwrap();
        es.append(
            "new1",
            &EventType::ToolCallStart,
            r#"{"tool":"shell"}"#,
            Some(0),
        )
        .unwrap();
        es.append(
            "new1",
            &EventType::ToolCallEnd,
            r#"{"tool":"shell","success":false}"#,
            Some(0),
        )
        .unwrap();
        es.append(
            "new1",
            &EventType::ToolCallStart,
            r#"{"tool":"shell"}"#,
            Some(1),
        )
        .unwrap();
        es.append(
            "new1",
            &EventType::ToolCallEnd,
            r#"{"tool":"shell","success":false}"#,
            Some(1),
        )
        .unwrap();
        es.append("new1", &EventType::SessionEnd, "{}", None)
            .unwrap();

        // since_id=0 → 全期間で 2 件取得 (既存 extract_failed_trajectories と等価)
        let all = es.extract_failed_trajectories_since_id(0, 0.8, 2).unwrap();
        assert_eq!(all.len(), 2, "since_id=0 で 2 件 (old1+new1)");

        // since_id=snapshot_id → cycle 2 のみ 1 件
        let scoped = es
            .extract_failed_trajectories_since_id(snapshot_id, 0.8, 2)
            .unwrap();
        assert_eq!(
            scoped.len(),
            1,
            "snapshot_id 以降の cycle 2 のみ 1 件 (new1)"
        );
        assert_eq!(scoped[0].session_id, "new1");
    }
}
