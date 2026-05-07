//! In-memory `EventRepository` mock。SQLite 不要で AgentHER / ERL / Self-Verify
//! の test を高速化する (Clean Architecture Repository pattern、項目 209)。
//!
//! Phase 1 (Red): 構造体定義 + 全 method `todo!()` stub。
//! Phase 2 (Green): `Vec<Event>` ベースの in-memory 実装に差し替え。

use std::sync::Mutex;

use anyhow::Result;

use crate::agent::event_store::{Event, EventRepository, EventType, TrajectoryCandidate};

/// `Vec<Event>` 内蔵の test 用 mock。`Send + Sync` 互換のため `Mutex` でラップ。
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
        _session_id: &str,
        _event_type: &EventType,
        _event_data: &str,
        _step_index: Option<usize>,
    ) -> Result<i64> {
        todo!("Phase 2 Green で in-memory append")
    }

    fn replay(&self, _session_id: &str) -> Result<Vec<Event>> {
        todo!("Phase 2 Green で events filter by session_id")
    }

    fn count_by_type(&self, _session_id: &str) -> Result<Vec<(String, usize)>> {
        todo!("Phase 2 Green で events group by event_type")
    }

    fn total_count(&self) -> Result<usize> {
        todo!("Phase 2 Green で events.len()")
    }

    fn list_sessions(&self) -> Result<Vec<String>> {
        todo!("Phase 2 Green で SessionStart distinct session_id")
    }

    fn extract_successful_trajectories(
        &self,
        _min_tool_success_rate: f64,
        _min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>> {
        todo!("Phase 2 Green で extract_successful_trajectories_since_id(0, ...) 委譲")
    }

    fn extract_successful_trajectories_since_id(
        &self,
        _since_event_id: i64,
        _min_tool_success_rate: f64,
        _min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>> {
        todo!("Phase 2 Green で in-memory trajectory build")
    }

    fn extract_failed_trajectories(
        &self,
        _max_tool_success_rate: f64,
        _min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>> {
        todo!("Phase 2 Green で extract_failed_trajectories_since_id(0, ...) 委譲")
    }

    fn extract_failed_trajectories_since_id(
        &self,
        _since_event_id: i64,
        _max_tool_success_rate: f64,
        _min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>> {
        todo!("Phase 2 Green で in-memory failed trajectory build")
    }

    fn current_max_id(&self) -> Result<i64> {
        todo!("Phase 2 Green で next_id - 1 (cold-start で 0)")
    }
}
