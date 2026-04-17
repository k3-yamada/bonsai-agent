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


/// ツール引数の型強制（hermes-agent知見: LLMが数値を文字列で返す問題）
///
/// JSON値を走査し、数値文字列を数値に、bool文字列をboolに変換。
/// 例: `{"count": "42"}` → `{"count": 42}`
/// 例: `{"flag": "true"}` → `{"flag": true}`
pub fn coerce_tool_arguments(args: &mut serde_json::Value) {
    match args {
        serde_json::Value::Object(map) => {
            for (_key, val) in map.iter_mut() {
                coerce_value(val);
            }
        }
        _ => {}
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
        if let Ok(n) = s.parse::<f64>() {
            if s.contains('.') {
                *val = serde_json::json!(n);
            }
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
}
