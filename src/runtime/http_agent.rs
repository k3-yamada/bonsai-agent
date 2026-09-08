//! 統一 HTTP Agent — socket-level timeout 強制で recvfrom hang を防ぐ
//!
//! Lab v13 で MLX サーバが応答停止 → ureq の bare `ureq::get/post` が
//! デフォルト無制限のため kernel `recvfrom` で永久待機する事象が発生。
//!
//! 全モジュール共通で `shared_agent()` を経由させることで、
//! `timeout_global` / `timeout_recv_body` / `timeout_recv_response` を
//! 必ず適用し、socket recv が deadline を超えたら自動 abort される。

use std::time::Duration;

use ureq::Agent;
use ureq::config::Config;

const DEFAULT_GLOBAL_TIMEOUT_SECS: u64 = 300;
const DEFAULT_RECV_RESPONSE_TIMEOUT_SECS: u64 = 30;
const DEFAULT_RECV_BODY_TIMEOUT_SECS: u64 = 180;
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;

#[derive(Debug, Clone, Copy)]
pub struct AgentTimeouts {
    pub global: Duration,
    pub connect: Duration,
    pub recv_response: Duration,
    pub recv_body: Duration,
}

impl Default for AgentTimeouts {
    fn default() -> Self {
        Self {
            global: Duration::from_secs(DEFAULT_GLOBAL_TIMEOUT_SECS),
            connect: Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS),
            recv_response: Duration::from_secs(DEFAULT_RECV_RESPONSE_TIMEOUT_SECS),
            recv_body: Duration::from_secs(DEFAULT_RECV_BODY_TIMEOUT_SECS),
        }
    }
}

impl AgentTimeouts {
    /// 短時間用（health check など、すぐ応答しないなら諦める）
    pub fn short() -> Self {
        Self {
            global: Duration::from_secs(15),
            connect: Duration::from_secs(5),
            recv_response: Duration::from_secs(10),
            recv_body: Duration::from_secs(15),
        }
    }

    /// SSE ストリーミング用（H18: ureq timeout 契約再設計）
    ///
    /// - `recv_response`: HTTP リクエスト送信完了からレスポンスヘッダ（初トークン）受信までのタイムアウト (TTFT)。
    ///   MLX 等のプリフィル待機時間を考慮し、引数で渡される秒数を設定。
    /// - `recv_body`: ヘッダ受信後、ストリーム全体の完了までのタイムアウト。
    ///   ストリーミング中のトークン生成が途中で不意に切断されないよう、十分な上限（最低 600s または TTFT の 4 倍）を確保。
    /// - `global`: コネクション確立から完了までの大域デッドライン。
    pub fn streaming(initial_chunk_secs: u64) -> Self {
        let ttft = if initial_chunk_secs == 0 {
            Duration::from_secs(DEFAULT_RECV_RESPONSE_TIMEOUT_SECS)
        } else {
            Duration::from_secs(initial_chunk_secs)
        };
        let stream_total = Duration::from_secs(600).max(ttft * 4);
        Self {
            global: stream_total + Duration::from_secs(120),
            connect: Duration::from_secs(10),
            recv_response: ttft,
            recv_body: stream_total,
        }
    }
}

/// 共通 ureq Agent — 全モジュールはこれ経由で HTTP 呼出する
pub fn shared_agent() -> Agent {
    build_agent(AgentTimeouts::default())
}

/// short timeout 用 agent（health check 等）
pub fn short_agent() -> Agent {
    build_agent(AgentTimeouts::short())
}

/// streaming 用 agent（指定秒数の初期チャンク待機時間 TTFT、0 はデフォルト 30s。ストリーム全体は十分な上限を確保）
pub fn streaming_agent(initial_chunk_secs: u64) -> Agent {
    build_agent(AgentTimeouts::streaming(initial_chunk_secs))
}

pub fn build_agent(t: AgentTimeouts) -> Agent {
    let config = Config::builder()
        .timeout_global(Some(t.global))
        .timeout_connect(Some(t.connect))
        .timeout_recv_response(Some(t.recv_response))
        .timeout_recv_body(Some(t.recv_body))
        .build();
    Agent::new_with_config(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_default_timeouts_are_set() {
        let t = AgentTimeouts::default();
        assert_eq!(t.global, Duration::from_secs(300));
        assert_eq!(t.connect, Duration::from_secs(10));
        assert_eq!(t.recv_response, Duration::from_secs(30));
        assert_eq!(t.recv_body, Duration::from_secs(180));
    }

    #[test]
    fn t_short_timeouts_are_smaller_than_default() {
        let s = AgentTimeouts::short();
        let d = AgentTimeouts::default();
        assert!(s.global < d.global);
        assert!(s.recv_body < d.recv_body);
    }

    #[test]
    fn t_streaming_configures_ttft_and_generous_body() {
        let t = AgentTimeouts::streaming(120);
        assert_eq!(t.recv_response, Duration::from_secs(120));
        assert_eq!(t.recv_body, Duration::from_secs(600));
        assert!(t.global > t.recv_body);
    }

    #[test]
    fn t_streaming_zero_uses_default_ttft() {
        let t = AgentTimeouts::streaming(0);
        assert_eq!(
            t.recv_response,
            Duration::from_secs(DEFAULT_RECV_RESPONSE_TIMEOUT_SECS)
        );
        assert_eq!(t.recv_body, Duration::from_secs(600));
    }

    #[test]
    fn t_shared_agent_is_constructible() {
        let _ = shared_agent();
        let _ = short_agent();
        let _ = streaming_agent(60);
    }

    /// ローカル listen → 受信して何も返さない（hang）→ deadline で abort
    #[test]
    #[ignore = "requires real local loopback socket binding (hangs in sandboxed environments)"]
    fn t_short_agent_aborts_on_hung_server() {
        use std::io::Read;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let handle = std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf);
                std::thread::sleep(Duration::from_secs(20));
            }
        });

        let url = format!("http://127.0.0.1:{port}/");
        let agent = short_agent();
        let start = std::time::Instant::now();
        let result = agent.get(&url).call();
        let elapsed = start.elapsed();

        assert!(result.is_err(), "should error out on hung server");
        assert!(
            elapsed < Duration::from_secs(18),
            "should abort within short global timeout, took {elapsed:?}"
        );

        let _ = handle.join();
    }
}
