use crate::agent::conversation::{Message, Role};
use crate::memory::store::MemoryStore;
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
    for (i, msg) in messages[..boundary].iter_mut().enumerate() {
        if protected.contains(&i) { continue; }
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
    for msg in messages[..t - config.placeholder_keep_recent].iter_mut() {
        if matches!(msg.role, Role::Assistant) && msg.content.len() > config.summary_max_chars {
            let s: String = msg.content.chars().take(config.summary_max_chars).collect();
            msg.content = format!("{s}...[summarized]");
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
/// 1bitモデルが指示と混同しないよう「Remaining Work」命名を使用。
fn build_handoff_summary(messages: &[Message], config: &CompactionConfig) -> Option<Message> {
    let boundary = messages.len().saturating_sub(config.emergency_keep);
    if boundary < 2 {
        return None;
    }
    // 圧縮対象のAssistantメッセージから要約を構築
    let mut resolved = Vec::new();
    let mut tools_used = Vec::new();
    for msg in &messages[..boundary] {
        if matches!(msg.role, Role::Assistant) && msg.content.len() > 20 {
            let preview: String = msg.content.chars().take(80).collect();
            resolved.push(preview);
        }
        if matches!(msg.role, Role::Tool) {
            if let Some(id) = &msg.tool_call_id {
                if !tools_used.contains(id) {
                    tools_used.push(id.clone());
                }
            }
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
    let tools_text = if tools_used.is_empty() {
        String::new()
    } else {
        format!("\nUsed tools: {}", tools_used.iter().take(5).cloned().collect::<Vec<_>>().join(", "))
    };
    Some(Message::system(format!(
        "[Context handoff] 前のアシスタントの作業を引き継ぎます。\n         Resolved:\n{resolved_text}{tools_text}\n         Remaining Work: 直近のメッセージに基づいて作業を続行してください。"
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
}