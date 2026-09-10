//! Builtin tool 登録の SSOT (Issue #28)。
//!
//! 本番 `setup_tools()`（`src/main.rs`）と `agent::subagent::known_tool_names()`
//! (テスト専用) が参照する一次情報をここに集約する。
//!
//! tool 追加時は必ず [`BUILTIN_TOOL_NAMES`] と [`register_builtin_tools`] の
//! **両方**を更新すること。片方だけの更新は `t_builtin_names_match_registration`
//! が即 FAIL する。
//! `main.rs::setup_tools()` で直接 `registry.register(Box::new(..))` を
//! 呼ばないこと（`tests/structural.rs::t_setup_tools_registers_only_via_builtin_factory`
//! が検出する）。

use crate::tools::ToolRegistry;
use crate::tools::arxiv::ArxivTool;
use crate::tools::file::{FileReadTool, FileWriteTool, MultiEditTool};
use crate::tools::git::GitTool;
use crate::tools::memory::{RecallTool, RememberTool};
use crate::tools::repomap::RepoMapTool;
use crate::tools::shell::ShellTool;
use crate::tools::typed::TypedTool;
use crate::tools::web::{WebFetchTool, WebSearchTool};

/// builtin tool 構築に必要な外部依存。main 層が `AppConfig` / `get_db_path()` から
/// 詰め替えて渡す境界 DTO。tools 層は `AppConfig` 全体を知らない。
#[derive(Clone, Debug)]
pub struct BuiltinToolDeps {
    pub shell_timeout_secs: u64,
    pub cancel: crate::cancel::CancellationToken,
    pub path_guard: crate::safety::path_guard::PathGuard,
    pub db_path: String,
}

/// 本番 `setup_tools()` が登録する builtin tool 名の一次情報 (SSOT, Issue #28)。
pub const BUILTIN_TOOL_NAMES: &[&str] = &[
    <ShellTool as TypedTool>::NAME,
    <FileReadTool as TypedTool>::NAME,
    <FileWriteTool as TypedTool>::NAME,
    <MultiEditTool as TypedTool>::NAME,
    <GitTool as TypedTool>::NAME,
    <WebSearchTool as TypedTool>::NAME,
    <WebFetchTool as TypedTool>::NAME,
    <ArxivTool as TypedTool>::NAME,
    <RepoMapTool as TypedTool>::NAME,
    <RememberTool as TypedTool>::NAME,
    <RecallTool as TypedTool>::NAME,
];

/// builtin tool を registry へ一括登録する唯一の経路。
pub fn register_builtin_tools(registry: &mut ToolRegistry, deps: &BuiltinToolDeps) {
    registry.register(Box::new(
        ShellTool::new()
            .with_timeout(deps.shell_timeout_secs)
            .with_cancel(deps.cancel.clone())
            .with_path_guard(deps.path_guard.clone()),
    ));
    registry.register(Box::new(FileReadTool));
    registry.register(Box::new(FileWriteTool));
    registry.register(Box::new(MultiEditTool));
    registry.register(Box::new(GitTool));
    registry.register(Box::new(WebSearchTool));
    registry.register(Box::new(WebFetchTool));
    registry.register(Box::new(ArxivTool));
    registry.register(Box::new(RepoMapTool));
    // 能動的記憶ツール (①パーソナル知識デーモン Phase 1)
    registry.register(Box::new(RememberTool::new(deps.db_path.clone())));
    registry.register(Box::new(RecallTool::new(deps.db_path.clone())));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::whitelist::READONLY_TOOL_WHITELIST;

    /// env を一切読まないテスト用 deps。db_path はファイルが作られない一時パスのみ渡す。
    fn test_deps() -> BuiltinToolDeps {
        BuiltinToolDeps {
            shell_timeout_secs: 30,
            cancel: crate::cancel::CancellationToken::new(),
            path_guard: crate::safety::path_guard::PathGuard::new(vec![]),
            db_path: std::env::temp_dir()
                .join(format!("bonsai_builtin_test_{}.db", std::process::id()))
                .to_string_lossy()
                .to_string(),
        }
    }

    #[test]
    fn t_builtin_names_match_registration() {
        let mut registry = ToolRegistry::new();
        register_builtin_tools(&mut registry, &test_deps());
        let registered = registry.sorted_names();
        let mut expected: Vec<&str> = BUILTIN_TOOL_NAMES.to_vec();
        expected.sort_unstable();

        let registered_only: Vec<&&str> = registered
            .iter()
            .filter(|n| !expected.contains(n))
            .collect();
        let expected_only: Vec<&&str> = expected
            .iter()
            .filter(|n| !registered.contains(n))
            .collect();
        assert_eq!(
            registered, expected,
            "register_builtin_tools() の登録結果と BUILTIN_TOOL_NAMES が不一致。\
             registry のみ: {registered_only:?}, BUILTIN_TOOL_NAMES のみ: {expected_only:?}"
        );
    }

    #[test]
    fn t_builtin_names_no_duplicates() {
        let mut registry = ToolRegistry::new();
        register_builtin_tools(&mut registry, &test_deps());
        assert_eq!(
            registry.len(),
            BUILTIN_TOOL_NAMES.len(),
            "registry.len() が BUILTIN_TOOL_NAMES.len() と不一致（重複登録の疑い）"
        );

        let mut sorted = BUILTIN_TOOL_NAMES.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            BUILTIN_TOOL_NAMES.len(),
            "BUILTIN_TOOL_NAMES 内に重複名がある"
        );
    }

    #[test]
    fn t_builtin_names_are_not_mcp_style() {
        for name in BUILTIN_TOOL_NAMES {
            assert!(
                !name.contains(':'),
                "builtin tool 名 '{name}' に ':' が含まれている \
                 (select_relevant_split の builtin/MCP 分離前提が壊れる)"
            );
        }
    }

    #[test]
    fn t_readonly_whitelist_is_subset_of_builtin() {
        let missing: Vec<&str> = READONLY_TOOL_WHITELIST
            .iter()
            .copied()
            .filter(|name| !BUILTIN_TOOL_NAMES.contains(name))
            .collect();
        assert!(
            missing.is_empty(),
            "READONLY_TOOL_WHITELIST に BUILTIN_TOOL_NAMES 未収載の名前がある: {missing:?}"
        );
    }
}
