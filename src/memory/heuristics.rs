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

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::agent::conversation::Message;
use crate::agent::event_store::{Event, EventRepository};
use crate::cancel::CancellationToken;
use crate::runtime::inference::LlmBackend;

/// Reflection prompt template (`prompts/heuristic_reflection.txt`、compile-time embed)。
const REFLECTION_PROMPT_TEMPLATE: &str = include_str!("../../prompts/heuristic_reflection.txt");

/// `BONSAI_ERL_ENABLED=1` (or "true"、case-insensitive) で ERL 機構全体を opt-in。
///
/// - production default = env unset = false 返却 = OFF (項目 216、Lab v17 REJECT 反映)
/// - opt-in 復活: `BONSAI_ERL_ENABLED=1` で `inject_heuristics` +
///   `run_heuristics_pass` を有効化、項目 213 (Phase 2 Green) 動作の再現用
/// - 値が "0" / "false" / 空文字列など `1`/`true` 以外なら enabled 扱いせず false
///
/// 切替経緯: Lab v17 effectiveness 検証 (項目 215) で paired t-test
/// Δmean=−0.0014 / p=0.5072 → ACCEPT 基準 (Δ≥+0.015 AND p<0.1) 両条件未達で
/// REJECT 確定。H_ERL 仮説棄却を受け production default を ON → OFF に切替。
/// 副次 finding (stability 軸 ON 優位) は Plan B (ReviewState V12) で別軸対応。
/// dead-code 化は将来別 plan、env=1 で復活可能性を残す。
pub(crate) fn is_erl_enabled() -> bool {
    std::env::var("BONSAI_ERL_ENABLED")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// `detect_tool_chain_in_advice` のデフォルト ALLOWLIST。
/// 現行 ToolRegistry に含まれる tool 名と整合 (項目 213 plan §4.4)。
pub const KNOWN_TOOLS: &[&str] = &[
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

/// reflection 抽出結果の richer 版 (`run_heuristics_pass` 用、tool_chain advice の routing
/// を caller 責務にしつつ、parse_failures count を summary log に伝える)。
#[derive(Debug, Clone, Default)]
pub struct ReflectionResult {
    pub candidates: Vec<HeuristicCandidate>,
    /// (tool_chain, advice, source_session_id) — caller (run_heuristics_pass)
    /// が `SkillStore::promote_from_erl_advice` に routing する責務。
    pub tool_chain_advice: Vec<(Vec<String>, String, String)>,
    pub parse_failures: usize,
}

/// SQLite-backed heuristic 永続化ストア。
pub struct HeuristicStore<'a> {
    conn: &'a Connection,
}

impl<'a> HeuristicStore<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// 自然言語助言を保管。content fingerprint dedup (advice 先頭 80 chars + trigger_hash) あり。
    /// 重複時は既存 id を返す (項目 206 deterministic dedup と同方針)。
    pub fn save(
        &self,
        advice: &str,
        triggers: &[String],
        source_session_id: Option<&str>,
        source_task: &str,
        category: &str,
    ) -> Result<i64> {
        let fp = fingerprint(advice, triggers);
        // 既存判定: fingerprint UNIQUE を活用
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM heuristics WHERE fingerprint = ?1",
                params![&fp],
                |row| row.get(0),
            )
            .ok();
        if let Some(id) = existing {
            return Ok(id);
        }

        let triggers_json = serde_json::to_string(triggers).unwrap_or_else(|_| "[]".to_string());
        let truncated_task: String = source_task.chars().take(80).collect();
        let now = chrono::Utc::now().to_rfc3339();

        self.conn.execute(
            "INSERT INTO heuristics
             (advice, trigger_patterns, source_session_id, source_task, category,
              score, used_count, success_after_use, fingerprint, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 0.5, 0, 0, ?6, ?7)",
            params![
                advice,
                &triggers_json,
                source_session_id,
                &truncated_task,
                category,
                &fp,
                &now,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// `task_context` の trigger_patterns マッチ + score 順 top-K。
    ///
    /// マッチ判定: 各 trigger 文字列が `task_context` の case-sensitive 部分文字列なら hit。
    pub fn find_top_k_for_task(&self, task_context: &str, k: usize) -> Result<Vec<Heuristic>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, advice, trigger_patterns, source_session_id, source_task, category,
                    score, used_count, success_after_use, created_at, last_used_at
             FROM heuristics
             ORDER BY score DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Heuristic {
                id: row.get(0)?,
                advice: row.get(1)?,
                trigger_patterns: row.get(2)?,
                source_session_id: row.get(3)?,
                source_task: row.get(4)?,
                category: row.get(5)?,
                score: row.get(6)?,
                used_count: row.get(7)?,
                success_after_use: row.get(8)?,
                created_at: row.get(9)?,
                last_used_at: row.get(10)?,
            })
        })?;

        let mut hits: Vec<Heuristic> = Vec::new();
        for row in rows {
            let h = row?;
            if matches_trigger(&h.trigger_patterns, task_context) {
                hits.push(h);
                if hits.len() >= k {
                    break;
                }
            }
        }
        Ok(hits)
    }

    /// 注入済 heuristic に対する task 完了結果を反映 (utility update)。
    pub fn record_outcome(&self, id: i64, task_succeeded: bool) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let success_inc: i64 = if task_succeeded { 1 } else { 0 };
        self.conn.execute(
            "UPDATE heuristics
             SET used_count = used_count + 1,
                 success_after_use = success_after_use + ?1,
                 last_used_at = ?2
             WHERE id = ?3",
            params![success_inc, &now, id],
        )?;
        Ok(())
    }

    /// 月次 prune (score < 0.2 + used_count 条件 / 上限 200 超過は score 昇順削除)。
    pub fn prune(&self) -> Result<usize> {
        let mut total: usize = 0;
        // 1. 低スコア + 試行回数が enough にも関わらず utility が伸びない
        total += self.conn.execute(
            "DELETE FROM heuristics WHERE score < 0.2 AND used_count >= 5",
            [],
        )?;
        // 2. 30 日経過 + 1 度も使われていない
        total += self.conn.execute(
            "DELETE FROM heuristics
             WHERE used_count = 0
               AND created_at < datetime('now', '-30 day')",
            [],
        )?;
        // 3. 総数 200 超過 → score 昇順で超過分削除
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM heuristics", [], |row| row.get(0))?;
        if count > 200 {
            let excess = count - 200;
            total += self.conn.execute(
                "DELETE FROM heuristics WHERE id IN (
                     SELECT id FROM heuristics ORDER BY score ASC, id ASC LIMIT ?1
                 )",
                params![excess],
            )?;
        }
        Ok(total)
    }

    /// Lab cycle 跨ぎの汚染リセット (項目 206 reset_session_data_for_lab と協調)。
    /// 明示的 reset 要求時のみ呼ばれる前提 (production 通常運用では使わない)。
    pub fn reset_for_lab(&self) -> Result<()> {
        self.conn.execute("DELETE FROM heuristics", [])?;
        Ok(())
    }
}

/// fingerprint = advice 先頭 80 chars (UTF-8 char unit) + trigger ハッシュ。
fn fingerprint(advice: &str, triggers: &[String]) -> String {
    let head: String = advice.chars().take(80).collect();
    let mut hasher = DefaultHasher::new();
    for t in triggers {
        t.hash(&mut hasher);
    }
    format!("{head}|{:016x}", hasher.finish())
}

/// trigger_patterns (JSON 文字列) が task_context にマッチするか判定。
fn matches_trigger(trigger_patterns_json: &str, task_context: &str) -> bool {
    let triggers: Vec<String> = serde_json::from_str(trigger_patterns_json).unwrap_or_default();
    triggers
        .iter()
        .any(|t| !t.is_empty() && task_context.contains(t.as_str()))
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
    Ok(extract_reflection_full(events, since_event_id, backend)?.candidates)
}

/// run_heuristics_pass 内部用 — heuristic 候補 + tool_chain advice + parse_failures を同時返却。
pub(crate) fn extract_reflection_full(
    events: &dyn EventRepository,
    since_event_id: i64,
    backend: &dyn LlmBackend,
) -> Result<ReflectionResult> {
    const MIN_STEPS: usize = 2;
    const FAILED_MAX_RATE: f64 = 0.8;
    const SUCCESS_MIN_RATE: f64 = 0.8;

    let failed =
        events.extract_failed_trajectories_since_id(since_event_id, FAILED_MAX_RATE, MIN_STEPS)?;
    let successful = events.extract_successful_trajectories_since_id(
        since_event_id,
        SUCCESS_MIN_RATE,
        MIN_STEPS,
    )?;

    let mut all: Vec<(String, String, bool)> = Vec::new();
    for c in failed {
        all.push((c.session_id, c.task_description, false));
    }
    for c in successful {
        all.push((c.session_id, c.task_description, true));
    }

    let mut result = ReflectionResult::default();
    let cancel = CancellationToken::new();

    for (session_id, task_description, is_success) in all {
        let evts = events.replay(&session_id)?;
        let event_summary = summarize_events(&evts);
        let outcome_label = if is_success { "success" } else { "failure" };
        let user_prompt = REFLECTION_PROMPT_TEMPLATE
            .replace("{task_description}", &task_description)
            .replace("{outcome}", outcome_label)
            .replace("{event_summary}", &event_summary);
        let messages = vec![Message::user(user_prompt)];

        let mut on_token = |_: &str| {};
        // F7 plan v2 / Codex audit MEDIUM #3: reflection は temp=0.3 max_tokens=400
        // で cap (Lab duration regression / 1bit malformed 出力 抑制)。
        let params = crate::config::InferenceParams {
            temperature: 0.3,
            max_tokens: 400,
            ..crate::config::InferenceParams::default()
        };
        let llm_out =
            match backend.generate_with_params(&messages, &[], &mut on_token, &cancel, &params) {
                Ok(g) => g,
                Err(_) => {
                    result.parse_failures += 1;
                    continue;
                }
            };

        let raw_items: Vec<RawHeuristic> = match parse_reflection_json(&llm_out.text) {
            Some(items) => items,
            None => {
                result.parse_failures += 1;
                continue;
            }
        };

        for item in raw_items.into_iter().take(3) {
            if !is_valid_raw(&item) {
                continue;
            }
            let truncated_task: String = task_description.chars().take(80).collect();
            if let Some(chain) = detect_tool_chain_in_advice(&item.advice, KNOWN_TOOLS) {
                result
                    .tool_chain_advice
                    .push((chain, item.advice, session_id.clone()));
            } else {
                result.candidates.push(HeuristicCandidate {
                    advice: item.advice,
                    trigger_patterns: item.trigger_patterns,
                    category: item.category,
                    source_session_id: session_id.clone(),
                    source_task: truncated_task,
                });
            }
        }
    }

    Ok(result)
}

/// reflection LLM 出力 (期待: JSON-only) を strict にパース (Codex audit MEDIUM #2)。
/// `[`...`]` の前後に prose や markdown が混じる出力は parse 失敗 = parse_failures カウント
/// 対象 = prompt 不遵守の signal。前後の空白のみ trim する。
fn parse_reflection_json(text: &str) -> Option<Vec<RawHeuristic>> {
    let trimmed = text.trim();
    if !trimmed.starts_with('[') || !trimmed.ends_with(']') {
        return None;
    }
    serde_json::from_str::<Vec<RawHeuristic>>(trimmed).ok()
}

#[derive(Debug, Clone, Deserialize)]
struct RawHeuristic {
    advice: String,
    trigger_patterns: Vec<String>,
    category: String,
}

fn is_valid_raw(item: &RawHeuristic) -> bool {
    if item.advice.trim().is_empty() {
        return false;
    }
    if item.advice.chars().count() > 200 {
        return false;
    }
    if item.trigger_patterns.len() < 2 || item.trigger_patterns.len() > 5 {
        return false;
    }
    if item.trigger_patterns.iter().any(|t| t.chars().count() < 4) {
        return false;
    }
    matches!(
        item.category.as_str(),
        "failure_recovery" | "efficiency" | "verification"
    )
}

/// reflection prompt 用のイベント要約 (token 抑制のため tool_call_*/user_message を 1 行ずつ)。
fn summarize_events(events: &[Event]) -> String {
    let mut lines = Vec::new();
    for ev in events {
        match ev.event_type.as_str() {
            "user_message" => {
                let snippet = ev.event_data.chars().take(120).collect::<String>();
                lines.push(format!("USER: {snippet}"));
            }
            "tool_call_start" => {
                let snippet = ev.event_data.chars().take(120).collect::<String>();
                lines.push(format!("TOOL_START: {snippet}"));
            }
            "tool_call_end" => {
                let snippet = ev.event_data.chars().take(120).collect::<String>();
                lines.push(format!("TOOL_END: {snippet}"));
            }
            _ => {}
        }
        if lines.len() >= 30 {
            // 上限で切り詰め (Bonsai-8B context 節約)
            lines.push("... (truncated)".to_string());
            break;
        }
    }
    lines.join("\n")
}

/// 助言テキストに 2 個以上の known tool 名が「順序付き、≤8 token gap、両者間に逆接なし」で
/// 出現するか検出。検出時は SkillStore へ routing (caller 責任)。
///
/// Codex audit HIGH #2 / MEDIUM #1 反映:
/// - token 境界は backtick / 括弧 / 引用符を strip して word-boundary 化
/// - 隣接 hit pair を windowing で順次評価し、最初の有効 chain を採用
///   (`Run shell, but check repomap and grep` → `repomap -> grep` を見つける)
pub(crate) fn detect_tool_chain_in_advice(
    advice: &str,
    known_tools: &[&str],
) -> Option<Vec<String>> {
    const MAX_GAP: usize = 8;
    let tokens: Vec<String> = advice
        .split(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    ',' | '.' | ';' | ':' | '!' | '?' | '、' | '。' | '，' | '．'
                )
        })
        .map(|s| s.trim_matches(|c: char| !(c.is_alphanumeric() || c == '_')))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();

    let mut hits: Vec<(usize, String)> = Vec::new();
    for (i, tok) in tokens.iter().enumerate() {
        for &known in known_tools {
            if tok == known {
                hits.push((i, known.to_string()));
                break;
            }
        }
    }

    if hits.len() < 2 {
        return None;
    }

    // Windowing: 各隣接 hit pair を評価、逆接なし & gap≤8 を満たす最初の chain を返す。
    for window_start in 0..hits.len() - 1 {
        let (i1, t1) = hits[window_start].clone();
        let (i2, t2) = hits[window_start + 1].clone();
        if i2.saturating_sub(i1) > MAX_GAP {
            continue;
        }
        let between = &tokens[i1 + 1..i2];
        if between.iter().any(|tok| {
            matches!(
                tok.to_lowercase().as_str(),
                "but" | "however" | "しかし" | "ただし"
            )
        }) {
            continue;
        }
        let mut chain = vec![t1, t2];
        let mut last_idx = i2;
        for (i, t) in hits.iter().skip(window_start + 2) {
            if i.saturating_sub(last_idx) > MAX_GAP {
                break;
            }
            chain.push(t.clone());
            last_idx = *i;
        }
        return Some(chain);
    }
    None
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
        assert!(
            result.is_ok(),
            "parse 失敗は Err 化せず non-fatal (F7 audit)"
        );
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

    const KNOWN_TOOLS_TEST: &[&str] = &[
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
        let result =
            detect_tool_chain_in_advice("Use file_read then shell to verify", KNOWN_TOOLS_TEST);
        assert_eq!(
            result,
            Some(vec!["file_read".to_string(), "shell".to_string()]),
            "順序付き 2 tool は tool_chain 検出"
        );
    }

    #[test]
    fn t_detect_tool_chain_positive_japanese_glue() {
        let result =
            detect_tool_chain_in_advice("shell でテスト後 file_write で記録", KNOWN_TOOLS_TEST);
        assert_eq!(
            result,
            Some(vec!["shell".to_string(), "file_write".to_string()]),
            "日本語接続でも 2 tool 順序付き検出"
        );
    }

    #[test]
    fn t_detect_tool_chain_negative_single_tool() {
        let result = detect_tool_chain_in_advice("file_read だけで OK", KNOWN_TOOLS_TEST);
        assert_eq!(result, None, "tool 1 個のみは tool_chain ではない");
    }

    #[test]
    fn t_detect_tool_chain_negative_disjunction() {
        let result =
            detect_tool_chain_in_advice("file_read but use file_write instead", KNOWN_TOOLS_TEST);
        assert_eq!(result, None, "逆接 (but) で tool_chain 不成立");
    }

    #[test]
    fn t_detect_tool_chain_negative_gap_too_large() {
        let filler = (0..50)
            .map(|i| format!("word{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let advice = format!("file_read {filler} shell");
        let result = detect_tool_chain_in_advice(&advice, KNOWN_TOOLS_TEST);
        assert_eq!(result, None, "gap > 8 token は tool_chain ではない");
    }

    #[test]
    fn t_detect_tool_chain_positive_three_tools_subset() {
        let result =
            detect_tool_chain_in_advice("Run shell, but check repomap and grep", KNOWN_TOOLS_TEST);
        // 逆接 (`but`) で先頭 pair (shell, repomap) は拒否、windowing で後続
        // pair (repomap, grep) が採用される (Codex audit HIGH #2)。
        assert!(
            result.is_some(),
            "逆接で拒否された first pair の後続 pair を windowing で検出: {result:?}"
        );
        let chain = result.unwrap();
        assert_eq!(chain, vec!["repomap".to_string(), "grep".to_string()]);
    }

    #[test]
    fn t_detect_tool_chain_backticked_tools() {
        // Codex audit MEDIUM #1: backtick / 括弧 / 引用符などの非英数記号を
        // token 両端から strip して word-boundary 化。
        let result =
            detect_tool_chain_in_advice("Use `file_read` then `shell` to verify", KNOWN_TOOLS_TEST);
        assert_eq!(
            result,
            Some(vec!["file_read".to_string(), "shell".to_string()]),
            "backtick 付き tool 名でも word-boundary で検出"
        );
    }

    // ===========================================================================
    // Mock event repository test (1 件、項目 209 dividend、F8 audit)
    // ===========================================================================

    // ===========================================================================
    // is_erl_enabled toggle (項目 216 defaults OFF 切替、env BONSAI_ERL_ENABLED)
    //
    // Lab v17 REJECT (項目 215) を受け production default を OFF に切替。
    // env=1 で opt-in 復活 (項目 213 動作の再現可能性を維持)。
    //
    // env mutation race を避けるため module-local Mutex で serialize する
    // (smoke_correction tests と同パターン、serial_test crate を増やさない方針)。
    // ===========================================================================

    static ERL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn reset_erl_env() {
        unsafe {
            std::env::remove_var("BONSAI_ERL_ENABLED");
        }
    }

    #[test]
    fn t_is_erl_enabled_default_unset() {
        let _g = ERL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_erl_env();
        assert!(
            !is_erl_enabled(),
            "env unset で false (production default = OFF、項目 216)"
        );
    }

    #[test]
    fn t_is_erl_enabled_explicit_1() {
        let _g = ERL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_erl_env();
        unsafe {
            std::env::set_var("BONSAI_ERL_ENABLED", "1");
        }
        assert!(
            is_erl_enabled(),
            "env=1 で true (opt-in 復活 = 項目 213 動作)"
        );
        reset_erl_env();
    }

    #[test]
    fn t_is_erl_enabled_case_insensitive_true() {
        let _g = ERL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_erl_env();
        for value in ["true", "TRUE", "True"] {
            unsafe {
                std::env::set_var("BONSAI_ERL_ENABLED", value);
            }
            assert!(is_erl_enabled(), "env={value} (case-insensitive) で true");
        }
        reset_erl_env();
    }

    #[test]
    fn t_is_erl_enabled_other_values_treated_as_false() {
        let _g = ERL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_erl_env();
        for value in ["0", "false", "no", ""] {
            unsafe {
                std::env::set_var("BONSAI_ERL_ENABLED", value);
            }
            assert!(!is_erl_enabled(), "env={value} は enabled 扱いせず false");
        }
        reset_erl_env();
    }

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
