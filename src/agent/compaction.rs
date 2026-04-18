use crate::agent::conversation::{Message, Role};
use crate::memory::store::MemoryStore;
use std::collections::HashMap;
pub struct CompactionConfig {
    pub large_output_threshold: usize,
    pub placeholder_keep_recent: usize,
    pub summary_max_chars: usize,
    pub emergency_keep: usize,
    pub max_context_tokens: usize,
}
impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            large_output_threshold: 5000,
            placeholder_keep_recent: 6,
            summary_max_chars: 200,
            emergency_keep: 4,
            max_context_tokens: 14000,
        }
    }
}
pub fn estimate_tokens(messages: &[Message]) -> usize {
    messages.iter().map(|m| m.content.len().div_ceil(4)).sum()
}
/// AI+Toolメッセージペアを検出
pub fn find_ai_tool_pairs(messages: &[Message]) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    for i in 0..messages.len().saturating_sub(1) {
        if matches!(messages[i].role, Role::Assistant) && matches!(messages[i + 1].role, Role::Tool) {
            pairs.push((i, i + 1));
        }
    }
    pairs
}

/// Assistantメッセージ内の<tool_call>からツール名を抽出し、使用回数を集計
///
/// tool_callのJSONから"name"フィールドを正規表現で取得するため、
/// parse.rsへの依存を避けつつ正確なツール名統計を提供する。
pub fn summarize_tool_usage(messages: &[Message]) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for msg in messages {
        if matches!(msg.role, Role::Assistant) {
            // <tool_call>ブロック内の"name":"xxx"を抽出
            let mut remaining = msg.content.as_str();
            while let Some(start) = remaining.find("<tool_call>") {
                let after_tag = &remaining[start + 11..];
                if let Some(end) = after_tag.find("</tool_call>") {
                    let block = &after_tag[..end];
                    // "name" : "tool_name" パターンを検索
                    if let Some(name) = extract_name_from_json(block) {
                        *counts.entry(name).or_insert(0) += 1;
                    }
                    remaining = &after_tag[end + 12..];
                } else {
                    break;
                }
            }
        }
    }
    counts
}

/// JSONブロックから"name"フィールドの値を抽出するヘルパー
fn extract_name_from_json(json_str: &str) -> Option<String> {
    // "name" の位置を検索（空白許容）
    let name_key = json_str.find("\"name\"")?;
    let after_key = &json_str[name_key + 6..];
    // コロンを探す
    let colon = after_key.find(':')?;
    let after_colon = after_key[colon + 1..].trim_start();
    // 値の開始引用符
    if !after_colon.starts_with('"') {
        return None;
    }
    let value_start = &after_colon[1..];
    let end_quote = value_start.find('"')?;
    Some(value_start[..end_quote].to_string())
}

/// Assistantメッセージから<think>ブロックの結論部分を抽出（GLM-5.1 Preserved Thinking知見）
///
/// 各thinkブロックの最後の文（結論部分）を最大3件保持し、
/// 推論の連続性を保護する。200文字で切り詰め。
pub fn extract_thinking_summary(messages: &[Message]) -> Vec<String> {
    let mut summaries = Vec::new();
    for msg in messages {
        if !matches!(msg.role, Role::Assistant) {
            continue;
        }
        let mut remaining = msg.content.as_str();
        while let Some(start) = remaining.find("<think>") {
            let after_tag = &remaining[start + 7..];
            if let Some(end) = after_tag.find("</think>") {
                let block = after_tag[..end].trim();
                if !block.is_empty() {
                    let last_sentence = extract_last_sentence(block);
                    if !last_sentence.is_empty() {
                        let truncated: String = last_sentence.chars().take(200).collect();
                        summaries.push(truncated);
                        if summaries.len() >= 3 {
                            return summaries;
                        }
                    }
                }
                remaining = &after_tag[end + 8..];
            } else {
                break;
            }
        }
    }
    summaries
}

/// thinkブロックから最後の文を抽出するヘルパー
fn extract_last_sentence(block: &str) -> String {
    let lines: Vec<&str> = block.lines().filter(|l| !l.trim().is_empty()).collect();
    if let Some(last_line) = lines.last() {
        last_line.trim().to_string()
    } else {
        block.trim().to_string()
    }
}

/// 最後のAssistant/Toolメッセージからタスクの成果を200文字以内で抽出
///
/// 最後のAssistantメッセージの内容を優先し、<think>タグは除外する。
/// Assistantメッセージがない場合は最後のToolメッセージから抽出。
pub fn extract_last_outcome(messages: &[Message]) -> Option<String> {
    // 最後のAssistantメッセージを探す（<think>を除外）
    let last_assistant = messages.iter().rev().find(|m| matches!(m.role, Role::Assistant));
    if let Some(msg) = last_assistant {
        let cleaned = strip_think_tags(&msg.content);
        let trimmed = cleaned.trim();
        if !trimmed.is_empty() {
            let outcome: String = trimmed.chars().take(200).collect();
            return Some(outcome);
        }
    }
    // フォールバック: 最後のToolメッセージ
    let last_tool = messages.iter().rev().find(|m| matches!(m.role, Role::Tool));
    if let Some(msg) = last_tool {
        let trimmed = msg.content.trim();
        if !trimmed.is_empty() {
            let outcome: String = trimmed.chars().take(200).collect();
            return Some(outcome);
        }
    }
    None
}

/// <think>...</think> タグとその中身を除去
fn strip_think_tags(s: &str) -> String {
    let mut result = String::new();
    let mut remaining = s;
    while let Some(start) = remaining.find("<think>") {
        result.push_str(&remaining[..start]);
        if let Some(end) = remaining[start..].find("</think>") {
            remaining = &remaining[start + end + 8..];
        } else {
            // 閉じタグなし: <think>以降を全て除去
            return result;
        }
    }
    result.push_str(remaining);
    result
}

/// メッセージの重要度スコアを計算（GLM-5.1 DSA知見）
///
/// トークン重要度による動的注意配分。重要度が低いメッセージから優先的に削除。
pub fn score_message_importance(msg: &Message) -> f64 {
    match msg.role {
        Role::User => 1.0,
        Role::System => {
            if msg.content.contains("<context") || msg.content.contains("<memory-context") {
                0.3
            } else {
                0.9
            }
        }
        Role::Assistant => {
            if msg.content.contains("<tool_call>") {
                0.7
            } else {
                0.4
            }
        }
        Role::Tool => {
            if msg.content.contains("error") || msg.content.contains("Error")
                || msg.content.contains("\u30a8\u30e9\u30fc") || msg.content.contains("failed")
                || msg.content.contains("Failed")
            {
                0.2
            } else {
                0.5
            }
        }
    }
}

/// Toolメッセージからエラー（未解決事項）を検出
fn collect_unresolved(messages: &[Message], boundary: usize) -> Vec<String> {
    let mut errors = Vec::new();
    // 圧縮対象の末尾付近のエラーを優先的に収集
    for msg in messages[..boundary].iter().rev().take(boundary) {
        if matches!(msg.role, Role::Tool) && msg.content.contains("エラー") {
            let preview: String = msg.content.chars().take(100).collect();
            if !errors.contains(&preview) {
                errors.push(preview);
            }
            if errors.len() >= 3 {
                break;
            }
        }
    }
    errors.reverse();
    errors
}

pub fn compact_level0(messages: &mut [Message], config: &CompactionConfig) -> Vec<String> {
    let mut off = Vec::new();
    for msg in messages.iter_mut() {
        if matches!(msg.role, Role::Tool) && msg.content.len() > config.large_output_threshold {
            let h = {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut ha = DefaultHasher::new();
                msg.content.hash(&mut ha);
                format!("{:x}", ha.finish())
            };
            let p = format!("/tmp/bonsai-out-{h}.txt");
            if std::fs::write(&p, &msg.content).is_ok() {
                let pv: String = msg.content.chars().take(200).collect();
                let l = msg.content.len();
                msg.content = format!("{pv}...\n[saved:{p}({l}c)]");
                off.push(p);
            }
        }
    }
    off
}
pub fn compact_level1(messages: &mut [Message], config: &CompactionConfig) {
    let t = messages.len();
    if t <= config.placeholder_keep_recent {
        return;
    }
    let boundary = t - config.placeholder_keep_recent;
    let pairs = find_ai_tool_pairs(messages);
    let protected: std::collections::HashSet<usize> = pairs
        .iter()
        .flat_map(|&(a, b)| {
            if a >= boundary || b >= boundary { vec![a, b] } else { vec![] }
        })
        .collect();

    // 重要度ベース適応的削除（GLM-5.1 DSA知見）:
    // 最初と最後のUserメッセージは絶対保護
    let first_user_idx = messages[..boundary]
        .iter()
        .position(|m| matches!(m.role, Role::User));
    let last_user_idx = messages[..boundary]
        .iter()
        .rposition(|m| matches!(m.role, Role::User));

    // 重要度スコアが低い順にソートした削除候補
    let mut candidates: Vec<(usize, f64)> = (0..boundary)
        .filter(|&i| {
            !protected.contains(&i)
                && Some(i) != first_user_idx
                && Some(i) != last_user_idx
        })
        .map(|i| (i, score_message_importance(&messages[i])))
        .collect();
    candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    for (i, _score) in &candidates {
        let msg = &mut messages[*i];
        if matches!(msg.role, Role::Tool) && msg.content.len() > 50 {
            let id = msg.tool_call_id.as_deref().unwrap_or("?");
            msg.content = format!("[prev:{id}]");
        }
    }
}
pub fn compact_level2(messages: &mut [Message], config: &CompactionConfig) {
    let t = messages.len();
    if t <= config.placeholder_keep_recent {
        return;
    }
    let boundary = t - config.placeholder_keep_recent;
    // Preserved Thinking: 削除対象から思考サマリーを抽出してから要約
    let thinking_summaries = extract_thinking_summary(&messages[..boundary]);
    for msg in messages[..boundary].iter_mut() {
        if matches!(msg.role, Role::Assistant) && msg.content.len() > config.summary_max_chars {
            let s: String = msg.content.chars().take(config.summary_max_chars).collect();
            msg.content = format!("{s}...[summarized]");
        }
    }
    // 思考サマリーを最後の要約済みAssistantメッセージに追加
    if !thinking_summaries.is_empty() {
        let thinking_text = thinking_summaries
            .iter()
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n");
        if let Some(last_assistant) = messages[..boundary]
            .iter_mut()
            .rev()
            .find(|m| matches!(m.role, Role::Assistant))
        {
            last_assistant.content.push_str(&format!(
                "\n[Preserved Thinking]\n{thinking_text}"
            ));
        }
    }
}
pub fn compact_level3(messages: &mut Vec<Message>, config: &CompactionConfig) {
    if messages.len() <= config.emergency_keep + 1 {
        return;
    }
    let sys: Vec<Message> = messages
        .iter()
        .filter(|m| matches!(m.role, Role::System))
        .cloned()
        .collect();
    // Handoff framing: 圧縮前の要約を「引継ぎ」として挿入（hermes-agent知見）
    let handoff = build_handoff_summary(messages, config);
    let rec: Vec<Message> = messages
        .iter()
        .rev()
        .take(config.emergency_keep)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    messages.clear();
    messages.extend(sys);
    if let Some(h) = handoff {
        messages.push(h);
    }
    messages.extend(rec);
}

/// Handoff framing: 圧縮対象から要約を構築（hermes-agent/macOS26パターン）
///
/// 「別のアシスタントが引き継ぎ」として、解決済み/未解決を整理。
/// ツール使用統計・最終成果・未解決事項を含む高品質な引継ぎサマリーを生成。
/// 1bitモデルが指示と混同しないよう「Remaining Work」命名を使用。
fn build_handoff_summary(messages: &[Message], config: &CompactionConfig) -> Option<Message> {
    let boundary = messages.len().saturating_sub(config.emergency_keep);
    if boundary < 2 {
        return None;
    }
    let compressed = &messages[..boundary];

    // 圧縮対象のAssistantメッセージから解決済みタスクの要約を構築
    let mut resolved = Vec::new();
    for msg in compressed {
        if matches!(msg.role, Role::Assistant) && msg.content.len() > 20 {
            let preview: String = msg.content.chars().take(80).collect();
            resolved.push(preview);
        }
    }
    if resolved.is_empty() {
        return None;
    }

    let resolved_text = resolved
        .iter()
        .take(3)
        .map(|r| format!("- {r}"))
        .collect::<Vec<_>>()
        .join("\n");

    // ツール使用統計: どのツールを何回使ったか
    let tool_stats = summarize_tool_usage(compressed);
    let tool_stats_text = if tool_stats.is_empty() {
        String::new()
    } else {
        let mut entries: Vec<_> = tool_stats.iter().collect();
        entries.sort_by(|a, b| b.1.cmp(a.1));
        let stats_str = entries
            .iter()
            .take(8)
            .map(|(name, count)| format!("{name}:{count}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("\nTool stats: {stats_str}")
    };

    // 最終成果の要約
    let outcome_text = match extract_last_outcome(compressed) {
        Some(outcome) => format!("\nLast outcome: {outcome}"),
        None => String::new(),
    };

    // 未解決事項（エラー）の検出
    let unresolved = collect_unresolved(messages, boundary);
    let unresolved_text = if unresolved.is_empty() {
        String::new()
    } else {
        let items = unresolved
            .iter()
            .map(|e| format!("- {e}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("\nUnresolved issues:\n{items}")
    };

    Some(Message::system(format!(
        "[Context handoff] 前のアシスタントの作業を引き継ぎます。\n\
         Resolved:\n{resolved_text}{tool_stats_text}{outcome_text}{unresolved_text}\n\
         Remaining Work: 直近のメッセージに基づいて作業を続行してください。"
    )))
}

/// コンパクション前のメモリフラッシュ: 削除対象のAssistant発言を要約してMemoryStoreに退避
pub fn flush_before_compaction(messages: &[Message], store: Option<&MemoryStore>) {
    let Some(store) = store else { return };
    let boundary = messages.len().saturating_sub(6);
    let mut flushed = Vec::new();
    for msg in &messages[..boundary] {
        if matches!(msg.role, Role::Assistant) && msg.content.len() > 100 {
            let summary: String = msg.content.chars().take(200).collect();
            flushed.push(summary);
        }
    }
    if flushed.is_empty() { return; }
    let combined = flushed.join("\n---\n");
    if let Err(e) = store.save_memory(
        &combined,
        "context_flush",
        &["compaction".to_string()],
    ) {
        eprintln!("[flush] メモリ保存失敗: {e}");
    }
}
#[allow(clippy::possible_missing_else)]
pub fn compact_if_needed(
    messages: &mut Vec<Message>,
    config: &CompactionConfig,
) -> (u8, Vec<String>) {
    let off = compact_level0(messages, config);
    let mut lv = 0u8;
    if estimate_tokens(messages) > config.max_context_tokens * 3 / 4 {
        compact_level1(messages, config);
        lv = 1;
    }
    if estimate_tokens(messages) > config.max_context_tokens * 9 / 10 {
        compact_level2(messages, config);
        lv = 2;
    }
    if estimate_tokens(messages) > config.max_context_tokens {
        compact_level3(messages, config);
        lv = 3;
    }
    (lv, off)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn mk(n: usize, sz: usize) -> Vec<Message> {
        let mut v = vec![Message::system("s")];
        for i in 0..n {
            v.push(Message::user(format!("q{i}")));
            v.push(Message::assistant("x".repeat(sz)));
            v.push(Message::tool("y".repeat(sz), format!("t{i}")));
        }
        v
    }

    /// ツール呼び出しを含むAssistantメッセージを持つテスト用メッセージ列を生成
    fn mk_with_tool_calls(calls: &[(&str, &str)]) -> Vec<Message> {
        let mut v = vec![Message::system("s")];
        for (tool_name, result) in calls {
            v.push(Message::user("q"));
            v.push(Message::assistant(format!(
                "<think>plan</think>\n<tool_call>{{\"name\":\"{tool_name}\",\"arguments\":{{}}}}</tool_call>"
            )));
            v.push(Message::tool(result.to_string(), format!("call_{tool_name}")));
        }
        v
    }

    #[test]
    fn t_tok() {
        assert_eq!(estimate_tokens(&[Message::user("hello world")]), 3);
    }
    #[test]
    fn t_l0() {
        let mut m = vec![Message::tool("x".repeat(10000), "b")];
        let o = compact_level0(
            &mut m,
            &CompactionConfig {
                large_output_threshold: 5000,
                ..Default::default()
            },
        );
        assert_eq!(o.len(), 1);
        for p in &o {
            std::fs::remove_file(p).ok();
        }
    }
    #[test]
    fn t_l1() {
        let mut m = mk(10, 100);
        compact_level1(
            &mut m,
            &CompactionConfig {
                placeholder_keep_recent: 4,
                ..Default::default()
            },
        );
        assert!(m.iter().any(|x| x.content.contains("[prev:")));
    }
    #[test]
    fn t_l2() {
        let mut m = mk(10, 500);
        compact_level2(
            &mut m,
            &CompactionConfig {
                placeholder_keep_recent: 4,
                summary_max_chars: 100,
                ..Default::default()
            },
        );
        assert!(m.iter().any(|x| x.content.contains("[summarized]")));
    }
    #[test]
    fn t_l3() {
        let mut m = mk(20, 100);
        compact_level3(
            &mut m,
            &CompactionConfig {
                emergency_keep: 4,
                ..Default::default()
            },
        );
        assert!(m.len() <= 6, "system+handoff+keep4=最大6");
    }
    #[test]
    fn t_noop() {
        let mut m = vec![Message::user("hi")];
        let (lv, _) = compact_if_needed(&mut m, &CompactionConfig::default());
        assert_eq!(lv, 0);
    }

    #[test]
    fn t_find_pairs() {
        let m = vec![Message::system("s"), Message::user("q"), Message::assistant("a"), Message::tool("r", "t1")];
        assert_eq!(find_ai_tool_pairs(&m), vec![(2, 3)]);
    }
    #[test]
    fn t_pair_multiple() {
        let m = vec![Message::assistant("a1"), Message::tool("r1", "t1"), Message::assistant("a2"), Message::tool("r2", "t2")];
        assert_eq!(find_ai_tool_pairs(&m).len(), 2);
    }
    #[test]
    fn t_pair_none() {
        let m = vec![Message::user("q"), Message::assistant("a"), Message::user("q2")];
        assert!(find_ai_tool_pairs(&m).is_empty());
    }
    #[test]
    fn t_l1_no_orphan() {
        let mut m = vec![
            Message::system("s"), Message::user("q0"),
            Message::assistant("assistant output here"),
            Message::tool("tool result with enough content to compress and it must be over fifty characters long for testing", "t0"),
            Message::user("q1"), Message::assistant("a1"), Message::tool("r1 short", "t1"),
        ];
        compact_level1(&mut m, &CompactionConfig { placeholder_keep_recent: 3, ..Default::default() });
        // idx3はペア(2,3)の一部だが、境界=4なのでidx3<4→圧縮対象
        assert!(m[3].content.contains("[prev:"));
    }
    #[test]
    fn t_l1_protect_boundary_pair() {
        let mut m = vec![
            Message::system("s"), Message::user("q0"),
            Message::assistant("old assistant content long"),
            Message::tool("old tool content long enough", "old"),
            Message::assistant("boundary assistant"),
            Message::tool("boundary tool content long enough", "bnd"),
        ];
        // keep_recent=2 → boundary=4, pair(4,5) both >= 4 → protected
        compact_level1(&mut m, &CompactionConfig { placeholder_keep_recent: 2, ..Default::default() });
        assert!(!m[5].content.contains("[prev:"));
    }

    #[test]
    fn t_flush_saves_to_store() {
        let store = crate::memory::store::MemoryStore::in_memory().unwrap();
        let mut msgs = vec![Message::system("s")];
        for i in 0..10 {
            msgs.push(Message::user(format!("q{i}")));
            msgs.push(Message::assistant("important context data ".repeat(8)));
        }
        flush_before_compaction(&msgs, Some(&store));
        let results = store.search_memories("important", 10).unwrap();
        assert!(!results.is_empty(), "フラッシュされたメモリが検索可能であること");
    }
    #[test]
    fn t_flush_no_store() {
        let msgs = vec![Message::assistant("x".repeat(200))];
        flush_before_compaction(&msgs, None);
        // パニックしないことを確認
    }

    #[test]
    fn t_handoff_summary() {
        let mut msgs = mk(5, 200);
        let config = CompactionConfig {
            max_context_tokens: 100,
            emergency_keep: 4,
            ..Default::default()
        };
        compact_level3(&mut msgs, &config);
        // systemメッセージ + handoff + 直近4件
        let has_handoff = msgs.iter().any(|m| m.content.contains("handoff"));
        assert!(has_handoff, "Handoff summary が挿入されるべき");
    }

    #[test]
    fn t_handoff_short_session_skipped() {
        let mut msgs = vec![Message::system("s"), Message::user("q"), Message::assistant("a")];
        let config = CompactionConfig {
            emergency_keep: 4,
            ..Default::default()
        };
        compact_level3(&mut msgs, &config);
        // 短すぎてhandoff不要
        let has_handoff = msgs.iter().any(|m| m.content.contains("handoff"));
        assert!(!has_handoff);
    }

    // --- 新規テスト: summarize_tool_usage ---

    #[test]
    fn t_summarize_tool_usage_basic() {
        let msgs = mk_with_tool_calls(&[
            ("shell", "ok"),
            ("file_read", "content"),
            ("shell", "done"),
            ("file_write", "written"),
        ]);
        let stats = summarize_tool_usage(&msgs);
        assert_eq!(stats.get("shell"), Some(&2), "shellは2回使用");
        assert_eq!(stats.get("file_read"), Some(&1), "file_readは1回使用");
        assert_eq!(stats.get("file_write"), Some(&1), "file_writeは1回使用");
    }

    #[test]
    fn t_summarize_tool_usage_empty() {
        let msgs = vec![Message::system("s"), Message::user("q"), Message::assistant("no tools")];
        let stats = summarize_tool_usage(&msgs);
        assert!(stats.is_empty(), "ツール呼び出しがなければ空");
    }

    #[test]
    fn t_summarize_tool_usage_multiple_in_one_message() {
        let msgs = vec![
            Message::assistant(
                "<tool_call>{\"name\":\"shell\",\"arguments\":{}}</tool_call>\n\
                 <tool_call>{\"name\":\"git\",\"arguments\":{}}</tool_call>"
                    .to_string(),
            ),
        ];
        let stats = summarize_tool_usage(&msgs);
        assert_eq!(stats.get("shell"), Some(&1));
        assert_eq!(stats.get("git"), Some(&1));
    }

    // --- 新規テスト: extract_last_outcome ---

    #[test]
    fn t_extract_last_outcome_assistant() {
        let msgs = vec![
            Message::system("s"),
            Message::user("q"),
            Message::assistant("ファイルの修正が完了しました。テストも全件パスしています。"),
        ];
        let outcome = extract_last_outcome(&msgs);
        assert!(outcome.is_some());
        assert!(outcome.unwrap().contains("修正が完了"));
    }

    #[test]
    fn t_extract_last_outcome_strips_think() {
        let msgs = vec![
            Message::assistant("<think>内部思考</think>タスク完了: 3ファイル修正済み"),
        ];
        let outcome = extract_last_outcome(&msgs).unwrap();
        assert!(!outcome.contains("内部思考"), "thinkタグの中身は除外");
        assert!(outcome.contains("タスク完了"), "thinkタグ外の内容は保持");
    }

    #[test]
    fn t_extract_last_outcome_fallback_to_tool() {
        let msgs = vec![
            Message::system("s"),
            Message::assistant(""),  // 空のAssistantメッセージ
            Message::tool("ビルド成功: 0 errors, 0 warnings", "build"),
        ];
        let outcome = extract_last_outcome(&msgs).unwrap();
        assert!(outcome.contains("ビルド成功"), "Toolメッセージにフォールバック");
    }

    #[test]
    fn t_extract_last_outcome_truncates() {
        let long_msg = "a".repeat(300);
        let msgs = vec![Message::assistant(long_msg)];
        let outcome = extract_last_outcome(&msgs).unwrap();
        assert_eq!(outcome.chars().count(), 200, "200文字に切り詰め");
    }

    #[test]
    fn t_extract_last_outcome_empty() {
        let msgs = vec![Message::system("s"), Message::user("q")];
        assert!(extract_last_outcome(&msgs).is_none());
    }

    // --- 新規テスト: level3にツール統計と成果が含まれる ---

    #[test]
    fn t_l3_handoff_includes_tool_stats() {
        let mut msgs = mk_with_tool_calls(&[
            ("shell", "ok"),
            ("shell", "ok"),
            ("file_read", "content"),
            ("shell", "ok"),
            ("file_write", "done"),
            ("git", "committed"),
            ("shell", "final"),
        ]);
        let config = CompactionConfig {
            emergency_keep: 3,
            ..Default::default()
        };
        compact_level3(&mut msgs, &config);
        let handoff = msgs.iter().find(|m| m.content.contains("handoff")).unwrap();
        assert!(handoff.content.contains("Tool stats:"), "ツール統計が含まれるべき");
        assert!(handoff.content.contains("shell:"), "shellの統計が含まれるべき");
    }

    #[test]
    fn t_l3_handoff_includes_outcome() {
        let mut msgs = vec![Message::system("s")];
        for i in 0..8 {
            msgs.push(Message::user(format!("q{i}")));
            msgs.push(Message::assistant(format!("作業ステップ{i}を完了しました。次に進みます。")));
            msgs.push(Message::tool(format!("result{i}"), format!("t{i}")));
        }
        let config = CompactionConfig {
            emergency_keep: 3,
            ..Default::default()
        };
        compact_level3(&mut msgs, &config);
        let handoff = msgs.iter().find(|m| m.content.contains("handoff")).unwrap();
        assert!(handoff.content.contains("Last outcome:"), "最終成果が含まれるべき");
    }

    #[test]
    fn t_l3_handoff_includes_unresolved() {
        let mut msgs = vec![Message::system("s")];
        for i in 0..8 {
            msgs.push(Message::user(format!("q{i}")));
            msgs.push(Message::assistant(format!("ステップ{i}を実行します。長い文章にするため追加テキスト。")));
            if i == 3 {
                msgs.push(Message::tool("ツール実行エラー: ファイルが見つかりません".to_string(), format!("t{i}")));
            } else {
                msgs.push(Message::tool(format!("ok{i}"), format!("t{i}")));
            }
        }
        let config = CompactionConfig {
            emergency_keep: 3,
            ..Default::default()
        };
        compact_level3(&mut msgs, &config);
        let handoff = msgs.iter().find(|m| m.content.contains("handoff")).unwrap();
        assert!(handoff.content.contains("Unresolved"), "未解決事項が含まれるべき");
        assert!(handoff.content.contains("エラー"), "エラー内容が含まれるべき");
    
    // --- Preserved Thinking テスト ---

    #[test]
    fn t_extract_thinking_summary() {
        let msgs = vec![
            Message::assistant(
                "<think>まずファイル構造を確認する。\n次にテストを書く。\n結論: TDDアプローチで進める。</think>実装開始"
                    .to_string(),
            ),
            Message::assistant(
                "<think>エラーの原因を分析。\n借用チェッカーが問題。\n解決策: Cloneを導入する。</think>修正完了"
                    .to_string(),
            ),
        ];
        let summaries = extract_thinking_summary(&msgs);
        assert_eq!(summaries.len(), 2, "2つのthinkブロックからサマリー抽出");
        assert!(summaries[0].contains("TDD"), "最後の文（結論）が抽出される");
        assert!(summaries[1].contains("Clone"), "2つ目の結論も抽出される");
    }

    #[test]
    fn t_extract_thinking_empty() {
        let msgs = vec![
            Message::assistant("ツール呼び出しのみ".to_string()),
            Message::user("質問"),
        ];
        let summaries = extract_thinking_summary(&msgs);
        assert!(summaries.is_empty(), "thinkブロックがなければ空");
    }

    #[test]
    fn t_score_importance_user() {
        let msg = Message::user("タスクの定義");
        assert_eq!(score_message_importance(&msg), 1.0, "Userメッセージは最高スコア");
    }

    #[test]
    fn t_score_importance_error() {
        let msg = Message::tool("error: file not found", "t1");
        assert_eq!(score_message_importance(&msg), 0.2, "エラーToolは低スコア");
    }

    #[test]
    fn t_level2_preserves_thinking() {
        let mut msgs = vec![Message::system("s")];
        for i in 0..6 {
            msgs.push(Message::user(format!("q{i}")));
            msgs.push(Message::assistant(format!(
                "<think>ステップ{i}の分析。{}結論: 方針{i}で進める。</think>{}",
                "x".repeat(300),
                "y".repeat(100),
            )));
            msgs.push(Message::tool(format!("result{i}"), format!("t{i}")));
        }
        let config = CompactionConfig {
            placeholder_keep_recent: 3,
            summary_max_chars: 50,
            ..Default::default()
        };
        compact_level2(&mut msgs, &config);
        let has_preserved = msgs.iter().any(|m| m.content.contains("[Preserved Thinking]"));
        assert!(has_preserved, "level2後に思考サマリーが残るべき");
    }

    // --- 重要度スコア追加テスト ---

    #[test]
    fn t_score_importance_tool_call() {
        let msg = Message::assistant(r#"<tool_call>{"name":"shell"}</tool_call>"#);
        assert_eq!(score_message_importance(&msg), 0.7, "tool_call含むAssistantは0.7");
    }

    #[test]
    fn t_score_importance_system_context() {
        let msg = Message::system("<context>injected</context>");
        assert_eq!(score_message_importance(&msg), 0.3, "注入コンテキストSystemは0.3");
    }

    #[test]
    fn t_score_importance_system_normal() {
        let msg = Message::system("通常のシステムプロンプト");
        assert_eq!(score_message_importance(&msg), 0.9, "通常Systemは0.9");
    }

}
