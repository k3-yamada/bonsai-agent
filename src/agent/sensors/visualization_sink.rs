//! リアルタイム記憶可視化シンク (VisualizationSink)
//!
//! 第7章「可視化層への投資」に基づき、記憶ストアの変更や知覚イベントをフックし、
//! スロットリング（デバウンス）制御を伴いながら記憶力学グラフ HTML を自律更新する。

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::memory::html_viewer::export_and_render_html;
use crate::memory::store::MemoryStore;
use crate::observability::logger::{LogLevel, log_event};

/// リアルタイム記憶グラフ更新シンク
pub struct VisualizationSink {
    output_path: PathBuf,
    min_interval: Duration,
    filter_privacy: bool,
    last_updated: Mutex<Option<Instant>>,
    update_count: AtomicUsize,
}

impl VisualizationSink {
    /// 新規シンクを構築（デフォルト最小間隔: 2.0秒）
    pub fn new(
        output_path: impl Into<PathBuf>,
        min_interval: Duration,
        filter_privacy: bool,
    ) -> Self {
        Self {
            output_path: output_path.into(),
            min_interval,
            filter_privacy,
            last_updated: Mutex::new(None),
            update_count: AtomicUsize::new(0),
        }
    }

    /// 変更通知を受信。スロットル間隔を満たしていれば HTML を自動再生成して保存。
    /// 更新された場合は Ok(true)、スロットルでスキップされた場合は Ok(false) を返す。
    pub fn notify_change(&self, store: &MemoryStore) -> Result<bool> {
        let now = Instant::now();
        let mut last = self.last_updated.lock().unwrap();

        if let Some(prev) = *last
            && now.duration_since(prev) < self.min_interval
        {
            return Ok(false);
        }

        self.render_and_write(store, &self.output_path)?;
        *last = Some(now);
        self.update_count.fetch_add(1, Ordering::Relaxed);
        Ok(true)
    }

    /// スロットルを無視して強制的に最新状態をファイルへ出力
    pub fn force_sync(&self, store: &MemoryStore) -> Result<()> {
        let now = Instant::now();
        self.render_and_write(store, &self.output_path)?;
        let mut last = self.last_updated.lock().unwrap();
        *last = Some(now);
        self.update_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// これまでに実行された更新回数
    pub fn update_count(&self) -> usize {
        self.update_count.load(Ordering::Relaxed)
    }

    /// 出力先パスを取得
    pub fn output_path(&self) -> &Path {
        &self.output_path
    }

    fn render_and_write(&self, store: &MemoryStore, path: &Path) -> Result<()> {
        let html = export_and_render_html(store, self.filter_privacy)?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, html)?;
        log_event(
            LogLevel::Debug,
            "visualize",
            &format!("記憶力学グラフを自動更新: {}", path.display()),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_visualization_sink_throttling_and_force_sync() {
        let store = MemoryStore::in_memory().unwrap();
        store.save_memory("Sink test node 1", "fact", &[]).unwrap();

        let temp_path =
            std::env::temp_dir().join(format!("bonsai_sink_test_{}.html", std::process::id()));

        // 1秒間隔のスロットル設定
        let sink = VisualizationSink::new(&temp_path, Duration::from_millis(500), true);
        assert_eq!(sink.update_count(), 0);

        // 1回目の通知 -> 即時更新成功
        let updated1 = sink.notify_change(&store).unwrap();
        assert!(updated1, "初回は更新されること");
        assert_eq!(sink.update_count(), 1);
        assert!(temp_path.exists());

        // 直後の2回目通知（スロットル未満） -> スキップされること
        let updated2 = sink.notify_change(&store).unwrap();
        assert!(!updated2, "500ms以内の再通知はスキップされること");
        assert_eq!(sink.update_count(), 1);

        // force_sync -> スロットルに関係なく強制更新されること
        sink.force_sync(&store).unwrap();
        assert_eq!(sink.update_count(), 2);

        let content = std::fs::read_to_string(&temp_path).unwrap();
        assert!(content.contains("Sink test node 1"));

        let _ = std::fs::remove_file(&temp_path);
    }
}
