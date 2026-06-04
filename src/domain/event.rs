//! Event Sourcing のドメイン型・port・純粋ロジック。
//! 具象 EventStore<'a> (SQLite-backed) は agent::event_store に残置。
//! layer 順: domain 最下層 (他層依存ゼロ)。

use anyhow::Result;
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

pub(crate) fn compute_duration_ms(start: &str, end: &str) -> u64 {
    let parse = |s: &str| chrono::DateTime::parse_from_rfc3339(s).ok();
    match (parse(start), parse(end)) {
        (Some(s), Some(e)) => (e - s).num_milliseconds().max(0) as u64,
        _ => 0,
    }
}

/// `&[Event]` から TrajectoryCandidate を構築する pure helper (項目 209)。
///
/// `EventStore::build_trajectory` (SQLite) と `MockEventRepository` (in-memory) の
/// 両方から共有される。SessionEnd 不在時は `None`、tool_call_start/end の JSON
/// payload から `tool_sequence` と `tool_success_rate` を計算する。
pub(crate) fn build_trajectory_from_events(
    session_id: &str,
    events: &[Event],
) -> Option<TrajectoryCandidate> {
    if events.is_empty() {
        return None;
    }

    let has_session_end = events.iter().any(|e| e.event_type == "session_end");
    if !has_session_end {
        return None;
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

    for ev in events {
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

    Some(TrajectoryCandidate {
        session_id: session_id.to_string(),
        task_description,
        tool_sequence,
        tool_success_rate,
        total_steps: events
            .iter()
            .filter(|e| e.event_type == "tool_call_start")
            .count(),
        duration_ms,
    })
}

/// task_context から task_type を deterministic 分類 (項目 210)。
///
/// 4 カテゴリ: `code_edit` / `code_read` / `shell_exec` / `other`。優先順位は
/// **shell_exec → code_edit → code_read → other** (「実行」が最強指標、「実装」も
/// code_edit 扱い、「確認/読」は code_read fallback)。
pub(crate) fn classify_task_type(task_context: &str) -> &'static str {
    if task_context.contains("実行") || task_context.contains("コマンド") {
        return "shell_exec";
    }
    if task_context.contains("編集")
        || task_context.contains("修正")
        || task_context.contains("変更")
        || task_context.contains("リファクタ")
        || task_context.contains("実装")
    {
        return "code_edit";
    }
    if task_context.contains("読") || task_context.contains("確認") || task_context.contains("見て")
    {
        return "code_read";
    }
    "other"
}

/// 1 session の events から検証成功 (Verification Dilemma 文脈) 判定 (項目 210)。
///
/// Returns:
/// - `Some(true)`  — task_type 一致 + SessionEnd 済 + AssistantMessage[last] に
///   `[検証済]` 含有 + 全 ToolCallEnd success → 成功 sample
/// - `Some(false)` — task_type 一致 + SessionEnd 済 だが上記成功条件不満足 → 失敗 sample
/// - `None` — task_type 不一致 / SessionEnd 不在 → sample 対象外
pub(crate) fn classify_session_for_verification(
    events: &[Event],
    target_task_type: &str,
) -> Option<bool> {
    if !events.iter().any(|e| e.event_type == "session_end") {
        return None;
    }
    let task_ctx = events
        .iter()
        .find(|e| e.event_type == "user_message")
        .and_then(|e| serde_json::from_str::<serde_json::Value>(&e.event_data).ok())
        .and_then(|v| v.get("content").and_then(|c| c.as_str()).map(String::from))
        .unwrap_or_default();
    if classify_task_type(&task_ctx) != target_task_type {
        return None;
    }
    let last_assistant_marker = events
        .iter()
        .rfind(|e| e.event_type == "assistant_message")
        .and_then(|e| serde_json::from_str::<serde_json::Value>(&e.event_data).ok())
        .and_then(|v| v.get("content").and_then(|c| c.as_str()).map(String::from))
        .map(|s| s.contains("[検証済]"))
        .unwrap_or(false);
    let all_tools_ok = events
        .iter()
        .filter(|e| e.event_type == "tool_call_end")
        .all(|e| {
            serde_json::from_str::<serde_json::Value>(&e.event_data)
                .ok()
                .and_then(|v| v.get("success").and_then(|s| s.as_bool()))
                .unwrap_or(false)
        });
    Some(last_assistant_marker && all_tools_ok)
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
///
/// # 設計判断 (Phase 3 Refactor)
/// - **`Send + Sync` bound**: 付与しない (最小制約)。multi-thread 配信が必要に
///   なれば後付けで `: Send + Sync` 追加可。Mock は `Mutex` で thread-safe。
/// - **`TrajectoryCandidate::tool_chain_key`**: trait に含めず inherent 維持
///   (Event-agnostic な構造体 method、trait 化すると mock 側で重複定義必要)。
/// - **Mock の feature gate**: 採用せず production binary 込み (~150 行で size
///   影響軽微、ERL/Self-Verify 等の別モジュール test から import 容易)。
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

    /// 検証 step の経験的成功率 (Self-Verification Dilemma、項目 210、arxiv 2602.03485)。
    ///
    /// task_type 別に過去の SessionEnd を辿り、`[検証済]` を含む FinalAnswer かつ
    /// 全 ToolCallEnd success の session を「成功」と定義してその比率を返す。
    /// sample 数が `min_samples` 未満なら `None` (cold-start fallback で skip 無効)。
    ///
    /// 用途: `AdvisorConfig::dynamic_skip_threshold` と比較して
    /// `inject_verification_step` の skip 判断に使用。
    fn verification_success_rate(&self, task_type: &str, min_samples: usize)
    -> Result<Option<f64>>;
}
