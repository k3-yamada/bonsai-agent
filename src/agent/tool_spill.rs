//! ツール出力スピルオーバー一時ファイルのライフサイクル管理 (Issue #22 B3-1)。
//!
//! 由来: `tool_exec::truncate_tool_output` は `max_chars` を超えるツール出力の全文を
//! `/tmp/bonsai-overflow-{hash}.txt` に fire-and-forget で書き出していたが、削除処理が
//! 一切なく、長時間 Lab run では無制限に累積していた (AC3、実害あり)。
//!
//! ## 設計方針
//! - 保存先をプロセス単位のサブディレクトリ (`{TMPDIR}/bonsai-agent/spill-{pid}/`) に集約
//! - 件数/バイト数の上限超過時は最古 (mtime 昇順) から削除
//! - プロセス正常終了時 (`SpillGuard::drop`) にディレクトリごと削除
//! - 起動時に自 pid 以外の生存していない spill ディレクトリを掃除 (異常終了分の回収)
//! - ファイルは `0600`、ディレクトリは `0700` (unix)
//! - `BONSAI_TOOL_SPILL=0` で完全無効化でき、無効時は一切ファイルを作らない
//!
//! パス生成・上限判定・pid 解析はすべて純関数として切り出し単体テスト可能にする
//! (`src/agent/working_memory.rs` の env 読み取りパターンを踏襲)。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::observability::logger::{LogLevel, log_event};

pub(crate) const SPILL_ROOT_DIR: &str = "bonsai-agent";
pub(crate) const SPILL_DIR_PREFIX: &str = "spill-";
pub(crate) const DEFAULT_MAX_FILES: usize = 64;
pub(crate) const DEFAULT_MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

const MIN_MAX_FILES: usize = 1;
const MAX_MAX_FILES: usize = 4096;
const MIN_MAX_TOTAL_BYTES: u64 = 1024 * 1024;
const MAX_MAX_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// `BONSAI_TOOL_SPILL` env でスピルオーバー機能の有効/無効を判定する。
///
/// 本機能の production default は **有効** (working_memory の cap 系と逆)。
/// `0`/`false`/`no` (case-insensitive) を明示指定した場合のみ無効化する。
pub(crate) fn is_enabled() -> bool {
    std::env::var("BONSAI_TOOL_SPILL")
        .map(|v| !(v == "0" || v.eq_ignore_ascii_case("false") || v.eq_ignore_ascii_case("no")))
        .unwrap_or(true)
}

/// `BONSAI_TOOL_SPILL_MAX_FILES` env から件数上限を取得する (default 64、範囲 1..=4096)。
///
/// 入力検証: 非数値 / 範囲外は default に巻き戻す。
pub(crate) fn max_files_from_env() -> usize {
    std::env::var("BONSAI_TOOL_SPILL_MAX_FILES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| (MIN_MAX_FILES..=MAX_MAX_FILES).contains(&n))
        .unwrap_or(DEFAULT_MAX_FILES)
}

/// `BONSAI_TOOL_SPILL_MAX_BYTES` env から合計バイト数上限を取得する (default 64MiB、範囲 1MiB..=4GiB)。
///
/// 入力検証: 非数値 / 範囲外は default に巻き戻す。
pub(crate) fn max_total_bytes_from_env() -> u64 {
    std::env::var("BONSAI_TOOL_SPILL_MAX_BYTES")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&n| (MIN_MAX_TOTAL_BYTES..=MAX_MAX_TOTAL_BYTES).contains(&n))
        .unwrap_or(DEFAULT_MAX_TOTAL_BYTES)
}

/// spill ルートディレクトリ (`<tmp>/bonsai-agent`) を返す純関数。
pub(crate) fn spill_root(tmp_dir: &Path) -> PathBuf {
    tmp_dir.join(SPILL_ROOT_DIR)
}

/// pid 単位の spill ディレクトリ (`<tmp>/bonsai-agent/spill-<pid>`) を返す純関数。
pub(crate) fn spill_dir(tmp_dir: &Path, pid: u32) -> PathBuf {
    spill_root(tmp_dir).join(format!("{SPILL_DIR_PREFIX}{pid}"))
}

/// spill ファイル名 (`overflow-<hash:x>.txt`) を返す純関数。
pub(crate) fn spill_file_name(hash: u64) -> String {
    format!("overflow-{hash:x}.txt")
}

/// spill ファイルのフルパスを返す純関数。
pub(crate) fn spill_file_path(dir: &Path, hash: u64) -> PathBuf {
    dir.join(spill_file_name(hash))
}

/// spill ディレクトリ名 ("spill-1234") から pid を解析する純関数。
/// prefix が一致しない、数値変換に失敗した場合、または `pid` が通常のプロセスpidとして
/// あり得ない値 (`0`、または `i32::MAX` 超過) の場合は `None`。
///
/// 上限ガード (Issue #22 qa-reviewer LOW-3): `u32::MAX` を `libc::pid_t` (`i32`) に
/// キャストすると `-1` になり `libc::kill(-1, 0)` は「全プロセスへのブロードキャスト」
/// 形式になるため常に生存扱いになってしまう (`spill-4294967295` が永久に回収されない)。
/// `u32::MAX - 1` も同様に `-2` となり「プロセスグループ2への問い合わせ」になる環境依存の
/// 問題がある。通常のOS pidは `i32::MAX` を超えないため、ここで弾いて `is_pid_alive` に
/// 到達させない。
pub(crate) fn parse_spill_dir_pid(dir_name: &str) -> Option<u32> {
    let pid = dir_name
        .strip_prefix(SPILL_DIR_PREFIX)?
        .parse::<u32>()
        .ok()?;
    if pid == 0 || pid > i32::MAX as u32 {
        return None;
    }
    Some(pid)
}

/// eviction 判定対象のファイルエントリ。
#[derive(Debug, Clone)]
pub(crate) struct SpillEntry {
    pub path: PathBuf,
    pub modified: std::time::SystemTime,
    pub size: u64,
}

/// 上限超過時に削除すべきファイルを mtime 昇順 (最古優先) で決定する純関数。
///
/// `keep` に指定したパス (直近書き込み分) は決して削除候補に選ばない。
pub(crate) fn select_evictions(
    entries: &[SpillEntry],
    max_files: usize,
    max_total_bytes: u64,
    keep: Option<&Path>,
) -> Vec<PathBuf> {
    let mut candidates: Vec<&SpillEntry> = entries
        .iter()
        .filter(|e| keep != Some(e.path.as_path()))
        .collect();
    // 新しい順 (mtime 降順) に処理し、上限に収まる分だけ保持対象とする。
    // 収まらなくなった時点以降 (=より古いもの) は削除対象になる。
    candidates.sort_by(|a, b| b.modified.cmp(&a.modified));

    let mut count: usize = usize::from(keep.is_some());
    let mut total_bytes: u64 = entries
        .iter()
        .filter(|e| keep == Some(e.path.as_path()))
        .map(|e| e.size)
        .sum();

    let mut evict = Vec::new();
    for e in candidates {
        if count < max_files && total_bytes.saturating_add(e.size) <= max_total_bytes {
            count += 1;
            total_bytes += e.size;
        } else {
            evict.push(e.path.clone());
        }
    }
    evict
}

/// スピルオーバーファイルを管理する I/O 型。プロセスにつき1つ (`process_default`)。
#[derive(Debug, Clone)]
pub(crate) struct SpillStore {
    dir: PathBuf,
    enabled: bool,
    max_files: usize,
    max_total_bytes: u64,
}

impl SpillStore {
    /// テスト用: 任意ディレクトリを指定して有効な `SpillStore` を構築する。
    /// production 呼出元は `process_default()` のみのため、テスト以外では不要。
    #[cfg(test)]
    pub(crate) fn new_at(dir: PathBuf, max_files: usize, max_total_bytes: u64) -> Self {
        Self {
            dir,
            enabled: true,
            max_files,
            max_total_bytes,
        }
    }

    /// プロセス単位で一度だけ解決される既定 `SpillStore`。env 設定を反映する。
    pub(crate) fn process_default() -> &'static SpillStore {
        static INSTANCE: OnceLock<SpillStore> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            let enabled = is_enabled();
            let dir = spill_dir(&std::env::temp_dir(), std::process::id());
            Self {
                dir,
                enabled,
                max_files: max_files_from_env(),
                max_total_bytes: max_total_bytes_from_env(),
            }
        })
    }

    #[cfg(test)]
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// 全文を書き出し、案内文に載せるパスを返す。無効時・I/O 失敗時は `None`。
    ///
    /// ディレクトリは `DirBuilder::mode(0o700).recursive(true)`、ファイルは
    /// `OpenOptions::new().write(true).create(true).truncate(true).mode(0o600)` で作成する
    /// (作成後の `set_permissions` だと 0644 で存在する競合窓が生じるため不可)。
    pub(crate) fn write(&self, hash: u64, content: &str) -> Option<PathBuf> {
        if !self.enabled {
            return None;
        }
        if let Err(e) = self.ensure_dir() {
            log_event(
                LogLevel::Warn,
                "tool_spill",
                &format!("spill ディレクトリ作成失敗: {e}"),
            );
            return None;
        }
        let path = spill_file_path(&self.dir, hash);
        if let Err(e) = self.write_file(&path, content) {
            log_event(
                LogLevel::Warn,
                "tool_spill",
                &format!("spill ファイル書き込み失敗: {e}"),
            );
            return None;
        }
        self.enforce_limits(Some(&path));
        Some(path)
    }

    #[cfg(unix)]
    fn ensure_dir(&self) -> std::io::Result<()> {
        use std::fs::DirBuilder;
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(&self.dir)
    }

    #[cfg(not(unix))]
    fn ensure_dir(&self) -> std::io::Result<()> {
        std::fs::DirBuilder::new().recursive(true).create(&self.dir)
    }

    #[cfg(unix)]
    fn write_file(&self, path: &Path, content: &str) -> std::io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(content.as_bytes())
    }

    #[cfg(not(unix))]
    fn write_file(&self, path: &Path, content: &str) -> std::io::Result<()> {
        std::fs::write(path, content)
    }

    /// ディレクトリ内のファイル一覧を件数/バイト数上限に基づき整理し、
    /// 超過分を削除する。削除した件数を返す。
    pub(crate) fn enforce_limits(&self, keep: Option<&Path>) -> usize {
        let Ok(read_dir) = std::fs::read_dir(&self.dir) else {
            return 0;
        };
        let entries: Vec<SpillEntry> = read_dir
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let metadata = e.metadata().ok()?;
                if !metadata.is_file() {
                    return None;
                }
                let modified = metadata.modified().ok()?;
                Some(SpillEntry {
                    path: e.path(),
                    modified,
                    size: metadata.len(),
                })
            })
            .collect();
        let to_remove = select_evictions(&entries, self.max_files, self.max_total_bytes, keep);
        let removed = to_remove.len();
        for path in to_remove {
            let _ = std::fs::remove_file(path);
        }
        removed
    }

    /// プロセス終了時、自身の spill ディレクトリを丸ごと削除する。
    /// production では `SpillGuard::drop` が同等の処理を直接行うため、テスト専用。
    #[cfg(test)]
    pub(crate) fn cleanup_self(&self) -> std::io::Result<()> {
        if self.dir.exists() {
            std::fs::remove_dir_all(&self.dir)
        } else {
            Ok(())
        }
    }
}

/// root 直下の `spill-<pid>` のうち、自 pid 以外かつ生存していない pid のものを削除する。
/// 削除した件数を返す。
pub fn cleanup_stale_dirs(root: &Path, self_pid: u32) -> usize {
    let Ok(read_dir) = std::fs::read_dir(root) else {
        return 0;
    };
    let mut removed = 0;
    for entry in read_dir.filter_map(|e| e.ok()) {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Some(pid) = parse_spill_dir_pid(&name) else {
            continue;
        };
        if pid == self_pid || is_pid_alive(pid) {
            continue;
        }
        if std::fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// pid が生存しているかを判定する。unix では `libc::kill(pid, 0)` を用い、
/// `EPERM` (権限なし=他ユーザ所有だが生存) も生存扱いとする。非 unix では常に false。
#[cfg(unix)]
fn is_pid_alive(pid: u32) -> bool {
    // signal 0 はプロセスへシグナルを送らず存在確認のみ行う。
    // 戻り値0=生存、-1かつerrno=ESRCH=非生存、-1かつerrno=EPERM=生存 (権限なし)。
    let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if ret == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn is_pid_alive(_pid: u32) -> bool {
    false
}

/// プロセス正常終了時に spill ディレクトリを削除する RAII ガード。
pub struct SpillGuard {
    dir: PathBuf,
}

impl Drop for SpillGuard {
    fn drop(&mut self) {
        // ベストエフォート: 削除失敗はプロセス終了処理を妨げない。
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// main から呼ぶ唯一の入口。
///
/// 1. 過去プロセスの spill 残骸 (異常終了分) を掃除する。
/// 2. 機能が有効な場合のみ `SpillGuard` を返す (無効時は `None`、後始末不要)。
pub fn install_process_spill() -> Option<SpillGuard> {
    let tmp_dir = std::env::temp_dir();
    let root = spill_root(&tmp_dir);
    let self_pid = std::process::id();
    let removed = cleanup_stale_dirs(&root, self_pid);
    if removed > 0 {
        log_event(
            LogLevel::Debug,
            "tool_spill",
            &format!("起動時クリーンアップ: 残骸 spill ディレクトリ{removed}件を削除"),
        );
    }
    if !is_enabled() {
        return None;
    }
    Some(SpillGuard {
        dir: spill_dir(&tmp_dir, self_pid),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    // env mutation race を避けるため module-local Mutex で serialize する
    // (working_memory.rs の WORKING_CAP_TEST_LOCK と同パターン)。
    static SPILL_ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn reset_spill_env() {
        unsafe {
            std::env::remove_var("BONSAI_TOOL_SPILL");
            std::env::remove_var("BONSAI_TOOL_SPILL_MAX_FILES");
            std::env::remove_var("BONSAI_TOOL_SPILL_MAX_BYTES");
        }
    }

    #[test]
    fn t_is_enabled_default_true() {
        let _g = SPILL_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_spill_env();
        assert!(is_enabled(), "env unset で default 有効");
        for value in ["0", "false", "FALSE", "no", "NO"] {
            unsafe {
                std::env::set_var("BONSAI_TOOL_SPILL", value);
            }
            assert!(!is_enabled(), "env={value} で無効");
        }
        unsafe {
            std::env::set_var("BONSAI_TOOL_SPILL", "1");
        }
        assert!(is_enabled(), "env=1 で有効");
        reset_spill_env();
    }

    #[test]
    fn t_max_files_from_env_bounds() {
        let _g = SPILL_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_spill_env();
        assert_eq!(
            max_files_from_env(),
            DEFAULT_MAX_FILES,
            "env unset で default"
        );
        unsafe {
            std::env::set_var("BONSAI_TOOL_SPILL_MAX_FILES", "10");
        }
        assert_eq!(max_files_from_env(), 10);
        unsafe {
            std::env::set_var("BONSAI_TOOL_SPILL_MAX_FILES", "0");
        }
        assert_eq!(
            max_files_from_env(),
            DEFAULT_MAX_FILES,
            "範囲外(0)は default に巻き戻し"
        );
        unsafe {
            std::env::set_var("BONSAI_TOOL_SPILL_MAX_FILES", "abc");
        }
        assert_eq!(max_files_from_env(), DEFAULT_MAX_FILES, "非数値は default");
        reset_spill_env();
    }

    #[test]
    fn t_max_total_bytes_from_env_bounds() {
        let _g = SPILL_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_spill_env();
        assert_eq!(
            max_total_bytes_from_env(),
            DEFAULT_MAX_TOTAL_BYTES,
            "env unset で default"
        );
        unsafe {
            std::env::set_var("BONSAI_TOOL_SPILL_MAX_BYTES", "2097152");
        }
        assert_eq!(max_total_bytes_from_env(), 2_097_152);
        unsafe {
            std::env::set_var("BONSAI_TOOL_SPILL_MAX_BYTES", "1");
        }
        assert_eq!(
            max_total_bytes_from_env(),
            DEFAULT_MAX_TOTAL_BYTES,
            "範囲外(1)は default に巻き戻し"
        );
        reset_spill_env();
    }

    #[test]
    fn t_spill_path_layout_is_pid_scoped() {
        let tmp = PathBuf::from("/tmp/example");
        assert_eq!(spill_root(&tmp), PathBuf::from("/tmp/example/bonsai-agent"));
        assert_eq!(
            spill_dir(&tmp, 4242),
            PathBuf::from("/tmp/example/bonsai-agent/spill-4242")
        );
        assert_eq!(spill_file_name(0xdead), "overflow-dead.txt");
        assert_eq!(
            spill_file_path(&spill_dir(&tmp, 4242), 0xdead),
            PathBuf::from("/tmp/example/bonsai-agent/spill-4242/overflow-dead.txt")
        );
    }

    #[test]
    fn t_parse_spill_dir_pid() {
        assert_eq!(parse_spill_dir_pid("spill-42"), Some(42));
        // Issue #22 qa-reviewer LOW-3: pid=0 は通常のプロセスpidとしてあり得ないため拒否。
        assert_eq!(parse_spill_dir_pid("spill-0"), None);
        assert_eq!(parse_spill_dir_pid("notspill-42"), None);
        assert_eq!(parse_spill_dir_pid("spill-abc"), None);
        assert_eq!(parse_spill_dir_pid("spill-"), None);
    }

    /// Issue #22 qa-reviewer LOW-3: `i32::MAX` を超える pid は
    /// `libc::pid_t` (`i32`) キャストで符号反転し `kill()` の意味が変わる
    /// (`-1`=全プロセス, `-2`=プロセスグループ) ため、パース段階で拒否する。
    #[test]
    fn t_parse_spill_dir_pid_rejects_out_of_range() {
        assert_eq!(
            parse_spill_dir_pid(&format!("spill-{}", i32::MAX as u32)),
            Some(i32::MAX as u32),
            "i32::MAXはOS上あり得るpidとして許容"
        );
        assert_eq!(
            parse_spill_dir_pid(&format!("spill-{}", i32::MAX as u32 + 1)),
            None,
            "i32::MAX超過は拒否 (符号反転でkillの意味が変わるため)"
        );
        assert_eq!(
            parse_spill_dir_pid(&format!("spill-{}", u32::MAX)),
            None,
            "u32::MAX (pid_t=-1、全プロセスブロードキャスト) は拒否"
        );
        assert_eq!(
            parse_spill_dir_pid(&format!("spill-{}", u32::MAX - 1)),
            None,
            "u32::MAX-1 (pid_t=-2、プロセスグループ) は拒否"
        );
    }

    #[cfg(unix)]
    #[test]
    fn t_spill_file_permission_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
            "bonsai_tool_spill_test_perm_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = SpillStore::new_at(dir.clone(), DEFAULT_MAX_FILES, DEFAULT_MAX_TOTAL_BYTES);
        let path = store.write(1, "hello").expect("write は成功するはず");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "spill ファイルは0600であるべき");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn entry(path: &str, secs_ago: u64, size: u64) -> SpillEntry {
        SpillEntry {
            path: PathBuf::from(path),
            modified: SystemTime::now() - Duration::from_secs(secs_ago),
            size,
        }
    }

    #[test]
    fn t_select_evictions_by_count() {
        let entries = vec![
            entry("/tmp/a", 30, 10),
            entry("/tmp/b", 20, 10),
            entry("/tmp/c", 10, 10),
        ];
        let evict = select_evictions(&entries, 2, u64::MAX, None);
        assert_eq!(evict, vec![PathBuf::from("/tmp/a")], "最古1件のみ削除対象");

        // keep 指定パスは選ばれない (古くても保護)。
        let evict_keep = select_evictions(&entries, 1, u64::MAX, Some(Path::new("/tmp/a")));
        assert!(!evict_keep.contains(&PathBuf::from("/tmp/a")));
    }

    #[test]
    fn t_select_evictions_by_bytes() {
        let entries = vec![
            entry("/tmp/a", 30, 40),
            entry("/tmp/b", 20, 40),
            entry("/tmp/c", 10, 40),
        ];
        // 上限100バイトのため、新しい2件(80バイト)は残り、最古1件が削除対象。
        let evict = select_evictions(&entries, usize::MAX, 100, None);
        assert_eq!(evict, vec![PathBuf::from("/tmp/a")]);
    }

    #[test]
    fn t_spill_store_enforce_limits_deletes_oldest() {
        let dir = std::env::temp_dir().join(format!(
            "bonsai_tool_spill_test_enforce_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = SpillStore::new_at(dir.clone(), 2, DEFAULT_MAX_TOTAL_BYTES);
        store.write(1, "one").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        store.write(2, "two").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        store.write(3, "three").unwrap();

        let remaining: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(remaining.len(), 2, "max_files=2 で最古1件が削除される");
        assert!(
            !remaining.contains(&spill_file_name(1)),
            "最古のfile1は削除される"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn t_spill_disabled_writes_nothing() {
        let dir = std::env::temp_dir().join(format!(
            "bonsai_tool_spill_test_disabled_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = SpillStore {
            dir: dir.clone(),
            enabled: false,
            max_files: DEFAULT_MAX_FILES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
        };
        let result = store.write(1, "should not be written");
        assert!(result.is_none(), "無効時はNoneを返す");
        assert!(!dir.exists(), "無効時はディレクトリすら作らない");
    }

    #[test]
    fn t_spill_store_dir_and_cleanup_self() {
        let dir = std::env::temp_dir().join(format!(
            "bonsai_tool_spill_test_cleanup_self_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = SpillStore::new_at(dir.clone(), DEFAULT_MAX_FILES, DEFAULT_MAX_TOTAL_BYTES);
        assert_eq!(store.dir(), dir.as_path());

        store.write(1, "content").expect("write は成功するはず");
        assert!(dir.exists());
        store.cleanup_self().expect("cleanup_self は成功するはず");
        assert!(!dir.exists(), "cleanup_self でディレクトリが削除される");
        // 存在しない状態での再呼び出しはエラーにならない (Ok(()))。
        store.cleanup_self().expect("再呼び出しもOk");
    }

    /// テスト用に「存在しない」かつ `i32::MAX` 以下のpidを1件見つける。
    ///
    /// Issue #22 qa-reviewer LOW-3: 以前は `u32::MAX - 1` (pid_t上は `-2`) を使っていたが、
    /// `libc::kill(-2, 0)` は「プロセスグループ2への問い合わせ」という別の意味になり、
    /// そのプロセスグループが存在するホストでは誤って生存扱いになる環境依存のflakyさが
    /// あった。ここでは通常のpidレンジ内の候補を `is_pid_alive` で実際に確認してから使う。
    fn find_dead_pid_for_test() -> u32 {
        // 通常のOSではpid_maxが百万台に達することは稀であり、以下の候補群が
        // すべて生存していることは実運用上ほぼ考えられないが、念のため複数候補を
        // 順に確認し、最初に「非生存」と判定できたものを返す。
        const CANDIDATES: [u32; 5] = [999_999, 1_234_567, 2_345_678, 3_456_789, 1_999_999];
        for &candidate in &CANDIDATES {
            if !is_pid_alive(candidate) {
                return candidate;
            }
        }
        // 全候補が(あり得ないが)生存扱いだった場合のfallback。
        CANDIDATES[0]
    }

    #[test]
    fn t_cleanup_stale_dirs_skips_self_and_live() {
        let root = std::env::temp_dir().join(format!(
            "bonsai_tool_spill_test_stale_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let self_pid = std::process::id();
        // 自pid: 削除されない (skip条件: pid == self_pid)
        std::fs::create_dir_all(root.join(format!("spill-{self_pid}"))).unwrap();
        // 存在しないpid: 削除される (skip条件: is_pid_alive(pid) == false)。
        // i32::MAX以下の通常レンジから実際に非生存であることを確認して選ぶ
        // (u32::MAX系の値はpid_tキャストで符号反転し意味が変わるため使わない)。
        let dead_pid = find_dead_pid_for_test();
        std::fs::create_dir_all(root.join(format!("spill-{dead_pid}"))).unwrap();
        // prefixが違うので対象外
        std::fs::create_dir_all(root.join("not-a-spill-dir")).unwrap();

        let removed = cleanup_stale_dirs(&root, self_pid);
        assert_eq!(removed, 1, "存在しないpidのディレクトリのみ削除される");
        assert!(
            root.join(format!("spill-{self_pid}")).exists(),
            "自pidは残る"
        );
        assert!(
            !root.join(format!("spill-{dead_pid}")).exists(),
            "非生存pidは削除される"
        );
        assert!(
            root.join("not-a-spill-dir").exists(),
            "prefix不一致は対象外"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn t_spill_guard_removes_dir_on_drop() {
        let dir = std::env::temp_dir().join(format!(
            "bonsai_tool_spill_test_guard_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(dir.exists());
        {
            let _guard = SpillGuard { dir: dir.clone() };
        }
        assert!(!dir.exists(), "Drop でディレクトリが削除される");
    }
}
