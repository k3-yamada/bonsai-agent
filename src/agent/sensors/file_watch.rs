//! フェーズ1 センサー: ファイル監視センサー (FileWatchSensor)
//!
//! 指定ディレクトリを軽量ポーリングで監視し、新規作成されたファイルを検知する。
//! プライバシーフィルタ（PrivacyFilter）により、機微パスは事前に除外される。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use crate::agent::sensors::sensor::{Sensor, SensorEvent};
use crate::cancel::CancellationToken;
use crate::safety::sensor_filter::PrivacyFilter;

pub struct FileWatchSensor {
    pub watch_name: &'static str,
    pub path: PathBuf,
    pub poll_interval: Duration,
    pub filter: PrivacyFilter,
}

impl FileWatchSensor {
    pub fn new(watch_name: &'static str, path: impl Into<PathBuf>) -> Self {
        Self {
            watch_name,
            path: path.into(),
            poll_interval: Duration::from_millis(500),
            filter: PrivacyFilter::new(),
        }
    }

    /// 単発スキャンを行い、新規ファイルを収集して既知セットを更新する
    pub fn scan_new_files(&self, dir: &Path, known: &mut HashSet<PathBuf>) -> Vec<PathBuf> {
        let mut new_files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() && !known.contains(&p) {
                    known.insert(p.clone());
                    if self.filter.is_path_safe(&p) {
                        new_files.push(p);
                    }
                }
            }
        }
        new_files
    }
}

impl Sensor for FileWatchSensor {
    fn name(&self) -> &'static str {
        self.watch_name
    }

    fn run(&self, tx: mpsc::Sender<SensorEvent>, cancel: CancellationToken) {
        let mut known = HashSet::new();

        // 初回は既存ファイルを記録（通知はしない）
        let _ = self.scan_new_files(&self.path, &mut known);

        while !cancel.is_cancelled() {
            std::thread::sleep(self.poll_interval);
            if cancel.is_cancelled() {
                break;
            }

            let new_files = self.scan_new_files(&self.path, &mut known);
            for path in new_files {
                let ev = SensorEvent::FileAdded {
                    path,
                    watch_name: self.watch_name,
                };
                if tx.send(ev).is_err() {
                    return; // 受信側がクローズされたら終了
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_watch_sensor_detects_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let sensor = FileWatchSensor::new("test_watch", dir.path());

        let mut known = HashSet::new();
        // 初期スキャン: 空
        let new1 = sensor.scan_new_files(dir.path(), &mut known);
        assert!(new1.is_empty());

        // 新規ファイル作成
        let new_file_path = dir.path().join("entry.txt");
        std::fs::write(&new_file_path, "hello").unwrap();

        // 2回目スキャン: 検知
        let new2 = sensor.scan_new_files(dir.path(), &mut known);
        assert_eq!(new2.len(), 1);
        assert_eq!(new2[0], new_file_path);

        // 3回目スキャン: 既知のため0
        let new3 = sensor.scan_new_files(dir.path(), &mut known);
        assert!(new3.is_empty());
    }

    #[test]
    fn test_file_watch_sensor_filters_sensitive_file() {
        let dir = tempfile::tempdir().unwrap();
        let sensor = FileWatchSensor::new("test_watch", dir.path());

        let mut known = HashSet::new();

        // 機微ファイル作成 (.env)
        let secret_path = dir.path().join(".env");
        std::fs::write(&secret_path, "SECRET=123").unwrap();

        let new_files = sensor.scan_new_files(dir.path(), &mut known);
        assert!(new_files.is_empty(), "機微ファイルは除外されること");
    }
}
