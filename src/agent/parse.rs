use crate::agent::conversation::{ParsedOutput, ToolCall};
use anyhow::Result;

/// LLMの生出力をパースする。
/// `<think>` ブロックから思考テキスト、`<tool_call>` ブロックからツール呼び出し、
/// 残りのテキストを最終回答として抽出する。
pub fn parse_assistant_output(raw: &str) -> Result<ParsedOutput> {
    let mut thinking = None;
    let mut tool_calls = Vec::new();
    let mut text_parts = Vec::new();

    let mut remaining = raw;

    while !remaining.is_empty() {
        if let Some(think_start) = remaining.find("<think>") {
            // <think> タグの前のテキストを回収
            let before = remaining[..think_start].trim();
            if !before.is_empty() {
                text_parts.push(before.to_string());
            }

            if let Some(think_end) = remaining[think_start..].find("</think>") {
                let think_content = &remaining[think_start + 7..think_start + think_end];
                thinking = Some(think_content.trim().to_string());
                remaining = &remaining[think_start + think_end + 8..];
            } else {
                // 閉じタグなし — 残り全体を思考として扱う
                let think_content = &remaining[think_start + 7..];
                thinking = Some(think_content.trim().to_string());
                remaining = "";
            }
        } else if let Some(tc_start) = remaining.find("<tool_call>") {
            // <tool_call> タグの前のテキストを回収
            let before = remaining[..tc_start].trim();
            if !before.is_empty() {
                text_parts.push(before.to_string());
            }

            if let Some(tc_end) = remaining[tc_start..].find("</tool_call>") {
                let tc_content = &remaining[tc_start + 11..tc_start + tc_end];
                match serde_json::from_str::<ToolCall>(tc_content.trim()) {
                    Ok(call) => tool_calls.push(call),
                    Err(e) => {
                        anyhow::bail!(
                            "tool_callのJSONパースに失敗: {e}。内容: {}",
                            tc_content.trim()
                        );
                    }
                }
                remaining = &remaining[tc_start + tc_end + 12..];
            } else {
                // 閉じタグなし
                anyhow::bail!("</tool_call> 閉じタグが見つかりません");
            }
        } else {
            // タグなし — 残り全体をテキストとして回収
            let trimmed = remaining.trim();
            if !trimmed.is_empty() {
                text_parts.push(trimmed.to_string());
            }
            remaining = "";
        }
    }

    let text = if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join("\n"))
    };

    Ok(ParsedOutput {
        thinking,
        tool_calls,
        text,
    })
}

/// FunctionGemma形式の出力をパースする。
/// `<start_function_call>call:name{key:<escape>val<escape>}<end_function_call>` 形式を
/// 標準のToolCall構造体に変換する。
pub fn parse_functiongemma_output(raw: &str) -> Result<ParsedOutput> {
    let mut tool_calls = Vec::new();
    let mut text_parts = Vec::new();
    let mut remaining = raw;

    while !remaining.is_empty() {
        if let Some(fc_start) = remaining.find("<start_function_call>") {
            let before = remaining[..fc_start].trim();
            if !before.is_empty() {
                text_parts.push(before.to_string());
            }
            if let Some(fc_end) = remaining[fc_start..].find("<end_function_call>") {
                let fc_content = &remaining[fc_start + 21..fc_start + fc_end];
                match parse_fg_call(fc_content.trim()) {
                    Ok(call) => tool_calls.push(call),
                    Err(e) => {
                        anyhow::bail!(
                            "FunctionGemma呼出のパースに失敗: {e}。内容: {}",
                            fc_content.trim()
                        );
                    }
                }
                remaining = &remaining[fc_start + fc_end + 19..];
            } else {
                anyhow::bail!("<end_function_call> 閉じタグが見つかりません");
            }
        } else {
            let trimmed = remaining.trim();
            if !trimmed.is_empty() {
                text_parts.push(trimmed.to_string());
            }
            remaining = "";
        }
    }

    let text = if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join("
"))
    };

    Ok(ParsedOutput {
        thinking: None,
        tool_calls,
        text,
    })
}

/// `call:name{key:<escape>val<escape>,key2:<escape>val2<escape>}` をToolCallに変換
fn parse_fg_call(content: &str) -> Result<ToolCall> {
    let content = content.strip_prefix("call:").unwrap_or(content);

    // 関数名と引数部分を分離（最初の`{`で分割）
    let (name, args_str) = match content.find('{') {
        Some(idx) => (&content[..idx], &content[idx..]),
        None => anyhow::bail!("関数名と引数の区切り '{{' が見つかりません"),
    };

    // 引数パース: `{key:<escape>val<escape>,key2:<escape>val2<escape>}` → JSON
    let inner = args_str
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or("");

    let mut arguments = serde_json::Map::new();
    if !inner.is_empty() {
        for pair in split_fg_params(inner) {
            if let Some((key, val)) = pair.split_once(':') {
                let val = val
                    .strip_prefix("<escape>")
                    .unwrap_or(val)
                    .strip_suffix("<escape>")
                    .unwrap_or(val);
                arguments.insert(
                    key.to_string(),
                    serde_json::Value::String(val.to_string()),
                );
            }
        }
    }

    Ok(ToolCall {
        name: name.to_string(),
        arguments: serde_json::Value::Object(arguments),
    })
}

/// FunctionGemmaパラメータ文字列をカンマ区切りで分割
/// `<escape>`の中のカンマは無視する
fn split_fg_params(s: &str) -> Vec<&str> {
    let mut results = Vec::new();
    let mut start = 0;
    let mut in_escape = false;

    for (i, c) in s.char_indices() {
        if s[i..].starts_with("<escape>") {
            in_escape = !in_escape;
        }
        if c == ',' && !in_escape {
            results.push(&s[start..i]);
            start = i + 1;
        }
    }
    if start < s.len() {
        results.push(&s[start..]);
    }
    results
}

/// ToolSchemaをFunctionGemma宣言形式に変換
pub fn format_functiongemma_declaration(schema: &crate::tools::ToolSchema) -> String {
    let mut decl = format!(
        "<start_function_declaration>declaration:{}{{description:<escape>{}<escape>",
        schema.name, schema.description
    );

    if let Some(props) = schema.parameters.get("properties") {
        decl.push_str(",parameters:{properties:{");
        let obj = props.as_object().unwrap();
        let mut first = true;
        for (key, val) in obj {
            if !first {
                decl.push(',');
            }
            first = false;
            let desc = val
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("");
            let typ = val.get("type").and_then(|t| t.as_str()).unwrap_or("string");
            decl.push_str(&format!(
                "{key}:{{description:<escape>{desc}<escape>,type:<escape>{}<escape>}}",
                typ.to_uppercase()
            ));
        }
        decl.push('}');

        // required
        if let Some(req) = schema.parameters.get("required")
            && let Some(arr) = req.as_array()
        {
            let items: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| format!("<escape>{s}<escape>"))
                .collect();
            decl.push_str(&format!(",required:[{}]", items.join(",")));
        }

        decl.push_str(",type:<escape>OBJECT<escape>}");
    }

    decl.push_str("}<end_function_declaration>");
    decl
}

/// 複数のToolSchemaからFunctionGemmaシステムプロンプトを構築
pub fn format_functiongemma_system_prompt(schemas: &[crate::tools::ToolSchema]) -> String {
    let mut prompt =
        String::from("You are a model that can do function calling with the following functions");
    for schema in schemas {
        prompt.push_str(&format_functiongemma_declaration(schema));
    }
    prompt
}

/// ツール引数の型強制（hermes-agent知見: LLMが数値を文字列で返す問題）
///
/// JSON値を走査し、数値文字列を数値に、bool文字列をboolに変換。
/// 例: `{"count": "42"}` → `{"count": 42}`
/// 例: `{"flag": "true"}` → `{"flag": true}`
pub fn coerce_tool_arguments(args: &mut serde_json::Value) {
    if let serde_json::Value::Object(map) = args {
        for (_key, val) in map.iter_mut() {
            coerce_value(val);
        }
    }
}

fn coerce_value(val: &mut serde_json::Value) {
    if let serde_json::Value::String(s) = val {
        // bool変換
        if s == "true" {
            *val = serde_json::Value::Bool(true);
            return;
        }
        if s == "false" {
            *val = serde_json::Value::Bool(false);
            return;
        }
        // 整数変換
        if let Ok(n) = s.parse::<i64>() {
            *val = serde_json::json!(n);
            return;
        }
        // 浮動小数点変換
        if let Ok(n) = s.parse::<f64>()
            && s.contains('.')
        {
            *val = serde_json::json!(n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // テスト1: プレーンテキストのみ
    #[test]
    fn test_plain_text_only() {
        let result = parse_assistant_output("東京の天気は晴れです。").unwrap();
        assert!(result.thinking.is_none());
        assert!(result.tool_calls.is_empty());
        assert_eq!(result.text, Some("東京の天気は晴れです。".to_string()));
    }

    // テスト2: <think>ブロックのみ
    #[test]
    fn test_think_block_only() {
        let input = "<think>ユーザーは天気を知りたいようだ</think>";
        let result = parse_assistant_output(input).unwrap();
        assert_eq!(
            result.thinking,
            Some("ユーザーは天気を知りたいようだ".to_string())
        );
        assert!(result.tool_calls.is_empty());
        assert!(result.text.is_none());
    }

    // テスト3: 単一<tool_call>
    #[test]
    fn test_single_tool_call() {
        let input = r#"<tool_call>{"name":"shell","arguments":{"command":"date"}}</tool_call>"#;
        let result = parse_assistant_output(input).unwrap();
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].name, "shell");
        assert_eq!(result.tool_calls[0].arguments["command"], "date");
        assert!(result.text.is_none());
    }

    // テスト4: 複数<tool_call>
    #[test]
    fn test_multiple_tool_calls() {
        let input = r#"<tool_call>{"name":"shell","arguments":{"command":"ls"}}</tool_call>
<tool_call>{"name":"file_read","arguments":{"path":"README.md"}}</tool_call>"#;
        let result = parse_assistant_output(input).unwrap();
        assert_eq!(result.tool_calls.len(), 2);
        assert_eq!(result.tool_calls[0].name, "shell");
        assert_eq!(result.tool_calls[1].name, "file_read");
    }

    // テスト5: <think> + <tool_call> + テキスト混在
    #[test]
    fn test_mixed_content() {
        let input = r#"<think>ファイル一覧を確認しよう</think>
まずディレクトリを確認します。
<tool_call>{"name":"shell","arguments":{"command":"ls -la"}}</tool_call>
結果を確認中..."#;
        let result = parse_assistant_output(input).unwrap();
        assert_eq!(
            result.thinking,
            Some("ファイル一覧を確認しよう".to_string())
        );
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].name, "shell");
        let text = result.text.unwrap();
        assert!(text.contains("まずディレクトリを確認します"));
        assert!(text.contains("結果を確認中"));
    }

    // テスト6: 不正JSON
    #[test]
    fn test_invalid_json() {
        let input = r#"<tool_call>{invalid json}</tool_call>"#;
        let result = parse_assistant_output(input);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("JSONパースに失敗"));
    }

    // テスト7: <think>の閉じタグなし（graceful handling）
    #[test]
    fn test_unclosed_think_tag() {
        let input = "<think>考え中...";
        let result = parse_assistant_output(input).unwrap();
        assert_eq!(result.thinking, Some("考え中...".to_string()));
        assert!(result.text.is_none());
    }

    // テスト8: 空の<tool_call>
    #[test]
    fn test_empty_tool_call() {
        let input = "<tool_call></tool_call>";
        let result = parse_assistant_output(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_coerce_string_to_int() {
        let mut v = serde_json::json!({"count": "42", "name": "test"});
        coerce_tool_arguments(&mut v);
        assert_eq!(v["count"], 42);
        assert_eq!(v["name"], "test"); // 文字列のまま
    }

    #[test]
    fn test_coerce_string_to_bool() {
        let mut v = serde_json::json!({"flag": "true", "other": "false"});
        coerce_tool_arguments(&mut v);
        assert_eq!(v["flag"], true);
        assert_eq!(v["other"], false);
    }

    #[test]
    fn test_coerce_string_to_float() {
        let mut v = serde_json::json!({"ratio": "3.14"});
        coerce_tool_arguments(&mut v);
        assert!((v["ratio"].as_f64().unwrap() - 3.14).abs() < 1e-9);
    }

    #[test]
    fn test_coerce_non_numeric_string_unchanged() {
        let mut v = serde_json::json!({"path": "/tmp/file.txt"});
        coerce_tool_arguments(&mut v);
        assert_eq!(v["path"], "/tmp/file.txt");
    }

    // --- FunctionGemma パーサーテスト ---

    #[test]
    fn test_fg_single_call() {
        let input = "<start_function_call>call:get_weather{location:<escape>Tokyo<escape>}<end_function_call>";
        let result = parse_functiongemma_output(input).unwrap();
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].name, "get_weather");
        assert_eq!(result.tool_calls[0].arguments["location"], "Tokyo");
    }

    #[test]
    fn test_fg_multiple_params() {
        let input = "<start_function_call>call:shell{command:<escape>ls -la<escape>,timeout:<escape>30<escape>}<end_function_call>";
        let result = parse_functiongemma_output(input).unwrap();
        assert_eq!(result.tool_calls[0].name, "shell");
        assert_eq!(result.tool_calls[0].arguments["command"], "ls -la");
        assert_eq!(result.tool_calls[0].arguments["timeout"], "30");
    }

    #[test]
    fn test_fg_no_params() {
        let input = "<start_function_call>call:list_tools{}<end_function_call>";
        let result = parse_functiongemma_output(input).unwrap();
        assert_eq!(result.tool_calls[0].name, "list_tools");
        assert!(result.tool_calls[0].arguments.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_fg_with_text_before() {
        let input = "Let me check the weather.
<start_function_call>call:get_weather{city:<escape>Osaka<escape>}<end_function_call>";
        let result = parse_functiongemma_output(input).unwrap();
        assert_eq!(result.tool_calls.len(), 1);
        assert!(result.text.unwrap().contains("Let me check"));
    }

    #[test]
    fn test_fg_unclosed_tag() {
        let input = "<start_function_call>call:shell{command:<escape>date<escape>}";
        let result = parse_functiongemma_output(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_fg_text_only() {
        let input = "I cannot call any function for this request.";
        let result = parse_functiongemma_output(input).unwrap();
        assert!(result.tool_calls.is_empty());
        assert!(result.text.is_some());
    }

    // --- FunctionGemma プロンプトフォーマッタテスト ---

    #[test]
    fn test_fg_format_tool_declaration() {
        let schema = crate::tools::ToolSchema {
            name: "shell".to_string(),
            description: "Execute a shell command".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The command to run"
                    }
                },
                "required": ["command"]
            }),
        };
        let decl = format_functiongemma_declaration(&schema);
        assert!(decl.contains("<start_function_declaration>"));
        assert!(decl.contains("<end_function_declaration>"));
        assert!(decl.contains("declaration:shell"));
        assert!(decl.contains("<escape>Execute a shell command<escape>"));
    }

    #[test]
    fn test_fg_format_system_prompt() {
        let schemas = vec![
            crate::tools::ToolSchema {
                name: "shell".to_string(),
                description: "Execute a shell command".to_string(),
                parameters: serde_json::json!({"type":"object","properties":{"command":{"type":"string","description":"cmd"}},"required":["command"]}),
            },
            crate::tools::ToolSchema {
                name: "file_read".to_string(),
                description: "Read a file".to_string(),
                parameters: serde_json::json!({"type":"object","properties":{"path":{"type":"string","description":"file path"}},"required":["path"]}),
            },
        ];
        let prompt = format_functiongemma_system_prompt(&schemas);
        assert!(prompt.contains("You are a model that can do function calling"));
        assert!(prompt.contains("declaration:shell"));
        assert!(prompt.contains("declaration:file_read"));
    }

}
