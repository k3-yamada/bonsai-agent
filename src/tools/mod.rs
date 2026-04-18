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

    /// 読取専用ツールか（並列実行の対象判定用）
    fn is_read_only(&self) -> bool {
        false
    }

    /// スキーマ情報を生成
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters_schema(),
        }
    }
}

/// タスク種別 — ツール選択のフィルタリングに使用
/// CrewAI知見: 選択肢が少ないほど1ビットモデルは正確に動作する
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskType {
    /// ファイル操作: file_read, file_write, repo_map
    FileOperation,
    /// コード実行: shell, git
    CodeExecution,
    /// リサーチ: web_search, web_fetch, arxiv_search
    Research,
    /// 全ツール使用可能（フィルタなし）
    General,
}

/// クエリ文字列からタスク種別を推定する
/// 日本語キーワードマッチングで判定（1ビットモデル向けにツール選択肢を絞る）
pub fn detect_task_type(query: &str) -> TaskType {
    let q = query.to_lowercase();

    // ファイル操作キーワード
    if q.contains("ファイル") || q.contains("読") || q.contains("書") || q.contains("編集") {
        return TaskType::FileOperation;
    }

    // コード実行キーワード
    if q.contains("実行") || q.contains("ビルド") || q.contains("テスト") || q.contains("コマンド") {
        return TaskType::CodeExecution;
    }

    // リサーチキーワード
    if q.contains("検索") || q.contains("調べ") || q.contains("論文") || q.contains("url") {
        return TaskType::Research;
    }

    TaskType::General
}

impl TaskType {
    /// このタスク種別で許可されるツール名プレフィックスを返す
    /// GeneralはNone（フィルタなし）
    fn allowed_prefixes(&self) -> Option<&[&str]> {
        match self {
            TaskType::FileOperation => Some(&["file_read", "file_write", "repo_map"]),
            TaskType::CodeExecution => Some(&["shell", "git"]),
            TaskType::Research => Some(&["web_search", "web_fetch", "arxiv_search"]),
            TaskType::General => None,
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
    /// タスク種別に基づいてツールをフィルタリングしてから選択する
    /// GeneralはフィルタなしでIALを維持（既存動作と同一）
    pub fn select_relevant_with_type(&self, query: &str, max: usize) -> Vec<&dyn Tool> {
        let task_type = detect_task_type(query);
        let allowed = task_type.allowed_prefixes();

        let query_lower = query.to_lowercase();
        let query_words: Vec<&str> = query_lower.split_whitespace().collect();
        let task_boost = Self::detect_task_boost(&query_lower);

        let mut scored: Vec<(&dyn Tool, usize)> = self
            .tools
            .values()
            .filter(|tool| {
                // Generalならフィルタなし、それ以外は許可リストでフィルタ
                match allowed {
                    None => true,
                    Some(prefixes) => prefixes.iter().any(|p| tool.name() == *p),
                }
            })
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

        scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name().cmp(b.0.name())));
        scored.into_iter().take(max).map(|(t, _)| t).collect()
    }

    /// 段階的開示: 第1段階は名前+summaryのみ（超軽量）、第2段階は選択ツールの全スキーマ展開
    ///
    /// compactとの違い: compactは名前+description、progressiveは名前+summary（より短い1行）
    pub fn format_schemas_progressive(
        &self,
        tools: &[&dyn Tool],
        expanded_names: &[&str],
    ) -> String {
        if tools.is_empty() {
            return String::new();
        }

        let mut output = String::from("# 使用可能なツール

");

        for tool in tools {
            if expanded_names.contains(&tool.name()) {
                // 第2段階: 全スキーマ展開
                output.push_str(&format!("## {}
", tool.name()));
                output.push_str(&format!("{}
", tool.description()));
                output.push_str(&format!(
                    "パラメータ: {}

",
                    serde_json::to_string_pretty(&tool.parameters_schema())
                        .unwrap_or_else(|_| "{}".to_string())
                ));
            } else {
                // 第1段階: 名前+summaryのみ（descriptionの先頭40文字をsummary代替）
                let desc = tool.description();
                let summary = if desc.chars().count() <= 40 {
                    desc.to_string()
                } else {
                    match desc.char_indices().nth(40) {
                        Some((idx, _)) => format!("{}…", &desc[..idx]),
                        None => desc.to_string(),
                    }
                };
                output.push_str(&format!("- **{}**: {}
", tool.name(), summary));
            }
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
    fn test_is_read_only_default_false() {
        // Tool トレイトの is_read_only() デフォルト値は false
        let tool = DummyTool::new("test", "test tool");
        assert!(!tool.is_read_only(), "デフォルトのis_read_onlyはfalseであるべき");
    }

    /// is_read_only を true にオーバーライドするテスト用ツール
    struct ReadOnlyTool;

    impl Tool for ReadOnlyTool {
        fn name(&self) -> &str { "read_only_tool" }
        fn description(&self) -> &str { "読取専用テストツール" }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn permission(&self) -> Permission { Permission::Auto }
        fn call(&self, _args: serde_json::Value) -> Result<ToolResult> {
            Ok(ToolResult { output: "ok".to_string(), success: true })
        }
        fn is_read_only(&self) -> bool { true }
    }

    #[test]
    fn test_is_read_only_override_true() {
        // is_read_only() をオーバーライドして true にできる
        let tool = ReadOnlyTool;
        assert!(tool.is_read_only(), "オーバーライドでtrueになるべき");
    }
    #[test]
    fn test_task_boost_no_boost() {
        let reg = build_registry();
        // ブーストキーワードなしのクエリ
        let selected = reg.select_relevant("天気を教えて", 3);
        assert_eq!(selected.len(), 3);
    }

    /// Tool トレイトが Send+Sync を要求していることのコンパイル時保証
    fn _assert_tool_send_sync<T: Tool>() {}

    #[test]
    fn test_tool_send_sync_compile_time() {
        _assert_tool_send_sync::<DummyTool>();
    }

    #[test]
    fn test_tool_parallel_call_via_thread_scope() {
        let reg = build_registry();
        let tool = reg.get("shell").unwrap();

        std::thread::scope(|s| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    s.spawn(|| {
                        let result = tool.call(serde_json::json!({})).unwrap();
                        assert!(result.success);
                        result.output.clone()
                    })
                })
                .collect();
            let results: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            assert_eq!(results.len(), 4);
            assert!(results.iter().all(|r| r == "ok"));
        });
    }

    #[test]
    fn test_registry_send_sync() {
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<ToolRegistry>();
    }

    #[test]
    fn test_detect_task_type_file_operation() {
        assert_eq!(detect_task_type("ファイルを読みたい"), TaskType::FileOperation);
        assert_eq!(detect_task_type("設定を書き込む"), TaskType::FileOperation);
        assert_eq!(detect_task_type("コードを編集する"), TaskType::FileOperation);
    }

    #[test]
    fn test_detect_task_type_code_execution() {
        assert_eq!(detect_task_type("コマンドを実行する"), TaskType::CodeExecution);
        assert_eq!(detect_task_type("プロジェクトをビルドしたい"), TaskType::CodeExecution);
        assert_eq!(detect_task_type("テストを走らせる"), TaskType::CodeExecution);
    }

    #[test]
    fn test_detect_task_type_research() {
        assert_eq!(detect_task_type("Webで検索して"), TaskType::Research);
        assert_eq!(detect_task_type("この問題を調べて"), TaskType::Research);
        assert_eq!(detect_task_type("最新の論文を探す"), TaskType::Research);
        assert_eq!(detect_task_type("このURLを開いて"), TaskType::Research);
    }

    #[test]
    fn test_detect_task_type_general() {
        assert_eq!(detect_task_type("天気を教えて"), TaskType::General);
        assert_eq!(detect_task_type("こんにちは"), TaskType::General);
    }

    #[test]
    fn test_select_relevant_with_type_file_operation() {
        let reg = build_registry();
        // ファイル操作クエリ → file_read, file_writeのみに絞られる
        let selected = reg.select_relevant_with_type("ファイルを読みたい", 10);
        let names: Vec<&str> = selected.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"file_read"));
        assert!(names.contains(&"file_write"));
        // shell, git, web_search等は含まれない
        assert!(!names.contains(&"shell"));
        assert!(!names.contains(&"git"));
        assert!(!names.contains(&"web_search"));
    }

    #[test]
    fn test_select_relevant_with_type_code_execution() {
        let reg = build_registry();
        let selected = reg.select_relevant_with_type("コマンドを実行する", 10);
        let names: Vec<&str> = selected.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"shell"));
        assert!(names.contains(&"git"));
        assert!(!names.contains(&"file_read"));
    }

    #[test]
    fn test_select_relevant_with_type_research() {
        let reg = build_registry();
        let selected = reg.select_relevant_with_type("Webで検索して", 10);
        let names: Vec<&str> = selected.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"web_search"));
        assert!(!names.contains(&"shell"));
    }

    #[test]
    fn test_select_relevant_with_type_general_no_filter() {
        let reg = build_registry();
        // Generalは全ツールが候補（既存動作と同一）
        let selected = reg.select_relevant_with_type("天気を教えて", 10);
        assert_eq!(selected.len(), 6); // build_registry()は6ツール登録
    }

    #[test]
    fn test_select_relevant_with_type_respects_max() {
        let reg = build_registry();
        let selected = reg.select_relevant_with_type("天気を教えて", 2);
        assert!(selected.len() <= 2);
    }

    #[test]
    fn test_select_relevant_unchanged() {
        // 既存のselect_relevantが変更されていないことを確認
        let reg = build_registry();
        let selected = reg.select_relevant("ファイルを読みたい", 5);
        // select_relevantはフィルタなし → 5件返る
        assert_eq!(selected.len(), 5);
    }

    #[test]
    fn test_task_type_allowed_prefixes() {
        assert!(TaskType::FileOperation.allowed_prefixes().is_some());
        assert!(TaskType::CodeExecution.allowed_prefixes().is_some());
        assert!(TaskType::Research.allowed_prefixes().is_some());
        assert!(TaskType::General.allowed_prefixes().is_none());
    }

    #[test]
    fn test_format_schemas_progressive_collapsed() {
        // 展開対象なし — 全ツールがsummary形式（第1段階）
        let reg = build_registry();
        let all: Vec<&dyn Tool> = reg.tools.values().map(|t| t.as_ref()).collect();
        let progressive = reg.format_schemas_progressive(&all, &[]);
        // パラメータスキーマは含まれない
        assert!(!progressive.contains("properties"));
        // 名前は含まれる
        assert!(progressive.contains("shell"));
        assert!(progressive.contains("file_read"));
    }

    #[test]
    fn test_format_schemas_progressive_expanded() {
        // shellのみ展開 — 他はsummary形式
        let reg = build_registry();
        let all: Vec<&dyn Tool> = reg.tools.values().map(|t| t.as_ref()).collect();
        let progressive = reg.format_schemas_progressive(&all, &["shell"]);
        // shellはフルスキーマ展開（##ヘッダ + パラメータ）
        assert!(progressive.contains("## shell"));
        // 他のツールはsummary形式（- **name**: ...）
        assert!(progressive.contains("- **file_read**"));
    }

    #[test]
    fn test_format_schemas_progressive_empty() {
        let reg = build_registry();
        let progressive = reg.format_schemas_progressive(&[], &[]);
        assert!(progressive.is_empty());
    }

    #[test]
    fn test_format_schemas_progressive_shorter_than_full() {
        // progressive（第1段階）はフル展開より短い
        let reg = build_registry();
        let all: Vec<&dyn Tool> = reg.tools.values().map(|t| t.as_ref()).collect();
        let progressive = reg.format_schemas_progressive(&all, &[]);
        let full = reg.format_schemas(&all);
        assert!(progressive.len() < full.len());
    }

    #[test]
    fn test_format_schemas_progressive_all_expanded() {
        // 全ツール展開 — format_schemasと同等の情報量
        let reg = build_registry();
        let all: Vec<&dyn Tool> = reg.tools.values().map(|t| t.as_ref()).collect();
        let names: Vec<&str> = all.iter().map(|t| t.name()).collect();
        let progressive = reg.format_schemas_progressive(&all, &names);
        // 全ツールが ## ヘッダで展開
        assert!(progressive.contains("## shell"));
        assert!(progressive.contains("## file_read"));
        // summary形式（- **name**）は含まれない
        assert!(!progressive.contains("- **shell**"));
    }
}
