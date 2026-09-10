use super::*;
use crate::observability::logger::{LogLevel, log_event};
use std::process::Command;

/// 直接実行サンドボックス（プロセスグループ分離 + POSIX rlimit による OS レベル保護）
#[derive(Debug, Default, Clone)]
pub struct DirectSandbox;

impl Sandbox for DirectSandbox {
    fn execute(&self, command: &str, args: &[&str], limits: &ResourceLimits) -> Result<ExecResult> {
        // 事前キャンセル済みなら子プロセスを起動しない (Issue #22 AC-2-1)。
        // cancel が None の場合この分岐は評価されず既存挙動と1bitも変わらない。
        if limits.cancel.as_ref().is_some_and(|c| c.is_cancelled()) {
            return Ok(cancelled_result());
        }

        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);

            let timeout_secs = limits.timeout.as_secs().max(1);
            let max_bytes = limits.max_output_bytes as u64;

            unsafe {
                cmd.pre_exec(move || {
                    // RLIMIT_CPU: CPU 時間上限 (タイムアウト + マージン)
                    let cpu_limit = libc::rlimit {
                        rlim_cur: timeout_secs + 2,
                        rlim_max: timeout_secs + 5,
                    };
                    libc::setrlimit(libc::RLIMIT_CPU, &cpu_limit);

                    // RLIMIT_FSIZE: 生成ファイルサイズ上限
                    let fsize_limit = libc::rlimit {
                        rlim_cur: max_bytes,
                        rlim_max: max_bytes * 2,
                    };
                    libc::setrlimit(libc::RLIMIT_FSIZE, &fsize_limit);

                    Ok(())
                });
            }
        }

        let child = cmd.spawn();

        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                return Ok(ExecResult {
                    stdout: String::new(),
                    stderr: format!("コマンド起動失敗: {e}"),
                    exit_code: -1,
                    timed_out: false,
                    cancelled: false,
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

        // タイムアウト/キャンセル付きで待機
        match child.wait_timeout_or_cancel(limits.timeout, limits.cancel.as_ref()) {
            Ok(WaitOutcome::Exited(status)) => {
                let stdout = out_handle.join().unwrap_or_default();
                let stderr = err_handle.join().unwrap_or_default();
                Ok(ExecResult {
                    stdout,
                    stderr,
                    exit_code: status.code().unwrap_or(-1),
                    timed_out: false,
                    cancelled: false,
                })
            }
            Ok(WaitOutcome::TimedOut) => {
                // タイムアウト — プロセスグループ全体を kill (SIGKILL) して子プロセスの孤児化を防ぐ
                kill_group_and_reap(&mut child, out_handle, err_handle);
                Ok(ExecResult {
                    stdout: String::new(),
                    stderr: format!("タイムアウト: {}秒を超過しました", limits.timeout.as_secs()),
                    exit_code: -1,
                    timed_out: true,
                    cancelled: false,
                })
            }
            Ok(WaitOutcome::Cancelled) => {
                log_event(
                    LogLevel::Warn,
                    "sandbox",
                    "キャンセル検知: 子プロセスグループをSIGKILL",
                );
                kill_group_and_reap(&mut child, out_handle, err_handle);
                Ok(ExecResult {
                    stdout: String::new(),
                    stderr: "キャンセル: ユーザー操作により中断されました".to_string(),
                    exit_code: -1,
                    timed_out: false,
                    cancelled: true,
                })
            }
            Err(e) => {
                kill_group_and_reap(&mut child, out_handle, err_handle);
                anyhow::bail!("プロセス待機中にエラー: {e}");
            }
        }
    }
}

/// 事前キャンセル済みトークンで子プロセスを起動しない場合の即時結果 (Issue #22 AC-2-1)。
fn cancelled_result() -> ExecResult {
    ExecResult {
        stdout: String::new(),
        stderr: "キャンセル: ユーザー操作により中断されました".to_string(),
        exit_code: -1,
        timed_out: false,
        cancelled: true,
    }
}

/// timeout/cancel/error の3分岐が共有する後始末。
/// libc::kill(-pid, SIGKILL) → child.kill() → child.wait() → reader thread join。
fn kill_group_and_reap(
    child: &mut std::process::Child,
    out_handle: std::thread::JoinHandle<String>,
    err_handle: std::thread::JoinHandle<String>,
) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = out_handle.join();
    let _ = err_handle.join();
}

/// wait_timeout は std::process::Child に存在しないので拡張トレイトで追加
#[derive(Debug)]
enum WaitOutcome {
    Exited(std::process::ExitStatus),
    TimedOut,
    Cancelled,
}

trait ChildExt {
    fn wait_timeout_or_cancel(
        &mut self,
        timeout: Duration,
        cancel: Option<&crate::cancel::CancellationToken>,
    ) -> std::io::Result<WaitOutcome>;
}

impl ChildExt for std::process::Child {
    fn wait_timeout_or_cancel(
        &mut self,
        timeout: Duration,
        cancel: Option<&crate::cancel::CancellationToken>,
    ) -> std::io::Result<WaitOutcome> {
        let start = std::time::Instant::now();
        let poll_interval = Duration::from_millis(50); // 既存値を変えない

        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(WaitOutcome::Exited(status));
            }
            // cancelをtimeoutより先に判定（取消の即時性を優先）。
            // cancel == Noneのときこの分岐は常にfalse = 既存挙動と1bitも変わらない。
            if cancel.is_some_and(|c| c.is_cancelled()) {
                return Ok(WaitOutcome::Cancelled);
            }
            if start.elapsed() >= timeout {
                return Ok(WaitOutcome::TimedOut);
            }
            std::thread::sleep(poll_interval);
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
    fn test_exec_process_group_timeout_kills_children() {
        let sandbox = DirectSandbox;
        let limits = ResourceLimits {
            timeout: Duration::from_millis(200),
            ..Default::default()
        };
        // 子シェルやバックグラウンドプロセスを巻き込んでタイムアウトさせた場合に孤児化せず停止すること
        let result = sandbox
            .execute("sh", &["-c", "sleep 10 & wait"], &limits)
            .unwrap();
        assert!(result.timed_out);
        assert!(!result.success());
    }

    #[test]
    fn test_direct_sandbox_execute_script() {
        let sandbox = DirectSandbox;
        let result = sandbox
            .execute_script("echo script_hello", &ResourceLimits::default())
            .unwrap();
        assert!(result.success());
        assert_eq!(result.stdout.trim(), "script_hello");
    }

    // --- Issue #22 B3-2: CancellationToken 伝播 (Red → Green) ---

    #[test]
    fn t_exec_cancel_returns_within_2s() {
        let cancel = crate::cancel::CancellationToken::new();
        let cancel_clone = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            cancel_clone.cancel();
        });
        let sandbox = DirectSandbox;
        let limits = ResourceLimits {
            timeout: Duration::from_secs(30),
            cancel: Some(cancel),
            ..Default::default()
        };
        let start = std::time::Instant::now();
        let result = sandbox.execute("sleep", &["10"], &limits).unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "cancel検知は2秒以内に戻るべき"
        );
        assert!(result.cancelled);
        assert!(!result.timed_out);
        assert!(!result.success());
    }

    #[test]
    fn t_exec_cancel_kills_process_group() {
        let cancel = crate::cancel::CancellationToken::new();
        let cancel_clone = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            cancel_clone.cancel();
        });
        let sandbox = DirectSandbox;
        let limits = ResourceLimits {
            timeout: Duration::from_secs(30),
            cancel: Some(cancel),
            ..Default::default()
        };
        let start = std::time::Instant::now();
        // バックグラウンドプロセスを巻き込んだ子でもプロセスグループごと即座に消えること
        let result = sandbox
            .execute("sh", &["-c", "sleep 10 & wait"], &limits)
            .unwrap();
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(result.cancelled);
    }

    #[test]
    fn t_exec_precancelled_token_does_not_spawn() {
        let cancel = crate::cancel::CancellationToken::new();
        cancel.cancel();
        let sandbox = DirectSandbox;
        let limits = ResourceLimits {
            cancel: Some(cancel),
            ..Default::default()
        };
        let start = std::time::Instant::now();
        let result = sandbox.execute("sleep", &["10"], &limits).unwrap();
        assert!(result.cancelled);
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "事前キャンセル済みなら子プロセスを起動せず即時返るべき"
        );
    }

    #[test]
    fn t_exec_cancel_none_is_identical_to_legacy() {
        let sandbox = DirectSandbox;

        let echo = sandbox
            .execute("echo", &["hi"], &ResourceLimits::default())
            .unwrap();
        assert!(echo.success());
        assert!(!echo.cancelled);

        let fail = sandbox
            .execute("false", &[], &ResourceLimits::default())
            .unwrap();
        assert!(!fail.success());
        assert!(!fail.cancelled);

        let limits = ResourceLimits {
            timeout: Duration::from_millis(200),
            ..Default::default()
        };
        let timeout_res = sandbox.execute("sleep", &["10"], &limits).unwrap();
        assert!(timeout_res.timed_out);
        assert!(!timeout_res.cancelled);
    }

    #[test]
    fn t_exec_result_cancelled_is_not_success() {
        let result = ExecResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
            cancelled: true,
        };
        assert!(!result.success());
    }
}
