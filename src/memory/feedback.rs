use regex::Regex;
use std::sync::LazyLock;

/// ユーザーフィードバックの種類（DeerFlow方式）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedbackType {
    /// 修正（エージェントの出力が間違っていた）
    Correction,
    /// 強化（エージェントの出力が正しかった）
    Reinforcement,
    /// 中立（フィードバックなし）
    Neutral,
}

/// 検出結果
#[derive(Debug, Clone)]
pub struct FeedbackDetection {
    pub feedback_type: FeedbackType,
    pub confidence: f32,
    pub matched_pattern: Option<String>,
}

static CORRECTION_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)(that'?s\s+wrong|incorrect|not\s+right|redo|try\s+again|fix\s+(this|that|it))",
        r"(?i)(no,?\s+i\s+(meant|want|need)|that'?s\s+not\s+what)",
        r"(違う|間違|やり直|もう一度|そうじゃな|ちがう|正しくない|修正して|直して)",
        r"(ダメ|だめ|使えない|おかしい|変だ|へんだ)",
    ]
    .iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});

static REINFORCEMENT_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)(perfect|exactly|that'?s\s+(right|correct|great|it)|well\s+done|nice|good\s+job)",
        r"(?i)(yes,?\s+that'?s\s+what|thanks|thank\s+you|awesome|excellent)",
        r"(完璧|正解|そうそう|その通り|ありがとう|いいね|素晴らしい|OK|おk|ok)",
        r"(合ってる|あってる|正しい|良い|いい感じ|バッチリ|ばっちり)",
    ]
    .iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});

/// ユーザーメッセージからフィードバックの種類を検出
pub fn detect_feedback(message: &str) -> FeedbackDetection {
    // 修正パターンを先にチェック（修正は強化より優先）
    for pattern in CORRECTION_PATTERNS.iter() {
        if let Some(m) = pattern.find(message) {
            return FeedbackDetection {
                feedback_type: FeedbackType::Correction,
                confidence: 0.95,
                matched_pattern: Some(m.as_str().to_string()),
            };
        }
    }

    for pattern in REINFORCEMENT_PATTERNS.iter() {
        if let Some(m) = pattern.find(message) {
            return FeedbackDetection {
                feedback_type: FeedbackType::Reinforcement,
                confidence: 0.9,
                matched_pattern: Some(m.as_str().to_string()),
            };
        }
    }

    FeedbackDetection {
        feedback_type: FeedbackType::Neutral,
        confidence: 0.0,
        matched_pattern: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_correction_english() {
        let r = detect_feedback("That's wrong, try again");
        assert_eq!(r.feedback_type, FeedbackType::Correction);
        assert!(r.confidence >= 0.9);
    }

    #[test]
    fn test_correction_japanese() {
        let r = detect_feedback("違う、やり直して");
        assert_eq!(r.feedback_type, FeedbackType::Correction);
    }

    #[test]
    fn test_correction_fix() {
        let r = detect_feedback("No, fix this please");
        assert_eq!(r.feedback_type, FeedbackType::Correction);
    }

    #[test]
    fn test_reinforcement_english() {
        let r = detect_feedback("Perfect, that's exactly what I wanted");
        assert_eq!(r.feedback_type, FeedbackType::Reinforcement);
    }

    #[test]
    fn test_reinforcement_japanese() {
        let r = detect_feedback("完璧！ありがとう");
        assert_eq!(r.feedback_type, FeedbackType::Reinforcement);
    }

    #[test]
    fn test_reinforcement_thanks() {
        let r = detect_feedback("Thanks, good job");
        assert_eq!(r.feedback_type, FeedbackType::Reinforcement);
    }

    #[test]
    fn test_neutral() {
        let r = detect_feedback("次にファイルを読んで");
        assert_eq!(r.feedback_type, FeedbackType::Neutral);
    }

    #[test]
    fn test_correction_takes_priority() {
        // 修正と強化の両方にマッチしそうな場合、修正が優先
        let r = detect_feedback("No that's wrong, redo it");
        assert_eq!(r.feedback_type, FeedbackType::Correction);
    }
}
