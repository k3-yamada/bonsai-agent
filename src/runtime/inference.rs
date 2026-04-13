use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::agent::conversation::Message;
use crate::cancel::CancellationToken;
use crate::tools::ToolSchema;

/// トークン使用量
#[derive(Debug, Clone)]
pub struct TokenUsage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub duration: Duration,
}

/// LLM生成結果（トークン計装付き）
#[derive(Debug, Clone)]
pub struct GenerateResult {
    pub text: String,
    pub usage: TokenUsage,
    pub model_id: String,
}

/// LLM推論バックエンドの抽象化。
/// LlamaCppBackend、OllamaBackend、MockLlmBackend等が実装する。
pub trait LlmBackend: Send + Sync {
    /// モデルIDを返す（キャッシュキーに使用）
    fn model_id(&self) -> &str;

    /// テキスト生成（同期）。推論はCPU/GPU boundなのでasyncにする意味がない。
    fn generate(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        on_token: &mut dyn FnMut(&str),
        cancel: &CancellationToken,
    ) -> Result<GenerateResult>;
}

/// テスト用モックバックエンド。スクリプト化されたレスポンスを順番に返す。
pub struct MockLlmBackend {
    responses: Mutex<Vec<String>>,
    model_id: String,
}

impl MockLlmBackend {
    pub fn new(responses: Vec<String>) -> Self {
        // 逆順にしてpop()で先頭から取り出せるようにする
        let mut reversed = responses;
        reversed.reverse();
        Self {
            responses: Mutex::new(reversed),
            model_id: "mock".to_string(),
        }
    }

    /// 単一レスポンスのモック
    pub fn single(response: impl Into<String>) -> Self {
        Self::new(vec![response.into()])
    }
}

impl LlmBackend for MockLlmBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn generate(
        &self,
        _messages: &[Message],
        _tools: &[ToolSchema],
        on_token: &mut dyn FnMut(&str),
        cancel: &CancellationToken,
    ) -> Result<GenerateResult> {
        if cancel.is_cancelled() {
            anyhow::bail!("キャンセルされました");
        }

        let mut responses = self.responses.lock().unwrap();
        let text = responses
            .pop()
            .unwrap_or_else(|| "（モックのレスポンスが枯渇しました）".to_string());

        let start = Instant::now();

        // ストリーミングコールバックをシミュレーション
        for word in text.split_whitespace() {
            if cancel.is_cancelled() {
                anyhow::bail!("キャンセルされました");
            }
            on_token(word);
            on_token(" ");
        }

        Ok(GenerateResult {
            text: text.clone(),
            usage: TokenUsage {
                prompt_tokens: 10, // ダミー値
                completion_tokens: text.split_whitespace().count(),
                duration: start.elapsed(),
            },
            model_id: self.model_id.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_single_response() {
        let mock = MockLlmBackend::single("こんにちは");
        let cancel = CancellationToken::new();
        let mut tokens = Vec::new();

        let result = mock
            .generate(&[], &[], &mut |t| tokens.push(t.to_string()), &cancel)
            .unwrap();

        assert_eq!(result.text, "こんにちは");
        assert_eq!(result.model_id, "mock");
        assert!(!tokens.is_empty());
    }

    #[test]
    fn test_mock_multiple_responses() {
        let mock = MockLlmBackend::new(vec!["最初の回答".to_string(), "2番目の回答".to_string()]);
        let cancel = CancellationToken::new();
        let noop = &mut |_: &str| {};

        let r1 = mock.generate(&[], &[], noop, &cancel).unwrap();
        assert_eq!(r1.text, "最初の回答");

        let r2 = mock.generate(&[], &[], noop, &cancel).unwrap();
        assert_eq!(r2.text, "2番目の回答");
    }

    #[test]
    fn test_mock_exhausted() {
        let mock = MockLlmBackend::new(vec![]);
        let cancel = CancellationToken::new();
        let noop = &mut |_: &str| {};

        let result = mock.generate(&[], &[], noop, &cancel).unwrap();
        assert!(result.text.contains("枯渇"));
    }

    #[test]
    fn test_mock_cancelled() {
        let mock = MockLlmBackend::single("回答");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let noop = &mut |_: &str| {};

        let result = mock.generate(&[], &[], noop, &cancel);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("キャンセル"));
    }

    #[test]
    fn test_token_usage() {
        let mock = MockLlmBackend::single("hello world test");
        let cancel = CancellationToken::new();
        let noop = &mut |_: &str| {};

        let result = mock.generate(&[], &[], noop, &cancel).unwrap();
        assert_eq!(result.usage.completion_tokens, 3);
        assert_eq!(result.usage.prompt_tokens, 10);
        assert!(result.usage.duration.as_nanos() > 0);
    }

    #[test]
    fn test_streaming_callback() {
        let mock = MockLlmBackend::single("A B C");
        let cancel = CancellationToken::new();
        let mut collected = String::new();

        mock.generate(&[], &[], &mut |t| collected.push_str(t), &cancel)
            .unwrap();

        // "A" + " " + "B" + " " + "C" + " "
        assert!(collected.contains("A"));
        assert!(collected.contains("B"));
        assert!(collected.contains("C"));
    }

    #[test]
    fn test_model_id() {
        let mock = MockLlmBackend::single("test");
        assert_eq!(mock.model_id(), "mock");
    }
}
