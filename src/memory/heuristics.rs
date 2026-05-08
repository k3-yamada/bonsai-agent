//! ERL (Experiential Reflective Learning) heuristics pool
//!
//! Reflexion 由来の自然言語助言を、SkillStore (tool_chain) / ExperienceStore (record) /
//! Vault (rules) と並ぶ第 4 メモリ層として保管する。
//!
//! 由来: arxiv 2603.24639 (2026-03)、Gaia2 で +7.8% over ReAct。
//! plan: `.claude/plan/erl-heuristics-pool-impl-v2.md`
//! 項目: 213 候補 (CLAUDE.md、plan G-5 完了時に追記)
//!
//! # 設計
//! - SQLite が source-of-truth (V10 = heuristics テーブル新規)
//! - inherent API (trait 化なし、F8 audit)、event 読取のみ EventRepository trait 経由
//! - tool_chain 表現可能な advice は SkillStore::promote_from_erl_advice に流す (F5/F6 audit)
//! - reflection LLM call は session ごと 1 回、parse 失敗は non-fatal (F7 audit)

use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::agent::event_store::EventRepository;
use crate::runtime::inference::LlmBackend;

/// 自然言語助言 1 件 (≤200 chars 推奨)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heuristic {
    pub id: i64,
    pub advice: String,
    pub trigger_patterns: String,
    pub source_session_id: Option<String>,
    pub source_task: String,
    pub category: String,
    pub score: f64,
    pub used_count: i64,
    pub success_after_use: i64,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

/// `extract_heuristics_from_events` から返る pre-persistence 候補。
#[derive(Debug, Clone, PartialEq)]
pub struct HeuristicCandidate {
    pub advice: String,
    pub trigger_patterns: Vec<String>,
    pub category: String,
    pub source_session_id: String,
    pub source_task: String,
}

/// `run_heuristics_pass` の集計サマリ (Lab post hook log 用)。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HeuristicSummary {
    pub extracted: usize,
    pub saved: usize,
    pub skipped_to_skill: usize,
    pub pruned: usize,
    pub parse_failures: usize,
}

/// SQLite-backed heuristic 永続化ストア。
pub struct HeuristicStore<'a> {
    conn: &'a Connection,
}

impl<'a> HeuristicStore<'a> {
    /// new は構築のみ (Phase 1 Red 時点でも動作、conn を保持するだけ)。
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// 自然言語助言を保管。content fingerprint dedup (advice 先頭 80 chars + trigger_hash) あり。
    pub fn save(
        &self,
        advice: &str,
        triggers: &[String],
        source_session_id: Option<&str>,
        source_task: &str,
        category: &str,
    ) -> Result<i64> {
        let _ = (
            &self.conn,
            advice,
            triggers,
            source_session_id,
            source_task,
            category,
        );
        todo!("Phase 2 Green: ERL HeuristicStore::save (項目 213)")
    }

    /// `task_context` の trigger_patterns マッチ + score 順 top-K。
    pub fn find_top_k_for_task(&self, task_context: &str, k: usize) -> Result<Vec<Heuristic>> {
        let _ = (&self.conn, task_context, k);
        todo!("Phase 2 Green: ERL HeuristicStore::find_top_k_for_task")
    }

    /// 注入済 heuristic に対する task 完了結果を反映 (utility update)。
    pub fn record_outcome(&self, id: i64, task_succeeded: bool) -> Result<()> {
        let _ = (&self.conn, id, task_succeeded);
        todo!("Phase 2 Green: ERL HeuristicStore::record_outcome")
    }

    /// 月次 prune (score < 0.2 + used_count 条件 / created_at 30 日 + 上限 200)。
    pub fn prune(&self) -> Result<usize> {
        let _ = &self.conn;
        todo!("Phase 2 Green: ERL HeuristicStore::prune")
    }

    /// Lab cycle 跨ぎの汚染リセット (項目 206 reset_session_data_for_lab と協調)。
    pub fn reset_for_lab(&self) -> Result<()> {
        let _ = &self.conn;
        todo!("Phase 2 Green: ERL HeuristicStore::reset_for_lab")
    }
}

/// `EventRepository` (項目 209) 経由で events を読み、Reflexion で heuristic 候補を抽出。
///
/// - 1 session につき 1 LLM call (Bonsai-8B context 短さ対応)
/// - JSON-only output 厳格 prompt (parse 失敗 session は skip、Lab failure 化させない)
/// - tool_chain 表現可能な advice は本関数の戻り値から **除外** (caller の `run_heuristics_pass`
///   が SkillStore::promote_from_erl_advice に routing する設計、F5/F6 audit)
pub fn extract_heuristics_from_events(
    events: &dyn EventRepository,
    since_event_id: i64,
    backend: &dyn LlmBackend,
) -> Result<Vec<HeuristicCandidate>> {
    let _ = (events, since_event_id, backend);
    todo!("Phase 2 Green: ERL extract_heuristics_from_events")
}

/// 助言テキストに 2 個以上の known tool 名が「順序付き、≤8 token gap、両者間に逆接なし」で
/// 出現するか検出。検出時は SkillStore へ routing (caller 責任)。
pub(crate) fn detect_tool_chain_in_advice(
    advice: &str,
    known_tools: &[&str],
) -> Option<Vec<String>> {
    let _ = (advice, known_tools);
    todo!("Phase 2 Green: ERL detect_tool_chain_in_advice")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::event_store::EventType;
    use crate::memory::mocks::event_repository_mock::MockEventRepository;
    use crate::memory::store::MemoryStore;
    use crate::runtime::inference::MockLlmBackend;

    // ===========================================================================
    // HeuristicStore basic operations (8 tests、plan §5 Phase 1 Red)
    // ===========================================================================

    #[test]
    fn t_heuristic_store_save_basic() {
        let store = MemoryStore::in_memory().unwrap();
        let h = HeuristicStore::new(store.conn());
        let id = h
            .save(
                "ファイル不在エラーは create_dir_all で親ディレクトリを先回り作成",
                &["not found".to_string(), "ENOENT".to_string()],
                Some("session_001"),
                "FizzBuzz 実装",
                "failure_recovery",
            )
            .unwrap();
        assert!(id > 0, "save は正の id を返す");
    }

    #[test]
    fn t_heuristic_store_dedup_fingerprint() {
        let store = MemoryStore::in_memory().unwrap();
        let h = HeuristicStore::new(store.conn());
        let id1 = h
            .save(
                "Same advice content for fingerprint test",
                &["x".to_string(), "y".to_string()],
                None,
                "task1",
                "efficiency",
            )
            .unwrap();
        let id2 = h
            .save(
                "Same advice content for fingerprint test",
                &["x".to_string(), "y".to_string()],
                None,
                "task2",
                "efficiency",
            )
            .unwrap();
        assert_eq!(
            id1, id2,
            "advice 先頭 80 chars + trigger_hash fingerprint で dedup"
        );
    }

    #[test]
    fn t_extract_heuristics_requires_session_end() {
        let mock = MockEventRepository::new();
        mock.append("s1", &EventType::SessionStart, "{}", None)
            .unwrap();
        mock.append(
            "s1",
            &EventType::ToolCallStart,
            r#"{"tool":"shell"}"#,
            Some(1),
        )
        .unwrap();
        mock.append(
            "s1",
            &EventType::ToolCallEnd,
            r#"{"tool":"shell","success":false}"#,
            Some(1),
        )
        .unwrap();
        let backend = MockLlmBackend::new(vec!["[]".to_string()]);
        let candidates = extract_heuristics_from_events(&mock, 0, &backend).unwrap();
        assert!(
            candidates.is_empty(),
            "SessionEnd 不在で extract は 0 件 (項目 162 同基準)"
        );
    }

    #[test]
    fn t_extract_heuristics_min_steps() {
        let mock = MockEventRepository::new();
        mock.append("s1", &EventType::SessionStart, "{}", None)
            .unwrap();
        mock.append(
            "s1",
            &EventType::ToolCallStart,
            r#"{"tool":"shell"}"#,
            Some(1),
        )
        .unwrap();
        mock.append("s1", &EventType::SessionEnd, "{}", None)
            .unwrap();
        let backend = MockLlmBackend::new(vec!["[]".to_string()]);
        let candidates = extract_heuristics_from_events(&mock, 0, &backend).unwrap();
        assert!(
            candidates.is_empty(),
            "min_steps 2 未満で extract は 0 件 (項目 201 HSL と同基準)"
        );
    }

    #[test]
    fn t_extract_heuristics_returns_skill_for_tool_chain() {
        let mock = MockEventRepository::new();
        mock.append("s1", &EventType::SessionStart, "{}", None)
            .unwrap();
        mock.append(
            "s1",
            &EventType::UserMessage,
            r#"{"content":"FizzBuzz 実装"}"#,
            None,
        )
        .unwrap();
        mock.append(
            "s1",
            &EventType::ToolCallStart,
            r#"{"tool":"file_read"}"#,
            Some(1),
        )
        .unwrap();
        mock.append(
            "s1",
            &EventType::ToolCallEnd,
            r#"{"tool":"file_read","success":false}"#,
            Some(1),
        )
        .unwrap();
        mock.append(
            "s1",
            &EventType::ToolCallStart,
            r#"{"tool":"shell"}"#,
            Some(2),
        )
        .unwrap();
        mock.append(
            "s1",
            &EventType::ToolCallEnd,
            r#"{"tool":"shell","success":false}"#,
            Some(2),
        )
        .unwrap();
        mock.append("s1", &EventType::SessionEnd, "{}", None)
            .unwrap();
        let backend = MockLlmBackend::new(vec![
            r#"[{"advice":"Use file_read then shell to verify","trigger_patterns":["fizzbuzz","verify"],"category":"efficiency"}]"#
                .to_string(),
        ]);
        let candidates = extract_heuristics_from_events(&mock, 0, &backend).unwrap();
        assert!(
            candidates.is_empty(),
            "tool_chain 表現可能 advice は extract 戻り値から除外 (skill ルート、F5/F6 audit)"
        );
    }

    #[test]
    fn t_extract_heuristics_parse_failure_skips_session() {
        let mock = MockEventRepository::new();
        mock.append("s1", &EventType::SessionStart, "{}", None)
            .unwrap();
        mock.append(
            "s1",
            &EventType::ToolCallStart,
            r#"{"tool":"shell"}"#,
            Some(1),
        )
        .unwrap();
        mock.append(
            "s1",
            &EventType::ToolCallEnd,
            r#"{"tool":"shell","success":false}"#,
            Some(1),
        )
        .unwrap();
        mock.append(
            "s1",
            &EventType::ToolCallStart,
            r#"{"tool":"shell"}"#,
            Some(2),
        )
        .unwrap();
        mock.append(
            "s1",
            &EventType::ToolCallEnd,
            r#"{"tool":"shell","success":false}"#,
            Some(2),
        )
        .unwrap();
        mock.append("s1", &EventType::SessionEnd, "{}", None)
            .unwrap();
        let backend = MockLlmBackend::new(vec!["this is not JSON {{{".to_string()]);
        let result = extract_heuristics_from_events(&mock, 0, &backend);
        assert!(result.is_ok(), "parse 失敗は Err 化せず non-fatal (F7 audit)");
        assert!(
            result.unwrap().is_empty(),
            "malformed JSON session は skip して 0 件返却"
        );
    }

    #[test]
    fn t_find_top_k_for_task_filters_by_trigger() {
        let store = MemoryStore::in_memory().unwrap();
        let h = HeuristicStore::new(store.conn());
        h.save(
            "advice A: about file errors",
            &["file_not_found".to_string(), "ENOENT".to_string()],
            None,
            "task A",
            "failure_recovery",
        )
        .unwrap();
        h.save(
            "advice B: about shell timeouts",
            &["timeout".to_string(), "shell hang".to_string()],
            None,
            "task B",
            "efficiency",
        )
        .unwrap();
        let results = h
            .find_top_k_for_task("ファイルが見つからない (file_not_found)", 5)
            .unwrap();
        assert_eq!(
            results.len(),
            1,
            "trigger_patterns マッチでフィルタ、無関係 advice は返さない"
        );
        assert!(results[0].advice.contains("file errors"));
    }

    #[test]
    fn t_record_outcome_updates_score() {
        let store = MemoryStore::in_memory().unwrap();
        let h = HeuristicStore::new(store.conn());
        let id = h
            .save(
                "test advice for outcome",
                &["xyz".to_string(), "test trigger".to_string()],
                None,
                "task",
                "efficiency",
            )
            .unwrap();
        h.record_outcome(id, true).unwrap();
        let results = h
            .find_top_k_for_task("xyz pattern test trigger", 5)
            .unwrap();
        let target = results
            .iter()
            .find(|h| h.id == id)
            .expect("saved heuristic 検索可");
        assert_eq!(target.used_count, 1, "record_outcome で used_count++");
        assert_eq!(
            target.success_after_use, 1,
            "task_succeeded=true で success_after_use++"
        );
    }

    // ===========================================================================
    // detect_tool_chain_in_advice (table-driven、6 cases、plan §4.4 / F6 audit)
    // ===========================================================================

    const KNOWN_TOOLS: &[&str] = &[
        "file_read",
        "file_write",
        "shell",
        "git",
        "web_fetch",
        "repomap",
        "multi_edit",
        "grep",
        "glob",
    ];

    #[test]
    fn t_detect_tool_chain_positive_two_tools_ordered() {
        let result = detect_tool_chain_in_advice("Use file_read then shell to verify", KNOWN_TOOLS);
        assert_eq!(
            result,
            Some(vec!["file_read".to_string(), "shell".to_string()]),
            "順序付き 2 tool は tool_chain 検出"
        );
    }

    #[test]
    fn t_detect_tool_chain_positive_japanese_glue() {
        let result = detect_tool_chain_in_advice("shell でテスト後 file_write で記録", KNOWN_TOOLS);
        assert_eq!(
            result,
            Some(vec!["shell".to_string(), "file_write".to_string()]),
            "日本語接続でも 2 tool 順序付き検出"
        );
    }

    #[test]
    fn t_detect_tool_chain_negative_single_tool() {
        let result = detect_tool_chain_in_advice("file_read だけで OK", KNOWN_TOOLS);
        assert_eq!(result, None, "tool 1 個のみは tool_chain ではない");
    }

    #[test]
    fn t_detect_tool_chain_negative_disjunction() {
        let result =
            detect_tool_chain_in_advice("file_read but use file_write instead", KNOWN_TOOLS);
        assert_eq!(result, None, "逆接 (but) で tool_chain 不成立");
    }

    #[test]
    fn t_detect_tool_chain_negative_gap_too_large() {
        let filler = (0..50)
            .map(|i| format!("word{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let advice = format!("file_read {filler} shell");
        let result = detect_tool_chain_in_advice(&advice, KNOWN_TOOLS);
        assert_eq!(result, None, "gap > 8 token は tool_chain ではない");
    }

    #[test]
    fn t_detect_tool_chain_positive_three_tools_subset() {
        let result =
            detect_tool_chain_in_advice("Run shell, then check repomap and grep", KNOWN_TOOLS);
        assert!(
            result.is_some(),
            "3 tool でも先頭 2 つで tool_chain 検出: {result:?}"
        );
        let chain = result.unwrap();
        assert!(chain.len() >= 2, "≥2 tool 検出");
    }

    // ===========================================================================
    // Mock event repository test (1 件、項目 209 dividend、F8 audit)
    // ===========================================================================

    #[test]
    fn t_extract_heuristics_with_mock_event_repository() {
        let mock = MockEventRepository::new();
        mock.append("s_mock", &EventType::SessionStart, "{}", None)
            .unwrap();
        mock.append(
            "s_mock",
            &EventType::UserMessage,
            r#"{"content":"テスト"}"#,
            None,
        )
        .unwrap();
        mock.append(
            "s_mock",
            &EventType::ToolCallStart,
            r#"{"tool":"file_read"}"#,
            Some(1),
        )
        .unwrap();
        mock.append(
            "s_mock",
            &EventType::ToolCallEnd,
            r#"{"tool":"file_read","success":false}"#,
            Some(1),
        )
        .unwrap();
        mock.append(
            "s_mock",
            &EventType::ToolCallStart,
            r#"{"tool":"file_read"}"#,
            Some(2),
        )
        .unwrap();
        mock.append(
            "s_mock",
            &EventType::ToolCallEnd,
            r#"{"tool":"file_read","success":false}"#,
            Some(2),
        )
        .unwrap();
        mock.append("s_mock", &EventType::SessionEnd, "{}", None)
            .unwrap();
        let backend = MockLlmBackend::new(vec![
            r#"[{"advice":"存在しないファイルを開く前に存在確認","trigger_patterns":["not found","ENOENT","ファイル不在"],"category":"failure_recovery"}]"#
                .to_string(),
        ]);
        let candidates = extract_heuristics_from_events(&mock, 0, &backend).unwrap();
        assert!(
            !candidates.is_empty(),
            "Mock 経由でも extract は機能 (SQLite 不要、F8 audit)"
        );
        let c = &candidates[0];
        assert_eq!(c.category, "failure_recovery");
        assert_eq!(c.source_session_id, "s_mock");
    }
}
