//! agent_loop の補助関数集約モジュール（refactor 4/8）
//!
//! 出力ハッシュ計算、不変条件チェック、成功/中断記録、回答整形といった
//! 雑多な補助関数を集約。

use crate::agent::conversation::{ParsedOutput, Role, Session};
use crate::memory::experience::{ExperienceStore, ExperienceType, RecordParams};
use crate::memory::skill::SkillStore;
use crate::memory::store::MemoryStore;

/// セッション末尾メッセージのハッシュを計算（StallDetector 用）
pub(super) fn compute_output_hash(session: &Session) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    if let Some(last) = session.messages.last() {
        last.content.hash(&mut h);
    }
    h.finish()
}

/// タスク完了時の不変条件チェック（PaperOrchestra知見）
pub(super) fn check_invariants(session: &Session, task_context: &str) -> Vec<String> {
    let mut violations = Vec::new();
    let tool_msgs: Vec<_> = session
        .messages
        .iter()
        .filter(|m| m.role == Role::Tool)
        .collect();
    if !tool_msgs.is_empty() {
        // tool_exec.rs:78 の実エラー format ("ツール実行エラー: ...") を prefix で照合。
        // 旧実装 `content.contains("エラー")` は file_read で読んだソース内の
        // 「エラー」「失敗」単語に偽陽性反応し、Lab 中に常時 0% 警告を出していた。
        let success_count = tool_msgs
            .iter()
            .filter(|m| {
                let trimmed = m.content.trim_start();
                !trimmed.starts_with("ツール実行エラー")
                    && !trimmed.starts_with("Error:")
                    && !trimmed.starts_with("[Tool error]")
            })
            .count();
        let rate = success_count as f64 / tool_msgs.len() as f64;
        if rate < 0.5 {
            violations.push(format!("ツール成功率が低い: {:.0}%", rate * 100.0));
        }
    }
    if let Some(answer) = session
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant)
        && answer.content.len() < 10
        && task_context.len() > 50
    {
        violations.push("回答が短すぎる可能性".to_string());
    }
    violations
}

/// 成功時のセッション保存・経験記録・スキル昇格
pub(super) fn record_success(
    store: Option<&MemoryStore>,
    session: &Session,
    task_context: &str,
    answer: &str,
) {
    let Some(s) = store else { return };
    let _ = s.save_session(session);
    let exp = ExperienceStore::new(s.conn());
    let _ = exp.record(&RecordParams {
        exp_type: ExperienceType::Success,
        task_context,
        action: answer,
        outcome: "completed",
        lesson: None,
        tool_name: None,
        error_type: None,
        error_detail: None,
    });
    let skills = SkillStore::new(s.conn());
    let _ = skills.promote_from_experiences(s.conn(), 3);
    let evo = crate::memory::evolution::EvolutionEngine::new(s);
    let _ = evo.auto_collect();
    let _ = evo.apply_improvements();
}

/// 中断時のセッション保存・経験記録
pub(super) fn record_abort(
    store: Option<&MemoryStore>,
    session: &Session,
    task_context: &str,
    reason: &str,
) {
    let Some(s) = store else { return };
    let _ = s.save_session(session);
    let exp = ExperienceStore::new(s.conn());
    let _ = exp.record(&RecordParams {
        exp_type: ExperienceType::Insight,
        task_context,
        action: "aborted",
        outcome: reason,
        lesson: Some(reason),
        tool_name: None,
        error_type: Some("Aborted"),
        error_detail: None,
    });
}

/// ParsedOutputから回答テキストを構築
pub(super) fn build_answer(parsed: &ParsedOutput) -> String {
    let raw = parsed
        .text
        .clone()
        .unwrap_or_else(|| "(回答なし)".to_string());
    clean_response(&raw)
}

pub(super) fn clean_response(text: &str) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    lines.dedup();
    let joined = lines.join("\n");
    let chars: Vec<char> = joined.chars().collect();
    if chars.len() > 100 {
        let half = chars.len() / 2;
        let first: String = chars[..half].iter().collect();
        let second: String = chars[half..].iter().collect();
        let check: String = first.chars().take(30).collect();
        if second.contains(&check) {
            return first.trim_end().to_string();
        }
    }
    joined
}
