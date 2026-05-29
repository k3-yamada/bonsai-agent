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
//! # Phase 4 wiring (Phase 2 Green 後)
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
    unimplemented!("Phase 2 Green で本実装 (env parse + 受理値 match)")
}

/// augmentation mode 取得 (Phase 2a は `BONSAI_T6_MEMORY_AUG_MODE=in_session` のみ受理、default InSession).
pub fn t6_memory_aug_mode() -> T6AugMode {
    unimplemented!("Phase 2 Green で本実装 (Phase 2a は InSession 固定)")
}

/// 素 Jaccard overlap (|A ∩ B| / |A ∪ B|, 空集合は 0.0).
///
/// `pick_top_k_in_session` のランキング基底。
pub fn jaccard_overlap(_a: &[String], _b: &[String]) -> f32 {
    unimplemented!("Phase 2 Green で本実装 (HashSet intersection / union)")
}

/// in_session history から `current_input` のキーワードと最も近い K 件を Jaccard 降順で返す.
///
/// tie-break は score 降順 (高 score 優先)。K > history.len() は history 全件返却。
pub fn pick_top_k_in_session(
    _history: &[T6SuccessRecord],
    _current_input: &str,
    _k: usize,
) -> Vec<T6SuccessRecord> {
    unimplemented!("Phase 2 Green で本実装 (tokenize_task_input → Jaccard → sort)")
}

/// records を固定 prefix + tool_chain + final_keywords block にフォーマット.
///
/// 出力 marker (Phase 2 Green test contract):
/// 1. `[T6 Past Success Examples]` header
/// 2. `tool_chain:` prefix (各 record)
/// 3. `final_keywords:` prefix (各 record)
///
/// 上限 1500 tokens (`T6_AUG_MAX_TOKEN_BUDGET`、Phase 3 で extract)。
pub fn format_aug_block(_records: &[T6SuccessRecord]) -> String {
    unimplemented!("Phase 2 Green で本実装 (header + per-record formatting + budget cap)")
}

/// `task_tier == LongHorizonPlanning` AND env=1 AND history 非空 で system prompt 末尾に append.
///
/// それ以外 (non-T6 / env=0 / history 空) は `system.to_string()` を pass-through.
/// 項目 262 `augment_system_prompt` (案 A) の directive 直後に呼び出すことで
/// `base → directive → examples` の inject 順を実現。
pub fn augment_system_prompt_with_memory(
    _system: &str,
    _task_tier: CapabilityTier,
    _history: &[T6SuccessRecord],
    _current_input: &str,
) -> String {
    unimplemented!("Phase 2 Green で本実装 (env+tier+history guard 後 append)")
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
        let _guard = ENV_LOCK.lock().unwrap();
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
        let _guard = ENV_LOCK.lock().unwrap();
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
        // history: 3 record、current_input と overlap 順 = r2 (3 match) > r0 (1 match) > r1 (0 match)
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
        let _guard = ENV_LOCK.lock().unwrap();
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
