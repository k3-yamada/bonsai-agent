//! MiniCPM5 native XML tool-call形式のフォールバック抽出（Issue #13）。
//!
//! MiniCPM5 の chat template（`chat_template.jinja`）は tool call を
//! `<function name="...">＜param name="...">value</param></function>` の
//! XML形式で学習している。一方 `agent::parse` は system prompt でツール
//! スキーマを提示し `<tool_call>{JSON}</tool_call>` 形式を期待する
//! （ADR-011: chat templateはbackend委譲、tool callのpost-hoc抽出は
//! harness側）。本モジュールはモデルが native XML形式で出力した場合の
//! フォールバック抽出を提供する。
//!
//! `BONSAI_XML_TOOLCALL_FALLBACK=1` で opt-in（production default = OFF）。
//! Lab paired evidence (ADR-003) でtool call成功率の改善がACCEPTされる
//! までは既定OFFを維持する。

use anyhow::Result;

use crate::domain::conversation::{ParsedOutput, ToolCall};

/// `BONSAI_XML_TOOLCALL_FALLBACK=1` (or "true"、case-insensitive) で
/// XML tool-callフォールバック抽出を opt-in 有効化する。
///
/// production default = env unset = false（既存の `<tool_call>{JSON}</tool_call>`
/// 抽出のみ）。
pub(crate) fn is_xml_toolcall_fallback_enabled() -> bool {
    std::env::var("BONSAI_XML_TOOLCALL_FALLBACK")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// タグ内の `attr="value"` 属性値を抽出する。
fn extract_xml_attr(tag: &str, attr: &str) -> Option<String> {
    let pattern = format!("{attr}=\"");
    let start = tag.find(&pattern)? + pattern.len();
    let end = tag[start..].find('"')?;
    Some(tag[start..start + end].to_string())
}

/// `<![CDATA[...]]>` で囲まれたパラメータ値からCDATAマーカーを除去する。
/// CDATAでない場合はそのまま返す。
fn strip_cdata(value: &str) -> String {
    const CDATA_PREFIX: &str = "<![CDATA[";
    const CDATA_SUFFIX: &str = "]]>";
    value
        .strip_prefix(CDATA_PREFIX)
        .and_then(|s| s.strip_suffix(CDATA_SUFFIX))
        .unwrap_or(value)
        .to_string()
}

/// `<function` タグの開始位置を、直後の文字が空白（属性が続く）または `>`
/// （属性なし）であることを確認したうえで探す。`<functional>` や
/// `<function_call>` のような紛らわしい文字列に誤マッチしないための境界判定
/// （Issue #13 P2-1）。
pub(crate) fn find_function_tag_start(haystack: &str) -> Option<usize> {
    const TAG: &str = "<function";
    let mut search_from = 0;
    while let Some(rel_idx) = haystack[search_from..].find(TAG) {
        let idx = search_from + rel_idx;
        let boundary_char = haystack[idx + TAG.len()..].chars().next();
        if matches!(boundary_char, Some(' ') | Some('>')) {
            return Some(idx);
        }
        // 境界不一致（<functional> 等）: この出現をスキップして次を探す
        search_from = idx + TAG.len();
    }
    None
}

/// `haystack` に境界判定込みの `<function` タグが含まれるかどうか。
pub(crate) fn contains_function_tag(haystack: &str) -> bool {
    find_function_tag_start(haystack).is_some()
}

/// CDATA区間（`<![CDATA[` ～ `]]>`）をスキップしながら、`haystack` 内で
/// `needle` の最初の出現位置を探す。CDATA区間内に出現する `needle` は
/// 候補として扱わない（Issue #13 P2-2: CDATA内の `</param>` / `</function>`
/// 文字列で抽出が壊れる不具合への対応）。
fn find_outside_cdata(haystack: &str, needle: &str) -> Option<usize> {
    const CDATA_START: &str = "<![CDATA[";
    const CDATA_END: &str = "]]>";

    let mut pos = 0;
    loop {
        let next_needle = haystack[pos..].find(needle).map(|i| pos + i);
        let next_cdata_start = haystack[pos..].find(CDATA_START).map(|i| pos + i);

        match (next_needle, next_cdata_start) {
            (Some(needle_idx), Some(cdata_idx)) if cdata_idx < needle_idx => {
                // needleより先にCDATA区間が始まる場合は区間全体を読み飛ばす
                let content_start = cdata_idx + CDATA_START.len();
                let after_cdata = haystack[content_start..]
                    .find(CDATA_END)
                    .map(|i| content_start + i + CDATA_END.len())
                    .unwrap_or(haystack.len());
                pos = after_cdata;
            }
            (Some(needle_idx), _) => return Some(needle_idx),
            (None, _) => return None,
        }
    }
}

/// `<function name="...">...</function>` ブロック単体をToolCallに変換する。
fn parse_xml_function_call(block: &str) -> Result<ToolCall> {
    let tag_end = block
        .find('>')
        .ok_or_else(|| anyhow::anyhow!("<function> タグが不正です（'>' が見つかりません）"))?;
    let open_tag = &block[..tag_end];
    let name = extract_xml_attr(open_tag, "name").ok_or_else(|| {
        anyhow::anyhow!("<function> タグに name 属性が見つかりません: {open_tag}>")
    })?;

    let close_tag = "</function>";
    let body_start = tag_end + 1;
    let body_end = find_outside_cdata(block, close_tag)
        .ok_or_else(|| anyhow::anyhow!("</function> 閉じタグが見つかりません"))?;
    let body = &block[body_start..body_end];

    let mut arguments = serde_json::Map::new();
    let mut remaining = body;
    while let Some(p_start) = remaining.find("<param") {
        let p_tag_end = p_start
            + remaining[p_start..]
                .find('>')
                .ok_or_else(|| anyhow::anyhow!("<param> タグが不正です（'>' が見つかりません）"))?;
        let p_open_tag = &remaining[p_start..p_tag_end];
        let p_name = extract_xml_attr(p_open_tag, "name").ok_or_else(|| {
            anyhow::anyhow!("<param> タグに name 属性が見つかりません: {p_open_tag}>")
        })?;

        let p_close_tag = "</param>";
        let p_body_start = p_tag_end + 1;
        let p_body_end_rel = find_outside_cdata(&remaining[p_body_start..], p_close_tag)
            .ok_or_else(|| {
                anyhow::anyhow!("</param> 閉じタグが見つかりません（param: {p_name}）")
            })?;
        let p_body_end = p_body_start + p_body_end_rel;

        let raw_value = remaining[p_body_start..p_body_end].trim();
        arguments.insert(p_name, serde_json::Value::String(strip_cdata(raw_value)));

        remaining = &remaining[p_body_end + p_close_tag.len()..];
    }

    Ok(ToolCall {
        name,
        arguments: serde_json::Value::Object(arguments),
    })
}

/// MiniCPM5 native XML tool-call形式 (`<function name=...><param name=...>`) を
/// フォールバック抽出する（Issue #13、`BONSAI_XML_TOOLCALL_FALLBACK=1` でopt-in）。
///
/// `<think>` ブロックの扱いは `agent::parse::parse_assistant_output` と同じ。
/// 複数の `<function>` 呼び出しは `<tool_sep>` 区切りで連結される
/// （MiniCPM5 chat_template.jinja 準拠）。この関数は
/// `parse_assistant_output` から、入力に `<tool_call>` が一切存在しない
/// 場合のみ呼び出される（既存JSON抽出との優先順位: `<tool_call>` が優先）。
pub(crate) fn parse_xml_function_output(normalized: &str) -> Result<ParsedOutput> {
    let mut thinking = None;
    let mut tool_calls = Vec::new();
    let mut text_parts = Vec::new();
    let mut remaining = normalized;

    while !remaining.is_empty() {
        if let Some(think_start) = remaining.find("<think>") {
            let before = remaining[..think_start].trim();
            if !before.is_empty() {
                text_parts.push(before.to_string());
            }

            if let Some(think_end) = remaining[think_start..].find("</think>") {
                let think_content = &remaining[think_start + 7..think_start + think_end];
                thinking = Some(think_content.trim().to_string());
                remaining = &remaining[think_start + think_end + 8..];
            } else {
                let think_content = &remaining[think_start + 7..];
                thinking = Some(think_content.trim().to_string());
                remaining = "";
            }
        } else if let Some(fn_start) = find_function_tag_start(remaining) {
            let before = remaining[..fn_start].trim();
            if !before.is_empty() {
                text_parts.push(before.to_string());
            }

            let close_tag = "</function>";
            if let Some(fn_end_rel) = find_outside_cdata(&remaining[fn_start..], close_tag) {
                let fn_end = fn_start + fn_end_rel + close_tag.len();
                let fn_block = &remaining[fn_start..fn_end];
                tool_calls.push(parse_xml_function_call(fn_block)?);

                remaining = remaining[fn_end..].trim_start();
                if let Some(rest) = remaining.strip_prefix("<tool_sep>") {
                    remaining = rest;
                }
            } else {
                anyhow::bail!("</function> 閉じタグが見つかりません");
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
        Some(text_parts.join("\n"))
    };

    Ok(ParsedOutput {
        thinking,
        tool_calls,
        text,
    })
}

#[cfg(test)]
mod tests {
    use crate::agent::parse::parse_assistant_output;

    // env mutation race を避けるため module-local Mutex で serialize する
    // (memory/decay.rs DECAY_TEST_LOCK と同パターン)。
    static XML_TOOLCALL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn reset_xml_toolcall_env() {
        unsafe {
            std::env::remove_var("BONSAI_XML_TOOLCALL_FALLBACK");
        }
    }

    fn enable_xml_toolcall_fallback() {
        unsafe {
            std::env::set_var("BONSAI_XML_TOOLCALL_FALLBACK", "1");
        }
    }

    #[test]
    fn test_xml_function_call_disabled_by_default() {
        let _g = XML_TOOLCALL_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_xml_toolcall_env();
        let input = r#"<function name="get_weather"><param name="city">Tokyo</param></function>"#;
        let result = parse_assistant_output(input).unwrap();
        assert!(
            result.tool_calls.is_empty(),
            "env未設定時はXML fallbackが無効でtool_callが抽出されない"
        );
        assert!(
            result.text.unwrap().contains("<function"),
            "env未設定時は<function>ブロックがプレーンテキストとして残る"
        );
    }

    #[test]
    fn test_xml_function_call_simple() {
        let _g = XML_TOOLCALL_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_xml_toolcall_env();
        enable_xml_toolcall_fallback();
        let input = r#"<function name="get_weather"><param name="city">Tokyo</param></function>"#;
        let result = parse_assistant_output(input);
        reset_xml_toolcall_env();
        let result = result.unwrap();
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].name, "get_weather");
        assert_eq!(result.tool_calls[0].arguments["city"], "Tokyo");
    }

    #[test]
    fn test_xml_function_call_multiple_params() {
        let _g = XML_TOOLCALL_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_xml_toolcall_env();
        enable_xml_toolcall_fallback();
        let input = r#"<function name="shell"><param name="command">ls -la</param><param name="timeout">30</param></function>"#;
        let result = parse_assistant_output(input);
        reset_xml_toolcall_env();
        let result = result.unwrap();
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].name, "shell");
        assert_eq!(result.tool_calls[0].arguments["command"], "ls -la");
        assert_eq!(result.tool_calls[0].arguments["timeout"], "30");
    }

    #[test]
    fn test_xml_function_call_cdata_param_value() {
        let _g = XML_TOOLCALL_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_xml_toolcall_env();
        enable_xml_toolcall_fallback();
        let input = "<function name=\"file_write\"><param name=\"content\"><![CDATA[line1\n<tag>not xml</tag>\nline2]]></param></function>";
        let result = parse_assistant_output(input);
        reset_xml_toolcall_env();
        let result = result.unwrap();
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].name, "file_write");
        assert_eq!(
            result.tool_calls[0].arguments["content"],
            "line1\n<tag>not xml</tag>\nline2"
        );
    }

    #[test]
    fn test_xml_function_call_multiple_via_tool_sep() {
        let _g = XML_TOOLCALL_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_xml_toolcall_env();
        enable_xml_toolcall_fallback();
        let input = r#"<function name="shell"><param name="command">ls</param></function><tool_sep><function name="file_read"><param name="path">README.md</param></function>"#;
        let result = parse_assistant_output(input);
        reset_xml_toolcall_env();
        let result = result.unwrap();
        assert_eq!(result.tool_calls.len(), 2);
        assert_eq!(result.tool_calls[0].name, "shell");
        assert_eq!(result.tool_calls[0].arguments["command"], "ls");
        assert_eq!(result.tool_calls[1].name, "file_read");
        assert_eq!(result.tool_calls[1].arguments["path"], "README.md");
    }

    #[test]
    fn test_xml_function_call_coexists_with_tool_call_json_prefers_tool_call() {
        let _g = XML_TOOLCALL_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_xml_toolcall_env();
        enable_xml_toolcall_fallback();
        let input = r#"<tool_call>{"name":"shell","arguments":{"command":"date"}}</tool_call><function name="ignored"><param name="x">y</param></function>"#;
        let result = parse_assistant_output(input);
        reset_xml_toolcall_env();
        let result = result.unwrap();
        assert_eq!(
            result.tool_calls.len(),
            1,
            "tool_callが存在する場合はXML fallbackを無視しJSON抽出を優先する"
        );
        assert_eq!(result.tool_calls[0].name, "shell");
        let text = result.text.unwrap();
        assert!(
            text.contains(r#"<function name="ignored">"#),
            "無視された<function>ブロックはプレーンテキストとして残る"
        );
    }

    #[test]
    fn test_xml_function_call_only_tool_call_present_normal_operation() {
        // fallback有効時でも<tool_call>のみの入力は既存動作のまま変わらない
        let _g = XML_TOOLCALL_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_xml_toolcall_env();
        enable_xml_toolcall_fallback();
        let input = r#"<tool_call>{"name":"shell","arguments":{"command":"date"}}</tool_call>"#;
        let result = parse_assistant_output(input);
        reset_xml_toolcall_env();
        let result = result.unwrap();
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].name, "shell");
        assert_eq!(result.tool_calls[0].arguments["command"], "date");
    }

    #[test]
    fn test_xml_function_call_missing_function_closing_tag_errors() {
        let _g = XML_TOOLCALL_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_xml_toolcall_env();
        enable_xml_toolcall_fallback();
        let input = r#"<function name="shell"><param name="command">ls</param>"#; // </function> 欠損
        let result = parse_assistant_output(input);
        reset_xml_toolcall_env();
        assert!(result.is_err());
    }

    #[test]
    fn test_xml_function_call_missing_param_closing_tag_errors() {
        let _g = XML_TOOLCALL_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_xml_toolcall_env();
        enable_xml_toolcall_fallback();
        let input = r#"<function name="shell"><param name="command">ls</function>"#; // </param> 欠損
        let result = parse_assistant_output(input);
        reset_xml_toolcall_env();
        assert!(result.is_err());
    }

    #[test]
    fn test_xml_function_call_missing_name_attribute_errors() {
        let _g = XML_TOOLCALL_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_xml_toolcall_env();
        enable_xml_toolcall_fallback();
        let input = r#"<function ><param name="command">ls</param></function>"#; // name属性欠損
        let result = parse_assistant_output(input);
        reset_xml_toolcall_env();
        assert!(result.is_err());
    }

    #[test]
    fn test_xml_function_call_with_think_block() {
        let _g = XML_TOOLCALL_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_xml_toolcall_env();
        enable_xml_toolcall_fallback();
        let input = r#"<think>天気を調べよう</think><function name="get_weather"><param name="city">Osaka</param></function>"#;
        let result = parse_assistant_output(input);
        reset_xml_toolcall_env();
        let result = result.unwrap();
        assert_eq!(result.thinking, Some("天気を調べよう".to_string()));
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].name, "get_weather");
        assert_eq!(result.tool_calls[0].arguments["city"], "Osaka");
    }

    // --- P2-1: `<function` の前方一致が長いタグ名に誤マッチする不具合の回帰テスト ---

    #[test]
    fn test_xml_function_tag_boundary_ignores_functional_tag() {
        let _g = XML_TOOLCALL_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_xml_toolcall_env();
        enable_xml_toolcall_fallback();
        let input = "<functional>これはfunction callではない</functional>";
        let result = parse_assistant_output(input);
        reset_xml_toolcall_env();
        let result = result.unwrap();
        assert!(
            result.tool_calls.is_empty(),
            "<functional>は<function>タグの境界（直後が空白または'>'）に一致しないためfallbackが発動してはならない"
        );
        assert_eq!(
            result.text.unwrap(),
            "<functional>これはfunction callではない</functional>"
        );
    }

    #[test]
    fn test_xml_function_tag_boundary_ignores_function_call_tag() {
        let _g = XML_TOOLCALL_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_xml_toolcall_env();
        enable_xml_toolcall_fallback();
        let input = "<function_call>foo</function_call>";
        let result = parse_assistant_output(input);
        reset_xml_toolcall_env();
        let result = result.unwrap();
        assert!(
            result.tool_calls.is_empty(),
            "<function_call>は<function>タグの境界に一致しないためfallbackが発動してはならない"
        );
        assert_eq!(result.text.unwrap(), "<function_call>foo</function_call>");
    }

    #[test]
    fn test_xml_function_tag_boundary_still_matches_real_function_tag() {
        // 境界判定を追加しても、正規の<function name="...">は引き続き検出されること
        let _g = XML_TOOLCALL_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_xml_toolcall_env();
        enable_xml_toolcall_fallback();
        let input = r#"<function name="get_weather"><param name="city">Tokyo</param></function>"#;
        let result = parse_assistant_output(input);
        reset_xml_toolcall_env();
        let result = result.unwrap();
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].name, "get_weather");
    }

    // --- P2-2: CDATA内の`</param>`/`</function>`で抽出が壊れる不具合の回帰テスト ---

    #[test]
    fn test_xml_function_call_cdata_containing_closing_tag_substrings() {
        let _g = XML_TOOLCALL_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_xml_toolcall_env();
        enable_xml_toolcall_fallback();
        let input = "<function name=\"file_write\"><param name=\"content\">\
            <![CDATA[before </param> middle </function> after]]>\
            </param></function>";
        let result = parse_assistant_output(input);
        reset_xml_toolcall_env();
        let result = result.unwrap();
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].name, "file_write");
        assert_eq!(
            result.tool_calls[0].arguments["content"],
            "before </param> middle </function> after"
        );
    }

    #[test]
    fn test_xml_function_call_cdata_closing_tag_substring_with_trailing_param() {
        // CDATA内の</param>で誤って区切られていた場合、後続のparamが誤解釈されないこと
        let _g = XML_TOOLCALL_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_xml_toolcall_env();
        enable_xml_toolcall_fallback();
        let input = "<function name=\"shell\">\
            <param name=\"command\"><![CDATA[echo </param>fake]]></param>\
            <param name=\"timeout\">30</param>\
            </function>";
        let result = parse_assistant_output(input);
        reset_xml_toolcall_env();
        let result = result.unwrap();
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(
            result.tool_calls[0].arguments["command"],
            "echo </param>fake"
        );
        assert_eq!(result.tool_calls[0].arguments["timeout"], "30");
    }
}
