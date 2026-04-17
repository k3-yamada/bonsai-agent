use std::path::Path;

use anyhow::Result;

use crate::tools::permission::Permission;
use crate::tools::{Tool, ToolResult};

/// ファイル読み取りツール
pub struct FileReadTool;

impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        "ファイルの内容を読み取る。pathパラメータにファイルパスを指定。"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "読み取るファイルのパス" },
                "offset": { "type": "integer", "description": "読み始める行(0始まり)" },
                "limit": { "type": "integer", "description": "最大行数(省略時100)" }
            },
            "required": ["path"]
        })
    }

    fn permission(&self) -> Permission {
        Permission::Auto
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn call(&self, args: serde_json::Value) -> Result<ToolResult> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("'path' パラメータが必要です"))?;

        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
        match std::fs::read_to_string(path) {
            Ok(fc) => {
                let lines: Vec<&str> = fc.lines().collect();
                let total = lines.len();
                let s = offset.min(total);
                let e = (s + limit).min(total);
                let numbered: Vec<String> = lines[s..e].iter().enumerate()
                    .map(|(i, l)| format!("{:4}| {l}", s + i + 1)).collect();
                let hdr = format!("[{path}] ({total}行, 表示:{}-{})", s + 1, e);
                Ok(ToolResult { output: format!("{hdr}\n{}", numbered.join("\n")), success: true })
            },
            Err(e) => Ok(ToolResult {
                output: format!("ファイル読み取りエラー: {e}"),
                success: false,
            }),
        }
    }
}

/// ファイル書き込みツール（全文置換 + search/replace差分適用）
pub struct FileWriteTool;

impl FileWriteTool {
    /// git管理下であれば書き込み前にコミット
    fn git_snapshot(path: &str) -> Option<()> {
        let file_path = Path::new(path);
        if !file_path.exists() {
            return None;
        }

        // gitリポジトリかチェック
        let status = std::process::Command::new("git")
            .args(["rev-parse", "--is-inside-work-tree"])
            .current_dir(file_path.parent().unwrap_or(Path::new(".")))
            .output();

        if let Ok(out) = status
            && out.status.success()
        {
            // 変更があればスナップショットコミット
            let _ = std::process::Command::new("git")
                .args(["add", path])
                .output();
            let _ = std::process::Command::new("git")
                .args([
                    "commit",
                    "-m",
                    &format!(
                        "bonsai: snapshot before edit {}",
                        file_path.file_name().unwrap_or_default().to_string_lossy()
                    ),
                    "--allow-empty",
                ])
                .output();
            return Some(());
        }
        None
    }
}

impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        "file_write"
    }

    fn description(&self) -> &str {
        "ファイルに書き込む。全文置換(content)またはsearch/replace差分適用(old_text/new_text)。git管理下では自動スナップショット。"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "書き込み先のファイルパス" },
                "content": { "type": "string", "description": "全文置換する内容（old_text/new_textと排他）" },
                "old_text": { "type": "string", "description": "置換対象のテキスト" },
                "new_text": { "type": "string", "description": "置換後のテキスト" }
            },
            "required": ["path"]
        })
    }

    fn permission(&self) -> Permission {
        Permission::Confirm
    }

    fn call(&self, args: serde_json::Value) -> Result<ToolResult> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("'path' パラメータが必要です"))?;

        // git-first: 書き込み前にスナップショット
        Self::git_snapshot(path);

        if let Some(content) = args.get("content").and_then(|v| v.as_str()) {
            // 全文置換
            if let Some(parent) = Path::new(path).parent() {
                std::fs::create_dir_all(parent).ok();
            }
            match std::fs::write(path, content) {
                Ok(()) => Ok(ToolResult {
                    output: format!("ファイルを書き込みました: {path}"),
                    success: true,
                }),
                Err(e) => Ok(ToolResult {
                    output: format!("書き込みエラー: {e}"),
                    success: false,
                }),
            }
        } else if let (Some(old_text), Some(new_text)) = (
            args.get("old_text").and_then(|v| v.as_str()),
            args.get("new_text").and_then(|v| v.as_str()),
        ) {
            // search/replace差分適用
            match std::fs::read_to_string(path) {
                Ok(current) => {
                    let (updated, warning) = if current.contains(old_text) {
                        (current.replacen(old_text, new_text, 1), None)
                    } else if let Some((fuzzy_result, msg)) =
                        fuzzy_find_replace(&current, old_text, new_text)
                    {
                        (fuzzy_result, Some(msg))
                    } else {
                        return Ok(ToolResult {
                            output: format!("置換対象テキストがファイル内に見つかりません: {path}"),
                            success: false,
                        });
                    };
                    match std::fs::write(path, &updated) {
                        Ok(()) => {
                            let msg = if let Some(w) = warning {
                                format!("差分適用しました（{w}）: {path}")
                            } else {
                                format!("差分適用しました: {path}")
                            };
                            Ok(ToolResult {
                                output: msg,
                                success: true,
                            })
                        }
                        Err(e) => Ok(ToolResult {
                            output: format!("書き込みエラー: {e}"),
                            success: false,
                        }),
                    }
                }
                Err(e) => Ok(ToolResult {
                    output: format!("ファイル読み取りエラー: {e}"),
                    success: false,
                }),
            }
        } else {
            Ok(ToolResult {
                output: "'content' または 'old_text'+'new_text' のいずれかが必要です".to_string(),
                success: false,
            })
        }
    }
}


/// 空白を正規化（連続空白→単一スペース、先頭末尾trim）
fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 7段階fuzzyマッチで置換を試みる（hermes-agentパターン準拠）。
/// 完全一致失敗時のフォールバック。成功時は (置換後テキスト, 警告メッセージ) を返す。
///
/// 戦略:
/// 1. 空白正規化（連続空白→単一スペース）
/// 2. Trim一致（先頭/末尾空白除去）
/// 3. インデント柔軟（各行の先頭空白を無視して比較）
/// 4. Unicode正規化（スマートクォート→ASCII、全角→半角）
/// 5. エスケープ正規化（\\n→改行、\\t→タブ）
/// 6. Blockアンカー（先頭行+末尾行アンカー、中間行は類似度50%で一致）
/// 7. 境界Trim（old_textの最初/最後の空行を除去して再検索）
fn fuzzy_find_replace(content: &str, old_text: &str, new_text: &str) -> Option<(String, String)> {
    if old_text.trim().is_empty() {
        return None;
    }

    // 戦略1: 空白正規化
    if let Some(r) = try_whitespace_normalized(content, old_text, new_text) {
        return Some((r, "模糊一致（空白正規化）".into()));
    }
    // 戦略2: Trim一致
    if let Some(r) = try_trimmed(content, old_text, new_text) {
        return Some((r, "模糊一致（先頭/末尾Trim）".into()));
    }
    // 戦略3: インデント柔軟
    if let Some(r) = try_indent_flexible(content, old_text, new_text) {
        return Some((r, "模糊一致（インデント差異）".into()));
    }
    // 戦略4: Unicode正規化
    if let Some(r) = try_unicode_normalized(content, old_text, new_text) {
        return Some((r, "模糊一致（Unicode正規化）".into()));
    }
    // 戦略5: エスケープ正規化
    if let Some(r) = try_escape_normalized(content, old_text, new_text) {
        return Some((r, "模糊一致（エスケープ正規化）".into()));
    }
    // 戦略6: Blockアンカー
    if let Some(r) = try_block_anchor(content, old_text, new_text) {
        return Some((r, "模糊一致（Blockアンカー）".into()));
    }
    // 戦略7: 境界Trim
    if let Some(r) = try_boundary_trim(content, old_text, new_text) {
        return Some((r, "模糊一致（境界Trim）".into()));
    }
    None
}

/// 戦略1: 空白正規化（既存ロジック）
fn try_whitespace_normalized(content: &str, old_text: &str, new_text: &str) -> Option<String> {
    let content_lines: Vec<&str> = content.lines().collect();
    let old_lines: Vec<&str> = old_text.lines().collect();
    if old_lines.is_empty() {
        return None;
    }
    let norm_first = normalize_whitespace(old_lines[0]);
    for (i, cl) in content_lines.iter().enumerate() {
        if normalize_whitespace(cl) == norm_first
            && i + old_lines.len() <= content_lines.len()
            && old_lines.iter().enumerate().all(|(j, ol)| {
                normalize_whitespace(content_lines[i + j]) == normalize_whitespace(ol)
            })
        {
            return Some(replace_lines(content, &content_lines, i, old_lines.len(), new_text));
        }
    }
    None
}

/// 戦略2: Trim一致
fn try_trimmed(content: &str, old_text: &str, new_text: &str) -> Option<String> {
    let trimmed = old_text.trim();
    if trimmed != old_text && content.contains(trimmed) && content.matches(trimmed).count() == 1 {
        Some(content.replacen(trimmed, new_text.trim(), 1))
    } else {
        None
    }
}

/// 戦略3: インデント柔軟（各行の先頭空白を無視）
fn try_indent_flexible(content: &str, old_text: &str, new_text: &str) -> Option<String> {
    let content_lines: Vec<&str> = content.lines().collect();
    let old_lines: Vec<&str> = old_text.lines().collect();
    if old_lines.is_empty() {
        return None;
    }
    let stripped_first = old_lines[0].trim_start();
    for (i, cl) in content_lines.iter().enumerate() {
        if cl.trim_start() == stripped_first
            && i + old_lines.len() <= content_lines.len()
            && old_lines.iter().enumerate().all(|(j, ol)| {
                content_lines[i + j].trim_start() == ol.trim_start()
            })
        {
            return Some(replace_lines(content, &content_lines, i, old_lines.len(), new_text));
        }
    }
    None
}

/// 戦略4: Unicode正規化（スマートクォート→ASCII、全角英数→半角）
fn try_unicode_normalized(content: &str, old_text: &str, new_text: &str) -> Option<String> {
    let norm_old = normalize_unicode(old_text);
    let norm_content = normalize_unicode(content);
    if norm_old == normalize_unicode(new_text) {
        return None; // old == new なら意味なし
    }
    if norm_content.contains(&norm_old) && norm_content.matches(&norm_old).count() == 1 {
        // 元テキストで対応位置を行単位で検索
        let content_lines: Vec<&str> = content.lines().collect();
        let old_lines: Vec<&str> = old_text.lines().collect();
        if old_lines.is_empty() {
            return None;
        }
        let norm_first = normalize_unicode(old_lines[0]);
        for (i, cl) in content_lines.iter().enumerate() {
            if normalize_unicode(cl) == norm_first
                && i + old_lines.len() <= content_lines.len()
                && old_lines.iter().enumerate().all(|(j, ol)| {
                    normalize_unicode(content_lines[i + j]) == normalize_unicode(ol)
                })
            {
                return Some(replace_lines(content, &content_lines, i, old_lines.len(), new_text));
            }
        }
    }
    None
}

/// 戦略5: エスケープ正規化（\n→改行、\t→タブ）
fn try_escape_normalized(content: &str, old_text: &str, new_text: &str) -> Option<String> {
    let unescaped = old_text.replace("\\n", "\n").replace("\\t", "\t");
    if unescaped != old_text && content.contains(&unescaped) && content.matches(&unescaped).count() == 1 {
        Some(content.replacen(&unescaped, new_text, 1))
    } else {
        None
    }
}

/// 戦略6: Blockアンカー（先頭行+末尾行が一致、中間行は50%以上の行が一致）
fn try_block_anchor(content: &str, old_text: &str, new_text: &str) -> Option<String> {
    let content_lines: Vec<&str> = content.lines().collect();
    let old_lines: Vec<&str> = old_text.lines().collect();
    if old_lines.len() < 3 {
        return None; // 3行未満はアンカー意味なし
    }
    let first = normalize_whitespace(old_lines[0]);
    let last = normalize_whitespace(old_lines[old_lines.len() - 1]);
    let middle_count = old_lines.len() - 2;
    let threshold = (middle_count as f64 * 0.5).ceil() as usize;

    for (i, cl) in content_lines.iter().enumerate() {
        if normalize_whitespace(cl) != first {
            continue;
        }
        let end = i + old_lines.len();
        if end > content_lines.len() {
            continue;
        }
        if normalize_whitespace(content_lines[end - 1]) != last {
            continue;
        }
        // 中間行の類似度チェック
        let matched = (1..old_lines.len() - 1)
            .filter(|&j| {
                normalize_whitespace(content_lines[i + j]) == normalize_whitespace(old_lines[j])
            })
            .count();
        if matched >= threshold {
            return Some(replace_lines(content, &content_lines, i, old_lines.len(), new_text));
        }
    }
    None
}

/// 戦略7: 境界Trim（old_textの先頭/末尾の空行を除去して再検索）
fn try_boundary_trim(content: &str, old_text: &str, new_text: &str) -> Option<String> {
    let old_lines: Vec<&str> = old_text.lines().collect();
    let start = old_lines.iter().position(|l| !l.trim().is_empty())?;
    let end = old_lines.iter().rposition(|l| !l.trim().is_empty())?;
    if start == 0 && end == old_lines.len() - 1 {
        return None; // 境界空行なし
    }
    let trimmed: String = old_lines[start..=end].join("\n");
    if content.contains(&trimmed) && content.matches(&trimmed).count() == 1 {
        Some(content.replacen(&trimmed, new_text.trim(), 1))
    } else {
        None
    }
}

/// Unicode正規化: スマートクォート→ASCII、全角英数→半角
fn normalize_unicode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            '\u{2014}' => '-',
            '\u{2013}' => '-',
            '\u{2026}' => '.',
            // 全角英数→半角
            '\u{FF01}'..='\u{FF5E}' => ((c as u32 - 0xFF01 + 0x21) as u8) as char,
            _ => c,
        })
        .collect()
}

/// 行置換ヘルパー（末尾改行を保持）
fn replace_lines(content: &str, content_lines: &[&str], start: usize, count: usize, new_text: &str) -> String {
    let mut result_lines = Vec::new();
    result_lines.extend_from_slice(&content_lines[..start]);
    for new_line in new_text.lines() {
        result_lines.push(new_line);
    }
    result_lines.extend_from_slice(&content_lines[start + count..]);
    let result = result_lines.join("\n");
    if content.ends_with('\n') && !result.ends_with('\n') {
        result + "\n"
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_path(name: &str) -> String {
        format!("/tmp/bonsai-test-{}-{}", name, uuid::Uuid::new_v4())
    }

    // FileReadTool
    #[test]
    fn test_read_existing_file() {
        let path = temp_path("read");
        fs::write(&path, "hello world").unwrap();

        let tool = FileReadTool;
        let result = tool.call(serde_json::json!({"path": path})).unwrap();
        assert!(result.success);
        assert!(result.output.contains("hello world"), "出力に内容が含まれること");

        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_read_nonexistent_file() {
        let tool = FileReadTool;
        let result = tool
            .call(serde_json::json!({"path": "/tmp/nonexistent-bonsai-xyz"}))
            .unwrap();
        assert!(!result.success);
        assert!(result.output.contains("エラー"));
    }

    #[test]
    fn test_read_missing_param() {
        let tool = FileReadTool;
        let result = tool.call(serde_json::json!({}));
        assert!(result.is_err());
    }

    // FileWriteTool — 全文置換
    #[test]
    fn test_write_full_content() {
        let path = temp_path("write-full");
        let tool = FileWriteTool;

        let result = tool
            .call(serde_json::json!({"path": &path, "content": "new content"}))
            .unwrap();
        assert!(result.success);
        assert_eq!(fs::read_to_string(&path).unwrap(), "new content");

        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_write_creates_parent_dirs() {
        let path = format!(
            "/tmp/bonsai-test-nested-{}/sub/file.txt",
            uuid::Uuid::new_v4()
        );
        let tool = FileWriteTool;

        let result = tool
            .call(serde_json::json!({"path": &path, "content": "test"}))
            .unwrap();
        assert!(result.success);

        // cleanup
        if let Some(parent) = Path::new(&path).parent() {
            fs::remove_dir_all(parent.parent().unwrap()).ok();
        }
    }

    // FileWriteTool — 差分適用
    #[test]
    fn test_write_search_replace() {
        let path = temp_path("write-diff");
        fs::write(&path, "hello world").unwrap();

        let tool = FileWriteTool;
        let result = tool
            .call(serde_json::json!({
                "path": &path,
                "old_text": "world",
                "new_text": "rust"
            }))
            .unwrap();
        assert!(result.success);
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello rust");

        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_write_search_replace_not_found() {
        let path = temp_path("write-notfound");
        fs::write(&path, "hello world").unwrap();

        let tool = FileWriteTool;
        let result = tool
            .call(serde_json::json!({
                "path": &path,
                "old_text": "xyz",
                "new_text": "abc"
            }))
            .unwrap();
        assert!(!result.success);
        assert!(result.output.contains("見つかりません"));

        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_write_no_content_params() {
        let tool = FileWriteTool;
        let result = tool.call(serde_json::json!({"path": "/tmp/x"})).unwrap();
        assert!(!result.success);
    }

    // メタデータ
    #[test]
    fn test_file_read_permission() {
        assert_eq!(FileReadTool.permission(), Permission::Auto);
    }

    #[test]
    fn test_file_write_permission() {
        assert_eq!(FileWriteTool.permission(), Permission::Confirm);
    }

    // --- fuzzyマッチテスト ---

    #[test]
    fn test_normalize_whitespace() {
        assert_eq!(normalize_whitespace("  hello   world  "), "hello world");
    }

    #[test]
    fn test_fuzzy_replace_whitespace_difference() {
        let content = "alpha   beta";
        let old = "  alpha beta  ";
        let result = fuzzy_find_replace(content, old, "gamma");
        assert!(result.is_some());
        let (_, w) = result.unwrap();
        assert!(w.contains("模糊"));
    }

    #[test]
    fn test_fuzzy_replace_trailing_whitespace() {
        let content = "hello world";
        let old_text = "  hello world  ";
        let new_text = "hello rust";
        let result = fuzzy_find_replace(content, old_text, new_text);
        assert!(result.is_some());
    }

    #[test]
    fn test_fuzzy_replace_exact_still_preferred() {
        let path = temp_path("fuzzy-exact");
        fs::write(&path, "hello world").unwrap();
        let tool = FileWriteTool;
        let result = tool
            .call(serde_json::json!({
                "path": &path,
                "old_text": "hello",
                "new_text": "greet"
            }))
            .unwrap();
        assert!(result.success);
        assert!(!result.output.contains("模糊一致"));
        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_fuzzy_replace_not_found_returns_none() {
        let content = "hello world";
        let old_text = "completely different text";
        let result = fuzzy_find_replace(content, old_text, "new");
        assert!(result.is_none());
    }

    #[test]
    fn test_read_with_line_numbers() {
        let path = temp_path("lines");
        fs::write(&path, "line1\nline2\nline3").unwrap();
        let tool = FileReadTool;
        let result = tool.call(serde_json::json!({"path": &path})).unwrap();
        assert!(result.output.contains("1|"), "行番号が含まれること");
        assert!(result.output.contains("line1"));
        assert!(result.output.contains("3行"));
        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_read_with_offset_limit() {
        let path = temp_path("offset");
        fs::write(&path, "a\nb\nc\nd\ne").unwrap();
        let tool = FileReadTool;
        let result = tool.call(serde_json::json!({"path": &path, "offset": 1, "limit": 2})).unwrap();
        assert!(result.output.contains("b"), "offset=1でbから開始");
        assert!(result.output.contains("c"), "limit=2でcまで含む");
        assert!(!result.output.contains("| a"), "aは含まない");
        fs::remove_file(&path).ok();
    }
}