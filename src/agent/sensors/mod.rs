//! 外部知覚センサーモジュール
//!
//! ファイル監視、アイドル検知等のセンサーを統合し、
//! 同期スレッドモデルでイベントを集約する。

pub mod event_handler;
pub mod file_watch;
pub mod hub;
pub mod idle;
pub mod sensor;
pub mod visualization_sink;
pub mod window;

pub use event_handler::SensorEventHandler;
pub use file_watch::FileWatchSensor;
pub use hub::SensorHub;
pub use idle::IdleSensor;
pub use sensor::{Sensor, SensorEvent};
pub use visualization_sink::VisualizationSink;
pub use window::{PermissionState, WindowChangedSensor, check_accessibility_permission};
