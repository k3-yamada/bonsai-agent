use std::sync::OnceLock;

/// ログレベル（BONSAI_LOG 環境変数で制御）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
}

impl LogLevel {
    fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "error" => Self::Error,
            "warn" => Self::Warn,
            "debug" => Self::Debug,
            _ => Self::Info,
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
        }
    }
}

/// グローバルログレベル（1回だけ初期化）
static LOG_LEVEL: OnceLock<LogLevel> = OnceLock::new();

fn current_level() -> LogLevel {
    *LOG_LEVEL.get_or_init(|| {
        std::env::var("BONSAI_LOG")
            .map(|s| LogLevel::from_str(&s))
            .unwrap_or(LogLevel::Info)
    })
}

/// 構造化ログ出力（カテゴリ付き）
///
/// `BONSAI_LOG` 環境変数でフィルタ可能:
/// - `error`: エラーのみ
/// - `warn`: 警告以上
/// - `info`: 情報以上（デフォルト）
/// - `debug`: 全て
pub fn log_event(level: LogLevel, category: &str, message: &str) {
    if level > current_level() {
        return;
    }
    eprintln!("[{}][{}] {}", level.tag(), category, message);
}

/// 構造化ログマクロ
///
/// ```ignore
/// bonsai_log!(Info, "advisor", "外部API応答取得 ({}文字)", len);
/// bonsai_log!(Debug, "compact", "level {} applied", lv);
/// ```
#[macro_export]
macro_rules! bonsai_log {
    ($level:ident, $cat:expr, $($arg:tt)*) => {
        $crate::observability::logger::log_event(
            $crate::observability::logger::LogLevel::$level,
            $cat,
            &format!($($arg)*),
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_ordering() {
        assert!(LogLevel::Error < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Debug);
    }

    #[test]
    fn test_log_level_from_str() {
        assert_eq!(LogLevel::from_str("error"), LogLevel::Error);
        assert_eq!(LogLevel::from_str("WARN"), LogLevel::Warn);
        assert_eq!(LogLevel::from_str("debug"), LogLevel::Debug);
        assert_eq!(LogLevel::from_str("unknown"), LogLevel::Info);
    }

    #[test]
    fn test_log_level_tag() {
        assert_eq!(LogLevel::Error.tag(), "ERROR");
        assert_eq!(LogLevel::Info.tag(), "INFO");
    }

    #[test]
    fn test_log_event_does_not_panic() {
        // フィルタレベルに関係なくパニックしない
        log_event(LogLevel::Error, "test", "エラーテスト");
        log_event(LogLevel::Debug, "test", "デバッグテスト");
    }

    #[test]
    fn test_bonsai_log_macro() {
        bonsai_log!(Info, "test", "マクロテスト {}", 42);
        bonsai_log!(Debug, "test", "デバッグ {}", "msg");
    }
}
