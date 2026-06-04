//! B-1: MLX サーバ lifecycle を握る LlmBackend デコレーター。
//!
//! generate 前に ensure_running (lazy respawn) + record_request (idle timer reset)。
//! supervisor が disabled (timeout=0) の場合 ensure_running は no-op なので
//! 既存挙動を完全に保持する。

use std::sync::Arc;

use anyhow::Result;

use crate::cancel::CancellationToken;
use crate::domain::conversation::Message;
use crate::domain::llm::{GenerateResult, LlmBackend};
use crate::domain::tool_schema::ToolSchema;
use crate::runtime::process_supervisor::ProcessSupervisor;

pub struct SupervisedBackend {
    inner: Box<dyn LlmBackend>,
    supervisor: Arc<ProcessSupervisor>,
}

impl SupervisedBackend {
    pub fn new(inner: Box<dyn LlmBackend>, supervisor: Arc<ProcessSupervisor>) -> Self {
        Self { inner, supervisor }
    }
}

impl LlmBackend for SupervisedBackend {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    fn generate(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        on_token: &mut dyn FnMut(&str),
        cancel: &CancellationToken,
    ) -> Result<GenerateResult> {
        // lazy respawn: server が落ちていれば起動を試みる (disabled では no-op)。
        let _ = self.supervisor.ensure_running();
        self.supervisor.record_request();
        let r = self.inner.generate(messages, tools, on_token, cancel);
        // 推論完了時刻でも idle timer をリセット (長時間 generate 中の誤 kill 回避)。
        self.supervisor.record_request();
        r
    }

    fn generate_with_params(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        on_token: &mut dyn FnMut(&str),
        cancel: &CancellationToken,
        params: &crate::config::InferenceParams,
    ) -> Result<GenerateResult> {
        let _ = self.supervisor.ensure_running();
        self.supervisor.record_request();
        let r = self
            .inner
            .generate_with_params(messages, tools, on_token, cancel, params);
        self.supervisor.record_request();
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::llm::MockLlmBackend;

    /// disabled supervisor (timeout=0, empty spawn) で wrap した MockLlmBackend に
    /// 委譲し、mock のテキストを返す。ensure_running は disabled で no-op = 高速。
    #[test]
    fn t_supervised_delegates_to_inner() {
        let inner = Box::new(MockLlmBackend::single("hello"));
        let supervisor = Arc::new(ProcessSupervisor::new(
            "http://127.0.0.1:1/health".to_string(),
            0,
        ));
        let backend = SupervisedBackend::new(inner, supervisor);

        let cancel = CancellationToken::new();
        let mut sink = |_: &str| {};
        let r = backend
            .generate(&[], &[], &mut sink, &cancel)
            .expect("generate ok");
        assert_eq!(r.text, "hello", "inner mock のテキストを委譲返却");
    }

    /// model_id も inner に委譲。
    #[test]
    fn t_supervised_model_id_delegates() {
        let inner = Box::new(MockLlmBackend::single("x"));
        let supervisor = Arc::new(ProcessSupervisor::new(
            "http://127.0.0.1:1/health".to_string(),
            0,
        ));
        let backend = SupervisedBackend::new(inner, supervisor);
        assert_eq!(backend.model_id(), "mock");
    }
}
