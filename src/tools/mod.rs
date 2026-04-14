pub mod arxiv;
pub mod file;
pub mod git;
pub mod hooks;
pub mod mcp_client;
pub mod permission;
pub mod plugin;
pub mod repomap;
pub mod sandbox;
pub mod shell;
pub mod web;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::tools::permission::Permission;

/// ツールのスキーマ情報（LLMのシステムプロンプトに注入する）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// ツールの実行結果
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub output: String,
    pub success: bool,
}

/// 全ツールが実装するトレイト
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;
    fn permission(&self) -> Permission;

    /// ツールを実行する（同期）
    fn call(&self, args: serde_json::Value) -> Result<ToolResult>;

    /// スキーマ情報を生成
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters_schema(),
        }
    }
}

/// ツールレジストリ — 登録・検索・動的選択を管理
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// ツールを登録
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// 名前でツールを取得
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// 登録済みツール名の一覧
    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }

    /// 登録済みツール数
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// クエリに関連するツールを動的に選択（上位max件）。
    /// キーワードマッチングでスコアリングし、スコアの高い順に返す。
    pub fn select_relevant(&self, query: &str, max: usize) -> Vec<&dyn Tool> {
        let query_lower = query.to_lowercase();
        let query_words: Vec<&str> = query_lower.split_whitespace().collect();
        let task_boost = Self::detect_task_boost(&query_lower);

        let mut scored: Vec<(&dyn Tool, usize)> = self
            .tools
            .values()
            .map(|tool| {
                let name = tool.name().to_lowercase();
                let desc = tool.description().to_lowercase();
                let mut score = query_words
                    .iter()
                    .filter(|w| name.contains(*w) || desc.contains(*w))
                    .count();
                if task_boost.iter().any(|b| name.contains(b)) { score += 2; }
                (tool.as_ref(), score)
            })
            .collect();

        // スコア降順でソート（同スコアはツール名のアルファベット順）
        scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name().cmp(b.0.name())));

        scored.into_iter().take(max).map(|(t, _)| t).collect()
    }

    /// タスク種別からブーストするツール名プレフィックスを検出
    fn detect_task_boost(query: &str) -> Vec<&'static str> {
        let mut b = Vec::new();
        if query.contains("ファイル") || query.contains("読") || query.contains("書") { b.push("file"); }
        if query.contains("コマンド") || query.contains("実行") || query.contains("ビルド") { b.push("shell"); }
        if query.contains("git") || query.contains("コミット") { b.push("git"); }
        if query.contains("検索") || query.contains("探") { b.push("web"); b.push("file"); }
        b
    }

    /// 選択されたツールのスキーマをシステムプロンプト用にフォーマット
    pub fn format_schemas(&self, tools: &[&dyn Tool]) -> String {
        if tools.is_empty() {
            return String::new();
        }

        let mut output = String::from("# 使用可能なツール\n\n");
        for tool in tools {
            output.push_str(&format!("## {}\n", tool.name()));
            output.push_str(&format!("{}\n", tool.description()));
            output.push_str(&format!(
                "パラメータ: {}\n\n",
                serde_json::to_string_pretty(&tool.parameters_schema())
                    .unwrap_or_else(|_| "{}".to_string())
            ));
        }
        output
    }


    /// 名前+説明のみのコンパクト形式（パラメータスキーマ省略でトークン節約）
    pub fn format_schemas_compact(&self, tools: &[&dyn Tool]) -> String {
        if tools.is_empty() {
            return String::new();
        }
        let mut output = String::from("# 使用可能なツール\n\n");
        for tool in tools {
            output.push_str(&format!("- **{}**: {}\n", tool.name(), tool.description()));
        }
        output
    }
    /// 登録済みツール名のHashSetを返す（バリデーション用）
    pub fn known_names(&self) -> std::collections::HashSet<String> {
        self.tools.keys().cloned().collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用のダミーツール
    struct DummyTool {
        name: String,
        description: String,
    }

    impl DummyTool {
        fn new(name: &str, desc: &str) -> Self {
            Self {
                name: name.to_string(),
                description: desc.to_string(),
            }
        }
    }

    impl Tool for DummyTool {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            &self.description
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn permission(&self) -> Permission {
            Permission::Auto
        }
        fn call(&self, _args: serde_json::Value) -> Result<ToolResult> {
            Ok(ToolResult {
                output: "ok".to_string(),
                success: true,
            })
        }
    }

    fn build_registry() -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(DummyTool::new(
            "shell",
            "シェルコマンドを実行する",
        )));
        reg.register(Box::new(DummyTool::new("file_read", "ファイルを読み込む")));
        reg.register(Box::new(DummyTool::new("file_write", "ファイルに書き込む")));
        reg.register(Box::new(DummyTool::new(
            "memory_search",
            "メモリを検索する",
        )));
        reg.register(Box::new(DummyTool::new("git", "Gitリポジトリを操作する")));
        reg.register(Box::new(DummyTool::new("web_search", "Webを検索する")));
        reg
    }

    #[test]
    fn test_register_and_get() {
        let reg = build_registry();
        assert_eq!(reg.len(), 6);
        assert!(reg.get("shell").is_some());
        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn test_names() {
        let reg = build_registry();
        let names = reg.names();
        assert_eq!(names.len(), 6);
        assert!(names.contains(&"shell"));
    }

    #[test]
    fn test_select_relevant_keyword_match() {
        let reg = build_registry();
        // 「ファイル」で検索 → file_read, file_write がマッチ
        let selected = reg.select_relevant("ファイルを読みたい", 5);
        assert!(!selected.is_empty());
        let names: Vec<&str> = selected.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"file_read"));
    }

    #[test]
    fn test_select_relevant_max_limit() {
        let reg = build_registry();
        let selected = reg.select_relevant("操作 実行 検索", 2);
        assert!(selected.len() <= 2);
    }

    #[test]
    fn test_select_relevant_no_match() {
        let reg = build_registry();
        // マッチしないクエリでも全ツール（スコア0）が返る
        let selected = reg.select_relevant("xyz123", 3);
        assert_eq!(selected.len(), 3);
    }

    #[test]
    fn test_format_schemas() {
        let reg = build_registry();
        let tools: Vec<&dyn Tool> = vec![reg.get("shell").unwrap()];
        let formatted = reg.format_schemas(&tools);
        assert!(formatted.contains("# 使用可能なツール"));
        assert!(formatted.contains("## shell"));
        assert!(formatted.contains("シェルコマンドを実行する"));
    }

    #[test]
    fn test_format_schemas_empty() {
        let reg = ToolRegistry::new();
        let formatted = reg.format_schemas(&[]);
        assert!(formatted.is_empty());
    }

    #[test]
    fn test_known_names() {
        let reg = build_registry();
        let known = reg.known_names();
        assert!(known.contains("shell"));
        assert!(!known.contains("unknown"));
    }

    #[test]
    fn test_tool_call() {
        let reg = build_registry();
        let tool = reg.get("shell").unwrap();
        let result = tool.call(serde_json::json!({})).unwrap();
        assert!(result.success);
    }

    #[test]
    fn test_tool_schema() {
        let reg = build_registry();
        let tool = reg.get("shell").unwrap();
        let schema = tool.schema();
        assert_eq!(schema.name, "shell");
        assert!(!schema.description.is_empty());
    }

    #[test]
    fn test_format_schemas_compact() {
        let reg = build_registry();
        let all: Vec<&dyn Tool> = reg.tools.values().map(|t| t.as_ref()).collect();
        let compact = reg.format_schemas_compact(&all);
        let full = reg.format_schemas(&all);
        // compact版はフル版より短い
        assert!(compact.len() < full.len());
        // 名前と説明は含まれる
        assert!(compact.contains("file_read"));
        assert!(compact.contains("shell"));
    }

    #[test]
    fn test_format_schemas_compact_empty() {
        let reg = build_registry();
        let compact = reg.format_schemas_compact(&[]);
        assert!(compact.is_empty());
    }

    #[test]
    fn test_format_schemas_compact_has_description() {
        let reg = build_registry();
        let tool = reg.get("file_read").unwrap();
        let compact = reg.format_schemas_compact(&[tool]);
        assert!(compact.contains("ファイル"));
    }

    #[test]
    fn test_format_schemas_compact_no_params() {
        let reg = build_registry();
        let tool = reg.get("shell").unwrap();
        let compact = reg.format_schemas_compact(&[tool]);
        // JSONスキーマが含まれない
        assert!(!compact.contains("properties"));
    }

    #[test]
    fn test_task_boost_file_query() {
        let reg = build_registry();
        let selected = reg.select_relevant("ファイルを読みたい", 3);
        let names: Vec<&str> = selected.iter().map(|t| t.name()).collect();
        // file系ツールがブーストされて上位に来る
        assert_eq!(names[0], "file_read");
    }

    #[test]
    fn test_task_boost_git_query() {
        let reg = build_registry();
        let selected = reg.select_relevant("gitのコミット履歴を見たい", 3);
        let names: Vec<&str> = selected.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"git"));
    }

    #[test]
    fn test_task_boost_no_boost() {
        let reg = build_registry();
        // ブーストキーワードなしのクエリ
        let selected = reg.select_relevant("天気を教えて", 3);
        assert_eq!(selected.len(), 3);
    }
}