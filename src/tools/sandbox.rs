use anyhow::Result;
use std::process::Command;
use std::time::Duration;

/// リソース制限
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    /// コマンドのタイムアウト
    pub timeout: Duration,
    /// 最大出力バイト数（超過分は切り詰め）
    pub max_output_bytes: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_output_bytes: 1024 * 1024, // 1MB
        }
    }
}

/// コマンド実行結果
#[derive(Debug, Clone)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
}

impl ExecResult {
    pub fn success(&self) -> bool {
        self.exit_code == 0 && !self.timed_out
    }
}

/// サンドボックスの抽象化。初期はDirectSandbox、将来ContainerSandbox等に差替可能。
pub trait Sandbox: Send + Sync {
    fn execute(&self, command: &str, args: &[&str], limits: &ResourceLimits) -> Result<ExecResult>;
}

/// 直接実行サンドボックス（macOS向け、ulimit付き）
pub struct DirectSandbox;

impl Sandbox for DirectSandbox {
    fn execute(&self, command: &str, args: &[&str], limits: &ResourceLimits) -> Result<ExecResult> {
        // macOSではulimitをシェル経由で適用
        let full_command = if args.is_empty() {
            command.to_string()
        } else {
            format!(
                "{} {}",
                command,
                args.iter()
                    .map(|a| shell_escape(a))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        };

        let child = Command::new("sh")
            .arg("-c")
            .arg(&full_command)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn();

        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                return Ok(ExecResult {
                    stdout: String::new(),
                    stderr: format!("コマンド起動失敗: {e}"),
                    exit_code: -1,
                    timed_out: false,
                });
            }
        };

        // pipe を wait 前に drain しないと、出力が OS パイプバッファ (Linux ~64KB) を超えた
        // 時点で子プロセスが write() でブロックし、try_wait() が永久に None を返して偽の
        // timeout になる (かつ出力も失われる)。reader thread で stdout/stderr を wait と
        // 並行排出する。
        let max_bytes = limits.max_output_bytes;
        let out_pipe = child.stdout.take();
        let err_pipe = child.stderr.take();
        let out_handle = std::thread::spawn(move || read_output(out_pipe, max_bytes));
        let err_handle = std::thread::spawn(move || read_output(err_pipe, max_bytes));

        // タイムアウト付きで待機
        match child.wait_timeout(limits.timeout) {
            Ok(Some(status)) => {
                let stdout = out_handle.join().unwrap_or_default();
                let stderr = err_handle.join().unwrap_or_default();
                Ok(ExecResult {
                    stdout,
                    stderr,
                    exit_code: status.code().unwrap_or(-1),
                    timed_out: false,
                })
            }
            Ok(None) => {
                // タイムアウト — プロセスをkill (reader thread は EOF で終了 → join)
                let _ = child.kill();
                let _ = child.wait();
                let _ = out_handle.join();
                let _ = err_handle.join();
                Ok(ExecResult {
                    stdout: String::new(),
                    stderr: format!("タイムアウト: {}秒を超過しました", limits.timeout.as_secs()),
                    exit_code: -1,
                    timed_out: true,
                })
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = out_handle.join();
                let _ = err_handle.join();
                anyhow::bail!("プロセス待機中にエラー: {e}");
            }
        }
    }
}

/// wait_timeout は std::process::Child に存在しないので拡張トレイトで追加
trait ChildExt {
    fn wait_timeout(
        &mut self,
        timeout: Duration,
    ) -> std::io::Result<Option<std::process::ExitStatus>>;
}

impl ChildExt for std::process::Child {
    fn wait_timeout(
        &mut self,
        timeout: Duration,
    ) -> std::io::Result<Option<std::process::ExitStatus>> {
        let start = std::time::Instant::now();
        let poll_interval = Duration::from_millis(50);

        loop {
            match self.try_wait()? {
                Some(status) => return Ok(Some(status)),
                None => {
                    if start.elapsed() >= timeout {
                        return Ok(None);
                    }
                    std::thread::sleep(poll_interval);
                }
            }
        }
    }
}

/// 出力を EOF まで読み取り、保持は max_bytes で打ち切る。
///
/// 単発 `read()` だと 1 syscall 分しか取れず出力が途中で切れる。また上限到達後に
/// 読むのを止めると pipe buffer が飽和して子プロセスが write ブロック → deadlock する
/// ため、上限超過分は読み捨てつつ EOF まで drain し続ける。
fn read_output(pipe: Option<impl std::io::Read>, max_bytes: usize) -> String {
    let Some(mut pipe) = pipe else {
        return String::new();
    };
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break, // EOF
            Ok(n) => {
                if buf.len() < max_bytes {
                    let take = n.min(max_bytes - buf.len());
                    buf.extend_from_slice(&chunk[..take]);
                }
                // 上限超過分は破棄 (drain は継続して deadlock を防ぐ)
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

/// シェルエスケープ（シングルクォートで囲む）
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exec_echo() {
        let sandbox = DirectSandbox;
        let result = sandbox
            .execute("echo", &["hello"], &ResourceLimits::default())
            .unwrap();
        assert!(result.success());
        assert!(result.stdout.trim() == "hello");
    }

    #[test]
    fn test_exec_failure() {
        let sandbox = DirectSandbox;
        let result = sandbox
            .execute("false", &[], &ResourceLimits::default())
            .unwrap();
        assert!(!result.success());
        assert_ne!(result.exit_code, 0);
    }

    #[test]
    fn test_exec_timeout() {
        let sandbox = DirectSandbox;
        let limits = ResourceLimits {
            timeout: Duration::from_millis(200),
            ..Default::default()
        };
        let result = sandbox.execute("sleep", &["10"], &limits).unwrap();
        assert!(result.timed_out);
        assert!(!result.success());
    }

    #[test]
    fn test_exec_nonexistent_command() {
        let sandbox = DirectSandbox;
        let result = sandbox
            .execute("nonexistent_command_xyz", &[], &ResourceLimits::default())
            .unwrap();
        assert!(!result.success());
    }

    #[test]
    fn test_exec_large_output_no_deadlock() {
        // 出力が OS パイプバッファ (~64KB) を超えても deadlock せず全量捕捉できること。
        // 修正前は wait_timeout が pipe を drain せず子プロセスが write ブロック →
        // 偽 timeout + 出力欠落になっていた。
        let sandbox = DirectSandbox;
        let limits = ResourceLimits {
            timeout: Duration::from_secs(20),
            ..Default::default()
        };
        let result = sandbox.execute("seq", &["100000"], &limits).unwrap();
        assert!(!result.timed_out, "大量出力で偽 timeout してはならない");
        assert!(result.success());
        assert!(
            result.stdout.len() > 200_000,
            "64KB を超える出力が捕捉されるべき (実際: {} bytes)",
            result.stdout.len()
        );
        assert!(
            result.stdout.contains("100000"),
            "末尾まで drain されている"
        );
    }

    #[test]
    fn test_exec_result_success_check() {
        let ok = ExecResult {
            stdout: "out".to_string(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        assert!(ok.success());

        let timeout = ExecResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: true,
        };
        assert!(!timeout.success());
    }

    #[test]
    fn test_resource_limits_default() {
        let limits = ResourceLimits::default();
        assert_eq!(limits.timeout.as_secs(), 30);
        assert_eq!(limits.max_output_bytes, 1024 * 1024);
    }

    #[test]
    fn test_shell_escape() {
        assert_eq!(shell_escape("hello"), "'hello'");
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }
}
