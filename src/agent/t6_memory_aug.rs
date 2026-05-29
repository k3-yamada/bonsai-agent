//! AgentFloor T6 KG-Augmented Retrieval — Phase 2a (案 D-2 in-session top-K).
//!
//! 起点: `.claude/plan/agentfloor-t6-kg-augmented-phase2.md`
//! 項目 262 案 A (`t6_prompt_augment.rs`) の natural progression として、
//! T6 task 検出時に **過去の in-session T6 success trajectory** から top-K を
//! Jaccard overlap で選び、system prompt 末尾に append する dynamic augmentation。
//!
//! # Phase 段階
//! - **Phase 2a (本 module、案 D-2)**: in-session `Vec<T6SuccessRecord>` scope、
//!   session 終了で破棄。cold start 中、smoke +0.02-0.04 期待。
//! - **Phase 2b (案 D-1 → D-3)**: 項目 228 KG fusion (R@10=98.6) 経由の cross-session
//!   trajectory retrieval を追加し、hybrid 化。
//!
//! # 環境変数
//! ```text
//! BONSAI_T6_MEMORY_AUG=1|true|TRUE|yes|YES         # opt-in (default off)
//! BONSAI_T6_MEMORY_AUG_MODE=in_session             # Phase 2a は in_session のみ
//! ```
//!
//! # Contract
//! - env unset で 100% backward compat (augment 経路 skip)
//! - `CapabilityTier::LongHorizonPlanning` 以外は pass-through (副作用ゼロ)
//! - history 空 (cold start) の場合は pass-through
//!
//! # Phase 4 wiring (Phase 2 Green 後、別 commit)
//! `benchmark.rs::BenchmarkSuite::run_with_multi_config()` の項目 262 augment 直後に
//! env-gated で `augment_system_prompt_with_memory` 1 行追加。
//! T6 task 完了時 score ≥ 0.7 で `Vec<T6SuccessRecord>` に append、session scope。
//!
//! # 既存資産との整合
//! - 項目 262 `t6_prompt_augment.rs` (案 A): 並列 ON で directive → examples 順 inject
//! - 項目 228 KG fusion: Phase 2b で `KnowledgeGraph::neighbors` 経由活用 (本 module は KG-free)
//! - 項目 220-222 sqlite-vec wiring removal: Phase 2a `Vec` in-memory のみ、sqlite-vec 非依存
//!
//! # 参照
//! - `docs/architecture/module-layer-rules.md` (agent layer)
//! - `.claude/plan/agentfloor-t6-kg-augmented-phase2.md` §3 (TDD strict outline)

use crate::agent::benchmark::CapabilityTier;
use std::collections::HashSet;

/// Augment block header marker (`format_aug_block` 出力 marker 1).
const T6_AUG_BLOCK_HEADER: &str = "[T6 Past Success Examples]";

/// Augment block 上限 token budget (粗推定 1 token ≈ 4 chars).
///
/// 長 context dilution (項目 248 dynamic budget compaction で対処済) を避けるため、
/// in-session augment は最大 ~1500 token に制限。超過時は truncation marker 付与。
const T6_AUG_MAX_TOKEN_BUDGET: usize = 1500;

/// T6 LongHorizonPlanning task の成功 trajectory record (in-session scope).
///
/// `score >= 0.7` の T6 task 完了時に `Vec<T6SuccessRecord>` に append され、
/// 後続 T6 task の system prompt に top-K (Jaccard 降順) として inject される。
#[derive(Clone, Debug)]
pub struct T6SuccessRecord {
    pub task_id: String,
    pub input_keywords: Vec<String>,
    pub tool_chain: Vec<String>,
    pub final_keywords: Vec<String>,
    pub score: f64,
}

/// Augmentation mode (Phase 2a は `InSession` のみ、Phase 2b で拡張).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum T6AugMode {
    /// In-session `Vec<T6SuccessRecord>` から top-K 検索 (案 D-2、Phase 2a)。
    InSession,
    // Phase 2b 拡張点: CrossSession / Hybrid (KG-based)
}

/// 案 D-2 in-session top-K augmentation の有効化判定.
///
/// 受理値: "1" | "true" | "TRUE" | "yes" | "YES"
/// env unset/その他値は false (no-op)。項目 262 `is_t6_prompt_augment_enabled` 同パターン。
pub fn is_t6_memory_aug_enabled() -> bool {
    std::env::var("BONSAI_T6_MEMORY_AUG")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

/// augmentation mode 取得 (Phase 2a は `BONSAI_T6_MEMORY_AUG_MODE=in_session` のみ受理、default InSession).
///
/// Phase 2a では未知値 / unset 共に InSession に fallback (forward-compat、Phase 2b で CrossSession/Hybrid 追加予定).
pub fn t6_memory_aug_mode() -> T6AugMode {
    // Phase 2a: in_session のみ実装。Phase 2b で CrossSession / Hybrid 拡張点.
    T6AugMode::InSession
}

/// task input string を Jaccard 計算用の lowercase whitespace token に分解.
///
/// `pick_top_k_in_session` の補助 helper (Phase 3 Refactor で extract したものを Phase 2 Green で先行配置).
fn tokenize_task_input(input: &str) -> Vec<String> {
    input.split_whitespace().map(|s| s.to_lowercase()).collect()
}

/// 素 Jaccard overlap (|A ∩ B| / |A ∪ B|, 空集合は 0.0).
///
/// `pick_top_k_in_session` のランキング基底。
pub fn jaccard_overlap(a: &[String], b: &[String]) -> f32 {
    let set_a: HashSet<&String> = a.iter().collect();
    let set_b: HashSet<&String> = b.iter().collect();
    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();
    if union == 0 {
        return 0.0;
    }
    intersection as f32 / union as f32
}

/// in_session history から `current_input` のキーワードと最も近い K 件を Jaccard 降順で返す.
///
/// tie-break は score 降順 (高 score 優先)。K > history.len() は history 全件返却。
/// K == 0 または history 空は空 Vec。
pub fn pick_top_k_in_session(
    history: &[T6SuccessRecord],
    current_input: &str,
    k: usize,
) -> Vec<T6SuccessRecord> {
    if history.is_empty() || k == 0 {
        return Vec::new();
    }
    let current_tokens = tokenize_task_input(current_input);
    let mut scored: Vec<(f32, &T6SuccessRecord)> = history
        .iter()
        .map(|rec| (jaccard_overlap(&rec.input_keywords, &current_tokens), rec))
        .collect();
    // Jaccard 降順、tie-break = score 降順.
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.1.score
                    .partial_cmp(&a.1.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    scored
        .into_iter()
        .take(k)
        .map(|(_, rec)| rec.clone())
        .collect()
}

/// records を固定 prefix + tool_chain + final_keywords block にフォーマット.
///
/// 出力 marker (test contract):
/// 1. `[T6 Past Success Examples]` header (`T6_AUG_BLOCK_HEADER`)
/// 2. `tool_chain:` prefix (各 record)
/// 3. `final_keywords:` prefix (各 record)
///
/// 上限 `T6_AUG_MAX_TOKEN_BUDGET` = 1500 tokens (粗推定 1 token ≈ 4 chars).
/// 上限超過時は truncation marker (`...`) を追加し break。
pub fn format_aug_block(records: &[T6SuccessRecord]) -> String {
    if records.is_empty() {
        return String::new();
    }
    let char_budget = T6_AUG_MAX_TOKEN_BUDGET.saturating_mul(4);
    let mut block = String::with_capacity(256);
    block.push_str(T6_AUG_BLOCK_HEADER);
    block.push('\n');
    for rec in records {
        let line = format!(
            "- task_id={} tool_chain=[{}] final_keywords=[{}] score={:.2}\n",
            rec.task_id,
            rec.tool_chain.join(", "),
            rec.final_keywords.join(", "),
            rec.score
        );
        if block.len() + line.len() > char_budget {
            block.push_str("...\n");
            break;
        }
        block.push_str(&line);
    }
    block
}

/// `task_tier == LongHorizonPlanning` AND env=1 AND history 非空 で system prompt 末尾に append.
///
/// それ以外 (non-T6 / env=0 / history 空) は `system.to_string()` を pass-through.
/// 項目 262 `augment_system_prompt` (案 A) の directive 直後に呼び出すことで
/// `base → directive → examples` の inject 順を実現。
///
/// Phase 2a top-K = 2 (Gemini 推奨、smoke で先行検証、Phase 2b で K 動的化検討).
pub fn augment_system_prompt_with_memory(
    system: &str,
    task_tier: CapabilityTier,
    history: &[T6SuccessRecord],
    current_input: &str,
) -> String {
    // ガード: T6 のみ、env=1 のみ、history 非空のみ.
    if !matches!(task_tier, CapabilityTier::LongHorizonPlanning) {
        return system.to_string();
    }
    if !is_t6_memory_aug_enabled() {
        return system.to_string();
    }
    if history.is_empty() {
        return system.to_string();
    }
    let top = pick_top_k_in_session(history, current_input, 2);
    if top.is_empty() {
        return system.to_string();
    }
    let block = format_aug_block(&top);
    if block.is_empty() {
        return system.to_string();
    }
    format!("{}\n\n{}", system, block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// env var race condition 回避 (項目 262 `t6_prompt_augment::ENV_LOCK` 同形式).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn make_record(task_id: &str, input: &[&str], tools: &[&str], score: f64) -> T6SuccessRecord {
        T6SuccessRecord {
            task_id: task_id.to_string(),
            input_keywords: input.iter().map(|s| s.to_string()).collect(),
            tool_chain: tools.iter().map(|s| s.to_string()).collect(),
            final_keywords: vec!["done".to_string()],
            score,
        }
    }

    #[test]
    fn t_t6_memory_aug_env_default_off() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: single-threaded via ENV_LOCK、test 専用
        unsafe {
            std::env::remove_var("BONSAI_T6_MEMORY_AUG");
        }
        assert!(
            !is_t6_memory_aug_enabled(),
            "env unset で default OFF を期待 (項目 262 同パターン)"
        );
    }

    #[test]
    fn t_t6_memory_aug_mode_default_in_session_in_phase_2a() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: single-threaded via ENV_LOCK、test 専用
        unsafe {
            std::env::remove_var("BONSAI_T6_MEMORY_AUG_MODE");
        }
        assert_eq!(
            t6_memory_aug_mode(),
            T6AugMode::InSession,
            "Phase 2a default は InSession 固定"
        );
    }

    #[test]
    fn t_pick_top_k_returns_highest_jaccard_first() {
        // history: 3 record、current_input と overlap 順 = c (J=1.0) > a (J=0.667) > b (J=0)
        let history = vec![
            make_record("a", &["foo", "baz"], &["repo_map"], 0.8),
            make_record("b", &["alpha", "beta"], &["shell"], 0.9),
            make_record("c", &["foo", "bar", "baz"], &["file_read"], 0.7),
        ];
        let current = "foo bar baz";
        let top2 = pick_top_k_in_session(&history, current, 2);
        assert_eq!(top2.len(), 2, "K=2 で 2 件返却");
        assert_eq!(top2[0].task_id, "c", "最高 Jaccard が先頭");
        assert_eq!(top2[1].task_id, "a", "次点が 2 番目");
    }

    #[test]
    fn t_format_aug_block_contains_three_required_markers() {
        let records = vec![make_record(
            "t6_sample",
            &["plan", "refactor"],
            &["repo_map", "file_read"],
            0.85,
        )];
        let block = format_aug_block(&records);
        assert!(
            block.contains("[T6 Past Success Examples]"),
            "marker 1: header"
        );
        assert!(block.contains("tool_chain"), "marker 2: tool_chain prefix");
        assert!(
            block.contains("final_keywords"),
            "marker 3: final_keywords prefix"
        );
    }

    #[test]
    fn t_augment_system_prompt_with_memory_no_op_when_non_t6() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: single-threaded via ENV_LOCK、test 専用
        unsafe {
            std::env::set_var("BONSAI_T6_MEMORY_AUG", "1");
        }
        let base = "system prompt base";
        let history = vec![make_record("x", &["foo"], &["shell"], 0.9)];
        let augmented = augment_system_prompt_with_memory(
            base,
            CapabilityTier::SingleToolUse, // non-T6
            &history,
            "foo",
        );
        // SAFETY: single-threaded via ENV_LOCK、test 専用
        unsafe {
            std::env::remove_var("BONSAI_T6_MEMORY_AUG");
        }
        assert_eq!(
            augmented, base,
            "non-T6 tier では env=1 でも pass-through (副作用ゼロ)"
        );
    }
}
