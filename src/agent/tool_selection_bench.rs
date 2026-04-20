//! FunctionGemma ツール選択精度ベンチマーク
//!
//! bonsai-agentのツール群をFunctionGemma形式で定義し、
//! 各タスクに対してどのツールを選択するかの精度を計測する。

use std::time::Instant;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::agent::parse::{format_functiongemma_system_prompt, parse_functiongemma_output};
use crate::tools::ToolSchema;

/// ツール選択テストケース
#[derive(Debug, Clone)]
pub struct ToolSelectionCase {
    pub id: String,
    /// ユーザーの入力クエリ
    pub query: String,
    /// 期待されるツール名（順不同、1つでも一致すれば成功）
    pub expected_tools: Vec<String>,
}

/// ツール選択結果（1ケース）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionResult {
    pub case_id: String,
    pub query: String,
    pub expected: Vec<String>,
    pub selected: Option<String>,
    pub correct: bool,
    pub latency_ms: u64,
    pub raw_output: String,
}

/// ベンチマーク全体の結果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSelectionBenchResult {
    pub model_id: String,
    pub results: Vec<SelectionResult>,
    pub total_cases: usize,
    pub correct_count: usize,
    pub accuracy: f64,
    pub avg_latency_ms: f64,
    pub total_duration_secs: f64,
}

impl ToolSelectionBenchResult {
    /// 結果サマリーを文字列で返す
    pub fn summary(&self) -> String {
        format!(
            "モデル: {}\n正答率: {}/{} ({:.1}%)\n平均レイテンシ: {:.0}ms\n総所要時間: {:.1}s",
            self.model_id,
            self.correct_count,
            self.total_cases,
            self.accuracy * 100.0,
            self.avg_latency_ms,
            self.total_duration_secs,
        )
    }
}

/// bonsai-agentの主要ツールスキーマ群（ベンチマーク用）
pub fn bonsai_tool_schemas() -> Vec<ToolSchema> {
    vec![
        ToolSchema {
            name: "shell".to_string(),
            description: "Execute a shell command and return its output".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Shell command to execute"}
                },
                "required": ["command"]
            }),
        },
        ToolSchema {
            name: "file_read".to_string(),
            description: "Read content from a file at the given path".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path to read"},
                    "offset": {"type": "integer", "description": "Start line number"},
                    "limit": {"type": "integer", "description": "Number of lines to read"}
                },
                "required": ["path"]
            }),
        },
        ToolSchema {
            name: "file_write".to_string(),
            description: "Write or edit content in a file using search and replace".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path to write"},
                    "content": {"type": "string", "description": "Content to write"},
                    "search": {"type": "string", "description": "Text to search for replacement"},
                    "replace": {"type": "string", "description": "Replacement text"}
                },
                "required": ["path"]
            }),
        },
        ToolSchema {
            name: "git".to_string(),
            description: "Execute git commands like status, log, diff, commit".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "subcommand": {"type": "string", "description": "Git subcommand (status, log, diff, commit, etc.)"},
                    "args": {"type": "string", "description": "Additional arguments"}
                },
                "required": ["subcommand"]
            }),
        },
        ToolSchema {
            name: "web_search".to_string(),
            description: "Search the web for information using a query".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query"}
                },
                "required": ["query"]
            }),
        },
        ToolSchema {
            name: "web_fetch".to_string(),
            description: "Fetch content from a URL and return the page text".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "URL to fetch"}
                },
                "required": ["url"]
            }),
        },
        ToolSchema {
            name: "repo_map".to_string(),
            description: "Generate a repository map showing file structure and key symbols".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Root path to map"}
                },
                "required": ["path"]
            }),
        },
    ]
}

/// デフォルトのツール選択テストケース（16件）
pub fn default_cases() -> Vec<ToolSelectionCase> {
    vec![
        // --- ファイル操作 ---
        ToolSelectionCase {
            id: "read_readme".into(),
            query: "Read the README.md file".into(),
            expected_tools: vec!["file_read".into()],
        },
        ToolSelectionCase {
            id: "read_config".into(),
            query: "Show me the contents of config.toml".into(),
            expected_tools: vec!["file_read".into()],
        },
        ToolSelectionCase {
            id: "write_file".into(),
            query: "Create a new file called hello.txt with 'Hello World'".into(),
            expected_tools: vec!["file_write".into()],
        },
        ToolSelectionCase {
            id: "edit_file".into(),
            query: "Replace 'foo' with 'bar' in src/main.rs".into(),
            expected_tools: vec!["file_write".into()],
        },
        // --- シェル ---
        ToolSelectionCase {
            id: "list_files".into(),
            query: "List all files in the current directory".into(),
            expected_tools: vec!["shell".into()],
        },
        ToolSelectionCase {
            id: "run_tests".into(),
            query: "Run the test suite with cargo test".into(),
            expected_tools: vec!["shell".into()],
        },
        ToolSelectionCase {
            id: "disk_usage".into(),
            query: "Check disk usage of the current directory".into(),
            expected_tools: vec!["shell".into()],
        },
        // --- Git ---
        ToolSelectionCase {
            id: "git_status".into(),
            query: "Show the current git status".into(),
            expected_tools: vec!["git".into()],
        },
        ToolSelectionCase {
            id: "git_log".into(),
            query: "Show recent git commit history".into(),
            expected_tools: vec!["git".into()],
        },
        ToolSelectionCase {
            id: "git_diff".into(),
            query: "Show the diff of uncommitted changes".into(),
            expected_tools: vec!["git".into()],
        },
        // --- Web ---
        ToolSelectionCase {
            id: "search_web".into(),
            query: "Search the web for Rust async patterns".into(),
            expected_tools: vec!["web_search".into()],
        },
        ToolSelectionCase {
            id: "fetch_url".into(),
            query: "Fetch the content of https://example.com".into(),
            expected_tools: vec!["web_fetch".into()],
        },
        // --- RepoMap ---
        ToolSelectionCase {
            id: "repo_structure".into(),
            query: "Show me the repository structure and important symbols".into(),
            expected_tools: vec!["repo_map".into()],
        },
        // --- 曖昧なケース ---
        ToolSelectionCase {
            id: "ambiguous_build".into(),
            query: "Build the project".into(),
            expected_tools: vec!["shell".into()],
        },
        ToolSelectionCase {
            id: "ambiguous_find_bug".into(),
            query: "Find the bug in the login function in auth.rs".into(),
            expected_tools: vec!["file_read".into()],
        },
        ToolSelectionCase {
            id: "ambiguous_deploy".into(),
            query: "Check the latest changes and commit them".into(),
            expected_tools: vec!["git".into()],
        },
    ]
}

/// FunctionGemmaが生成するツール名を正規化
/// 例: "git status" → "git", "file_read" → "file_read"
fn normalize_tool_name(name: &str) -> String {
    // スペースで分割し、最初の語をツール名とみなす
    // （FunctionGemmaが `call:git status{}` のようにサブコマンドを名前に含めるケース対応）
    name.split_whitespace()
        .next()
        .unwrap_or(name)
        .to_string()
}

/// FunctionGemma形式でllama-serverに問い合わせ、ツール選択を1件実行
fn run_single_case(
    base_url: &str,
    system_prompt: &str,
    case: &ToolSelectionCase,
    timeout_secs: u64,
) -> Result<SelectionResult> {
    let start = Instant::now();

    let body = serde_json::json!({
        "model": "functiongemma",
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": case.query}
        ],
        "max_tokens": 128,
        "temperature": 0.1,
        "stream": false,
        "stop": ["<end_function_call>"]
    });

    let resp: serde_json::Value = ureq::post(&format!("{base_url}/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(timeout_secs)))
        .build()
        .send_json(&body)?
        .body_mut()
        .read_json()?;

    let raw_output = resp["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();

    let latency_ms = start.elapsed().as_millis() as u64;

    // stop sequenceで切れた場合、閉じタグを補完してからパース
    let raw_for_parse = if raw_output.contains("<start_function_call>")
        && !raw_output.contains("<end_function_call>")
    {
        format!("{raw_output}<end_function_call>")
    } else {
        raw_output.clone()
    };

    // FunctionGemma出力をパース
    let selected = match parse_functiongemma_output(&raw_for_parse) {
        Ok(parsed) if !parsed.tool_calls.is_empty() => {
            Some(normalize_tool_name(&parsed.tool_calls[0].name))
        }
        _ => None,
    };

    let correct = match &selected {
        Some(name) => case.expected_tools.iter().any(|e| name.starts_with(e)),
        None => false,
    };

    Ok(SelectionResult {
        case_id: case.id.clone(),
        query: case.query.clone(),
        expected: case.expected_tools.clone(),
        selected,
        correct,
        latency_ms,
        raw_output,
    })
}

/// ツール選択精度ベンチマーク全体を実行
///
/// `base_url`: llama-serverのURL（例: "http://127.0.0.1:8081"）
/// `model_id`: 記録用モデルID
pub fn run_tool_selection_bench(
    base_url: &str,
    model_id: &str,
    cases: &[ToolSelectionCase],
    timeout_secs: u64,
) -> Result<ToolSelectionBenchResult> {
    let schemas = bonsai_tool_schemas();
    let system_prompt = format_functiongemma_system_prompt(&schemas);

    let overall_start = Instant::now();
    let mut results = Vec::new();

    for case in cases {
        match run_single_case(base_url, &system_prompt, case, timeout_secs) {
            Ok(r) => results.push(r),
            Err(e) => {
                results.push(SelectionResult {
                    case_id: case.id.clone(),
                    query: case.query.clone(),
                    expected: case.expected_tools.clone(),
                    selected: None,
                    correct: false,
                    latency_ms: 0,
                    raw_output: format!("Error: {e}"),
                });
            }
        }
    }

    let total_cases = results.len();
    let correct_count = results.iter().filter(|r| r.correct).count();
    let accuracy = if total_cases > 0 {
        correct_count as f64 / total_cases as f64
    } else {
        0.0
    };
    let avg_latency_ms = if total_cases > 0 {
        results.iter().map(|r| r.latency_ms as f64).sum::<f64>() / total_cases as f64
    } else {
        0.0
    };

    Ok(ToolSelectionBenchResult {
        model_id: model_id.to_string(),
        results,
        total_cases,
        correct_count,
        accuracy,
        avg_latency_ms,
        total_duration_secs: overall_start.elapsed().as_secs_f64(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bonsai_tool_schemas_count() {
        let schemas = bonsai_tool_schemas();
        assert_eq!(schemas.len(), 7);
    }

    #[test]
    fn test_bonsai_tool_schemas_names() {
        let schemas = bonsai_tool_schemas();
        let names: Vec<&str> = schemas.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"shell"));
        assert!(names.contains(&"file_read"));
        assert!(names.contains(&"git"));
        assert!(names.contains(&"web_search"));
    }

    #[test]
    fn test_default_cases_count() {
        let cases = default_cases();
        assert_eq!(cases.len(), 16);
    }

    #[test]
    fn test_default_cases_all_have_expected() {
        let cases = default_cases();
        for case in &cases {
            assert!(
                !case.expected_tools.is_empty(),
                "case {} has no expected tools",
                case.id
            );
        }
    }

    #[test]
    fn test_default_cases_unique_ids() {
        let cases = default_cases();
        let mut ids: Vec<&str> = cases.iter().map(|c| c.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), cases.len(), "重複IDあり");
    }

    #[test]
    fn test_system_prompt_contains_all_tools() {
        let schemas = bonsai_tool_schemas();
        let prompt = format_functiongemma_system_prompt(&schemas);
        for schema in &schemas {
            assert!(
                prompt.contains(&format!("declaration:{}", schema.name)),
                "{}がプロンプトに含まれない",
                schema.name
            );
        }
    }

    #[test]
    fn test_bench_result_summary() {
        let result = ToolSelectionBenchResult {
            model_id: "test-model".into(),
            results: vec![],
            total_cases: 10,
            correct_count: 8,
            accuracy: 0.8,
            avg_latency_ms: 150.0,
            total_duration_secs: 5.0,
        };
        let summary = result.summary();
        assert!(summary.contains("test-model"));
        assert!(summary.contains("80.0%"));
        assert!(summary.contains("8/10"));
    }

    #[test]
    fn test_selection_result_correct_logic() {
        // 正答ケース
        let r = SelectionResult {
            case_id: "test".into(),
            query: "test".into(),
            expected: vec!["shell".into()],
            selected: Some("shell".into()),
            correct: true,
            latency_ms: 100,
            raw_output: String::new(),
        };
        assert!(r.correct);

        // 誤答ケース
        let r2 = SelectionResult {
            case_id: "test2".into(),
            query: "test".into(),
            expected: vec!["git".into()],
            selected: Some("shell".into()),
            correct: false,
            latency_ms: 100,
            raw_output: String::new(),
        };
        assert!(!r2.correct);
    }

    /// 実機テスト: llama-server:8081でFunctionGemmaが起動している場合のみ
    #[test]
    #[ignore]
    fn test_live_tool_selection_bench() {
        let cases = default_cases();
        let result = run_tool_selection_bench(
            "http://127.0.0.1:8081",
            "functiongemma-270m-it-q8_0",
            &cases,
            30,
        )
        .unwrap();

        println!("\n{}", result.summary());
        println!("\n--- 詳細 ---");
        for r in &result.results {
            let mark = if r.correct { "✓" } else { "✗" };
            println!(
                "{} [{}] expected={:?} selected={:?} ({}ms)",
                mark, r.case_id, r.expected, r.selected, r.latency_ms
            );
            if !r.correct {
                println!("  raw: {}", &r.raw_output[..r.raw_output.len().min(200)]);
            }
        }

        assert!(
            result.accuracy > 0.0,
            "正答率が0%: FunctionGemmaが全問不正解"
        );
    }
}
