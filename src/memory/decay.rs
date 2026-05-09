//! Power-law fidelity decay model.
//!
//! Ported from cerememory-decay/src/math.rs (MIT, Copyright 2026 CORe Inc.,
//! commit b08d201, https://github.com/co-r-e/cerememory).
//! See ADR-005 of the source repository for design rationale.
//!
//! All functions are pure (stateless, side-effect-free), `#[inline]` for
//! hot-path performance, and rayon-parallel safe.
//!
//! ## bonsai 用法 (項目 217)
//! - `HeuristicStore::record_outcome` で `compute_stability_boost` を opt-in 適用
//! - `HeuristicStore::prune` で decay-adjusted score 経路を opt-in 提供
//! - `BONSAI_DECAY_ENABLED=1` で有効化、unset で legacy 経路 (default OFF)
//!
//! ## License
//! 本ファイルは MIT License で配布される Cerememory プロジェクトの
//! `cerememory-decay/src/math.rs` から逐語 port。MIT 全文は
//! `docs/THIRD_PARTY_LICENSES.md` を参照。

/// `BONSAI_DECAY_ENABLED=1` (or "true"、case-insensitive) で decay 経路 opt-in。
///
/// production default = env unset = false 返却 = legacy prune 経路 (項目 213 動作)。
/// `BONSAI_DECAY_ENABLED=1` で stability column 更新 + decay-adjusted prune 有効化。
///
/// 設計方針: 項目 214 (`BONSAI_ERL_ENABLED` opt-in) と env name は対称、
/// 「production default は既存挙動」という基本方針は一貫。
pub(crate) fn is_decay_enabled() -> bool {
    std::env::var("BONSAI_DECAY_ENABLED")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Compute decayed fidelity using the modified power-law formula.
///
/// ```text
/// F(t) = F_0 * (1 + t/S)^(-d) * E_mod
/// ```
///
/// Returns a value clamped to [0.0, 1.0].
///
/// # Arguments
/// * `f0` - initial fidelity score (0.0..=1.0)
/// * `t_secs` - elapsed time in seconds since last access
/// * `stability` - stability constant S (increases with retrieval)
/// * `decay_exponent` - exponent d (default: 0.3)
/// * `emotion_mod` - emotional modulation factor E_mod (>= 1.0 for emotional memories)
#[inline]
pub(crate) fn compute_fidelity(
    f0: f64,
    t_secs: f64,
    stability: f64,
    decay_exponent: f64,
    emotion_mod: f64,
) -> f64 {
    debug_assert!(stability > 0.0, "stability must be positive");

    if t_secs <= 0.0 {
        // No time has passed; fidelity unchanged (but still apply emotion_mod clamping).
        return (f0 * emotion_mod).clamp(0.0, 1.0);
    }

    // (1 + t/S)^(-d)
    let temporal_decay = (1.0 + t_secs / stability).powf(-decay_exponent);

    (f0 * temporal_decay * emotion_mod).clamp(0.0, 1.0)
}

/// Compute accumulated noise level.
///
/// ```text
/// N(t) = N_0 + interference_rate * sqrt(t) * (1 - F(t))
/// ```
///
/// Returns a value clamped to [0.0, 1.0].
///
/// # Arguments
/// * `n0` - initial noise level (0.0..=1.0)
/// * `t_secs` - elapsed time in seconds
/// * `fidelity` - current fidelity F(t) after decay
/// * `interference_rate` - rate constant (default: 0.1)
///
/// Note: bonsai では現時点で未使用、Cerememory との parity 維持のため port。
#[allow(dead_code)]
#[inline]
pub(crate) fn compute_noise(n0: f64, t_secs: f64, fidelity: f64, interference_rate: f64) -> f64 {
    if t_secs <= 0.0 {
        return n0.clamp(0.0, 1.0);
    }

    let noise_increment = interference_rate * t_secs.sqrt() * (1.0 - fidelity);

    (n0 + noise_increment).clamp(0.0, 1.0)
}

/// Compute the new stability constant after a retrieval/reinforcement event.
///
/// ```text
/// S_new = S_old * (1 + retrieval_boost * S_old^(-0.2))
/// ```
///
/// # Arguments
/// * `s_old` - current stability constant
/// * `retrieval_boost` - boost constant (default: 1.5)
#[inline]
pub(crate) fn compute_stability_boost(s_old: f64, retrieval_boost: f64) -> f64 {
    debug_assert!(s_old > 0.0, "stability must be positive");

    s_old * (1.0 + retrieval_boost * s_old.powf(-0.2))
}

/// Compute the emotional modulation factor from emotion intensity.
///
/// ```text
/// E_mod = 1.0 + emotion_intensity * 0.5
/// ```
///
/// Emotional memories decay more slowly because E_mod > 1.0 acts as a
/// scaling factor that partially counteracts temporal decay.
///
/// Note: bonsai では現時点で未使用、Cerememory との parity 維持のため port。
/// 将来 emotion-aware heuristic ranking が必要になった場合に活用。
#[allow(dead_code)]
#[inline]
pub(crate) fn compute_emotion_mod(emotion_intensity: f64) -> f64 {
    1.0 + emotion_intensity * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    // env mutation race を避けるため module-local Mutex で serialize する
    // (項目 214 ERL_TEST_LOCK と同パターン)。
    static DECAY_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn reset_decay_env() {
        unsafe {
            std::env::remove_var("BONSAI_DECAY_ENABLED");
        }
    }

    // ── is_decay_enabled (env toggle) ─────────────────────────────────────

    #[test]
    fn t_is_decay_enabled_default_false() {
        let _g = DECAY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_decay_env();
        assert!(
            !is_decay_enabled(),
            "env unset で false (production default = legacy prune)"
        );
    }

    #[test]
    fn t_is_decay_enabled_explicit_true() {
        let _g = DECAY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_decay_env();
        unsafe {
            std::env::set_var("BONSAI_DECAY_ENABLED", "1");
        }
        assert!(is_decay_enabled(), "env=1 で true (decay 経路 opt-in)");
        for value in ["true", "TRUE", "True"] {
            unsafe {
                std::env::set_var("BONSAI_DECAY_ENABLED", value);
            }
            assert!(is_decay_enabled(), "env={value} (case-insensitive) で true");
        }
        reset_decay_env();
    }

    // ── compute_fidelity ──────────────────────────────────────────────────

    #[test]
    fn t_compute_fidelity_no_time_elapsed_returns_f0_times_emod() {
        let f = compute_fidelity(1.0, 0.0, 1.0, 0.3, 1.0);
        assert!((f - 1.0).abs() < 1e-9, "F(0) = F_0 * E_mod = 1.0、got {f}");
    }

    #[test]
    fn t_compute_fidelity_decreases_over_time() {
        let f0 = compute_fidelity(1.0, 0.0, 100.0, 0.3, 1.0);
        let f1h = compute_fidelity(1.0, 3600.0, 100.0, 0.3, 1.0);
        assert!(
            f1h < f0,
            "Fidelity must decrease over time: f1h={f1h} < f0={f0}"
        );
        assert!(f1h > 0.0, "Fidelity must remain positive: {f1h}");
    }

    #[test]
    fn t_compute_fidelity_emotion_mod_amplifies() {
        let f_neutral = compute_fidelity(1.0, 3600.0, 100.0, 0.3, 1.0);
        let f_emotional = compute_fidelity(1.0, 3600.0, 100.0, 0.3, 1.5);
        assert!(
            f_emotional > f_neutral,
            "E_mod>1 で decay slowed: {f_emotional} > {f_neutral}"
        );
    }

    // ── compute_noise ─────────────────────────────────────────────────────

    #[test]
    fn t_compute_noise_increases_with_sqrt_t() {
        // 値域は clamp [0.0, 1.0] に到達しないよう small interference + high fidelity で抑制。
        // n_1: 0 + 0.001 * sqrt(100) * (1 - 0.99) = 0.0001
        // n_2: 0 + 0.001 * sqrt(400) * (1 - 0.99) = 0.0002 = 2 * n_1
        let n0 = 0.0;
        let n_1 = compute_noise(n0, 100.0, 0.99, 0.001);
        let n_2 = compute_noise(n0, 400.0, 0.99, 0.001);
        assert!(n_2 > n_1, "Noise must accumulate as sqrt(t): {n_2} > {n_1}");
        // sqrt(400/100) = 2、つまり n_2 ≈ 2 * n_1
        assert!(
            (n_2 / n_1 - 2.0).abs() < 0.01,
            "sqrt(4) = 2 倍関係: ratio={}",
            n_2 / n_1
        );
    }

    // ── compute_stability_boost ───────────────────────────────────────────

    #[test]
    fn t_compute_stability_boost_increases_monotonically() {
        let s0 = 1.0;
        let s1 = compute_stability_boost(s0, 1.5);
        let s2 = compute_stability_boost(s1, 1.5);
        assert!(s1 > s0, "1 回目 boost で stability 増加: {s1} > {s0}");
        assert!(s2 > s1, "2 回目 boost で stability さらに増加: {s2} > {s1}");
    }

    // ── compute_emotion_mod ───────────────────────────────────────────────

    #[test]
    fn t_compute_emotion_mod_linear() {
        assert!(
            (compute_emotion_mod(0.0) - 1.0).abs() < 1e-9,
            "intensity=0 で E_mod=1.0"
        );
        assert!(
            (compute_emotion_mod(1.0) - 1.5).abs() < 1e-9,
            "intensity=1 で E_mod=1.5"
        );
        assert!(
            (compute_emotion_mod(2.0) - 2.0).abs() < 1e-9,
            "intensity=2 で E_mod=2.0"
        );
    }
}
