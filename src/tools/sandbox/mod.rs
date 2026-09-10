use anyhow::Result;
use std::time::Duration;

mod direct;
mod native;

pub use direct::DirectSandbox;
pub use native::{BubblewrapSandbox, NativeSandbox, SeatbeltSandbox};

/// リソース制限
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    /// コマンドのタイムアウト
    pub timeout: Duration,
    /// 最大出力バイト数（超過分は切り詰め）
    pub max_output_bytes: usize,
    /// 取消トークン（cross-cutting）。Someのとき待機ループが50ms間隔でcancelを
    /// ポーリングし、検知時にtimeoutと同一のkillpg経路でプロセスグループごとSIGKILLする。
    /// None（既定）のとき挙動はIssue #22以前と完全に同一。
    pub cancel: Option<crate::cancel::CancellationToken>,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_output_bytes: 1024 * 1024, // 1MB
            cancel: None,
        }
    }
}

impl ResourceLimits {
    /// 取消トークンを注入するビルダー（Issue #22 B3-2）。
    pub fn with_cancel(mut self, cancel: crate::cancel::CancellationToken) -> Self {
        self.cancel = Some(cancel);
        self
    }
}

/// コマンド実行結果
#[derive(Debug, Clone)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
    /// 取消により中断されたか。timed_outと排他（両方trueにはしない）。
    pub cancelled: bool,
}

impl ExecResult {
    pub fn success(&self) -> bool {
        self.exit_code == 0 && !self.timed_out && !self.cancelled
    }
}

/// サンドボックスの抽象化。DirectSandbox, SeatbeltSandbox, BubblewrapSandbox 等に差替可能。
pub trait Sandbox: Send + Sync {
    /// 実行ファイルと引数を直接実行する
    fn execute(&self, command: &str, args: &[&str], limits: &ResourceLimits) -> Result<ExecResult>;

    /// シェルスクリプト文字列を単一レベルの sh -c で実行する (H1: 二重 sh 解消)
    fn execute_script(&self, script: &str, limits: &ResourceLimits) -> Result<ExecResult> {
        self.execute("sh", &["-c", script], limits)
    }
}

/// シェルエスケープ（シングルクォートで囲む）
pub(crate) fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exec_result_success_check() {
        let ok = ExecResult {
            stdout: "out".to_string(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
            cancelled: false,
        };
        assert!(ok.success());

        let timeout = ExecResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: true,
            cancelled: false,
        };
        assert!(!timeout.success());
    }

    #[test]
    fn test_resource_limits_default() {
        let limits = ResourceLimits::default();
        assert_eq!(limits.timeout.as_secs(), 30);
        assert_eq!(limits.max_output_bytes, 1024 * 1024);
        assert!(limits.cancel.is_none());
    }

    #[test]
    fn test_shell_escape() {
        assert_eq!(shell_escape("hello"), "'hello'");
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }
}
