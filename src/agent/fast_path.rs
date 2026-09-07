//! 反射層の軽量代替 — 高速ファストパス・ディスパッチャ。
//!
//! 12本の神経系や常時センサーを持つフルスケール反射層は過剰装備となるため、
//! 会話パイプラインの入口で定型入力・低価値な入力をルールベース（正規表現＋クールダウン）で
//! 即答し、重いLLM推論（Bonsai-8B/Unsloth）をバイパスする。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use regex::Regex;

/// ファストパスの判定ルール
pub struct FastPathRule {
    pub name: String,
    pub pattern: Regex,
    pub response: String,
    pub cooldown: Duration,
}

impl FastPathRule {
    pub fn new(
        name: impl Into<String>,
        pattern: &str,
        response: impl Into<String>,
        cooldown: Duration,
    ) -> Result<Self, regex::Error> {
        Ok(Self {
            name: name.into(),
            pattern: Regex::new(pattern)?,
            response: response.into(),
            cooldown,
        })
    }
}

/// 高速ファストパス・ディスパッチャ
pub struct FastPathDispatcher {
    rules: Vec<FastPathRule>,
    last_triggered: HashMap<String, Instant>,
}

impl FastPathDispatcher {
    /// 空のディスパッチャを生成
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            last_triggered: HashMap::new(),
        }
    }

    /// デフォルトの定型ルール群を備えたディスパッチャを生成
    pub fn with_default_rules() -> Self {
        let mut dispatcher = Self::new();

        // 疎通確認・挨拶
        dispatcher.add_rule(
            FastPathRule::new("ping", r"(?i)^(ping)$", "pong", Duration::from_secs(1))
                .expect("valid regex"),
        );

        dispatcher.add_rule(
            FastPathRule::new(
                "greeting",
                r"(?i)^(hello|hi|こんにちは|おはよう|こんばんは)$",
                "こんにちは！bonsai-agentです。何かお手伝いできることはありますか？",
                Duration::from_secs(3),
            )
            .expect("valid regex"),
        );

        // バージョン照会
        dispatcher.add_rule(
            FastPathRule::new(
                "version",
                r"(?i)^(version|--version|-v|バージョン)$",
                "bonsai-agent v0.1.0",
                Duration::from_secs(1),
            )
            .expect("valid regex"),
        );

        dispatcher
    }

    /// ルールを追加
    pub fn add_rule(&mut self, rule: FastPathRule) {
        self.rules.push(rule);
    }

    /// 入力文字列を判定し、マッチかつクールダウン経過済みなら定型応答を返す。
    /// 未マッチまたはクールダウン中なら None を返す。
    pub fn try_handle(&mut self, input: &str) -> Option<String> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return None;
        }

        let now = Instant::now();

        for rule in &self.rules {
            if rule.pattern.is_match(trimmed) {
                // クールダウン検査
                if let Some(last) = self.last_triggered.get(&rule.name)
                    && now.duration_since(*last) < rule.cooldown
                {
                    // クールダウン中はバイパスせず本パイプラインへ流す（または無視）
                    return None;
                }

                self.last_triggered.insert(rule.name.clone(), now);
                return Some(rule.response.clone());
            }
        }

        None
    }
}

impl Default for FastPathDispatcher {
    fn default() -> Self {
        Self::with_default_rules()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fast_path_ping_pong() {
        let mut d1 = FastPathDispatcher::with_default_rules();
        assert_eq!(d1.try_handle("ping"), Some("pong".to_string()));

        let mut d2 = FastPathDispatcher::with_default_rules();
        assert_eq!(d2.try_handle("PING"), Some("pong".to_string()));
    }

    #[test]
    fn test_fast_path_greeting() {
        let mut dispatcher = FastPathDispatcher::with_default_rules();
        let res = dispatcher.try_handle("こんにちは");
        assert!(res.is_some());
        assert!(res.unwrap().contains("bonsai-agent"));
    }

    #[test]
    fn test_fast_path_cooldown() {
        let mut dispatcher = FastPathDispatcher::new();
        dispatcher.add_rule(
            FastPathRule::new("quick", "^test$", "ok", Duration::from_millis(50)).unwrap(),
        );

        // 1回目は即時成功
        assert_eq!(dispatcher.try_handle("test"), Some("ok".to_string()));

        // クールダウン中は None
        assert_eq!(dispatcher.try_handle("test"), None);

        // クールダウン消化後は再度成功
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(dispatcher.try_handle("test"), Some("ok".to_string()));
    }

    #[test]
    fn test_fast_path_unmatched() {
        let mut dispatcher = FastPathDispatcher::with_default_rules();
        assert_eq!(dispatcher.try_handle("Rustのコードを書いて"), None);
        assert_eq!(dispatcher.try_handle(""), None);
    }
}
