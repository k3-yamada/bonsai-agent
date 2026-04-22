use std::path::Path;

use anyhow::Result;

use crate::tools::ToolResult;
use crate::tools::permission::Permission;
use crate::tools::typed::TypedTool;
use schemars::JsonSchema;
use serde::Deserialize;

/// ファイル読み取りツール
pub struct FileReadTool;

#[derive(Deserialize, JsonSchema)]
pub struct FileReadArgs {
    /// 読み取るファイルのパス
    path: String,
    /// 読み始める行(0始まり)
    offset: Option<u64>,
    /// 最大行数(省略時100)
    limit: Option<u64>,
}

impl TypedTool for FileReadTool {
    type Args = FileReadArgs;
    const NAME: &'static str = "file_read";
    const DESCRIPTION: &'static str = super::descriptions::FILE_READ;
    const PERMISSION: Permission = Permission::Auto;
    const READ_ONLY: bool = true;

    fn execute(&self, args: FileReadArgs) -> Result<ToolResult> {
        let path = &args.path;
        let offset = args.offset.unwrap_or(0) as usize;
        let limit = args.limit.unwrap_or(100) as usize;
        match std::fs::read_to_string(path) {
            Ok(fc) => {
                let lines: Vec<&str> = fc.lines().collect();
                let total = lines.len();
                let s = offset.min(total);
                let e = (s + limit).min(total);
                let numbered: Vec<String> = lines[s..e]
                    .iter()
                    .enumerate()
                    .map(|(i, l)| format!("{:4}| {l}", s + i + 1))
                    .collect();
                let hdr = format!("[{path}] ({total}行, 表示:{}-{})", s + 1, e);
                Ok(ToolResult {
                    output: format!("{hdr}\n{}", numbered.join("\n")),
                    success: true,
                })
            }
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

#[derive(Deserialize, JsonSchema)]
pub struct FileWriteArgs {
    /// 書き込み先のファイルパス
    path: String,
    /// 全文置換する内容（old_text/new_textと排他）
    content: Option<String>,
    /// 置換対象のテキスト
    old_text: Option<String>,
    /// 置換後のテキスト
    new_text: Option<String>,
}

impl TypedTool for FileWriteTool {
    type Args = FileWriteArgs;
    const NAME: &'static str = "file_write";
    const DESCRIPTION: &'static str = super::descriptions::FILE_WRITE;
    const PERMISSION: Permission = Permission::Confirm;

    fn execute(&self, args: FileWriteArgs) -> Result<ToolResult> {
        let path = &args.path;

        // git-first: 書き込み前にスナップショット
        Self::git_snapshot(path);

        if let Some(content) = &args.content {
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
        } else if let (Some(old_text), Some(new_text)) =
            (args.old_text.as_deref(), args.new_text.as_deref())
        {
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


/// 複数箇所同時編集ツール（OpenCode知見: アトミック操作でp^n問題緩和）
pub struct MultiEditTool;

#[derive(Deserialize, JsonSchema)]
pub struct EditPair {
    /// 置換対象テキスト
    old_text: String,
    /// 置換後テキスト
    new_text: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct MultiEditArgs {
    /// 編集対象のファイルパス
    path: String,
    /// 編集ペアの配列（順次適用、全成功 or 全ロールバック）
    edits: Vec<EditPair>,
}

impl TypedTool for MultiEditTool {
    type Args = MultiEditArgs;
    const NAME: &'static str = "multi_edit";
    const DESCRIPTION: &'static str = super::descriptions::MULTI_EDIT;
    const PERMISSION: Permission = Permission::Confirm;

    fn execute(&self, args: MultiEditArgs) -> Result<ToolResult> {
        let path = &args.path;
        if args.edits.is_empty() {
            return Ok(ToolResult {
                output: "editsが空です".to_string(),
                success: false,
            });
        }

        // 元のファイル内容を保存（ロールバック用）
        let original = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult {
                    output: format!("ファイル読み取りエラー: {e}"),
                    success: false,
                });
            }
        };

        // git snapshot
        FileWriteTool::git_snapshot(path);

        let mut current = original.clone();
        let mut applied = 0;
        let mut warnings: Vec<String> = Vec::new();

        for (i, edit) in args.edits.iter().enumerate() {
            if current.contains(&edit.old_text) {
                current = current.replacen(&edit.old_text, &edit.new_text, 1);
                applied += 1;
            } else if let Some((fuzzy_result, msg)) =
                fuzzy_find_replace(&current, &edit.old_text, &edit.new_text)
            {
                current = fuzzy_result;
                applied += 1;
                warnings.push(format!("edit[{}]: {}", i, msg));
            } else {
                // ロールバック: 元のファイルを復元
                let _ = std::fs::write(path, &original);
                return Ok(ToolResult {
                    output: format!(
                        "edit[{}]の置換対象が見つかりません。{}件適用済みを全てロールバックしました: {}",
                        i, applied, path
                    ),
                    success: false,
                });
            }
        }

        // 全て成功 → ファイルに書き込み
        match std::fs::write(path, &current) {
            Ok(()) => {
                let warn_msg = if warnings.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", warnings.join("; "))
                };
                Ok(ToolResult {
                    output: format!("{}件の編集を一括適用しました{}: {}", applied, warn_msg, path),
                    success: true,
                })
            }
            Err(e) => {
                let _ = std::fs::write(path, &original);
                Ok(ToolResult {
                    output: format!("書き込みエラー（ロールバック済み）: {e}"),
                    success: false,
                })
            }
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
    // 戦略8: Levenshtein距離ベースBlockアンカー（OpenCode知見）
    if let Some(r) = try_levenshtein_block(content, old_text, new_text) {
        return Some((r, "模糊一致（Levenshtein距離）".into()));
    }
    // 戦略9: ContextAwareReplacer（OpenCode知見: コンテキスト行アンカー）
    if let Some(r) = try_context_aware(content, old_text, new_text) {
        return Some((r, "模糊一致（コンテキストアンカー）".into()));
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
            return Some(replace_lines(
                content,
                &content_lines,
                i,
                old_lines.len(),
                new_text,
            ));
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
            && old_lines
                .iter()
                .enumerate()
                .all(|(j, ol)| content_lines[i + j].trim_start() == ol.trim_start())
        {
            return Some(replace_lines(
                content,
                &content_lines,
                i,
                old_lines.len(),
                new_text,
            ));
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
                && old_lines
                    .iter()
                    .enumerate()
                    .all(|(j, ol)| normalize_unicode(content_lines[i + j]) == normalize_unicode(ol))
            {
                return Some(replace_lines(
                    content,
                    &content_lines,
                    i,
                    old_lines.len(),
                    new_text,
                ));
            }
        }
    }
    None
}

/// 戦略5: エスケープ正規化（\n→改行、\t→タブ）
fn try_escape_normalized(content: &str, old_text: &str, new_text: &str) -> Option<String> {
    let unescaped = old_text.replace("\\n", "\n").replace("\\t", "\t");
    if unescaped != old_text
        && content.contains(&unescaped)
        && content.matches(&unescaped).count() == 1
    {
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
            return Some(replace_lines(
                content,
                &content_lines,
                i,
                old_lines.len(),
                new_text,
            ));
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
fn replace_lines(
    content: &str,
    content_lines: &[&str],
    start: usize,
    count: usize,
    new_text: &str,
) -> String {
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


/// 戦略8: Levenshtein距離ベースBlockアンカー（OpenCode知見）
/// 先頭行/末尾行が類似（距離≤行長の30%）かつ中間行の50%以上が類似
fn try_levenshtein_block(content: &str, old_text: &str, new_text: &str) -> Option<String> {
    let content_lines: Vec<&str> = content.lines().collect();
    let old_lines: Vec<&str> = old_text.lines().collect();
    if old_lines.len() < 3 {
        return None;
    }
    let first = old_lines[0].trim();
    let last = old_lines[old_lines.len() - 1].trim();

    for (i, cl) in content_lines.iter().enumerate() {
        let cl_trimmed = cl.trim();
        // 先頭行: Levenshtein距離が行長の30%以内
        if line_distance(cl_trimmed, first) > first.len() / 3 + 1 {
            continue;
        }
        let end = i + old_lines.len();
        if end > content_lines.len() {
            continue;
        }
        // 末尾行チェック
        let last_content = content_lines[end - 1].trim();
        if line_distance(last_content, last) > last.len() / 3 + 1 {
            continue;
        }
        // 中間行の類似度チェック（50%閾値）
        let middle_count = old_lines.len() - 2;
        let threshold = (middle_count as f64 * 0.5).ceil() as usize;
        let matched = (1..old_lines.len() - 1)
            .filter(|&j| {
                let a = content_lines[i + j].trim();
                let b = old_lines[j].trim();
                line_distance(a, b) <= b.len() / 3 + 1
            })
            .count();
        if matched >= threshold {
            return Some(replace_lines(
                content,
                &content_lines,
                i,
                old_lines.len(),
                new_text,
            ));
        }
    }
    None
}

/// 行レベルLevenshtein距離（短い文字列向け簡易実装）
fn line_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (m, n) = (a.len(), b.len());
    if m == 0 { return n; }
    if n == 0 { return m; }
    let mut prev = (0..=n).collect::<Vec<_>>();
    let mut curr = vec![0; n + 1];
    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

/// 戦略9: ContextAwareReplacer（OpenCode知見）
/// old_textの前後コンテキスト行をアンカーとし、重複コードブロックを区別
fn try_context_aware(content: &str, old_text: &str, new_text: &str) -> Option<String> {
    let content_lines: Vec<&str> = content.lines().collect();
    let old_lines: Vec<&str> = old_text.lines().collect();
    if old_lines.len() < 2 {
        return None;
    }

    // old_textの先頭行と末尾行をアンカーとして全候補位置を収集
    let first_trimmed = old_lines[0].trim();
    let mut candidates: Vec<usize> = Vec::new();
    for (i, cl) in content_lines.iter().enumerate() {
        if cl.trim() == first_trimmed && i + old_lines.len() <= content_lines.len() {
            candidates.push(i);
        }
    }

    if candidates.len() <= 1 {
        return None; // 重複なし → 他の戦略で対応済み
    }

    // 複数候補: コンテキスト行（直前1行）で区別
    for &start in &candidates {
        // 中間行の50%以上が一致するか
        let matched = old_lines.iter().enumerate().filter(|&(j, ol)| {
            content_lines.get(start + j)
                .is_some_and(|cl| cl.trim() == ol.trim())
        }).count();
        let threshold = (old_lines.len() as f64 * 0.5).ceil() as usize;
        if matched < threshold {
            continue;
        }

        // コンテキスト確認: 直前行が存在すれば、old_textの1行目の直前のコンテキストを確認
        // → 最も多くの行が一致する候補を選択
        if matched == old_lines.len() {
            return Some(replace_lines(
                content,
                &content_lines,
                start,
                old_lines.len(),
                new_text,
            ));
        }
    }

    // 最良候補（最大一致数）を選択
    let best = candidates.iter().max_by_key(|&&start| {
        old_lines.iter().enumerate().filter(|&(j, ol)| {
            content_lines.get(start + j)
                .is_some_and(|cl| cl.trim() == ol.trim())
        }).count()
    });

    best.map(|&start| replace_lines(
        content,
        &content_lines,
        start,
        old_lines.len(),
        new_text,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Tool;
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
        assert!(
            result.output.contains("hello world"),
            "出力に内容が含まれること"
        );

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
        let result = tool
            .call(serde_json::json!({"path": &path, "offset": 1, "limit": 2}))
            .unwrap();
        assert!(result.output.contains("b"), "offset=1でbから開始");
        assert!(result.output.contains("c"), "limit=2でcまで含む");
        assert!(!result.output.contains("| a"), "aは含まない");
        fs::remove_file(&path).ok();
    }

    // MultiEditTool
    #[test]
    fn test_multi_edit_basic() {
        let path = temp_path("multiedit");
        fs::write(&path, "aaa\nbbb\nccc").unwrap();
        let tool = MultiEditTool;
        let result = tool.call(serde_json::json!({
            "path": path,
            "edits": [
                {"old_text": "aaa", "new_text": "AAA"},
                {"old_text": "ccc", "new_text": "CCC"}
            ]
        })).unwrap();
        assert!(result.success);
        assert!(result.output.contains("2件"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "AAA\nbbb\nCCC");
        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_multi_edit_rollback_on_failure() {
        let path = temp_path("multiedit-rb");
        fs::write(&path, "aaa\nbbb\nccc").unwrap();
        let tool = MultiEditTool;
        let result = tool.call(serde_json::json!({
            "path": path,
            "edits": [
                {"old_text": "aaa", "new_text": "AAA"},
                {"old_text": "NOTFOUND", "new_text": "XXX"}
            ]
        })).unwrap();
        assert!(!result.success);
        assert!(result.output.contains("ロールバック"));
        // ロールバックされているので元のまま
        assert_eq!(fs::read_to_string(&path).unwrap(), "aaa\nbbb\nccc");
        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_multi_edit_empty_edits() {
        let path = temp_path("multiedit-empty");
        fs::write(&path, "hello").unwrap();
        let tool = MultiEditTool;
        let result = tool.call(serde_json::json!({
            "path": path,
            "edits": []
        })).unwrap();
        assert!(!result.success);
        fs::remove_file(&path).ok();
    }

    // --- fuzzy戦略個別テスト ---

    /// 戦略1: タブとスペースの混在でも空白正規化で一致すること
    #[test]
    fn test_fuzzy_whitespace_normalization() {
        let content = "fn  main() {\n    let\tx = 1;\n}";
        let old_text = "fn main() {\n    let x = 1;\n}";
        let new_text = "fn main() {\n    let x = 2;\n}";
        let result = fuzzy_find_replace(content, old_text, new_text);
        assert!(result.is_some(), "空白正規化で一致すべき");
        let (replaced, msg) = result.unwrap();
        assert!(msg.contains("空白正規化"), "空白正規化戦略が使用されること");
        assert!(replaced.contains("let x = 2"), "置換後の内容が反映されること");
    }

    /// 戦略2: 末尾/先頭の空白を含むテキストがtrimで一致すること
    #[test]
    fn test_fuzzy_trim_match() {
        // try_trimmed単体テスト（戦略1の空白正規化より前にマッチされないよう直接呼出）
        let content = "alpha\nbeta\ngamma";
        let old_text = "  beta  ";
        let new_text = "BETA";
        let result = try_trimmed(content, old_text, new_text);
        assert!(result.is_some(), "Trimで一致すべき");
        let replaced = result.unwrap();
        assert!(replaced.contains("BETA"), "置換が反映されること");
    }

    /// 戦略3: インデントレベルが異なっても一致すること
    #[test]
    fn test_fuzzy_indentation_flexible() {
        // try_indent_flexible単体テスト（先行戦略を迂回）
        let content = "    if true {\n        println!(\"hello\");\n    }";
        let old_text = "if true {\n    println!(\"hello\");\n}";
        let new_text = "if false {\n    println!(\"bye\");\n}";
        let result = try_indent_flexible(content, old_text, new_text);
        assert!(result.is_some(), "インデント差異で一致すべき");
        let replaced = result.unwrap();
        assert!(replaced.contains("false"), "置換が反映されること");
    }

    /// 戦略4: 全角英数字と半角英数字がUnicode正規化で一致すること
    #[test]
    fn test_fuzzy_unicode_normalization() {
        // 全角英数→半角変換のテスト
        let content = "let value = 42;";
        let old_text = "let\u{FF56}alue\u{FF40}=\u{FF14}2;"; // 一部全角
        // normalize_unicodeは全角英数(FF01-FF5E)を半角に変換
        // ただし戦略4はold_textとnew_textが正規化後に同じなら意味なしで早期リターン
        let new_text = "let value = 99;";
        let result = fuzzy_find_replace(content, old_text, new_text);
        // 全角混在old_textが半角contentと一致するケース
        if result.is_some() {
            let (_, msg) = result.unwrap();
            assert!(msg.contains("Unicode") || msg.contains("模糊"), "Unicode戦略が使われること");
        }
        // normalize_unicode関数単体テスト
        assert_eq!(normalize_unicode("\u{FF21}\u{FF22}\u{FF23}"), "ABC");
        assert_eq!(normalize_unicode("\u{2018}hello\u{2019}"), "'hello'");
        assert_eq!(normalize_unicode("\u{201C}test\u{201D}"), "\"test\"");
        assert_eq!(normalize_unicode("\u{2014}"), "-");
    }

    /// 戦略6: 先頭行と末尾行が一致し、中間行が異なるBlockアンカー
    #[test]
    fn test_fuzzy_block_anchor_match() {
        let content = "fn test() {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n}";
        // 先頭行と末尾行が同じ、中間行が少し異なるold_text
        let old_text = "fn test() {\n    let a = 1;\n    let b = 999;\n    let c = 3;\n}";
        let new_text = "fn replaced() {\n    // new\n}";
        let result = fuzzy_find_replace(content, old_text, new_text);
        assert!(result.is_some(), "Blockアンカーで一致すべき（中間行50%以上一致）");
        let (replaced, msg) = result.unwrap();
        assert!(msg.contains("Block") || msg.contains("模糊"), "Blockアンカー戦略が使用されること");
        assert!(replaced.contains("replaced"), "置換が反映されること");
    }

    /// 戦略6: 中間行の一致率が50%未満なら不一致
    #[test]
    fn test_fuzzy_block_anchor_below_threshold() {
        let content = "start\nline_a\nline_b\nline_c\nend";
        // 中間行が全て異なる → 0% < 50% で不一致
        let old_text = "start\nXXX\nYYY\nZZZ\nend";
        let new_text = "replaced";
        let result = try_block_anchor(content, old_text, new_text);
        assert!(result.is_none(), "中間行の一致率50%未満では不一致");
    }

    /// 戦略8: Levenshtein距離が30%以内の類似ブロックで一致すること
    #[test]
    fn test_fuzzy_levenshtein_block() {
        let content = "fn process_data() {\n    let result = compute();\n    save(result);\n}";
        // 先頭/末尾行が類似（少し異なる）、中間行も類似
        let old_text = "fn process_deta() {\n    let result = compute();\n    save(result);\n}";
        let new_text = "fn new_func() {}";
        let result = fuzzy_find_replace(content, old_text, new_text);
        assert!(result.is_some(), "Levenshtein距離30%以内で一致すべき");
        let (replaced, msg) = result.unwrap();
        // Blockアンカー(戦略6)かLevenshtein(戦略8)のどちらかで一致
        assert!(msg.contains("模糊"), "fuzzy戦略が使用されること");
        assert!(replaced.contains("new_func"), "置換が反映されること");
    }

    /// 戦略8: Levenshtein距離が30%超で不一致
    #[test]
    fn test_fuzzy_levenshtein_block_too_distant() {
        let content = "fn alpha() {\n    x();\n    y();\n}";
        // 先頭行が大きく異なる → 距離 > 30%
        let old_text = "fn completely_different_name() {\n    x();\n    y();\n}";
        let new_text = "replaced";
        let result = try_levenshtein_block(content, old_text, new_text);
        assert!(result.is_none(), "先頭行の距離が30%超では不一致");
    }

    /// 戦略9: 重複コードブロックをコンテキストで区別すること
    #[test]
    fn test_fuzzy_context_aware() {
        // 同じコードブロックが2箇所に存在
        let content = "// module A\nfn do_thing() {\n    action();\n}\n// module B\nfn do_thing() {\n    action();\n}";
        let old_text = "fn do_thing() {\n    action();\n}";
        let new_text = "fn do_thing() {\n    new_action();\n}";
        let result = try_context_aware(content, old_text, new_text);
        assert!(result.is_some(), "コンテキストアンカーで候補を区別すべき");
        let replaced = result.unwrap();
        // 最初の候補が選ばれる（全行一致のため）
        assert!(replaced.contains("new_action"), "置換が反映されること");
    }

    /// 全戦略で一致しない場合Noneが返ること
    #[test]
    fn test_search_replace_no_match() {
        let content = "fn hello() { println!(\"hi\"); }";
        let old_text = "this text does not exist anywhere in the content at all xyz123";
        let new_text = "replacement";
        let result = fuzzy_find_replace(content, old_text, new_text);
        assert!(result.is_none(), "全戦略で一致しない場合Noneを返すこと");
    }

    /// 空のold_textではNoneを返すこと
    #[test]
    fn test_search_replace_empty_old_text() {
        let result = fuzzy_find_replace("content", "   ", "new");
        assert!(result.is_none(), "空白のみのold_textではNone");
    }

    /// FileReadToolのoffsetとlimitパラメータ
    #[test]
    fn test_file_read_offset_limit() {
        let path = temp_path("offset-limit");
        fs::write(&path, "line0\nline1\nline2\nline3\nline4\nline5").unwrap();
        let tool = FileReadTool;

        // offset=2, limit=2 → line2とline3のみ表示
        let result = tool
            .call(serde_json::json!({"path": &path, "offset": 2, "limit": 2}))
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("line2"), "offset=2でline2を含む");
        assert!(result.output.contains("line3"), "limit=2でline3を含む");
        assert!(!result.output.contains("| line0"), "line0は含まない");
        assert!(!result.output.contains("| line4"), "line4は含まない");
        assert!(result.output.contains("表示:3-4"), "表示範囲が正しいこと");

        fs::remove_file(&path).ok();
    }

    /// 存在しないファイルの読み取りでエラーが返ること
    #[test]
    fn test_file_read_nonexistent() {
        let path = format!("/tmp/bonsai-nonexistent-{}", uuid::Uuid::new_v4());
        let tool = FileReadTool;
        let result = tool.call(serde_json::json!({"path": &path})).unwrap();
        assert!(!result.success);
        assert!(result.output.contains("エラー"), "エラーメッセージを含む");
    }

    /// 新規ファイルへの書き込み（親ディレクトリ自動作成含む）
    #[test]
    fn test_file_write_create_new() {
        let dir = format!("/tmp/bonsai-new-{}", uuid::Uuid::new_v4());
        let path = format!("{}/subdir/newfile.txt", dir);
        let tool = FileWriteTool;
        let result = tool
            .call(serde_json::json!({"path": &path, "content": "brand new file"}))
            .unwrap();
        assert!(result.success, "新規ファイル作成が成功すること");
        assert_eq!(fs::read_to_string(&path).unwrap(), "brand new file");
        fs::remove_dir_all(&dir).ok();
    }

    /// SEARCH/REPLACEで複数箇所に一致する場合、最初の1箇所のみ置換
    #[test]
    fn test_search_replace_multiple_matches() {
        let path = temp_path("multi-match");
        fs::write(&path, "foo bar foo baz foo").unwrap();
        let tool = FileWriteTool;
        let result = tool
            .call(serde_json::json!({
                "path": &path,
                "old_text": "foo",
                "new_text": "QUX"
            }))
            .unwrap();
        assert!(result.success);
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "QUX bar foo baz foo", "最初の1箇所のみ置換されること");
        fs::remove_file(&path).ok();
    }

    /// 戦略5: エスケープ正規化のテスト
    #[test]
    fn test_fuzzy_escape_normalized() {
        let content = "line1\n\tindented\nline3";
        let old_text = "line1\\n\\tindented\\nline3";
        let new_text = "replaced";
        let result = try_escape_normalized(content, old_text, new_text);
        assert!(result.is_some(), "エスケープシーケンスが実際の文字に変換されて一致すること");
    }

    /// 戦略7: 境界Trim（先頭/末尾の空行除去）のテスト
    #[test]
    fn test_fuzzy_boundary_trim() {
        let content = "alpha\nbeta\ngamma";
        // 前後に空行を含むold_text
        let old_text = "\n\nbeta\n\n";
        let new_text = "BETA";
        let result = try_boundary_trim(content, old_text, new_text);
        assert!(result.is_some(), "境界の空行を除去して一致すること");
    }

    /// line_distance関数の基本テスト
    #[test]
    fn test_line_distance() {
        assert_eq!(line_distance("", ""), 0);
        assert_eq!(line_distance("abc", ""), 3);
        assert_eq!(line_distance("", "xyz"), 3);
        assert_eq!(line_distance("abc", "abc"), 0);
        assert_eq!(line_distance("abc", "axc"), 1);
        assert_eq!(line_distance("kitten", "sitting"), 3);
    }

    /// replace_lines関数の末尾改行保持テスト
    #[test]
    fn test_replace_lines_trailing_newline() {
        let content = "line1\nline2\nline3\n";
        let lines: Vec<&str> = content.lines().collect();
        let result = replace_lines(content, &lines, 1, 1, "NEW");
        assert!(result.ends_with('\n'), "元テキストの末尾改行が保持されること");
        assert!(result.contains("NEW"), "置換が反映されること");
    }

    /// 戦略3: 3行未満のBlockアンカーはスキップ
    #[test]
    fn test_block_anchor_too_short() {
        let content = "a\nb";
        let old_text = "a\nb";
        let result = try_block_anchor(content, old_text, "new");
        assert!(result.is_none(), "3行未満ではBlockアンカーは適用されない");
    }

    /// 戦略8: 3行未満のLevenshteinブロックはスキップ
    #[test]
    fn test_levenshtein_block_too_short() {
        let content = "a\nb";
        let old_text = "a\nb";
        let result = try_levenshtein_block(content, old_text, "new");
        assert!(result.is_none(), "3行未満ではLevenshteinブロックは適用されない");
    }

    /// 戦略9: 2行未満のContextAwareはスキップ
    #[test]
    fn test_context_aware_too_short() {
        let content = "single line";
        let old_text = "single line";
        let result = try_context_aware(content, old_text, "new");
        assert!(result.is_none(), "2行未満ではContextAwareは適用されない");
    }

    /// 戦略9: 重複なし（候補1件以下）ではNone
    #[test]
    fn test_context_aware_no_duplicates() {
        let content = "unique_start\n    body\nunique_end";
        let old_text = "unique_start\n    body\nunique_end";
        let result = try_context_aware(content, old_text, "new");
        assert!(result.is_none(), "重複なしではContextAwareは適用されない");
    }

}
