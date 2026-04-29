use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::agent::conversation::Message;
use crate::cancel::CancellationToken;
use crate::observability::logger::{LogLevel, log_event};
use crate::runtime::model_router::{FallbackChain, FallbackEntry};
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

    /// タスク種別に応じた推論パラメータで生成（デフォルト実装: generate()に委譲）
    fn generate_with_params(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        on_token: &mut dyn FnMut(&str),
        cancel: &CancellationToken,
        _params: &crate::config::InferenceParams,
    ) -> Result<GenerateResult> {
        self.generate(messages, tools, on_token, cancel)
    }
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

// ──────────────────────────────────────────────────────────────────────
// FallbackBackend（Step 12 — メイン推論フォールバックラッパー）
// ──────────────────────────────────────────────────────────────────────

/// 複数バックエンドを `FallbackChain` 経由で切替えながら推論する LlmBackend
///
/// 構築時に `(FallbackEntry → Box<dyn LlmBackend>)` のマップを受け取り、
/// `generate` 失敗時に `record_failure` で次のバックエンドへ自動切替する。
pub struct FallbackBackend {
    chain: Arc<FallbackChain>,
    backends: HashMap<String, Box<dyn LlmBackend>>,
    /// model_id() が返す合成識別子
    synthetic_id: String,
}

impl FallbackBackend {
    pub fn new(chain: FallbackChain, backends: HashMap<String, Box<dyn LlmBackend>>) -> Self {
        Self {
            chain: Arc::new(chain),
            backends,
            synthetic_id: "fallback-chain".to_string(),
        }
    }

    pub fn key_for(entry: &FallbackEntry) -> String {
        format!("{:?}:{}", entry.backend, entry.model_id)
    }

    pub fn chain(&self) -> &FallbackChain {
        &self.chain
    }
}

impl LlmBackend for FallbackBackend {
    fn model_id(&self) -> &str {
        &self.synthetic_id
    }

    fn generate(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        on_token: &mut dyn FnMut(&str),
        cancel: &CancellationToken,
    ) -> Result<GenerateResult> {
        let mut last_err: Option<anyhow::Error> = None;
        // チェーン全長分まで試行（同一エントリでの retry も含む）
        let max_attempts = self.chain.entries().len().saturating_mul(2).max(1);
        for _ in 0..max_attempts {
            let Some(entry) = self.chain.current() else {
                break;
            };
            let key = Self::key_for(entry);
            let Some(backend) = self.backends.get(&key) else {
                anyhow::bail!("fallback backend not registered: {key}");
            };
            match backend.generate(messages, tools, on_token, cancel) {
                Ok(result) => {
                    self.chain.record_success();
                    return Ok(result);
                }
                Err(e) => {
                    log_event(
                        LogLevel::Warn,
                        "fallback",
                        &format!("backend {key} failed: {e}"),
                    );
                    let switched = self.chain.record_failure();
                    if switched.is_none() && self.chain.is_exhausted() {
                        last_err = Some(e);
                        break;
                    }
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("fallback chain exhausted")))
    }

    fn generate_with_params(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        on_token: &mut dyn FnMut(&str),
        cancel: &CancellationToken,
        params: &crate::config::InferenceParams,
    ) -> Result<GenerateResult> {
        let mut last_err: Option<anyhow::Error> = None;
        let max_attempts = self.chain.entries().len().saturating_mul(2).max(1);
        for _ in 0..max_attempts {
            let Some(entry) = self.chain.current() else {
                break;
            };
            let key = Self::key_for(entry);
            let Some(backend) = self.backends.get(&key) else {
                anyhow::bail!("fallback backend not registered: {key}");
            };
            match backend.generate_with_params(messages, tools, on_token, cancel, params) {
                Ok(result) => {
                    self.chain.record_success();
                    return Ok(result);
                }
                Err(e) => {
                    log_event(
                        LogLevel::Warn,
                        "fallback",
                        &format!("backend {key} failed (with_params): {e}"),
                    );
                    let switched = self.chain.record_failure();
                    if switched.is_none() && self.chain.is_exhausted() {
                        last_err = Some(e);
                        break;
                    }
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("fallback chain exhausted")))
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

    // ─── Step 12 FallbackBackend tests ────────────────────────────────

    use crate::config::ServerBackend;
    use crate::runtime::model_router::FallbackChain;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 指定回数だけ失敗してから成功を返すモックバックエンド
    struct FlakyBackend {
        id: String,
        fail_remaining: AtomicUsize,
        success_text: String,
    }

    impl FlakyBackend {
        fn new(id: &str, fail_count: usize, success_text: &str) -> Self {
            Self {
                id: id.to_string(),
                fail_remaining: AtomicUsize::new(fail_count),
                success_text: success_text.to_string(),
            }
        }
    }

    impl LlmBackend for FlakyBackend {
        fn model_id(&self) -> &str {
            &self.id
        }
        fn generate(
            &self,
            _messages: &[Message],
            _tools: &[ToolSchema],
            _on_token: &mut dyn FnMut(&str),
            _cancel: &CancellationToken,
        ) -> Result<GenerateResult> {
            let remaining = self.fail_remaining.load(Ordering::SeqCst);
            if remaining > 0 {
                self.fail_remaining.fetch_sub(1, Ordering::SeqCst);
                anyhow::bail!(
                    "flaky: forced failure ({remaining} remaining) for {}",
                    self.id
                );
            }
            Ok(GenerateResult {
                text: self.success_text.clone(),
                usage: TokenUsage {
                    prompt_tokens: 1,
                    completion_tokens: 1,
                    duration: Duration::from_millis(1),
                },
                model_id: self.id.clone(),
            })
        }
    }

    fn fb_entry(id: &str) -> FallbackEntry {
        FallbackEntry {
            backend: ServerBackend::MlxLm,
            model_id: id.to_string(),
            server_url: format!("http://localhost:8000/{id}"),
        }
    }

    fn build_fallback(primary_fails: usize, secondary_fails: usize) -> FallbackBackend {
        let entries = vec![fb_entry("primary"), fb_entry("secondary")];
        let chain = FallbackChain::new(entries.clone());
        let mut backends: HashMap<String, Box<dyn LlmBackend>> = HashMap::new();
        backends.insert(
            FallbackBackend::key_for(&entries[0]),
            Box::new(FlakyBackend::new("primary", primary_fails, "from-primary")),
        );
        backends.insert(
            FallbackBackend::key_for(&entries[1]),
            Box::new(FlakyBackend::new(
                "secondary",
                secondary_fails,
                "from-secondary",
            )),
        );
        FallbackBackend::new(chain, backends)
    }

    #[test]
    fn t_fallback_backend_uses_primary_on_success() {
        let fb = build_fallback(0, 0);
        let cancel = CancellationToken::new();
        let result = fb
            .generate(&[], &[], &mut |_| {}, &cancel)
            .expect("primary should succeed");
        assert_eq!(result.text, "from-primary");
        assert_eq!(result.model_id, "primary");
    }

    #[test]
    fn t_fallback_backend_switches_after_threshold() {
        // primary が 2 回失敗 → secondary に切替後成功（1 回の generate 呼出で完結）
        let fb = build_fallback(2, 0);
        let cancel = CancellationToken::new();
        let result = fb
            .generate(&[], &[], &mut |_| {}, &cancel)
            .expect("should succeed via secondary");
        assert_eq!(result.text, "from-secondary");
        assert_eq!(result.model_id, "secondary");
        // chain が secondary 位置に居る
        assert_eq!(fb.chain().current().unwrap().model_id, "secondary");
    }

    #[test]
    fn t_fallback_backend_returns_err_when_all_fail() {
        // 両方とも常時失敗
        let fb = build_fallback(usize::MAX / 2, usize::MAX / 2);
        let cancel = CancellationToken::new();
        let result = fb.generate(&[], &[], &mut |_| {}, &cancel);
        assert!(result.is_err(), "all-fail should return err");
    }

    #[test]
    fn t_fallback_backend_records_success_after_recovery() {
        // primary が 2 回失敗、secondary 成功 → record_success が呼ばれる
        let fb = build_fallback(2, 0);
        let cancel = CancellationToken::new();
        let _ = fb.generate(&[], &[], &mut |_| {}, &cancel);
        // 続けてもう一度 generate → secondary がそのまま使われる（カウンタリセット済）
        let r = fb.generate(&[], &[], &mut |_| {}, &cancel).unwrap();
        assert_eq!(r.model_id, "secondary");
    }

    #[test]
    fn t_fallback_backend_synthetic_model_id() {
        let fb = build_fallback(0, 0);
        assert_eq!(fb.model_id(), "fallback-chain");
    }
}
