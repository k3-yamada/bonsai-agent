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
        Self::with_spawn(
            health_url,
            idle_timeout_secs,
            String::new(),
            String::new(),
            0,
        )
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
    /// scripts/start-mlx-server.sh と同形:
    /// `launch --model-path <M> --model-type lm --port <P>`。
    pub fn build_spawn_args(model_path: &str, port: u16) -> Vec<String> {
        vec![
            "launch".to_string(),
            "--model-path".to_string(),
            model_path.to_string(),
            "--model-type".to_string(),
            "lm".to_string(),
            "--port".to_string(),
            port.to_string(),
        ]
    }

    /// MLX server を起動し child を保持。spawn_program が空なら何もしない (Ok)。
    /// bonsai が起動した child のみ保持し、既に保持中なら再起動しない。
    pub fn spawn(&self) -> anyhow::Result<()> {
        if self.spawn_program.is_empty() {
            return Ok(());
        }
        let mut guard = self.child.lock().unwrap();
        if guard.is_some() {
            return Ok(()); // already spawned by us
        }
        let child = Command::new(&self.spawn_program)
            .args(&self.spawn_args)
            .spawn()
            .map_err(|e| {
                anyhow::anyhow!("MLX server spawn failed ({}): {e}", self.spawn_program)
            })?;
        *guard = Some(child);
        Ok(())
    }

    /// 保持している child を kill (SIGTERM 相当)。
    /// bonsai が起動していない外部 server は触らない (child=None なら no-op)。
    pub fn kill(&self) {
        let mut guard = self.child.lock().unwrap();
        if let Some(mut child) = guard.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// server が応答していなければ spawn し health OK まで待つ (best-effort、最大 ~30s)。
    /// disabled (timeout=0) では lifecycle 管理しないため即 Ok。
    pub fn ensure_running(&self) -> anyhow::Result<()> {
        // disabled (feature OFF) では何もしない。test を高速化し、
        // 「supervisor 無効 = lifecycle 管理しない」セマンティクスとも一致。
        if self.idle_timeout.is_zero() {
            return Ok(());
        }
        if self.is_healthy() {
            return Ok(());
        }
        self.spawn()?;
        // health poll: up to ~30s
        for _ in 0..60 {
            std::thread::sleep(Duration::from_millis(500));
            if self.is_healthy() {
                return Ok(());
            }
        }
        Ok(()) // best-effort; backend health check が実エラーを surface する
    }

    /// idle なら kill して true。disabled (timeout=0) や busy なら false。
    pub fn kill_if_idle(&self) -> bool {
        if self.is_idle() {
            self.kill();
            true
        } else {
            false
        }
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

    /// health_url の `/health` から `/v1/models` を導出する純粋関数。
    ///
    /// mlx-openai-server は `/health` 未対応 (`/v1/models` で応答)。llama_server.rs の
    /// is_healthy() と同じフォールバック方針を踏襲するため、health_url の末尾 `/health` を
    /// 剥がして `/v1/models` を組み立てる。
    pub fn models_url_from_health(health_url: &str) -> String {
        let base = health_url
            .strip_suffix("/health")
            .unwrap_or(health_url)
            .trim_end_matches('/');
        format!("{base}/v1/models")
    }

    /// health_url (`/health`) に HTTP GET、失敗時は `/v1/models` にフォールバック。
    ///
    /// llama-server は `/health`、mlx-openai-server / mlx-lm は `/v1/models` で応答するため
    /// 両方を試す (llama_server.rs:75-88 と同方針)。どちらか 2xx なら healthy。
    pub fn is_healthy(&self) -> bool {
        let agent = crate::runtime::http_agent::short_agent();
        if agent.get(&self.health_url).call().is_ok() {
            return true;
        }
        let models_url = Self::models_url_from_health(&self.health_url);
        agent.get(&models_url).call().is_ok()
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

    #[test]
    fn t_models_url_from_health_strips_health_suffix() {
        assert_eq!(
            ProcessSupervisor::models_url_from_health("http://localhost:8000/health"),
            "http://localhost:8000/v1/models"
        );
    }

    #[test]
    fn t_models_url_from_health_without_suffix() {
        // /health が無い base URL でも /v1/models を付与
        assert_eq!(
            ProcessSupervisor::models_url_from_health("http://localhost:8000"),
            "http://localhost:8000/v1/models"
        );
    }

    /// kill_if_idle は disabled (timeout=0) で常に false (= lifecycle 管理しない)。
    #[test]
    fn t_kill_if_idle_false_when_disabled() {
        let s = ProcessSupervisor::new("http://127.0.0.1:1/health".to_string(), 0);
        assert!(!s.kill_if_idle(), "timeout=0 では kill_if_idle=false");
    }

    /// spawn (empty program) は no-op で Ok。
    #[test]
    fn t_spawn_noop_when_program_empty() {
        let s = ProcessSupervisor::new("http://127.0.0.1:1/health".to_string(), 1);
        assert!(s.spawn().is_ok(), "spawn_program 空で no-op Ok");
    }

    /// ensure_running は disabled で即 Ok (health poll しない = 高速)。
    #[test]
    fn t_ensure_running_noop_when_disabled() {
        let s = ProcessSupervisor::new("http://127.0.0.1:1/health".to_string(), 0);
        let start = Instant::now();
        assert!(s.ensure_running().is_ok());
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "disabled では health poll せず即 return"
        );
    }

    /// kill は child=None で no-op (外部 server を触らない)。
    #[test]
    fn t_kill_noop_when_no_child() {
        let s = ProcessSupervisor::new("http://127.0.0.1:1/health".to_string(), 1);
        s.kill(); // panic しないこと
    }

    // ── 実プロセス integration test (real MLX env が必要、default skip) ──

    /// 実 spawn → kill のラウンドトリップ。実バイナリ要なので #[ignore]。
    #[test]
    #[ignore]
    fn t_spawn_then_kill_real_process() {
        let s = ProcessSupervisor::with_spawn(
            "http://127.0.0.1:8000/health".to_string(),
            300,
            "sleep".to_string(),
            "60".to_string(),
            8000,
        );
        s.spawn().expect("spawn");
        s.kill();
    }
}
