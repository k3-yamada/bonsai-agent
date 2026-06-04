//! MLX サーバのプロセス監視 — アイドルタイムアウトによる自動 SIGTERM 機能
//!
//! M2 16GB RAM 圧迫対策として、`BONSAI_MLX_IDLE_TIMEOUT_SEC=N` を設定すると
//! N 秒間推論リクエストがない場合に MLX サーバを自動停止できる。
//! デフォルト (0 または未設定) は無効 — 後方互換。

use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct ProcessSupervisor {
    /// MLX サーバのヘルスチェック URL (例: "http://localhost:8888/health")
    pub health_url: String,
    last_used: Mutex<Instant>,
    /// Duration::ZERO = 無効
    pub idle_timeout: Duration,
}

impl ProcessSupervisor {
    pub fn new(health_url: String, idle_timeout_secs: u64) -> Self {
        Self {
            health_url,
            last_used: Mutex::new(Instant::now()),
            idle_timeout: Duration::from_secs(idle_timeout_secs),
        }
    }

    /// 推論リクエスト前後に呼び出してアイドルタイマーをリセットする
    pub fn record_request(&self) {
        *self.last_used.lock().unwrap() = Instant::now();
    }

    /// idle_timeout を超えてリクエストがない場合に true を返す。
    /// タイムアウト無効時 (0) は常に false。
    pub fn is_idle(&self) -> bool {
        if self.idle_timeout.is_zero() {
            return false;
        }
        self.last_used.lock().unwrap().elapsed() > self.idle_timeout
    }

    /// MLX サーバがヘルスチェックに応答するか確認する
    pub fn is_healthy(&self) -> bool {
        let agent = crate::runtime::http_agent::short_agent();
        agent.get(&self.health_url).call().is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn make_sup(secs: u64) -> ProcessSupervisor {
        ProcessSupervisor::new("http://localhost:8888/health".to_string(), secs)
    }

    #[test]
    fn t_idle_disabled_when_timeout_zero() {
        let sup = make_sup(0);
        assert!(!sup.is_idle(), "is_idle must be false when timeout=0");
    }

    #[test]
    fn t_not_idle_immediately_after_creation() {
        let sup = make_sup(60);
        assert!(
            !sup.is_idle(),
            "freshly created supervisor should not be idle"
        );
    }

    #[test]
    fn t_record_request_resets_idle_timer() {
        let sup = make_sup(1);
        thread::sleep(Duration::from_millis(800));
        sup.record_request();
        assert!(
            !sup.is_idle(),
            "should not be idle after record_request within timeout"
        );
    }

    #[test]
    fn t_mlx_idle_timeout_sec_default_zero() {
        // Safety: テスト専用、並列 env アクセスは単一テストスレッド内で完結
        unsafe { std::env::remove_var("BONSAI_MLX_IDLE_TIMEOUT_SEC") };
        assert_eq!(crate::config::mlx_idle_timeout_sec(), 0);
    }

    #[test]
    fn t_mlx_idle_timeout_sec_parses_env() {
        // Safety: テスト専用、並列 env アクセスは単一テストスレッド内で完結
        unsafe { std::env::set_var("BONSAI_MLX_IDLE_TIMEOUT_SEC", "600") };
        assert_eq!(crate::config::mlx_idle_timeout_sec(), 600);
        unsafe { std::env::remove_var("BONSAI_MLX_IDLE_TIMEOUT_SEC") };
    }
}
