use super::*;

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
///
/// cancel は `ResourceLimits` 経由で `DirectSandbox` へ委譲される。ここで
/// token を持たないのは意図的（Issue #22 B3-2、決定1参照）。
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

#[cfg(test)]
mod tests {
    use super::*;

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
