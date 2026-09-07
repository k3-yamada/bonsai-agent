//! 黄金比加法準乱数列によるジッタースケジューラ (DmnScheduler)
//!
//! 固定間隔や一様疑似乱数を避け、黄金比 φ ≈ 1.618... を使った加法的準乱数列で
//! 自発思考（DMNループ）の発火タイミングを散らす。
//! 短期的な偏り（連続して短間隔／長間隔が続く現象）を防ぎ、完全な周期性も排除する。

use std::time::Duration;

pub struct DmnScheduler {
    mean_sec: f64,
    min_gap_sec: f64,
    phase: f64, // 0.0..1.0, 呼ぶたびに phi だけ進めて fract() を取る
}

impl DmnScheduler {
    pub const PHI: f64 = 1.618_033_988_749_895;

    pub fn new(mean_sec: f64, min_gap_sec: f64) -> Self {
        Self {
            mean_sec,
            min_gap_sec,
            phase: 0.0,
        }
    }

    /// シード（初期フェーズ 0.0..1.0）を指定して生成（決定的テスト用）
    pub fn with_phase(mean_sec: f64, min_gap_sec: f64, phase: f64) -> Self {
        Self {
            mean_sec,
            min_gap_sec,
            phase: phase.fract().abs(),
        }
    }

    /// 次回発火までの待機時間を計算して返す。
    /// 長期平均は mean_sec に収束し、すべての値は min_gap_sec 以上となる。
    pub fn next_delay(&mut self) -> Duration {
        self.phase = (self.phase + Self::PHI).fract();
        // phase (0..1) を [min_gap, 2*mean - min_gap] に線形写像
        let span = 2.0 * (self.mean_sec - self.min_gap_sec).max(0.0);
        let delay = self.min_gap_sec + self.phase * span;
        Duration::from_secs_f64(delay.max(self.min_gap_sec))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dmn_scheduler_statistical_properties() {
        let mean = 60.0;
        let min_gap = 10.0;
        let mut scheduler = DmnScheduler::new(mean, min_gap);

        const N: usize = 10_000;
        let mut total_secs = 0.0;

        for _ in 0..N {
            let delay = scheduler.next_delay();
            let secs = delay.as_secs_f64();

            // すべての値が min_gap_sec 以上であること
            assert!(
                secs >= min_gap,
                "delay {secs} is less than min_gap {min_gap}"
            );

            total_secs += secs;
        }

        let empirical_mean = total_secs / N as f64;
        // 平均が mean_sec の ±5% 以内に収まること
        let diff_ratio = (empirical_mean - mean).abs() / mean;
        assert!(
            diff_ratio < 0.05,
            "empirical mean {empirical_mean} deviated from target {mean} by {diff_ratio:.4}"
        );
    }

    #[test]
    fn test_dmn_scheduler_deterministic_with_same_phase() {
        let mut s1 = DmnScheduler::with_phase(30.0, 5.0, 0.1234);
        let mut s2 = DmnScheduler::with_phase(30.0, 5.0, 0.1234);

        for _ in 0..100 {
            assert_eq!(s1.next_delay(), s2.next_delay());
        }
    }
}
