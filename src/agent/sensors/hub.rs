//! 同期 SensorHub（各センサースレッドの統括・イベント集約）

use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use crate::agent::sensors::sensor::{Sensor, SensorEvent};
use crate::cancel::CancellationToken;

pub struct SensorHub {
    sensors: Vec<Arc<dyn Sensor>>,
}

impl SensorHub {
    pub fn new(sensors: Vec<Arc<dyn Sensor>>) -> Self {
        Self { sensors }
    }

    /// 全センサーをバックグラウンド同期スレッドで起動し、
    /// イベント集約用の Receiver とスレッドハンドルのリストを返す。
    /// （OS権限が必要なセンサーは権限チェックを行い、未許可時は安全にスキップする）
    pub fn spawn_all(
        &self,
        cancel: CancellationToken,
    ) -> (mpsc::Receiver<SensorEvent>, Vec<JoinHandle<()>>) {
        self.spawn_all_with_permission_check(cancel)
    }

    /// 権限チェック付きで全センサーを起動する。
    /// 権限拒否されたセンサーのみスキップし、他のセンサーは正常起動する（分離設計）。
    pub fn spawn_all_with_permission_check(
        &self,
        cancel: CancellationToken,
    ) -> (mpsc::Receiver<SensorEvent>, Vec<JoinHandle<()>>) {
        let (tx, rx) = mpsc::channel();
        let mut handles = Vec::new();

        for sensor in &self.sensors {
            if sensor.requires_permission() {
                let perm = crate::agent::sensors::window::check_accessibility_permission();
                if perm != crate::agent::sensors::window::PermissionState::Granted {
                    crate::observability::logger::log_event(
                        crate::observability::logger::LogLevel::Warn,
                        "sensor_hub",
                        &format!(
                            "センサー '{}' は必要なOS権限 ({:?}) が未付与のためスキップします",
                            sensor.name(),
                            perm
                        ),
                    );
                    continue;
                }
            }

            let tx_clone = tx.clone();
            let cancel_clone = cancel.clone();
            let sensor_clone = Arc::clone(sensor);

            let handle = thread::spawn(move || {
                sensor_clone.run(tx_clone, cancel_clone);
            });
            handles.push(handle);
        }

        (rx, handles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct DummySensor(&'static str);
    impl Sensor for DummySensor {
        fn name(&self) -> &'static str {
            self.0
        }
        fn run(&self, tx: mpsc::Sender<SensorEvent>, cancel: CancellationToken) {
            let _ = tx.send(SensorEvent::UserMessage(format!("hello from {}", self.0)));
            while !cancel.is_cancelled() {
                thread::sleep(Duration::from_millis(10));
            }
        }
    }

    #[test]
    fn test_sensor_hub_spawn_and_shutdown() {
        let sensors: Vec<Arc<dyn Sensor>> = vec![
            Arc::new(DummySensor("sensor_1")),
            Arc::new(DummySensor("sensor_2")),
        ];
        let hub = SensorHub::new(sensors);
        let cancel = CancellationToken::new();

        let (rx, handles) = hub.spawn_all(cancel.clone());

        // 各センサーからメッセージを受信できること
        let msg1 = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let msg2 = rx.recv_timeout(Duration::from_secs(1)).unwrap();

        assert!(matches!(msg1, SensorEvent::UserMessage(_)));
        assert!(matches!(msg2, SensorEvent::UserMessage(_)));

        // キャンセルで全スレッドが正常終了すること
        cancel.cancel();
        for h in handles {
            h.join().unwrap();
        }
    }

    struct PermissionSensor(&'static str);
    impl Sensor for PermissionSensor {
        fn name(&self) -> &'static str {
            self.0
        }
        fn requires_permission(&self) -> bool {
            true
        }
        fn run(&self, tx: mpsc::Sender<SensorEvent>, _cancel: CancellationToken) {
            let _ = tx.send(SensorEvent::UserMessage(format!("hello from {}", self.0)));
        }
    }

    #[test]
    fn test_sensor_hub_skips_denied_permission_sensor() {
        unsafe { std::env::set_var("BONSAI_MOCK_ACCESSIBILITY", "denied") };

        let sensors: Vec<Arc<dyn Sensor>> = vec![
            Arc::new(DummySensor("normal_sensor")),
            Arc::new(PermissionSensor("restricted_sensor")),
        ];
        let hub = SensorHub::new(sensors);
        let cancel = CancellationToken::new();

        let (rx, handles) = hub.spawn_all_with_permission_check(cancel.clone());

        // 権限不要の normal_sensor だけが起動していること (handles.len() == 1)
        assert_eq!(handles.len(), 1);

        let msg = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            msg,
            SensorEvent::UserMessage("hello from normal_sensor".to_string())
        );

        // restricted_sensor からのメッセージは届かない
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());

        cancel.cancel();
        for h in handles {
            h.join().unwrap();
        }

        unsafe { std::env::remove_var("BONSAI_MOCK_ACCESSIBILITY") };
    }
}
