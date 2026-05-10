//! sqlite-vec vec_memories 動的 populate 機構の env opt-in toggle。
//!
//! plan: `.claude/plan/sqlite-vec-a1-a3-impl.md` D-1 / G-2.1
//! 親 plan: `.claude/plan/sqlite-vec-activation-impl.md` §2 G-2.5 / §8
//!
//! ## 背景
//! 項目 220 で sqlite-vec Phase 0-5 完遂、G-2.5 caller 配線「不要確定」を
//! 「未配線で score 退行ゼロ」観測に依拠して下した。本 toggle はその判断を
//! Lab paired t-test で再評価するための env スイッチを提供する。
//!
//! ## 設計方針
//! - Cerememory 三本柱 (項目 217 decay / 218 review / 219 working cap) と
//!   env name 対称: `BONSAI_VEC_INDEX_ENABLED`
//! - production default = env unset = false (= 既存挙動 100% 維持)
//! - opt-in: `BONSAI_VEC_INDEX_ENABLED=1` または `BONSAI_VEC_INDEX_ENABLED=true`
//!   (case-insensitive) で `MemoryStore::index_memory_if_enabled` の vec_memories
//!   投入を有効化

/// `BONSAI_VEC_INDEX_ENABLED=1` (or "true"、case-insensitive) で
/// `save_memory` 直後の vec_memories 動的 populate 経路を opt-in。
///
/// production default = env unset = false 返却 = 既存挙動（vec_memories は
/// `ensure_vec_table` の eager backfill 後は静的、新規 memory 不投入）。
///
/// `=1` / `=true` (case-insensitive) → true。
/// `=0` / `=false` / 空文字列 / その他任意の値 → false。
///
/// 設計方針: 項目 217/218/219 と env name 対称、`is_decay_enabled` と
/// 同じ判定ロジック (`v == "1" || v.eq_ignore_ascii_case("true")`)。
pub(crate) fn is_vec_index_enabled() -> bool {
    std::env::var("BONSAI_VEC_INDEX_ENABLED")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[cfg(test)]
pub(crate) static VEC_INDEX_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;

    /// env mutation race を serialize する RAII guard (項目 214/217/218 同 pattern)。
    fn lock_env() -> MutexGuard<'static, ()> {
        VEC_INDEX_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn t_1_1a_env_unset_returns_false() {
        let _g = lock_env();
        // SAFETY: test 専用 env mutation、TEST_LOCK で serialize 済。
        unsafe { std::env::remove_var("BONSAI_VEC_INDEX_ENABLED") };
        assert!(
            !is_vec_index_enabled(),
            "env unset で false 返却すべき (production default)"
        );
    }

    #[test]
    fn t_1_1b_env_one_returns_true() {
        let _g = lock_env();
        unsafe { std::env::set_var("BONSAI_VEC_INDEX_ENABLED", "1") };
        let result = is_vec_index_enabled();
        unsafe { std::env::remove_var("BONSAI_VEC_INDEX_ENABLED") };
        assert!(result, "env=1 で true 返却すべき");
    }

    #[test]
    fn t_1_1c_env_true_case_insensitive_returns_true() {
        let _g = lock_env();
        for val in ["true", "True", "TRUE", "TrUe"] {
            unsafe { std::env::set_var("BONSAI_VEC_INDEX_ENABLED", val) };
            let result = is_vec_index_enabled();
            unsafe { std::env::remove_var("BONSAI_VEC_INDEX_ENABLED") };
            assert!(result, "env={val} で true 返却すべき (case-insensitive)");
        }
    }

    #[test]
    fn t_1_1d_env_falsy_values_return_false() {
        let _g = lock_env();
        for val in ["0", "false", "False", "no", "yes", "", "garbage"] {
            unsafe { std::env::set_var("BONSAI_VEC_INDEX_ENABLED", val) };
            let result = is_vec_index_enabled();
            unsafe { std::env::remove_var("BONSAI_VEC_INDEX_ENABLED") };
            assert!(
                !result,
                "env={val:?} で false 返却すべき (1/true 以外は全て disabled)"
            );
        }
    }
}
