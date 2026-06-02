//! deny-by-default ツール whitelist (Z-NEW-E、okamyuji/go-llm-agent Ch05 適用).
//!
//! production 経路では全 tool が active (`setup_tools`)。Lab smoke / 明示 env 時のみ
//! whitelist を強制し、1bit モデルの誤 `file_write` 等による source 改変事故 (項目 243) を
//! 構造的に予防する。env 未設定で挙動は 100% 不変 (backward compat)。
//!
//! 優先順位: `BONSAI_ENABLED_TOOLS` 明示列挙 > `BONSAI_LAB_SMOKE` readonly default > None (全許可).
//!
//! layer 注: 本 module は tools 層。smoke 判定は agent 層 (`compaction::is_lab_smoke_mode_for_compaction`)
//! と semantic 同一だが、上位 layer への依存 (DEP-001) を避けるためローカルに再実装している。

/// smoke / Lab cycle で default 有効化する readonly tool 群.
///
/// 注: `Permission::Auto` は readonly と等価ではない (`remember` は Auto だが memory write)。
/// よって permission 由来の自動分類ではなく明示列挙で安全側に倒す。
pub const READONLY_TOOL_WHITELIST: &[&str] = &[
    "file_read",
    "repo_map",
    "recall",
    "web_fetch",
    "web_search",
    "arxiv_search",
];

const ENABLED_TOOLS_ENV: &str = "BONSAI_ENABLED_TOOLS";
const LAB_SMOKE_ENV: &str = "BONSAI_LAB_SMOKE";

/// `BONSAI_LAB_SMOKE ∈ {1, true, TRUE, yes, YES}` 判定
/// (`compaction::is_lab_smoke_mode_for_compaction` と同 pattern).
fn is_lab_smoke() -> bool {
    matches!(
        std::env::var(LAB_SMOKE_ENV).as_deref(),
        Ok("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

/// `BONSAI_ENABLED_TOOLS` の comma 区切りを parse. unset / 空白のみ → None.
pub fn parse_enabled_tools_env() -> Option<Vec<String>> {
    let raw = std::env::var(ENABLED_TOOLS_ENV).ok()?;
    let list: Vec<String> = raw
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    if list.is_empty() {
        None
    } else {
        Some(list)
    }
}

/// whitelist 強制が有効か (env 明示列挙 OR smoke mode).
pub fn is_tool_whitelist_enabled() -> bool {
    parse_enabled_tools_env().is_some() || is_lab_smoke()
}

/// 実効 whitelist。env 明示 > smoke readonly default > None (= 全 tool 有効).
///
/// 返り値が `None` の時は呼び側で whitelist を適用しない (全 tool 維持)。
/// `ToolRegistry::apply_whitelist` は空 slice を no-op とするため、
/// `effective_tool_whitelist().as_deref().unwrap_or(&[])` 形式でも backward compat。
pub fn effective_tool_whitelist() -> Option<Vec<String>> {
    if let Some(list) = parse_enabled_tools_env() {
        return Some(list);
    }
    if is_lab_smoke() {
        return Some(
            READONLY_TOOL_WHITELIST
                .iter()
                .map(|s| s.to_string())
                .collect(),
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_parse_enabled_tools_trims_and_filters_blanks() {
        let _g = crate::config::LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var(ENABLED_TOOLS_ENV, " file_read , , recall ,");
        }
        assert_eq!(
            parse_enabled_tools_env(),
            Some(vec!["file_read".to_string(), "recall".to_string()])
        );
        unsafe {
            std::env::remove_var(ENABLED_TOOLS_ENV);
        }
    }

    #[test]
    fn t_parse_blank_env_is_none() {
        let _g = crate::config::LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var(ENABLED_TOOLS_ENV, "  ,  ,");
        }
        assert!(parse_enabled_tools_env().is_none(), "空白のみは None");
        unsafe {
            std::env::remove_var(ENABLED_TOOLS_ENV);
        }
    }

    #[test]
    fn t_effective_none_when_unset() {
        let _g = crate::config::LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var(ENABLED_TOOLS_ENV);
            std::env::remove_var(LAB_SMOKE_ENV);
        }
        assert!(effective_tool_whitelist().is_none());
        assert!(!is_tool_whitelist_enabled());
    }

    #[test]
    fn t_effective_smoke_returns_readonly_default() {
        let _g = crate::config::LAB_RUNTIME_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var(ENABLED_TOOLS_ENV);
            std::env::set_var(LAB_SMOKE_ENV, "1");
        }
        assert_eq!(
            effective_tool_whitelist().as_deref(),
            Some(
                READONLY_TOOL_WHITELIST
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
                    .as_slice()
            )
        );
        assert!(is_tool_whitelist_enabled());
        unsafe {
            std::env::remove_var(LAB_SMOKE_ENV);
        }
    }
}
