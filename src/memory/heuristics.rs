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

use crate::agent::event_store::{Event, EventRepository};
use crate::cancel::CancellationToken;
use crate::domain::conversation::Message;
use crate::memory::decay;
use crate::memory::review::{
    self, ReviewOutcome, ReviewState, ReviewStatus, compute_next_review_at,
    estimate_volatility_from_category,
};
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
///
/// `review_state` は項目 218 候補 (Cerememory ADR-011 ReviewState port)。
/// V12 migration 適用済 DB から `find_top_k_for_task` で復元される。
/// V12 未適用環境 (壊れた migration) では `Default` 値が入る (defensive)。
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
    #[serde(default)]
    pub review_state: ReviewState,
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

/// `record_review` の SELECT 9 列 tuple 型 (clippy `type_complexity` 抑制用 alias)。
///
/// 順序: review_status / importance / volatility / freshness / source_confidence /
/// last_reviewed_at / next_review_at / review_count / stale_count。
type ReviewRowTuple = (
    String,
    f64,
    f64,
    f64,
    Option<f64>,
    Option<String>,
    Option<String>,
    u32,
    u32,
);

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
        let now_dt = chrono::Utc::now();
        let now = now_dt.to_rfc3339();

        // 項目 218 候補 (Cerememory ADR-011): volatility は category から推定、
        // next_review_at は base=30 day を volatility でスケール。
        let volatility = estimate_volatility_from_category(category);
        let next_review = compute_next_review_at(now_dt, volatility, 2_592_000).to_rfc3339();

        self.conn.execute(
            "INSERT INTO heuristics
             (advice, trigger_patterns, source_session_id, source_task, category,
              score, used_count, success_after_use, fingerprint, created_at,
              volatility, next_review_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 0.5, 0, 0, ?6, ?7, ?8, ?9)",
            params![
                advice,
                &triggers_json,
                source_session_id,
                &truncated_task,
                category,
                &fp,
                &now,
                volatility,
                &next_review,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// `task_context` の trigger_patterns マッチ + score 順 top-K。
    ///
    /// マッチ判定: 各 trigger 文字列が `task_context` の case-sensitive 部分文字列なら hit。
    /// 項目 218 候補: V12 列を SELECT して `review_state` を populate (Cerememory ADR-011 port)。
    pub fn find_top_k_for_task(&self, task_context: &str, k: usize) -> Result<Vec<Heuristic>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, advice, trigger_patterns, source_session_id, source_task, category,
                    score, used_count, success_after_use, created_at, last_used_at,
                    review_status, importance, volatility, freshness, source_confidence,
                    last_reviewed_at, next_review_at, review_count, stale_count
             FROM heuristics
             ORDER BY score DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let status_str: String = row.get(11)?;
            let last_reviewed: Option<String> = row.get(16)?;
            let next_review: Option<String> = row.get(17)?;
            let review_state = ReviewState {
                status: ReviewStatus::from_db_str(&status_str),
                importance: row.get(12)?,
                volatility: row.get(13)?,
                freshness: row.get(14)?,
                source_confidence: row.get(15)?,
                last_reviewed_at: last_reviewed.and_then(|s| {
                    chrono::DateTime::parse_from_rfc3339(&s)
                        .ok()
                        .map(|d| d.with_timezone(&chrono::Utc))
                }),
                next_review_at: next_review.and_then(|s| {
                    chrono::DateTime::parse_from_rfc3339(&s)
                        .ok()
                        .map(|d| d.with_timezone(&chrono::Utc))
                }),
                review_count: row.get(18)?,
                stale_count: row.get(19)?,
            };
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
                review_state,
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
    ///
    /// `BONSAI_DECAY_ENABLED=1` で opt-in: `compute_stability_boost(s_old, 1.5)` で
    /// stability column を retrieval 反応により増加 (項目 217、Cerememory ADR-005)。
    /// `s_old.max(0.001)` clamp で R2 (s_old=0 で powf(-0.2)=inf) を軽減。
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

        // 項目 217 (Cerememory decay port): opt-in stability boost
        if decay::is_decay_enabled() {
            let s_old: f64 = self
                .conn
                .query_row(
                    "SELECT stability FROM heuristics WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .unwrap_or(1.0);
            let s_new = decay::compute_stability_boost(s_old.max(0.001), 1.5);
            self.conn.execute(
                "UPDATE heuristics SET stability = ?1 WHERE id = ?2",
                params![s_new, id],
            )?;
        }

        Ok(())
    }

    /// 月次 prune (score < 0.2 + used_count 条件 / 上限 200 超過は score 昇順削除)。
    ///
    /// `BONSAI_DECAY_ENABLED=1` で opt-in: ステップ 3 (200 超過削除) のみ
    /// fidelity 昇順 (decay-adjusted) で削除順序を決定 (項目 217)。
    /// ステップ 1/2 は time-based filter として legacy 維持 (env と無関係)。
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
        // 3. 総数 200 超過 → score 昇順 (legacy) or fidelity 昇順 (decay opt-in)
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM heuristics", [], |row| row.get(0))?;
        if count > 200 {
            let excess = count - 200;
            if decay::is_decay_enabled() {
                total += self
                    .prune_decay_adjusted_excess(excess, chrono::Utc::now().timestamp() as f64)?;
            } else {
                total += self.conn.execute(
                    "DELETE FROM heuristics WHERE id IN (
                         SELECT id FROM heuristics ORDER BY score ASC, id ASC LIMIT ?1
                     )",
                    params![excess],
                )?;
            }
        }
        Ok(total)
    }

    /// 項目 217 (Cerememory decay port): fidelity 昇順で超過分削除。
    /// `now_secs` は fn 引数化で test の決定論性を保つ (R3 軽減、plan §4.4)。
    /// `last_used_at` 未設定の row は now_secs と等価とみなし elapsed=0 で扱う
    /// (= score 単体で順序、legacy と同等)。
    fn prune_decay_adjusted_excess(&self, excess: i64, now_secs: f64) -> Result<usize> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, score, stability, last_used_at FROM heuristics")?;
        let rows = stmt.query_map([], |row| {
            let id: i64 = row.get(0)?;
            let score: f64 = row.get(1)?;
            let stability: f64 = row.get(2)?;
            let last_used: Option<String> = row.get(3)?;
            Ok((id, score, stability, last_used))
        })?;

        let mut entries: Vec<(i64, f64)> = Vec::new();
        for row in rows {
            let (id, score, stability, last_used) = row?;
            let last_used_secs = last_used
                .as_ref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp() as f64)
                .unwrap_or(now_secs);
            let elapsed = (now_secs - last_used_secs).max(0.0);
            let fidelity = decay::compute_fidelity(score, elapsed, stability.max(0.001), 0.3, 1.0);
            entries.push((id, fidelity));
        }
        drop(stmt);

        // fidelity 昇順 sort、bottom `excess` 件削除
        entries.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        let to_delete: Vec<i64> = entries
            .iter()
            .take(excess as usize)
            .map(|(id, _)| *id)
            .collect();

        let mut deleted = 0usize;
        for id in to_delete {
            deleted += self
                .conn
                .execute("DELETE FROM heuristics WHERE id = ?1", params![id])?;
        }
        Ok(deleted)
    }

    /// Lab cycle 跨ぎの汚染リセット (項目 206 reset_session_data_for_lab と協調)。
    /// 明示的 reset 要求時のみ呼ばれる前提 (production 通常運用では使わない)。
    pub fn reset_for_lab(&self) -> Result<()> {
        self.conn.execute("DELETE FROM heuristics", [])?;
        Ok(())
    }

    /// 項目 218 候補 (Cerememory ADR-011 ReviewState port): scheduler API。
    /// `next_review_at <= now` の row IDs を `next_review_at ASC` で返す (上限 50)。
    /// `BONSAI_REVIEW_ENABLED` env unset で空 Vec 返却 (legacy 互換)。
    pub fn review_tick(&self, now: chrono::DateTime<chrono::Utc>) -> Result<Vec<i64>> {
        if !review::is_review_enabled() {
            return Ok(Vec::new());
        }
        let now_str = now.to_rfc3339();
        let mut stmt = self.conn.prepare(
            "SELECT id FROM heuristics
             WHERE next_review_at IS NOT NULL AND next_review_at <= ?1
             ORDER BY next_review_at ASC LIMIT 50",
        )?;
        let ids = stmt
            .query_map(params![&now_str], |row| row.get::<_, i64>(0))?
            .filter_map(Result::ok)
            .collect();
        Ok(ids)
    }

    /// 項目 218 候補 (Cerememory ADR-011 ReviewState port): record API。
    /// outcome を反映 (freshness 更新 + review_count++ + next_review_at 計算)。
    /// 既存 row の volatility を保持しつつ next_review_at を再計算 (base=30 day)。
    pub fn record_review(
        &self,
        id: i64,
        outcome: ReviewOutcome,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        // 現状の review_state を load (type は closure 内 row.get の型推論で確定)
        let row: ReviewRowTuple = self.conn.query_row(
            "SELECT review_status, importance, volatility, freshness, source_confidence,
                    last_reviewed_at, next_review_at, review_count, stale_count
             FROM heuristics WHERE id = ?1",
            params![id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, f64>(1)?,
                    r.get::<_, f64>(2)?,
                    r.get::<_, f64>(3)?,
                    r.get::<_, Option<f64>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, u32>(7)?,
                    r.get::<_, u32>(8)?,
                ))
            },
        )?;
        let (
            status_str,
            importance,
            volatility,
            freshness,
            source_confidence,
            last_reviewed_str,
            next_review_str,
            review_count,
            stale_count,
        ) = row;

        let mut state = ReviewState {
            status: ReviewStatus::from_db_str(&status_str),
            importance,
            volatility,
            freshness,
            source_confidence,
            last_reviewed_at: last_reviewed_str.and_then(|s| {
                chrono::DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|d| d.with_timezone(&chrono::Utc))
            }),
            next_review_at: next_review_str.and_then(|s| {
                chrono::DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|d| d.with_timezone(&chrono::Utc))
            }),
            review_count,
            stale_count,
        };

        outcome.apply_to(&mut state);
        state.last_reviewed_at = Some(now);
        state.next_review_at = Some(compute_next_review_at(now, state.volatility, 2_592_000));
        state.review_count = state.review_count.saturating_add(1);

        let last_str = state.last_reviewed_at.map(|d| d.to_rfc3339());
        let next_str = state.next_review_at.map(|d| d.to_rfc3339());

        self.conn.execute(
            "UPDATE heuristics SET
               review_status = ?1, freshness = ?2, last_reviewed_at = ?3,
               next_review_at = ?4, review_count = ?5, stale_count = ?6
             WHERE id = ?7",
            params![
                state.status.as_db_str(),
                state.freshness,
                last_str,
                next_str,
                state.review_count,
                state.stale_count,
                id,
            ],
        )?;
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

    // ===========================================================================
    // 項目 217: Cerememory power-law decay port 統合 test
    //
    // env mutation race を避けるため module-local Mutex で serialize する
    // (decay.rs DECAY_TEST_LOCK と独立、こちらは heuristics 統合専用)。
    // ===========================================================================

    static DECAY_INTEGRATION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn reset_decay_env() {
        unsafe {
            std::env::remove_var("BONSAI_DECAY_ENABLED");
        }
    }

    #[test]
    fn t_record_outcome_boosts_stability_when_decay_enabled() {
        let _g = DECAY_INTEGRATION_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_decay_env();
        let store = crate::memory::store::MemoryStore::in_memory().unwrap();
        let h = HeuristicStore::new(store.conn());
        let id = h
            .save(
                "テスト advice for decay",
                &["decay_test".to_string()],
                None,
                "test",
                "efficiency",
            )
            .unwrap();

        let s_before: f64 = store
            .conn()
            .query_row(
                "SELECT stability FROM heuristics WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            (s_before - 1.0).abs() < 1e-9,
            "V11 migration で default stability = 1.0、got {s_before}"
        );

        unsafe {
            std::env::set_var("BONSAI_DECAY_ENABLED", "1");
        }
        h.record_outcome(id, true).unwrap();

        let s_after: f64 = store
            .conn()
            .query_row(
                "SELECT stability FROM heuristics WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            s_after > s_before,
            "decay enabled で stability boost: s_after={s_after} > s_before={s_before}"
        );
        reset_decay_env();
    }

    #[test]
    fn t_record_outcome_does_not_change_stability_when_decay_disabled() {
        let _g = DECAY_INTEGRATION_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_decay_env();
        let store = crate::memory::store::MemoryStore::in_memory().unwrap();
        let h = HeuristicStore::new(store.conn());
        let id = h
            .save(
                "legacy advice",
                &["legacy_test".to_string()],
                None,
                "test",
                "efficiency",
            )
            .unwrap();

        h.record_outcome(id, true).unwrap();

        let s_after: f64 = store
            .conn()
            .query_row(
                "SELECT stability FROM heuristics WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            (s_after - 1.0).abs() < 1e-9,
            "env unset で stability=1.0 維持 (legacy 互換)、got {s_after}"
        );
    }

    #[test]
    fn t_prune_decay_adjusted_excess_with_fixed_now() {
        let _g = DECAY_INTEGRATION_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_decay_env();
        let store = crate::memory::store::MemoryStore::in_memory().unwrap();
        let h = HeuristicStore::new(store.conn());

        // 3 row insert: id=1 (古い、低 score)、id=2 (新しい、低 score)、id=3 (新しい、高 score)
        let now_secs: f64 = 1_700_000_000.0;
        let day_ago = chrono::DateTime::from_timestamp((now_secs - 86400.0) as i64, 0)
            .unwrap()
            .to_rfc3339();
        let sec_ago = chrono::DateTime::from_timestamp((now_secs - 1.0) as i64, 0)
            .unwrap()
            .to_rfc3339();

        for (i, (last_used, score)) in [(&day_ago, 0.5), (&sec_ago, 0.5), (&sec_ago, 0.9)]
            .iter()
            .enumerate()
        {
            store
                .conn()
                .execute(
                    "INSERT INTO heuristics (advice, trigger_patterns, source_task, category,
                     score, fingerprint, created_at, last_used_at, stability)
                     VALUES (?1, '[]', '', 'test', ?2, ?3, ?4, ?4, 1.0)",
                    params![
                        format!("advice_{}", i),
                        score,
                        format!("fp_{}", i),
                        last_used,
                    ],
                )
                .unwrap();
        }

        // 1 件削除 → 期待: id=1 (古い + 低 score) が最低 fidelity
        let deleted = h.prune_decay_adjusted_excess(1, now_secs).unwrap();
        assert_eq!(deleted, 1, "1 件削除");

        let remaining: Vec<i64> = store
            .conn()
            .prepare("SELECT id FROM heuristics ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(
            !remaining.contains(&1),
            "id=1 (古い、低 fidelity) が削除されること: remaining={remaining:?}"
        );
        assert_eq!(remaining.len(), 2, "残り 2 件");
    }

    #[test]
    fn t_schema_v11_migration_adds_stability_default_1_0() {
        let store = crate::memory::store::MemoryStore::in_memory().unwrap();
        let h = HeuristicStore::new(store.conn());
        let id = h
            .save("v11 test", &["v11".to_string()], None, "test", "efficiency")
            .unwrap();

        let stability: f64 = store
            .conn()
            .query_row(
                "SELECT stability FROM heuristics WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            (stability - 1.0).abs() < 1e-9,
            "V11 migration で stability column が DEFAULT 1.0 で挿入: {stability}"
        );
    }

    // ===========================================================================
    // 項目 218 候補: Cerememory ADR-011 ReviewState port 統合 test (Plan B §5)
    //
    // Phase 1 Red 段階では V12 migration 未追加 + review_tick/record_review todo!()
    // panic で失敗を確証。Phase 2 Green で 1118 passed 達成。
    // ===========================================================================

    #[test]
    fn t_save_initializes_review_state_with_volatility_from_category() {
        let store = crate::memory::store::MemoryStore::in_memory().unwrap();
        let h = HeuristicStore::new(store.conn());
        let id = h
            .save(
                "review state init test",
                &["init_trigger".to_string()],
                None,
                "test_task",
                "failure_recovery", // volatility=0.7 期待
            )
            .unwrap();

        // V12 migration で volatility 列追加、save 内で
        // estimate_volatility_from_category("failure_recovery") = 0.7 投入
        let volatility: f64 = store
            .conn()
            .query_row(
                "SELECT volatility FROM heuristics WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .expect("V12 で volatility 列が追加されるべき (Phase 2 Green)");
        assert!(
            (volatility - 0.7).abs() < 1e-9,
            "failure_recovery → volatility=0.7 投入、got {volatility}"
        );

        // next_review_at も投入されるべき
        let next: Option<String> = store
            .conn()
            .query_row(
                "SELECT next_review_at FROM heuristics WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .expect("V12 で next_review_at 列が追加されるべき");
        assert!(next.is_some(), "save 内で next_review_at が計算投入される");
    }

    #[test]
    fn t_review_tick_returns_due_ids_only() {
        let store = crate::memory::store::MemoryStore::in_memory().unwrap();
        let h = HeuristicStore::new(store.conn());
        let now = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();

        // Phase 1 Red: review_tick は todo!() panic
        let due = h
            .review_tick(now)
            .expect("review_tick が成功して due IDs を返す");

        // Phase 2 Green 期待: 空 DB なら 0 件
        assert_eq!(due.len(), 0, "空 DB は due 0 件");
    }

    #[test]
    fn t_record_review_confirmed_updates_freshness_and_next_review_at() {
        use crate::memory::review::ReviewOutcome;

        let store = crate::memory::store::MemoryStore::in_memory().unwrap();
        let h = HeuristicStore::new(store.conn());
        let id = h
            .save(
                "record review test",
                &["rec_trigger".to_string()],
                None,
                "test",
                "efficiency",
            )
            .unwrap();

        // freshness を 0.3 に下げてから Confirmed で 1.0 に reset 期待
        store
            .conn()
            .execute(
                "UPDATE heuristics SET freshness = 0.3 WHERE id = ?1",
                params![id],
            )
            .expect("V12 freshness 列 update");

        let now = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        h.record_review(id, ReviewOutcome::Confirmed, now)
            .expect("record_review が成功");

        let freshness: f64 = store
            .conn()
            .query_row(
                "SELECT freshness FROM heuristics WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            (freshness - 1.0).abs() < 1e-9,
            "Confirmed で freshness=1.0、got {freshness}"
        );

        // last_reviewed_at と next_review_at が更新される
        let (last, next): (Option<String>, Option<String>) = store
            .conn()
            .query_row(
                "SELECT last_reviewed_at, next_review_at FROM heuristics WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(last.is_some(), "last_reviewed_at が記録される");
        assert!(next.is_some(), "next_review_at が再計算される");
    }

    #[test]
    fn t_schema_v12_migration_adds_9_columns() {
        // V12 migration で 9 列が ALTER TABLE で追加されることを確認
        let store = crate::memory::store::MemoryStore::in_memory().unwrap();

        for col in [
            "review_status",
            "importance",
            "volatility",
            "freshness",
            "source_confidence",
            "last_reviewed_at",
            "next_review_at",
            "review_count",
            "stale_count",
        ] {
            // SELECT が成功すれば列が存在
            let result: Result<i64> = store
                .conn()
                .query_row(&format!("SELECT COUNT({col}) FROM heuristics"), [], |row| {
                    row.get(0)
                })
                .map_err(|e| e.into());
            assert!(
                result.is_ok(),
                "V12 で列 '{col}' が追加されるべき (Phase 2 Green)、err={result:?}"
            );
        }
    }

    // ===========================================================================
    // 項目 218 候補 Phase 4 Smoke: env-gate fixed-clock fixture (Plan B §5)
    //
    // env mutation race を避けるため module-local Mutex で serialize する。
    // ===========================================================================

    static REVIEW_PHASE4_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn reset_review_env_phase4() {
        unsafe {
            std::env::remove_var("BONSAI_REVIEW_ENABLED");
        }
    }

    #[test]
    fn t_review_tick_with_env_enabled_returns_due_rows_only() {
        let _g = REVIEW_PHASE4_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_review_env_phase4();

        let store = crate::memory::store::MemoryStore::in_memory().unwrap();
        let h = HeuristicStore::new(store.conn());

        // 2 row 投入: id=1 (next_review_at = 過去 = due)、id=2 (next_review_at = 未来 = not due)
        let now_secs: i64 = 1_700_000_000;
        let now = chrono::DateTime::from_timestamp(now_secs, 0).unwrap();
        let past = chrono::DateTime::from_timestamp(now_secs - 86400, 0)
            .unwrap()
            .to_rfc3339();
        let future = chrono::DateTime::from_timestamp(now_secs + 86400, 0)
            .unwrap()
            .to_rfc3339();

        for (i, when) in [&past, &future].iter().enumerate() {
            store
                .conn()
                .execute(
                    "INSERT INTO heuristics (advice, trigger_patterns, source_task, category,
                     score, fingerprint, created_at, next_review_at)
                     VALUES (?1, '[]', '', 'test', 0.5, ?2, ?3, ?4)",
                    params![format!("advice_{i}"), format!("fp4_{i}"), when, when,],
                )
                .unwrap();
        }

        // env unset → 空 Vec
        let due_off = h.review_tick(now).unwrap();
        assert!(due_off.is_empty(), "env unset で空 Vec (legacy 互換)");

        // env=1 → past の id のみ返却
        unsafe {
            std::env::set_var("BONSAI_REVIEW_ENABLED", "1");
        }
        let due_on = h.review_tick(now).unwrap();
        reset_review_env_phase4();

        assert_eq!(due_on.len(), 1, "past 1 件のみ due、got {due_on:?}");
        // id=1 が past (insertion 順)、id=2 が future
        let returned_id = due_on[0];
        let returned_when: String = store
            .conn()
            .query_row(
                "SELECT next_review_at FROM heuristics WHERE id = ?1",
                params![returned_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            returned_when, past,
            "返却された row の next_review_at は past"
        );
    }

    #[test]
    fn t_record_review_stale_decreases_freshness_and_increments_stale_count() {
        use crate::memory::review::ReviewOutcome;

        let store = crate::memory::store::MemoryStore::in_memory().unwrap();
        let h = HeuristicStore::new(store.conn());
        let id = h
            .save(
                "stale outcome integration test",
                &["stale_int_test".to_string()],
                None,
                "test",
                "verification",
            )
            .unwrap();

        // 初期 freshness=1.0、stale_count=0 (V12 default + save) を確認
        let (f0, sc0): (f64, u32) = store
            .conn()
            .query_row(
                "SELECT freshness, stale_count FROM heuristics WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(
            (f0 - 1.0).abs() < 1e-9,
            "save 直後 freshness=1.0 (V12 default)、got {f0}"
        );
        assert_eq!(sc0, 0, "save 直後 stale_count=0、got {sc0}");

        // record_review(Stale) → freshness -0.3 = 0.7、stale_count = 1
        let now = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        h.record_review(id, ReviewOutcome::Stale, now).unwrap();

        let (f1, sc1, status1): (f64, u32, String) = store
            .conn()
            .query_row(
                "SELECT freshness, stale_count, review_status FROM heuristics WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert!(
            (f1 - 0.7).abs() < 1e-9,
            "Stale で freshness 1.0 - 0.3 = 0.7、got {f1}"
        );
        assert_eq!(sc1, 1, "stale_count++、got {sc1}");
        assert_eq!(status1, "stale", "review_status='stale'、got {status1}");

        // 2 回目の Stale で freshness=0.4、stale_count=2
        h.record_review(id, ReviewOutcome::Stale, now).unwrap();
        let (f2, sc2): (f64, u32) = store
            .conn()
            .query_row(
                "SELECT freshness, stale_count FROM heuristics WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(
            (f2 - 0.4).abs() < 1e-9,
            "2 回目 Stale で freshness=0.4、got {f2}"
        );
        assert_eq!(sc2, 2, "2 回目で stale_count=2、got {sc2}");
    }
}
