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
    MagiHalt,
    MagiWarn,
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
            Self::MagiHalt => "magi_halt",
            Self::MagiWarn => "magi_warn",
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

/// ToolCall{Start,End} event payloadが「ユーザー取消 (Ctrl+C)」由来かを判定 (Issue #34)。
/// payload例: {"tool":"shell","success":false,"cancelled":true}。
/// - cancelledフィールド不在 → false (後方互換)
/// - parse不能なpayload → false (従来どおり失敗として集計、分母を変えない)
pub(crate) fn is_tool_call_cancelled(event_data: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(event_data)
        .ok()
        .and_then(|v| v.get("cancelled").and_then(|c| c.as_bool()))
        .unwrap_or(false)
}

/// session 内に「ユーザー取消 (Ctrl+C) 由来の tool call」が 1 件でも存在するか (Issue #34 follow-up)。
///
/// `tool_call_start` / `tool_call_end` の双方を対象にする
/// (`agent::tool_exec::apply_tool_result` は両 payload に `"cancelled": true` を刻む)。
///
/// # 用途
/// session を「エージェント能力のラベル付き標本」として使えるかの適格性判定。
/// 中断された session では「タスクが完遂したか」が観測不能になるため、
/// 成功・失敗いずれの学習信号にも使わない (docs/VALUES.md V1/V4、ADR-016)。
pub(crate) fn has_user_cancelled_tool_call(events: &[Event]) -> bool {
    events.iter().any(|e| {
        matches!(e.event_type.as_str(), "tool_call_start" | "tool_call_end")
            && is_tool_call_cancelled(&e.event_data)
    })
}

/// `&[Event]` から TrajectoryCandidate を構築する pure helper (項目 209)。
///
/// `EventStore::build_trajectory` (SQLite) と `MockEventRepository` (in-memory) の
/// 両方から共有される。SessionEnd 不在時は `None`。
/// `cancelled: true` な tool_call_start/end を 1 件でも含む session は、
/// 「タスクが完遂したか」が観測不能になるため trajectory 候補にしない (`None`、
/// Issue #34 follow-up、ADR-016)。この gate は構築点の単一箇所で強制されるため
/// `min_steps=0` の caller (項目 235 `BONSAI_FACTCHECK_ALL_TRAJECTORIES`) でも
/// 不変条件が保たれる。
///
/// gate を通過した session については tool_call_start/end の JSON payload から
/// `tool_sequence` と `tool_success_rate` を計算する。
///
/// **注意**: 上記 gate を緩める(部分キャンセル session を候補化する)変更を行う
/// 場合は、`tool_sequence` / `total_steps` / `tool_success_rate` から cancelled
/// tool call を除外する per-call フィルタを**同時に再導入**すること。gate と
/// per-call フィルタは一方だけでは信号純度を保てない。
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

    // Issue #34 follow-up: 部分キャンセルを含む session は「完遂したか」が観測
    // 不能なため、成功・失敗いずれの trajectory 候補にもしない (ADR-016)。
    if has_user_cancelled_tool_call(events) {
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
/// - `None` — task_type 不一致 / SessionEnd 不在 / cancelled な tool call (ユーザー
///   Ctrl+C 中断) を 1 件でも含む → sample 対象外 (Issue #34 follow-up、ADR-016)。
///   `build_trajectory_from_events` の gate と対称なポリシー
///   (`has_user_cancelled_tool_call` を共有)。
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
    // Issue #34 follow-up: cancelled な tool call を 1 件でも含む session は、
    // 「完遂したか」が観測不能なため検証サンプル対象外とする
    // (`build_trajectory_from_events` と対称、ADR-016)。
    if has_user_cancelled_tool_call(events) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用 `Event` を構築する (id/step_index/created_at は固定値、比較に不要)。
    fn ev(session_id: &str, event_type: &str, event_data: &str) -> Event {
        Event {
            id: 0,
            session_id: session_id.to_string(),
            event_type: event_type.to_string(),
            event_data: event_data.to_string(),
            step_index: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn t_is_tool_call_cancelled_detects_flag() {
        assert!(is_tool_call_cancelled(
            r#"{"tool":"shell","success":false,"cancelled":true}"#
        ));
    }

    #[test]
    fn t_is_tool_call_cancelled_absent_field_is_false() {
        assert!(!is_tool_call_cancelled(
            r#"{"tool":"shell","success":false}"#
        ));
    }

    #[test]
    fn t_is_tool_call_cancelled_malformed_json_is_false() {
        assert!(!is_tool_call_cancelled(""));
        assert!(!is_tool_call_cancelled("{"));
        assert!(!is_tool_call_cancelled("not json at all"));
    }

    #[test]
    fn t_build_trajectory_partial_cancel_is_not_a_candidate() {
        // Issue #34 follow-up: cancelled な tool call を 1 件でも含む session は
        // 分子・分母双方から除外するのではなく、trajectory 候補そのものから除外する
        // (部分キャンセルが「完遂」に反転する qa-reviewer 指摘への対応)。
        let events = vec![
            ev("s1", "session_start", "{}"),
            ev("s1", "tool_call_start", r#"{"tool":"a"}"#),
            ev("s1", "tool_call_end", r#"{"tool":"a","success":true}"#),
            ev("s1", "tool_call_start", r#"{"tool":"b"}"#),
            ev("s1", "tool_call_end", r#"{"tool":"b","success":false}"#),
            ev("s1", "tool_call_start", r#"{"tool":"c","cancelled":true}"#),
            ev(
                "s1",
                "tool_call_end",
                r#"{"tool":"c","success":false,"cancelled":true}"#,
            ),
            ev("s1", "session_end", "{}"),
        ];
        assert!(build_trajectory_from_events("s1", &events).is_none());
    }

    #[test]
    fn t_build_trajectory_all_cancelled_is_not_a_candidate() {
        let events = vec![
            ev("s1", "session_start", "{}"),
            ev("s1", "tool_call_start", r#"{"tool":"a","cancelled":true}"#),
            ev(
                "s1",
                "tool_call_end",
                r#"{"tool":"a","success":false,"cancelled":true}"#,
            ),
            ev("s1", "tool_call_start", r#"{"tool":"b","cancelled":true}"#),
            ev(
                "s1",
                "tool_call_end",
                r#"{"tool":"b","success":false,"cancelled":true}"#,
            ),
            ev("s1", "session_end", "{}"),
        ];
        assert!(build_trajectory_from_events("s1", &events).is_none());
    }

    #[test]
    fn t_build_trajectory_legacy_payload_unchanged() {
        let events = vec![
            ev("s1", "session_start", "{}"),
            ev("s1", "tool_call_start", r#"{"tool":"a"}"#),
            ev("s1", "tool_call_end", r#"{"tool":"a","success":true}"#),
            ev("s1", "tool_call_start", r#"{"tool":"b"}"#),
            ev("s1", "tool_call_end", r#"{"tool":"b","success":true}"#),
            ev("s1", "session_end", "{}"),
        ];
        let traj = build_trajectory_from_events("s1", &events).expect("session_end present");
        assert_eq!(traj.total_steps, 2);
        assert_eq!(traj.tool_success_rate, 1.0);
        assert_eq!(traj.tool_sequence, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn t_has_user_cancelled_tool_call_detects_start_only() {
        let events = vec![
            ev("s1", "session_start", "{}"),
            ev("s1", "tool_call_start", r#"{"tool":"a","cancelled":true}"#),
        ];
        assert!(has_user_cancelled_tool_call(&events));
    }

    #[test]
    fn t_has_user_cancelled_tool_call_false_on_clean_session() {
        let events = vec![
            ev("s1", "session_start", "{}"),
            ev("s1", "tool_call_start", r#"{"tool":"a"}"#),
            ev("s1", "tool_call_end", r#"{"tool":"a","success":true}"#),
            ev("s1", "session_end", "{}"),
        ];
        assert!(!has_user_cancelled_tool_call(&events));
    }

    #[test]
    fn t_build_trajectory_real_failure_without_cancel_still_candidate() {
        // 過剰除外していないことの回帰防止: cancelled 無しの実失敗は
        // 引き続き failed バケット経路 (Some) が生きている。
        let events = vec![
            ev("s1", "session_start", "{}"),
            ev("s1", "tool_call_start", r#"{"tool":"a"}"#),
            ev("s1", "tool_call_end", r#"{"tool":"a","success":true}"#),
            ev("s1", "tool_call_start", r#"{"tool":"b"}"#),
            ev("s1", "tool_call_end", r#"{"tool":"b","success":false}"#),
            ev("s1", "session_end", "{}"),
        ];
        let traj = build_trajectory_from_events("s1", &events).expect("no cancelled call");
        assert_eq!(traj.tool_success_rate, 0.5);
        assert_eq!(traj.total_steps, 2);
    }

    #[test]
    fn t_build_trajectory_malformed_payload_is_not_treated_as_cancelled() {
        // 後方互換: tool_call_end の payload が parse 不能でも `cancelled` 扱いにせず
        // 候補構築は継続する (`is_tool_call_cancelled` の malformed→false 方針と整合)。
        let events = vec![
            ev("s1", "session_start", "{}"),
            ev("s1", "tool_call_start", r#"{"tool":"a"}"#),
            ev("s1", "tool_call_end", "{"),
            ev("s1", "session_end", "{}"),
        ];
        assert!(build_trajectory_from_events("s1", &events).is_some());
    }

    #[test]
    fn t_build_trajectory_cancel_on_start_only_excludes_session() {
        // 防御的 parse 頑健性: cancelled フラグが tool_call_start にしか無くても
        // (tool_call_end 側に無くても) session を除外する。
        let events = vec![
            ev("s1", "session_start", "{}"),
            ev("s1", "tool_call_start", r#"{"tool":"a","cancelled":true}"#),
            ev("s1", "tool_call_end", r#"{"tool":"a","success":false}"#),
            ev("s1", "session_end", "{}"),
        ];
        assert!(build_trajectory_from_events("s1", &events).is_none());
    }

    #[test]
    fn t_classify_session_partial_cancel_is_not_a_sample() {
        // この test の前提が今回の修正対象: 部分キャンセルは `Some(true)` ではなく
        // `None` (標本対象外) を返す (Issue #34 follow-up)。
        let events = vec![
            ev("s1", "user_message", r#"{"content":"コマンドを実行して"}"#),
            ev("s1", "tool_call_start", r#"{"tool":"a"}"#),
            ev("s1", "tool_call_end", r#"{"tool":"a","success":true}"#),
            ev("s1", "tool_call_start", r#"{"tool":"b","cancelled":true}"#),
            ev(
                "s1",
                "tool_call_end",
                r#"{"tool":"b","success":false,"cancelled":true}"#,
            ),
            ev(
                "s1",
                "assistant_message",
                r#"{"content":"完了しました[検証済]"}"#,
            ),
            ev("s1", "session_end", "{}"),
        ];
        assert_eq!(
            classify_session_for_verification(&events, "shell_exec"),
            None
        );
    }

    #[test]
    fn t_classify_session_partial_cancel_without_marker_is_not_a_sample() {
        // 全 cancel 版 (`t_classify_session_all_cancelled_without_marker_is_not_a_sample`)
        // と対になる部分キャンセル版。
        let events = vec![
            ev("s1", "user_message", r#"{"content":"コマンドを実行して"}"#),
            ev("s1", "tool_call_start", r#"{"tool":"a"}"#),
            ev("s1", "tool_call_end", r#"{"tool":"a","success":true}"#),
            ev("s1", "tool_call_start", r#"{"tool":"b","cancelled":true}"#),
            ev(
                "s1",
                "tool_call_end",
                r#"{"tool":"b","success":false,"cancelled":true}"#,
            ),
            ev("s1", "assistant_message", r#"{"content":"中断しました"}"#),
            ev("s1", "session_end", "{}"),
        ];
        assert_eq!(
            classify_session_for_verification(&events, "shell_exec"),
            None
        );
    }

    #[test]
    fn t_classify_session_for_verification_still_false_on_real_failure() {
        let events = vec![
            ev("s1", "user_message", r#"{"content":"コマンドを実行して"}"#),
            ev("s1", "tool_call_start", r#"{"tool":"a"}"#),
            ev("s1", "tool_call_end", r#"{"tool":"a","success":false}"#),
            ev(
                "s1",
                "assistant_message",
                r#"{"content":"完了しました[検証済]"}"#,
            ),
            ev("s1", "session_end", "{}"),
        ];
        assert_eq!(
            classify_session_for_verification(&events, "shell_exec"),
            Some(false)
        );
    }

    #[test]
    fn t_classify_session_all_cancelled_is_not_a_sample() {
        let events = vec![
            ev("s1", "user_message", r#"{"content":"コマンドを実行して"}"#),
            ev("s1", "tool_call_start", r#"{"tool":"a","cancelled":true}"#),
            ev(
                "s1",
                "tool_call_end",
                r#"{"tool":"a","success":false,"cancelled":true}"#,
            ),
            ev("s1", "tool_call_start", r#"{"tool":"b","cancelled":true}"#),
            ev(
                "s1",
                "tool_call_end",
                r#"{"tool":"b","success":false,"cancelled":true}"#,
            ),
            ev(
                "s1",
                "assistant_message",
                r#"{"content":"完了しました[検証済]"}"#,
            ),
            ev("s1", "session_end", "{}"),
        ];
        assert_eq!(
            classify_session_for_verification(&events, "shell_exec"),
            None
        );
    }

    #[test]
    fn t_classify_session_all_cancelled_without_marker_is_not_a_sample() {
        let events = vec![
            ev("s1", "user_message", r#"{"content":"コマンドを実行して"}"#),
            ev("s1", "tool_call_start", r#"{"tool":"a","cancelled":true}"#),
            ev(
                "s1",
                "tool_call_end",
                r#"{"tool":"a","success":false,"cancelled":true}"#,
            ),
            ev("s1", "assistant_message", r#"{"content":"中断しました"}"#),
            ev("s1", "session_end", "{}"),
        ];
        assert_eq!(
            classify_session_for_verification(&events, "shell_exec"),
            None
        );
    }

    #[test]
    fn t_classify_session_no_tool_calls_unchanged() {
        let events = vec![
            ev("s1", "user_message", r#"{"content":"コマンドを実行して"}"#),
            ev(
                "s1",
                "assistant_message",
                r#"{"content":"完了しました[検証済]"}"#,
            ),
            ev("s1", "session_end", "{}"),
        ];
        assert_eq!(
            classify_session_for_verification(&events, "shell_exec"),
            Some(true)
        );
    }
}
