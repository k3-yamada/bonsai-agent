use regex::Regex;
use std::sync::LazyLock;

/// 会話のフロー（流れ）からストック（蓄積すべき知識）を抽出するルール
#[derive(Debug, Clone)]
pub struct StockEntry {
    pub category: StockCategory,
    pub content: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StockCategory {
    Decision,    // 意思決定（「〜にした」「〜を選んだ」）
    Fact,        // 事実（「〜は〜である」）
    Preference,  // 好み（「〜が好き」「〜を使いたい」）
    Pattern,     // パターン（繰り返し出現する手順）
    Insight,     // 洞察（「〜だとわかった」）
    Todo,        // やるべきこと（「〜必要がある」）
}

impl StockCategory {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Decision => "decisions",
            Self::Fact => "facts",
            Self::Preference => "preferences",
            Self::Pattern => "patterns",
            Self::Insight => "insights",
            Self::Todo => "todos",
        }
    }
}

static DECISION_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| vec![
    Regex::new(r"(?i)(decided|chose|selected|we('ll| will) use|going with|にした|選んだ|決めた|採用|使うことに)").unwrap(),
]);
static FACT_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| vec![
    Regex::new(r"(?i)(is a|are a|means|である|とは|ということ|だとわかった|が判明)").unwrap(),
]);
static PREFERENCE_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| vec![
    Regex::new(r"(?i)(prefer|like|want|好き|使いたい|ほしい|がいい|にしたい|お願い)").unwrap(),
]);
static TODO_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| vec![
    Regex::new(r"(?i)(need to|should|must|TODO|やる|必要|すべき|しなきゃ|忘れずに|覚えて)").unwrap(),
]);
static INSIGHT_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| vec![
    Regex::new(r"(?i)(realized|learned|found out|turns out|わかった|気づいた|発見|学んだ|判明)").unwrap(),
]);

/// ユーザーメッセージからストック候補を抽出
pub fn extract_stock(message: &str, source: &str) -> Vec<StockEntry> {
    let mut entries = Vec::new();
    // 短すぎるメッセージはスキップ
    if message.len() < 10 { return entries; }

    let checks: Vec<(&[Regex], StockCategory)> = vec![
        (&DECISION_PATTERNS, StockCategory::Decision),
        (&TODO_PATTERNS, StockCategory::Todo),
        (&INSIGHT_PATTERNS, StockCategory::Insight),
        (&PREFERENCE_PATTERNS, StockCategory::Preference),
        (&FACT_PATTERNS, StockCategory::Fact),
    ];

    for (patterns, category) in &checks {
        for pat in patterns.iter() {
            if pat.is_match(message) {
                entries.push(StockEntry {
                    category: category.clone(),
                    content: message.to_string(),
                    source: source.to_string(),
                });
                break; // 1カテゴリにつき1エントリ
            }
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn t_decision_ja() { let e = extract_stock("Rustを使うことにした", "s1"); assert!(e.iter().any(|x| x.category == StockCategory::Decision)); }
    #[test] fn t_decision_en() { let e = extract_stock("We decided to use reqwest", "s1"); assert!(e.iter().any(|x| x.category == StockCategory::Decision)); }
    #[test] fn t_todo() { let e = extract_stock("テストを書く必要がある", "s1"); assert!(e.iter().any(|x| x.category == StockCategory::Todo)); }
    #[test] fn t_insight() { let e = extract_stock("ureqではSSLが動かないとわかった", "s1"); assert!(e.iter().any(|x| x.category == StockCategory::Insight)); }
    #[test] fn t_preference() { let e = extract_stock("git-first設計がいい", "s1"); assert!(e.iter().any(|x| x.category == StockCategory::Preference)); }
    #[test] fn t_short_skip() { let e = extract_stock("ok", "s1"); assert!(e.is_empty()); }
    #[test] fn t_no_match() { let e = extract_stock("ファイルを読んで", "s1"); assert!(e.is_empty()); }
}
