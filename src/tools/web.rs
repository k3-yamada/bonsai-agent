use anyhow::Result;

use crate::tools::ToolResult;
use crate::tools::permission::Permission;
use crate::tools::typed::TypedTool;
use schemars::JsonSchema;
use serde::Deserialize;

/// Web検索ツール（DuckDuckGo Instant Answer API、API鍵不要）
pub struct WebSearchTool;

#[derive(Deserialize, JsonSchema)]
pub struct WebSearchArgs {
    /// 検索クエリ
    query: String,
}

impl TypedTool for WebSearchTool {
    type Args = WebSearchArgs;
    const NAME: &'static str = "web_search";
    const DESCRIPTION: &'static str =
        "Webを検索する。queryパラメータに検索クエリを指定。DuckDuckGo Instant Answer APIを使用。";
    const PERMISSION: Permission = Permission::Auto;
    const READ_ONLY: bool = true;

    fn execute(&self, args: WebSearchArgs) -> Result<ToolResult> {
        let query = &args.query;

        let url = format!(
            "https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1",
            urlencoding(query)
        );

        match ureq::get(&url).call() {
            Ok(mut response) => {
                let body: serde_json::Value = response.body_mut().read_json()?;
                let result = format_ddg_response(&body, query);
                Ok(ToolResult {
                    output: result,
                    success: true,
                })
            }
            Err(e) => Ok(ToolResult {
                output: format!("検索エラー: {e}"),
                success: false,
            }),
        }
    }
}

/// URLからテキストを取得するツール
pub struct WebFetchTool;

#[derive(Deserialize, JsonSchema)]
pub struct WebFetchArgs {
    /// 取得するURL
    url: String,
}

impl TypedTool for WebFetchTool {
    type Args = WebFetchArgs;
    const NAME: &'static str = "web_fetch";
    const DESCRIPTION: &'static str =
        "URLからWebページのテキスト内容を取得する。urlパラメータにURLを指定。";
    const PERMISSION: Permission = Permission::Auto;
    const READ_ONLY: bool = true;

    fn execute(&self, args: WebFetchArgs) -> Result<ToolResult> {
        let url = &args.url;

        match reqwest::blocking::get(url) {
            Ok(response) => {
                let body = response.text()?;
                // HTMLタグを簡易的に除去
                let text = strip_html_tags(&body);
                // 長すぎる場合は切り詰め
                let truncated = if text.len() > 4000 {
                    format!(
                        "{}...\n\n（{}文字中、最初の4000文字を表示）",
                        &text[..4000],
                        text.len()
                    )
                } else {
                    text
                };
                Ok(ToolResult {
                    output: truncated,
                    success: true,
                })
            }
            Err(e) => Ok(ToolResult {
                output: format!("取得エラー: {e}"),
                success: false,
            }),
        }
    }
}

/// 簡易URLエンコーディング
fn urlencoding(s: &str) -> String {
    let mut result = String::new();
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => result.push(c),
            ' ' => result.push('+'),
            _ => {
                for b in c.to_string().as_bytes() {
                    result.push_str(&format!("%{b:02X}"));
                }
            }
        }
    }
    result
}

/// DuckDuckGo APIレスポンスをテキストにフォーマット
fn format_ddg_response(body: &serde_json::Value, query: &str) -> String {
    let mut parts = Vec::new();

    // AbstractText（メイン回答）
    if let Some(abstract_text) = body.get("AbstractText").and_then(|v| v.as_str())
        && !abstract_text.is_empty()
    {
        parts.push(abstract_text.to_string());
        if let Some(source) = body.get("AbstractSource").and_then(|v| v.as_str()) {
            parts.push(format!("出典: {source}"));
        }
    }

    // Answer（直接回答）
    if let Some(answer) = body.get("Answer").and_then(|v| v.as_str())
        && !answer.is_empty()
    {
        parts.push(format!("回答: {answer}"));
    }

    // RelatedTopics（関連トピック）
    if let Some(topics) = body.get("RelatedTopics").and_then(|v| v.as_array()) {
        let related: Vec<String> = topics
            .iter()
            .filter_map(|t| {
                t.get("Text").and_then(|v| v.as_str()).map(|s| {
                    if s.len() > 200 {
                        format!("- {}...", &s[..200])
                    } else {
                        format!("- {s}")
                    }
                })
            })
            .take(5)
            .collect();
        if !related.is_empty() {
            parts.push(format!("関連:\n{}", related.join("\n")));
        }
    }

    if parts.is_empty() {
        format!("「{query}」の検索結果が見つかりませんでした。")
    } else {
        parts.join("\n\n")
    }
}

/// HTMLタグを簡易的に除去
fn strip_html_tags(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;

    for c in html.chars() {
        if c == '<' {
            in_tag = true;
            continue;
        }
        if c == '>' {
            in_tag = false;
            continue;
        }
        if !in_tag {
            result.push(c);
        }
    }

    // 連続空白を1つに圧縮
    let mut compressed = String::new();
    let mut prev_space = false;
    for c in result.chars() {
        if c.is_whitespace() {
            if !prev_space {
                compressed.push(' ');
            }
            prev_space = true;
        } else {
            compressed.push(c);
            prev_space = false;
        }
    }
    compressed.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Tool;

    #[test]
    fn test_urlencoding() {
        assert_eq!(urlencoding("hello world"), "hello+world");
        assert_eq!(urlencoding("rust lang"), "rust+lang");
        assert_eq!(urlencoding("a&b=c"), "a%26b%3Dc");
    }

    #[test]
    fn test_strip_html_tags() {
        assert_eq!(strip_html_tags("<p>hello</p>"), "hello");
        assert_eq!(strip_html_tags("<b>bold</b> text"), "bold text");
        assert_eq!(strip_html_tags("no tags"), "no tags");
    }

    #[test]
    fn test_strip_html_whitespace() {
        let result = strip_html_tags("<p>hello</p>   <p>world</p>");
        assert!(result.contains("hello"));
        assert!(result.contains("world"));
    }

    #[test]
    fn test_format_ddg_empty() {
        let body = serde_json::json!({});
        let result = format_ddg_response(&body, "test");
        assert!(result.contains("見つかりませんでした"));
    }

    #[test]
    fn test_format_ddg_with_abstract() {
        let body = serde_json::json!({
            "AbstractText": "Rust is a programming language.",
            "AbstractSource": "Wikipedia"
        });
        let result = format_ddg_response(&body, "rust");
        assert!(result.contains("Rust is a programming language"));
        assert!(result.contains("Wikipedia"));
    }

    #[test]
    fn test_format_ddg_with_answer() {
        let body = serde_json::json!({
            "Answer": "42"
        });
        let result = format_ddg_response(&body, "meaning of life");
        assert!(result.contains("42"));
    }

    #[test]
    fn test_web_search_metadata() {
        let tool = WebSearchTool;
        assert_eq!(tool.name(), "web_search");
        assert_eq!(tool.permission(), Permission::Auto);
    }

    #[test]
    fn test_web_fetch_metadata() {
        let tool = WebFetchTool;
        assert_eq!(tool.name(), "web_fetch");
        assert_eq!(tool.permission(), Permission::Auto);
    }

    #[test]
    fn test_web_search_missing_param() {
        let tool = WebSearchTool;
        assert!(tool.call(serde_json::json!({})).is_err());
    }

    #[test]
    fn test_web_fetch_missing_param() {
        let tool = WebFetchTool;
        assert!(tool.call(serde_json::json!({})).is_err());
    }

    // 実ネットワークテスト
    #[test]
    #[ignore]
    fn test_web_search_live() {
        let tool = WebSearchTool;
        let result = tool
            .call(serde_json::json!({"query": "Rust programming language"}))
            .unwrap();
        assert!(result.success);
        assert!(!result.output.is_empty());
    }

    #[test]
    #[ignore]
    fn test_web_fetch_live() {
        let tool = WebFetchTool;
        let result = tool
            .call(serde_json::json!({"url": "https://example.com"}))
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Example Domain"));
    }
}
