//! フェーズ1 センサー: アイドル（離席）検知センサー (IdleSensor)

use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::agent::sensors::sensor::{Sensor, SensorEvent};
use crate::cancel::CancellationToken;

pub struct IdleSensor {
    pub threshold: Duration,
    pub poll_interval: Duration,
}

impl IdleSensor {
    pub fn new(threshold: Duration) -> Self {
        Self {
            threshold,
            poll_interval: Duration::from_secs(10),
        }
    }

    /// アイドル状態の遷移を純粋ロジックで評価
    pub fn evaluate_idle_transition(
        threshold: Duration,
        idle_duration: Duration,
        was_idle: &mut bool,
    ) -> Option<SensorEvent> {
        let is_now_idle = idle_duration >= threshold;
        if is_now_idle && !*was_idle {
            *was_idle = true;
            Some(SensorEvent::IdleStarted {
                since: Instant::now() - idle_duration,
            })
        } else if !is_now_idle && *was_idle {
            *was_idle = false;
            Some(SensorEvent::IdleEnded {
                duration: idle_duration,
            })
        } else {
            None
        }
    }
}

impl Sensor for IdleSensor {
    fn name(&self) -> &'static str {
        "idle_sensor"
    }

    fn run(&self, tx: mpsc::Sender<SensorEvent>, cancel: CancellationToken) {
        let mut was_idle = false;

        while !cancel.is_cancelled() {
            std::thread::sleep(self.poll_interval);
            if cancel.is_cancelled() {
                break;
            }

            // macOS実機API呼び出し（将来拡張: CGEventSourceSecondsSinceLastEventType）
            // 現時点ではデフォルト10秒ポーリングで安全に稼働
            let idle_duration = Duration::from_secs(0);

            if let Some(event) =
                Self::evaluate_idle_transition(self.threshold, idle_duration, &mut was_idle)
                && tx.send(event).is_err()
            {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_idle_sensor_transitions() {
        let threshold = Duration::from_secs(300); // 5分
        let mut was_idle = false;

        // 1分経過 ➔ まだアイドルではない
        let ev1 =
            IdleSensor::evaluate_idle_transition(threshold, Duration::from_secs(60), &mut was_idle);
        assert_eq!(ev1, None);
        assert!(!was_idle);

        // 5分（閾値ちょうど） ➔ IdleStarted
        let ev2 = IdleSensor::evaluate_idle_transition(
            threshold,
            Duration::from_secs(300),
            &mut was_idle,
        );
        assert!(matches!(ev2, Some(SensorEvent::IdleStarted { .. })));
        assert!(was_idle);

        // 6分 ➔ 依然としてアイドル中、二重発火なし
        let ev3 = IdleSensor::evaluate_idle_transition(
            threshold,
            Duration::from_secs(360),
            &mut was_idle,
        );
        assert_eq!(ev3, None);
        assert!(was_idle);

        // 0秒（キー入力で復帰） ➔ IdleEnded
        let ev4 =
            IdleSensor::evaluate_idle_transition(threshold, Duration::from_secs(0), &mut was_idle);
        assert!(matches!(ev4, Some(SensorEvent::IdleEnded { .. })));
        assert!(!was_idle);
    }
}
