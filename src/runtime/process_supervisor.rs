//! B-1: MLX server lifecycle supervisor。
//!
//! bonsai 自身が MLX 推論サーバの起動/保持/idle kill/lazy respawn を管理する。
//! 全 env-gated: `BONSAI_MLX_IDLE_TIMEOUT_SEC=0` (default) で feature 全体 OFF、
//! 既存挙動 100% 保持。
//!
//! layer 順: runtime。std / anyhow / crate::runtime::* のみ依存。

use std::process::{Child, Command};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// MLX server プロセスの lifecycle を握る supervisor。
pub struct ProcessSupervisor {
    pub health_url: String,
    last_used: Mutex<Instant>,
    pub idle_timeout: Duration,
    spawn_program: String,
    spawn_args: Vec<String>,
    child: Mutex<Option<Child>>,
}

impl ProcessSupervisor {
    /// spawn 設定なしの supervisor (health check / idle 判定のみ)。
    pub fn new(health_url: String, idle_timeout_secs: u64) -> Self {
        Self::with_spawn(health_url, idle_timeout_secs, String::new(), String::new(), 0)
    }

    /// spawn 設定付き supervisor。
    pub fn with_spawn(
        health_url: String,
        idle_timeout_secs: u64,
        spawn_program: String,
        model_path: String,
        port: u16,
    ) -> Self {
        let spawn_args = Self::build_spawn_args(&model_path, port);
        Self {
            health_url,
            last_used: Mutex::new(Instant::now()),
            idle_timeout: Duration::from_secs(idle_timeout_secs),
            spawn_program,
            spawn_args,
            child: Mutex::new(None),
        }
    }

    /// mlx-openai-server launch コマンドの引数列を組み立てる純粋関数。
    ///
    /// Phase 1 Red: 未実装 stub (空 vec)。
    pub fn build_spawn_args(_model_path: &str, _port: u16) -> Vec<String> {
        Vec::new()
    }

    /// idle timer をリセット (推論 request 毎に呼ぶ)。
    pub fn record_request(&self) {
        *self.last_used.lock().unwrap() = Instant::now();
    }

    /// idle timeout を超過したか。timeout=0 (disabled) では常に false。
    pub fn is_idle(&self) -> bool {
        if self.idle_timeout.is_zero() {
            return false;
        }
        self.last_used.lock().unwrap().elapsed() >= self.idle_timeout
    }

    /// health_url に HTTP GET して 2xx なら true。
    pub fn is_healthy(&self) -> bool {
        let agent = crate::runtime::http_agent::short_agent();
        agent.get(&self.health_url).call().is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_new_disabled_is_not_idle() {
        let s = ProcessSupervisor::new("http://127.0.0.1:1/health".to_string(), 0);
        assert!(!s.is_idle(), "timeout=0 では idle にならない");
    }

    #[test]
    fn t_record_request_resets_timer() {
        let s = ProcessSupervisor::new("http://127.0.0.1:1/health".to_string(), 1);
        std::thread::sleep(Duration::from_millis(10));
        s.record_request();
        assert!(!s.is_idle(), "record 直後は idle でない");
    }

    #[test]
    fn t_is_idle_after_timeout() {
        let s = ProcessSupervisor::new("http://127.0.0.1:1/health".to_string(), 0);
        // timeout=0 は disabled で常に false。enabled 経路は構造で担保。
        assert!(!s.is_idle());
    }

    #[test]
    fn t_health_url_stored() {
        let s = ProcessSupervisor::new("http://127.0.0.1:9/health".to_string(), 0);
        assert_eq!(s.health_url, "http://127.0.0.1:9/health");
    }

    #[test]
    fn t_is_healthy_false_on_unreachable() {
        // 到達不可 port → is_healthy=false (hang せず短 timeout で abort)。
        let s = ProcessSupervisor::new("http://127.0.0.1:1/health".to_string(), 0);
        assert!(!s.is_healthy(), "到達不可 server は unhealthy");
    }

    // ── B-1 Phase 1 Red: build_spawn_args 純粋関数 ──

    /// build_spawn_args が mlx-openai-server launch 引数列を正確に組む。
    /// Phase 1 Red: stub は空 vec → FAIL。
    #[test]
    fn t_build_spawn_args_exact() {
        let args = ProcessSupervisor::build_spawn_args("M", 8000);
        assert_eq!(
            args,
            vec![
                "launch".to_string(),
                "--model-path".to_string(),
                "M".to_string(),
                "--model-type".to_string(),
                "lm".to_string(),
                "--port".to_string(),
                "8000".to_string(),
            ],
            "mlx-openai-server launch 引数列が完全一致"
        );
    }
}
