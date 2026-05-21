//! AgentFloor T6 (LongHorizonPlanning) tier 特化 prompt augmentation.
//!
//! 起点: `.claude/plan/agentfloor-t6-weakness-improvement.md` 案 A (Phase 2 Green + Phase 4 wiring)
//!
//! # 役割
//! T6 tier task 検出時に system prompt の末尾に追加 directive を inject し、
//! long-horizon plan の step-by-step 構造化を促す。env-gated (`BONSAI_T6_PROMPT_AUGMENT=1`)。
//!
//! # 環境変数
//! ```text
//! BONSAI_T6_PROMPT_AUGMENT=1|true|TRUE|yes|YES  # opt-in (default off)
//! ```
//! unset または未知値 → false (no-op)。既存 `is_dynamic_budget_enabled` と同パターン。
//!
//! # Contract
//! - env unset で 100% backward compat (augment 経路 skip、system prompt 不変)
//! - T6 (LongHorizonPlanning) tier 以外は pass-through (scope 制限、他 tier へ副作用ゼロ)
//! - directive は固定 text (3 件、後述)
//!
//! # Directive 設計理由
//! 3 件の directive は p^n cliff (ステップ蓄積による失敗確率指数的増大) への構造的対処:
//! 1. **step-by-step plan** (numbered list 先行記述): plan 欠如による中盤 drift を防止。
//!    T6 weakest score 0.47 の主因は長期 plan 欠如と判断 (AgentFloor T1-T6 baseline 参照)。
//! 2. **restate plan progress** (3 ツール呼び出しごと): middle-step drift 防止。
//!    長 horizon タスクでは context 中盤に goal が埋もれやすい (項目 6/12 compaction 関連)。
//! 3. **revise plan** (連続 2 失敗時): error recovery 強化。
//!    T5 ErrorRecovery score 0.70 と比較して T6 0.47 の差分が recovery 不在を示唆。
//!
//! # Phase 4 wiring
//! `benchmark.rs::BenchmarkSuite::run()` および `run_with_multi_config()` の
//! system_prompt 構築箇所に `augment_system_prompt(prompt, task.capability_tier)` を
//! 1 行で wire。env unset の場合は clone のみで副作用ゼロ。
//!
//! # 将来拡張 hook 点
//! - 案 D Phase 2: KG-augmented retrieval を本 module 経由で activate する場合、
//!   `augment_system_prompt` 内部で `is_t6_prompt_augment_enabled()` ガード後に
//!   KG context を append する拡張が自然な接続点となる。
//!
//! # 参照
//! - `docs/architecture/module-layer-rules.md` (LOG-001 / agent layer)
//! - `.claude/plan/agentfloor-t6-weakness-improvement.md` (本 module の設計 plan)

use crate::agent::benchmark::CapabilityTier;

/// `BONSAI_T6_PROMPT_AUGMENT` env を parse し、T6 prompt augment が有効かを返す。
///
/// 受理値: "1" | "true" | "TRUE" | "yes" | "YES"
/// 既存 `is_dynamic_budget_enabled` (compaction.rs) と同パターン。
/// env unset/その他値は false (no-op)。
pub fn is_t6_prompt_augment_enabled() -> bool {
    std::env::var("BONSAI_T6_PROMPT_AUGMENT")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

/// T6 long-horizon planning 向け固定 directive を返す。
///
/// 3 directive:
/// 1. step-by-step plan を numbered list で先行記述
/// 2. ツール呼び出し 3 回ごとに進捗を 1 文で restate
/// 3. 連続 2 回以上のツール失敗時に plan を revise
///
/// p^n cliff (ステップ蓄積による失敗確率指数的増大) への構造的対処。
pub fn t6_augment_directive() -> &'static str {
    "\n\n[T6 LongHorizon Planning Directives]\n\
     - Before any tool call, write step-by-step plan as a numbered list (1. ... 2. ... 3. ...).\n\
     - After every 3rd tool call, restate plan progress in 1 sentence.\n\
     - If 2 or more consecutive tool failures, stop and revise plan."
}

/// system prompt に T6 augment directive を append して返す。
///
/// env=1 かつ `task_tier == CapabilityTier::LongHorizonPlanning` のときのみ
/// `t6_augment_directive()` を末尾に付与する。それ以外は完全 pass-through。
///
/// # env unset (default)
/// `system` を clone して返す。production code への副作用ゼロ。
///
/// # T6 only scope
/// non-T6 tier では env=1 でも pass-through。他 tier に影響しない。
pub fn augment_system_prompt(system: &str, task_tier: CapabilityTier) -> String {
    // env unset で 100% backward compat
    if !is_t6_prompt_augment_enabled() {
        return system.to_string();
    }
    // T6 tier のみ directive append (scope 制限)
    if matches!(task_tier, CapabilityTier::LongHorizonPlanning) {
        format!("{system}{}", t6_augment_directive())
    } else {
        system.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // cross-test env 競合防止 (他 env-gated test と同パターン)
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn t_t6_prompt_augment_env_default_off() {
        let _guard = ENV_LOCK.lock().unwrap();
        // env unset で augment 経路通らない (本実装 env parse 確証)
        // SAFETY: single-threaded via ENV_LOCK、test 専用
        unsafe { std::env::remove_var("BONSAI_T6_PROMPT_AUGMENT") };
        assert!(!is_t6_prompt_augment_enabled(), "env unset = false");
    }

    #[test]
    fn t_t6_prompt_augment_env_on_t6_task_injected() {
        let _guard = ENV_LOCK.lock().unwrap();
        // env=1 + T6 task で directive が system prompt に含まれる
        // SAFETY: single-threaded via ENV_LOCK、test 専用
        unsafe { std::env::set_var("BONSAI_T6_PROMPT_AUGMENT", "1") };
        let base = "Base system prompt";
        let augmented = augment_system_prompt(base, CapabilityTier::LongHorizonPlanning);
        unsafe { std::env::remove_var("BONSAI_T6_PROMPT_AUGMENT") };
        assert!(
            augmented.contains("step-by-step plan"),
            "T6 task で 'step-by-step plan' directive 含む"
        );
    }

    #[test]
    fn t_t6_prompt_augment_env_on_non_t6_task_not_injected() {
        let _guard = ENV_LOCK.lock().unwrap();
        // env=1 + non-T6 task で directive 不在 (pass-through 確証、scope 制限)
        // SAFETY: single-threaded via ENV_LOCK、test 専用
        unsafe { std::env::set_var("BONSAI_T6_PROMPT_AUGMENT", "1") };
        let base = "Base system prompt";
        let augmented = augment_system_prompt(base, CapabilityTier::InstructionFollowing);
        unsafe { std::env::remove_var("BONSAI_T6_PROMPT_AUGMENT") };
        assert_eq!(augmented, base, "non-T6 tier では pass-through");
    }

    #[test]
    fn t_t6_prompt_augment_includes_three_directives() {
        // directive text が 3 keyword 含む (env 不要、pure text 確証)
        let dir = t6_augment_directive();
        assert!(
            dir.contains("step-by-step plan"),
            "directive 1: step-by-step plan"
        );
        assert!(
            dir.contains("restate plan progress"),
            "directive 2: restate plan progress"
        );
        assert!(dir.contains("revise plan"), "directive 3: revise plan");
    }

    #[test]
    fn t_t6_augment_wiring_env_on_includes_directive() {
        // Phase 4 wiring 確証: augment_system_prompt を直接呼び出し、
        // env=1 + T6 tier で directive が system prompt に含まれることを確認
        // (benchmark.rs の wiring 経路と同一 fn を通じた end-to-end 確証)
        let _guard = ENV_LOCK.lock().unwrap();
        // SAFETY: single-threaded via ENV_LOCK、test 専用
        unsafe { std::env::set_var("BONSAI_T6_PROMPT_AUGMENT", "1") };
        let base = "You are a helpful assistant.";
        let result = augment_system_prompt(base, CapabilityTier::LongHorizonPlanning);
        unsafe { std::env::remove_var("BONSAI_T6_PROMPT_AUGMENT") };
        assert!(result.starts_with(base), "base prompt は先頭に保持される");
        assert!(
            result.contains("[T6 LongHorizon Planning Directives]"),
            "wiring 経路で T6 directive ヘッダが含まれる"
        );
        assert!(result.len() > base.len(), "augment 後は base より長い");
    }
}
