//! 外部知覚センサーイベントハンドラ (SensorEventHandler)
//!
//! 外部知覚イベント（ファイル更新、離席・復帰、アプリ切り替え）を受信し、
//! プライバシー保護規律を適用したうえで、意識層（ExperienceStore）へ安全に還元する。

use std::path::Path;

use crate::agent::sensors::sensor::SensorEvent;
use crate::agent::sensors::visualization_sink::VisualizationSink;
use crate::memory::experience::{ExperienceStore, ExperienceType, RecordParams};
use crate::memory::store::MemoryStore;
use crate::observability::logger::{LogLevel, log_event};

#[derive(Default)]
pub struct SensorEventHandler {
    visualization_sink: Option<VisualizationSink>,
}

impl SensorEventHandler {
    pub fn new() -> Self {
        Self {
            visualization_sink: None,
        }
    }

    /// 記憶力学グラフの自動更新シンクを設定
    pub fn with_visualization_sink(mut self, sink: VisualizationSink) -> Self {
        self.visualization_sink = Some(sink);
        self
    }

    /// 単一の知覚イベントを処理し、ログ出力および意識層（ExperienceStore）への還元を行う。
    pub fn handle_event(&self, event: &SensorEvent, store: Option<&MemoryStore>) {
        match event {
            SensorEvent::FileAdded { path, watch_name } => {
                let file_name = Path::new(path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown_file");

                log_event(
                    LogLevel::Info,
                    "sensor",
                    &format!("[{watch_name}] ファイル更新検知: {file_name}"),
                );

                if let Some(s) = store {
                    let exp_store = ExperienceStore::new(s.conn());
                    let outcome = format!("ファイル更新検知: {file_name} ({watch_name})");
                    let _ = exp_store.record(&RecordParams {
                        exp_type: ExperienceType::Insight,
                        task_context: "external_sensor:file_watch",
                        action: "file_modified",
                        outcome: &outcome,
                        lesson: Some("file_changed_in_workspace"),
                        tool_name: None,
                        error_type: None,
                        error_detail: None,
                    });
                }
            }

            SensorEvent::IdleStarted { since: _ } => {
                log_event(
                    LogLevel::Info,
                    "sensor",
                    "アイドル状態検知: DMN自発思考を活性化",
                );

                if let Some(s) = store {
                    let exp_store = ExperienceStore::new(s.conn());
                    let _ = exp_store.record(&RecordParams {
                        exp_type: ExperienceType::Insight,
                        task_context: "external_sensor:idle",
                        action: "user_idle_started",
                        outcome: "ユーザーの離席・アイドル状態を検知",
                        lesson: Some("idle_state_active"),
                        tool_name: None,
                        error_type: None,
                        error_detail: None,
                    });
                }
            }

            SensorEvent::IdleEnded { duration } => {
                let secs = duration.as_secs();
                log_event(
                    LogLevel::Info,
                    "sensor",
                    &format!("アイドル復帰検知: {secs}秒間アイドルでした"),
                );

                if let Some(s) = store {
                    let exp_store = ExperienceStore::new(s.conn());
                    let outcome = format!("ユーザーが復帰しました (離席時間: {secs}秒)");
                    let _ = exp_store.record(&RecordParams {
                        exp_type: ExperienceType::Insight,
                        task_context: "external_sensor:idle",
                        action: "user_idle_ended",
                        outcome: &outcome,
                        lesson: Some("user_returned"),
                        tool_name: None,
                        error_type: None,
                        error_detail: None,
                    });
                }
            }

            SensorEvent::WindowChanged { app, title: _ } => {
                // プライバシー規律: ウィンドウタイトル生文字列は記録せず、アプリ名のみを記録
                log_event(
                    LogLevel::Info,
                    "sensor",
                    &format!("アクティブアプリ切り替え検知: {app}"),
                );

                if let Some(s) = store {
                    let exp_store = ExperienceStore::new(s.conn());
                    let outcome = format!("ユーザーがアプリ '{app}' で作業中");
                    let _ = exp_store.record(&RecordParams {
                        exp_type: ExperienceType::Insight,
                        task_context: "external_sensor:window_focus",
                        action: "app_switched",
                        outcome: &outcome,
                        lesson: Some("context_switched"),
                        tool_name: None,
                        error_type: None,
                        error_detail: None,
                    });
                }
            }

            SensorEvent::UserMessage(msg) => {
                log_event(
                    LogLevel::Info,
                    "sensor",
                    &format!("ユーザー入力受信: {msg}"),
                );
            }
        }

        // 知覚イベントによって意識層・記憶が更新された場合、可視化シンクに通知
        if let (Some(sink), Some(s)) = (&self.visualization_sink, store) {
            let _ = sink.notify_change(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    #[test]
    fn test_handle_file_added_records_to_store() {
        let store = MemoryStore::in_memory().unwrap();
        let handler = SensorEventHandler::new();

        let event = SensorEvent::FileAdded {
            path: PathBuf::from("/Users/test/workspace/README.md"),
            watch_name: "workspace",
        };

        handler.handle_event(&event, Some(&store));

        let exp_store = ExperienceStore::new(store.conn());
        let insights = exp_store
            .find_similar("external_sensor:file_watch", 5)
            .unwrap();
        assert!(!insights.is_empty());
        assert!(insights[0].outcome.contains("README.md"));
        assert_eq!(insights[0].task_context, "external_sensor:file_watch");
    }

    #[test]
    fn test_handle_window_changed_records_app_only() {
        let store = MemoryStore::in_memory().unwrap();
        let handler = SensorEventHandler::new();

        let event = SensorEvent::WindowChanged {
            app: "Terminal".to_string(),
            title: "zsh - cargo test".to_string(),
        };

        handler.handle_event(&event, Some(&store));

        let exp_store = ExperienceStore::new(store.conn());
        let insights = exp_store
            .find_similar("external_sensor:window_focus", 5)
            .unwrap();
        assert!(!insights.is_empty());
        assert!(insights[0].outcome.contains("Terminal"));
        // ウィンドウタイトルの生文字列が含まれていないことを確認（プライバシー規律）
        assert!(!insights[0].outcome.contains("cargo test"));
        assert_eq!(insights[0].task_context, "external_sensor:window_focus");
    }

    #[test]
    fn test_handle_idle_events_record_to_store() {
        let store = MemoryStore::in_memory().unwrap();
        let handler = SensorEventHandler::new();

        let start_event = SensorEvent::IdleStarted {
            since: Instant::now(),
        };
        handler.handle_event(&start_event, Some(&store));

        let end_event = SensorEvent::IdleEnded {
            duration: Duration::from_secs(120),
        };
        handler.handle_event(&end_event, Some(&store));

        let exp_store = ExperienceStore::new(store.conn());
        let insights = exp_store.find_similar("external_sensor:idle", 5).unwrap();
        assert!(!insights.is_empty());
    }

    #[test]
    fn test_handle_event_triggers_visualization_sink() {
        let store = MemoryStore::in_memory().unwrap();
        let temp_html = std::env::temp_dir().join(format!(
            "bonsai_handler_sink_test_{}.html",
            std::process::id()
        ));

        let sink = VisualizationSink::new(&temp_html, Duration::from_millis(50), true);
        let handler = SensorEventHandler::new().with_visualization_sink(sink);

        let event = SensorEvent::FileAdded {
            path: PathBuf::from("src/main.rs"),
            watch_name: "test_watch",
        };

        handler.handle_event(&event, Some(&store));
        assert!(temp_html.exists(), "HTMLファイルが自動更新生成されること");

        let content = std::fs::read_to_string(&temp_html).unwrap();
        assert!(content.contains("<!DOCTYPE html>"));

        let _ = std::fs::remove_file(&temp_html);
    }
}
