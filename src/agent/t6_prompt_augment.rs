//! AgentFloor T6 (LongHorizonPlanning) tier 特化 prompt augmentation.
//!
//! 起点: `.claude/plan/agentfloor-t6-weakness-improvement.md` 案 A (Phase 1 Red)
//!
//! 役割:
//!   T6 tier task 検出時に system prompt の末尾に追加 directive を inject し、
//!   long-horizon plan の step-by-step 構造化を促す。env-gated (`BONSAI_T6_PROMPT_AUGMENT=1`)。
//!
//! 環境変数:
//!   BONSAI_T6_PROMPT_AUGMENT=1   # opt-in (default off)、unset で完全 no-op
//!
//! Contract:
//!   - env unset で 100% backward compat (augment 経路 skip、system prompt 不変)
//!   - T6 tier 以外は pass-through (Phase 1 確証 test)
//!   - directive は固定 text (3 件)、各 task ごとの dynamic 拡張は将来 phase
//!
//! 将来拡張:
//!   - 案 D Phase 2 で KG-augmented retrieval を本 module の hook 経由で activate
//!
//! 参照: docs/architecture/module-layer-rules.md (LOG-001 / agent layer)

use crate::agent::benchmark::CapabilityTier;

/// env getter (Phase 1 Red: 常に false return stub)
pub fn is_t6_prompt_augment_enabled() -> bool {
    false
}

/// 固定 directive text (Phase 1 Red stub)
pub fn t6_augment_directive() -> &'static str {
    ""
}

/// system prompt に T6 augment を append (Phase 1 Red: pass-through stub)
pub fn augment_system_prompt(system: &str, _task_tier: CapabilityTier) -> String {
    system.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_t6_prompt_augment_env_default_off() {
        // env unset で augment 経路通らない (stub 常時 false 確証)
        assert!(!is_t6_prompt_augment_enabled(), "env unset = false");
    }

    #[test]
    fn t_t6_prompt_augment_env_on_t6_task_injected() {
        // env=1 + T6 task で directive が system prompt に含まれる
        // Phase 1 Red: stub では directive="" + augment は pass-through なので FAIL 期待
        let base = "Base system prompt";
        let augmented = augment_system_prompt(base, CapabilityTier::LongHorizonPlanning);
        assert!(
            augmented.contains("step-by-step plan"),
            "T6 task で 'step-by-step plan' directive 含む (stub: pass-through で FAIL)"
        );
    }

    #[test]
    fn t_t6_prompt_augment_env_on_non_t6_task_not_injected() {
        // env=1 + non-T6 task で directive 不在 (pass-through 確証、scope 制限)
        // Phase 1 Red: stub では pass-through なので PASS (副作用 trivially)
        let base = "Base system prompt";
        let augmented = augment_system_prompt(base, CapabilityTier::InstructionFollowing);
        assert_eq!(augmented, base, "non-T6 tier では pass-through");
    }

    #[test]
    fn t_t6_prompt_augment_includes_three_directives() {
        // directive text が 3 keyword 含む
        // Phase 1 Red: stub では "" return なので 3 件全 FAIL
        let dir = t6_augment_directive();
        assert!(dir.contains("step-by-step plan"), "directive 1: step-by-step plan");
        assert!(dir.contains("restate plan progress"), "directive 2: restate plan progress");
        assert!(dir.contains("revise plan"), "directive 3: revise plan");
    }
}
