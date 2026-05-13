//! LongMemEval-S dataset loader.
//!
//! Schema は HuggingFace `xiaowu0162/longmemeval-cleaned` の `longmemeval_s_cleaned.json` 実物に準拠。
//! 500 Q × ~53 haystack_sessions / 1 entry。

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct LongMemEvalEntry {
    pub question_id: String,
    pub question_type: String,
    pub question: String,
    pub question_date: String,
    pub answer: String,
    pub answer_session_ids: Vec<String>,
    pub haystack_dates: Vec<String>,
    pub haystack_session_ids: Vec<String>,
    pub haystack_sessions: Vec<Vec<HaystackTurn>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HaystackTurn {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub has_answer: Option<bool>,
}

pub fn load_dataset(path: &Path) -> Result<Vec<LongMemEvalEntry>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let entries: Vec<LongMemEvalEntry> = serde_json::from_reader(reader)?;
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn fixture_single_entry_json() -> &'static str {
        r#"[
          {
            "question_id": "q-001",
            "question_type": "single-session-user",
            "question": "What did I say about deadlines?",
            "question_date": "2024-01-15",
            "answer": "You said the deadline was Friday.",
            "answer_session_ids": ["s-003"],
            "haystack_dates": ["2024-01-10", "2024-01-12", "2024-01-15"],
            "haystack_session_ids": ["s-001", "s-002", "s-003"],
            "haystack_sessions": [
              [{"role": "user", "content": "Hello"}, {"role": "assistant", "content": "Hi"}],
              [{"role": "user", "content": "How are you?"}],
              [{"role": "user", "content": "deadline is Friday", "has_answer": true}]
            ]
          }
        ]"#
    }

    #[test]
    fn test_dataset_parse_single_entry() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(fixture_single_entry_json().as_bytes())
            .unwrap();
        let entries = load_dataset(tmp.path()).unwrap();
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.question_id, "q-001");
        assert_eq!(e.question_type, "single-session-user");
        assert_eq!(e.answer_session_ids, vec!["s-003".to_string()]);
        assert_eq!(e.haystack_session_ids.len(), 3);
        assert_eq!(e.haystack_sessions.len(), 3);
        assert_eq!(e.haystack_sessions[2][0].has_answer, Some(true));
    }

    #[test]
    fn test_dataset_parse_empty_array() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"[]").unwrap();
        let entries = load_dataset(tmp.path()).unwrap();
        assert!(entries.is_empty());
    }
}
