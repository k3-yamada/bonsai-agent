//! 外部知覚センサーの基本型と trait

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::cancel::CancellationToken;

/// 外部知覚イベント
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SensorEvent {
    /// ユーザーからの明示的入力
    UserMessage(String),
    /// ファイルの追加・更新検知（安全フィルタ通過済み）
    FileAdded {
        path: PathBuf,
        watch_name: &'static str,
    },
    /// アイドル（離席）開始検知
    IdleStarted { since: Instant },
    /// アイドル終了検知
    IdleEnded { duration: Duration },
    /// ウィンドウ/アクティブアプリ切り替え検知（フェーズ2、プライバシーフィルタ通過済み）
    WindowChanged { app: String, title: String },
}

/// 全センサー共通のインターフェース
pub trait Sensor: Send + Sync {
    /// センサーの識別名
    fn name(&self) -> &'static str;

    /// OS等の明示的なユーザー許可を要求するセンサーかどうか（デフォルト: false）
    fn requires_permission(&self) -> bool {
        false
    }

    /// センサーを実行し、イベントを送信する（キャンセルされるまで同期ループ）
    fn run(&self, tx: mpsc::Sender<SensorEvent>, cancel: CancellationToken);
}
