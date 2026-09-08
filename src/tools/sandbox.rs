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

/// サンドボックスの抽象化。DirectSandbox, SeatbeltSandbox, BubblewrapSandbox 等に差替可能。
pub trait Sandbox: Send + Sync {
    /// 実行ファイルと引数を直接実行する
    fn execute(&self, command: &str, args: &[&str], limits: &ResourceLimits) -> Result<ExecResult>;

    /// シェルスクリプト文字列を単一レベルの sh -c で実行する (H1: 二重 sh 解消)
    fn execute_script(&self, script: &str, limits: &ResourceLimits) -> Result<ExecResult> {
        self.execute("sh", &["-c", script], limits)
    }
}

/// 直接実行サンドボックス（プロセスグループ分離 + POSIX rlimit による OS レベル保護）
#[derive(Debug, Default, Clone)]
pub struct DirectSandbox;

impl Sandbox for DirectSandbox {
    fn execute(&self, command: &str, args: &[&str], limits: &ResourceLimits) -> Result<ExecResult> {
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
                // タイムアウト — プロセスグループ全体を kill (SIGKILL) して子プロセスの孤児化を防ぐ
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
                Ok(ExecResult {
                    stdout: String::new(),
                    stderr: format!("タイムアウト: {}秒を超過しました", limits.timeout.as_secs()),
                    exit_code: -1,
                    timed_out: true,
                })
            }
            Err(e) => {
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

/// macOS Seatbelt (`sandbox-exec`) を用いた OS レベルサンドボックス
#[derive(Debug, Clone)]
pub struct SeatbeltSandbox {
    available: bool,
    profile: String,
    fallback: DirectSandbox,
}

impl SeatbeltSandbox {
    pub fn new(denied_paths: &[String]) -> Self {
        let available = Self::check_available();
        let profile = Self::build_profile(denied_paths);
        Self {
            available,
            profile,
            fallback: DirectSandbox,
        }
    }

    pub fn is_available(&self) -> bool {
        self.available
    }

    pub fn profile(&self) -> &str {
        &self.profile
    }

    fn check_available() -> bool {
        #[cfg(target_os = "macos")]
        {
            // sandbox-exec が実行可能かつプロファイル適用可能かテスト
            // (ネストされた sandbox や権限不足では exit code 71 / sandbox_apply: Operation not permitted となる)
            std::process::Command::new("/usr/bin/sandbox-exec")
                .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
                .output()
                .map(|out| out.status.success())
                .unwrap_or(false)
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    pub fn build_profile(denied_paths: &[String]) -> String {
        let mut deny_rules = Vec::new();
        // デフォルトで拒否する危険パス
        let default_denies = ["/etc/shadow", "/etc/master.passwd", "/etc/sudoers"];
        for d in default_denies {
            deny_rules.push(format!("  (subpath \"{}\")", d));
            deny_rules.push(format!("  (literal \"{}\")", d));
        }

        let home_dir = std::env::var("HOME").ok();

        for p in denied_paths {
            let p_clean = p.trim();
            if !p_clean.is_empty() {
                let expanded = if let Some(ref home) = home_dir {
                    if p_clean == "~" {
                        home.clone()
                    } else if let Some(rest) = p_clean.strip_prefix("~/") {
                        format!("{}/{}", home, rest)
                    } else {
                        p_clean.to_string()
                    }
                } else {
                    p_clean.to_string()
                };

                deny_rules.push(format!("  (subpath \"{}\")", expanded));
                deny_rules.push(format!("  (literal \"{}\")", expanded));
                if p_clean.starts_with('.') || !p_clean.contains('/') {
                    deny_rules.push(format!("  (regex #\"{}.*\")", regex_escape(p_clean)));
                }
            }
        }
        deny_rules.sort();
        deny_rules.dedup();

        format!(
            "(version 1)\n(allow default)\n(deny file-read* file-write*\n{}\n)\n",
            deny_rules.join("\n")
        )
    }
}

impl Default for SeatbeltSandbox {
    fn default() -> Self {
        Self::new(&[])
    }
}

impl Sandbox for SeatbeltSandbox {
    fn execute(&self, command: &str, args: &[&str], limits: &ResourceLimits) -> Result<ExecResult> {
        if !self.available {
            return self.fallback.execute(command, args, limits);
        }

        let mut exec_args = vec!["-p", &self.profile, command];
        exec_args.extend_from_slice(args);
        self.fallback
            .execute("/usr/bin/sandbox-exec", &exec_args, limits)
    }

    fn execute_script(&self, script: &str, limits: &ResourceLimits) -> Result<ExecResult> {
        if !self.available {
            return self.fallback.execute_script(script, limits);
        }

        self.fallback.execute(
            "/usr/bin/sandbox-exec",
            &["-p", &self.profile, "sh", "-c", script],
            limits,
        )
    }
}

/// Linux Bubblewrap (`bwrap`) を用いた OS レベルサンドボックス
#[derive(Debug, Clone)]
pub struct BubblewrapSandbox {
    available: bool,
    denied_paths: Vec<String>,
    fallback: DirectSandbox,
}

impl BubblewrapSandbox {
    pub fn new(denied_paths: &[String]) -> Self {
        let available = Self::check_available();
        Self {
            available,
            denied_paths: denied_paths.to_vec(),
            fallback: DirectSandbox,
        }
    }

    pub fn is_available(&self) -> bool {
        self.available
    }

    fn check_available() -> bool {
        #[cfg(target_os = "linux")]
        {
            std::process::Command::new("bwrap")
                .arg("--version")
                .output()
                .map(|out| out.status.success())
                .unwrap_or(false)
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    /// bwrap の隔離実行引数リストを生成する
    pub fn build_bwrap_args(
        &self,
        cwd_opt: Option<&std::path::Path>,
        home_opt: Option<&str>,
        command: &str,
        args: &[&str],
    ) -> Vec<String> {
        let mut bwrap_args = vec![
            "--ro-bind".to_string(),
            "/".to_string(),
            "/".to_string(),
            "--dev".to_string(),
            "/dev".to_string(),
            "--proc".to_string(),
            "/proc".to_string(),
            "--tmpfs".to_string(),
            "/tmp".to_string(),
        ];

        if let Some(cwd) = cwd_opt {
            let cwd_str = cwd.to_string_lossy().to_string();
            bwrap_args.extend(["--bind".to_string(), cwd_str.clone(), cwd_str]);
        }

        let default_denies = ["/etc/shadow", "/etc/master.passwd", "/etc/sudoers"];
        let mut all_denies: Vec<String> = default_denies.iter().map(|s| s.to_string()).collect();
        all_denies.extend(self.denied_paths.clone());

        for p in &all_denies {
            let p_clean = p.trim();
            if p_clean.is_empty() {
                continue;
            }

            // ~ 展開
            let expanded_buf = if let Some(home) = home_opt {
                if p_clean == "~" {
                    std::path::PathBuf::from(home)
                } else if let Some(rest) = p_clean.strip_prefix("~/") {
                    std::path::PathBuf::from(home).join(rest)
                } else {
                    std::path::PathBuf::from(p_clean)
                }
            } else {
                std::path::PathBuf::from(p_clean)
            };

            // 相対パスの場合は cwd と結合して絶対パス化
            let abs_path = if expanded_buf.is_absolute() {
                expanded_buf
            } else if let Some(cwd) = cwd_opt {
                cwd.join(expanded_buf)
            } else {
                expanded_buf
            };

            let path_str = abs_path.to_string_lossy().to_string();

            if abs_path.is_dir() {
                // ディレクトリは空 tmpfs で隠蔽
                bwrap_args.extend(["--tmpfs".to_string(), path_str]);
            } else if abs_path.is_file() {
                // 実在ファイルは /dev/null を読み取り専用バインドして無力化
                bwrap_args.extend(["--ro-bind".to_string(), "/dev/null".to_string(), path_str]);
            } else {
                // 未存在または存在未確定のファイル/パスは --ro-bind-try で安全に無力化
                bwrap_args.extend([
                    "--ro-bind-try".to_string(),
                    "/dev/null".to_string(),
                    path_str,
                ]);
            }
        }

        bwrap_args.push("--".to_string());
        bwrap_args.push(command.to_string());
        for a in args {
            bwrap_args.push(a.to_string());
        }

        bwrap_args
    }
}

impl Default for BubblewrapSandbox {
    fn default() -> Self {
        Self::new(&[])
    }
}

impl Sandbox for BubblewrapSandbox {
    fn execute(&self, command: &str, args: &[&str], limits: &ResourceLimits) -> Result<ExecResult> {
        if !self.available {
            return self.fallback.execute(command, args, limits);
        }

        let cwd = std::env::current_dir().ok();
        let home = std::env::var("HOME").ok();
        let bwrap_args = self.build_bwrap_args(cwd.as_deref(), home.as_deref(), command, args);

        let arg_refs: Vec<&str> = bwrap_args.iter().map(|s| s.as_str()).collect();
        self.fallback.execute("bwrap", &arg_refs, limits)
    }

    fn execute_script(&self, script: &str, limits: &ResourceLimits) -> Result<ExecResult> {
        if !self.available {
            return self.fallback.execute_script(script, limits);
        }

        self.execute("sh", &["-c", script], limits)
    }
}

/// プラットフォームに応じた最適な OS レベルサンドボックスの自動選択
#[derive(Debug, Clone)]
pub enum NativeSandbox {
    Seatbelt(SeatbeltSandbox),
    Bubblewrap(BubblewrapSandbox),
    Direct(DirectSandbox),
}

impl NativeSandbox {
    pub fn new(denied_paths: &[String]) -> Self {
        #[cfg(target_os = "macos")]
        {
            let seatbelt = SeatbeltSandbox::new(denied_paths);
            if seatbelt.is_available() {
                return Self::Seatbelt(seatbelt);
            }
        }

        #[cfg(target_os = "linux")]
        {
            let bwrap = BubblewrapSandbox::new(denied_paths);
            if bwrap.is_available() {
                return Self::Bubblewrap(bwrap);
            }
        }

        let _ = denied_paths;
        Self::Direct(DirectSandbox)
    }

    pub fn is_isolated(&self) -> bool {
        match self {
            Self::Seatbelt(s) => s.is_available(),
            Self::Bubblewrap(b) => b.is_available(),
            Self::Direct(_) => false,
        }
    }
}

impl Default for NativeSandbox {
    fn default() -> Self {
        Self::new(&[])
    }
}

impl Sandbox for NativeSandbox {
    fn execute(&self, command: &str, args: &[&str], limits: &ResourceLimits) -> Result<ExecResult> {
        match self {
            Self::Seatbelt(s) => s.execute(command, args, limits),
            Self::Bubblewrap(b) => b.execute(command, args, limits),
            Self::Direct(d) => d.execute(command, args, limits),
        }
    }

    fn execute_script(&self, script: &str, limits: &ResourceLimits) -> Result<ExecResult> {
        match self {
            Self::Seatbelt(s) => s.execute_script(script, limits),
            Self::Bubblewrap(b) => b.execute_script(script, limits),
            Self::Direct(d) => d.execute_script(script, limits),
        }
    }
}

fn regex_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if matches!(
            c,
            '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// シェルエスケープ（シングルクォートで囲む）
pub(crate) fn shell_escape(s: &str) -> String {
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

    #[test]
    fn test_native_sandbox_fallback_execution() {
        let sandbox = NativeSandbox::new(&[".env".to_string()]);
        let result = sandbox
            .execute("echo", &["native_test"], &ResourceLimits::default())
            .unwrap();
        assert!(result.success());
        assert_eq!(result.stdout.trim(), "native_test");

        let script_res = sandbox
            .execute_script("echo native_script", &ResourceLimits::default())
            .unwrap();
        assert!(script_res.success());
        assert_eq!(script_res.stdout.trim(), "native_script");
    }

    #[test]
    fn test_seatbelt_profile_generation() {
        let profile =
            SeatbeltSandbox::build_profile(&[".env".to_string(), "/secret/token".to_string()]);
        assert!(profile.contains("(subpath \"/etc/shadow\")"));
        assert!(profile.contains("(subpath \"/secret/token\")"));
        assert!(profile.contains("(regex #\"\\.env.*\")"));
    }

    #[test]
    fn test_regex_escape() {
        assert_eq!(regex_escape(".env*"), "\\.env\\*");
        assert_eq!(regex_escape("abc/def"), "abc/def");
    }

    #[test]
    fn test_seatbelt_profile_tilde_expansion() {
        let profile = SeatbeltSandbox::build_profile(&["~/.ssh".to_string()]);
        if let Ok(home) = std::env::var("HOME") {
            assert!(profile.contains(&format!("(subpath \"{}/.ssh\")", home)));
        }
    }

    #[test]
    fn test_bubblewrap_build_args_isolation() {
        let sandbox = BubblewrapSandbox::new(&[".env".to_string(), "~/.ssh".to_string()]);
        let cwd = std::path::Path::new("/workspace");
        let args = sandbox.build_bwrap_args(Some(cwd), Some("/home/user"), "cat", &["foo.txt"]);
        assert!(args.contains(&"--ro-bind".to_string()));
        assert!(args.contains(&"--bind".to_string()));
        assert!(args.contains(&"/workspace".to_string()));
        // /etc/shadow, ~/.ssh, .env が解決されてマスクに含まれること
        assert!(args.iter().any(|a| a.contains("/etc/shadow")));
        assert!(args.iter().any(|a| a.contains("/home/user/.ssh")));
        assert!(args.iter().any(|a| a.contains("/workspace/.env")));
    }
}
