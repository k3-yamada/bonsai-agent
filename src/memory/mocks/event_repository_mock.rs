//! In-memory `EventRepository` mock。SQLite 不要で AgentHER / ERL / Self-Verify
//! の test を高速化する (Clean Architecture Repository pattern、項目 209)。
//!
//! `EventStore` (SQLite-backed) と挙動 parity を保ち、trait レベルでは差し替え可能。
//! trajectory 構築は `event_store::build_trajectory_from_events` 共有 helper を経由。

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use anyhow::Result;

use crate::agent::event_store::{
    Event, EventRepository, EventType, TrajectoryCandidate, build_trajectory_from_events,
};

/// `Vec<Event>` 内蔵の test 用 mock。`Send + Sync` 互換のため `Mutex` でラップ。
///
/// id は単調増加 (cold-start 1)、`current_max_id()` は最終付与 id を返す
/// (cold-start で 0、SQLite `COALESCE(MAX(id), 0)` と挙動 parity)。
pub struct MockEventRepository {
    events: Mutex<Vec<Event>>,
    next_id: Mutex<i64>,
}

impl MockEventRepository {
    pub fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            next_id: Mutex::new(1),
        }
    }
}

impl Default for MockEventRepository {
    fn default() -> Self {
        Self::new()
    }
}

impl EventRepository for MockEventRepository {
    fn append(
        &self,
        session_id: &str,
        event_type: &EventType,
        event_data: &str,
        step_index: Option<usize>,
    ) -> Result<i64> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut events = self.events.lock().unwrap();
        let mut next_id = self.next_id.lock().unwrap();
        let id = *next_id;
        events.push(Event {
            id,
            session_id: session_id.to_string(),
            event_type: event_type.as_str().to_string(),
            event_data: event_data.to_string(),
            step_index,
            created_at: now,
        });
        *next_id += 1;
        Ok(id)
    }

    fn replay(&self, session_id: &str) -> Result<Vec<Event>> {
        let events = self.events.lock().unwrap();
        Ok(events
            .iter()
            .filter(|e| e.session_id == session_id)
            .cloned()
            .collect())
    }

    fn count_by_type(&self, session_id: &str) -> Result<Vec<(String, usize)>> {
        let events = self.events.lock().unwrap();
        let mut counts: HashMap<String, usize> = HashMap::new();
        for e in events.iter().filter(|e| e.session_id == session_id) {
            *counts.entry(e.event_type.clone()).or_insert(0) += 1;
        }
        let mut sorted: Vec<(String, usize)> = counts.into_iter().collect();
        // SQL `ORDER BY COUNT(*) DESC` parity
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        Ok(sorted)
    }

    fn total_count(&self) -> Result<usize> {
        Ok(self.events.lock().unwrap().len())
    }

    fn list_sessions(&self) -> Result<Vec<String>> {
        let events = self.events.lock().unwrap();
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        // SQL `WHERE event_type = 'session_start' ORDER BY id` parity (Vec は挿入順 = id 昇順)
        for e in events.iter() {
            if e.event_type == "session_start" && seen.insert(e.session_id.clone()) {
                result.push(e.session_id.clone());
            }
        }
        Ok(result)
    }

    fn extract_successful_trajectories(
        &self,
        min_tool_success_rate: f64,
        min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>> {
        self.extract_successful_trajectories_since_id(0, min_tool_success_rate, min_steps)
    }

    fn extract_successful_trajectories_since_id(
        &self,
        since_event_id: i64,
        min_tool_success_rate: f64,
        min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>> {
        let session_ids = self.session_starts_since(since_event_id);
        let mut candidates = Vec::new();
        for sid in session_ids {
            let events = self.replay(&sid)?;
            if let Some(c) = build_trajectory_from_events(&sid, &events)
                && c.total_steps >= min_steps
                && c.tool_success_rate >= min_tool_success_rate
            {
                candidates.push(c);
            }
        }
        Ok(candidates)
    }

    fn extract_failed_trajectories(
        &self,
        max_tool_success_rate: f64,
        min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>> {
        self.extract_failed_trajectories_since_id(0, max_tool_success_rate, min_steps)
    }

    fn extract_failed_trajectories_since_id(
        &self,
        since_event_id: i64,
        max_tool_success_rate: f64,
        min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>> {
        let session_ids = self.session_starts_since(since_event_id);
        let mut candidates = Vec::new();
        for sid in session_ids {
            let events = self.replay(&sid)?;
            if let Some(c) = build_trajectory_from_events(&sid, &events)
                && c.total_steps >= min_steps
                && c.tool_success_rate < max_tool_success_rate
            {
                candidates.push(c);
            }
        }
        Ok(candidates)
    }

    fn current_max_id(&self) -> Result<i64> {
        // cold-start で next_id=1 → 0 を返す (SQLite COALESCE(MAX(id), 0) parity)
        Ok(*self.next_id.lock().unwrap() - 1)
    }

    fn verification_success_rate(
        &self,
        _task_type: &str,
        _min_samples: usize,
    ) -> Result<Option<f64>> {
        todo!("Phase 2 Green で in-memory events 走査 + task_type 分類 + 成功率計算")
    }
}

impl MockEventRepository {
    /// SessionStart event のうち id > since_event_id である session_id の Vec (id 昇順 distinct)。
    /// SQL `SELECT DISTINCT session_id FROM events WHERE event_type='session_start' AND id > ?1 ORDER BY id` parity。
    fn session_starts_since(&self, since_event_id: i64) -> Vec<String> {
        let events = self.events.lock().unwrap();
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        for e in events.iter() {
            if e.event_type == "session_start"
                && e.id > since_event_id
                && seen.insert(e.session_id.clone())
            {
                result.push(e.session_id.clone());
            }
        }
        result
    }
}
