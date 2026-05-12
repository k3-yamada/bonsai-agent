use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;

use crate::agent::conversation::{Message, Session};
use crate::cancel::CancellationToken;
use crate::config::InferenceParams;
use crate::memory::store::MemoryStore;
use crate::observability::audit::AuditLog;
use crate::runtime::inference::{GenerateResult, LlmBackend, TokenUsage};
use crate::runtime::model_router::{CriticConfig, CriticMode, CriticOutcome};
use crate::tools::ToolSchema;

use super::advisor_inject::{force_separate_backend_panic, inject_critic_review};

static CRITIC_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn reset_critic_env() {
    unsafe {
        std::env::remove_var("BONSAI_CRITIC_ENABLED");
        std::env::remove_var("BONSAI_CRITIC_MODE");
        std::env::remove_var("BONSAI_CRITIC_TEMPERATURE");
        std::env::remove_var("BONSAI_CRITIC_MAX_USES");
        std::env::remove_var("BONSAI_CRITIC_HOOK");
        std::env::remove_var("BONSAI_CRITIC_DISAGREEMENT");
    }
}

#[derive(Default)]
struct SpyLlmBackend {
    calls: Mutex<Vec<(Vec<Message>, InferenceParams)>>,
    responses: Mutex<Vec<String>>,
}

impl SpyLlmBackend {
    fn new(responses: Vec<&str>) -> Self {
        let mut responses: Vec<String> = responses.into_iter().map(str::to_string).collect();
        responses.reverse();
        Self {
            calls: Mutex::new(Vec::new()),
            responses: Mutex::new(responses),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    fn captured_calls(&self) -> Vec<(Vec<Message>, InferenceParams)> {
        self.calls.lock().unwrap().clone()
    }
}

impl LlmBackend for SpyLlmBackend {
    fn model_id(&self) -> &str {
        "spy"
    }

    fn generate(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        on_token: &mut dyn FnMut(&str),
        cancel: &CancellationToken,
    ) -> Result<GenerateResult> {
        self.generate_with_params(
            messages,
            tools,
            on_token,
            cancel,
            &InferenceParams::default(),
        )
    }

    fn generate_with_params(
        &self,
        messages: &[Message],
        _tools: &[ToolSchema],
        on_token: &mut dyn FnMut(&str),
        cancel: &CancellationToken,
        params: &InferenceParams,
    ) -> Result<GenerateResult> {
        if cancel.is_cancelled() {
            anyhow::bail!("キャンセルされました");
        }
        self.calls
            .lock()
            .unwrap()
            .push((messages.to_vec(), params.clone()));
        let text = self
            .responses
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_else(|| "AGREE: default".to_string());
        on_token(&text);
        Ok(GenerateResult {
            text: text.clone(),
            usage: TokenUsage {
                prompt_tokens: 1,
                completion_tokens: text.split_whitespace().count(),
                duration: Duration::from_millis(1),
            },
            model_id: self.model_id().to_string(),
        })
    }
}

fn run_critic(
    critic: &mut CriticConfig,
    backend: &dyn LlmBackend,
    store: Option<&MemoryStore>,
) -> CriticOutcome {
    let mut session = Session::new();
    let cancel = CancellationToken::new();
    inject_critic_review(
        &mut session,
        critic,
        backend,
        &InferenceParams::default(),
        "テストタスク",
        "テスト回答",
        &cancel,
        store,
    )
}

#[test]
fn t_critic_config_default_disabled() {
    let critic = CriticConfig::default();
    assert!(!critic.enabled);
    assert_eq!(critic.mode, CriticMode::SamePromptDifferentTemperature);
    assert_eq!(critic.max_critic_uses, 3);
    assert!((critic.critic_temperature - 0.7).abs() < 1e-6);
}

#[test]
fn t_critic_config_env_enabled_parse() {
    let _g = CRITIC_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_critic_env();
    unsafe {
        std::env::set_var("BONSAI_CRITIC_ENABLED", "1");
        std::env::set_var("BONSAI_CRITIC_MODE", "different_prompt");
    }
    let critic = CriticConfig::from_env();
    assert!(critic.enabled);
    assert_eq!(critic.mode, CriticMode::DifferentSystemPrompt);
    reset_critic_env();
}

#[test]
fn t_critic_short_circuit_when_disabled() {
    let mut critic = CriticConfig {
        enabled: false,
        ..CriticConfig::default()
    };
    let backend = SpyLlmBackend::new(vec!["AGREE: should not call"]);
    let outcome = run_critic(&mut critic, &backend, None);
    assert_eq!(outcome, CriticOutcome::Skipped { reason: "disabled" });
    assert_eq!(backend.call_count(), 0);
}

#[test]
fn t_critic_invokes_backend_with_critic_prompt() {
    let mut critic = CriticConfig {
        enabled: true,
        mode: CriticMode::DifferentSystemPrompt,
        ..CriticConfig::default()
    };
    let backend = SpyLlmBackend::new(vec!["AGREE: looks correct"]);
    let outcome = run_critic(&mut critic, &backend, None);
    assert!(matches!(outcome, CriticOutcome::Agree { .. }));
    let calls = backend.captured_calls();
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0].0[0]
            .content
            .contains("あなたは1bitローカルLLMの**critic**")
    );
}

#[test]
fn t_critic_invokes_with_temperature_override() {
    let mut critic = CriticConfig {
        enabled: true,
        critic_temperature: 0.7,
        ..CriticConfig::default()
    };
    let backend = SpyLlmBackend::new(vec!["AGREE: ok"]);
    let mut session = Session::new();
    let cancel = CancellationToken::new();
    let base = InferenceParams {
        temperature: 0.3,
        ..InferenceParams::default()
    };
    let outcome = inject_critic_review(
        &mut session,
        &mut critic,
        &backend,
        &base,
        "task",
        "answer",
        &cancel,
        None,
    );
    assert!(matches!(outcome, CriticOutcome::Agree { .. }));
    let calls = backend.captured_calls();
    assert!((calls[0].1.temperature - 0.7).abs() < f64::EPSILON);
}

#[test]
fn t_critic_parses_agree_response() {
    let mut critic = CriticConfig {
        enabled: true,
        ..CriticConfig::default()
    };
    let backend = SpyLlmBackend::new(vec!["AGREE: looks correct"]);
    let outcome = run_critic(&mut critic, &backend, None);
    assert_eq!(
        outcome,
        CriticOutcome::Agree {
            raw_response: "AGREE: looks correct".to_string()
        }
    );
}

#[test]
fn t_critic_parses_disagree_with_revision() {
    let mut critic = CriticConfig {
        enabled: true,
        ..CriticConfig::default()
    };
    let backend = SpyLlmBackend::new(vec!["DISAGREE: missing X\n修正案: Y"]);
    let outcome = run_critic(&mut critic, &backend, None);
    assert_eq!(
        outcome,
        CriticOutcome::Disagree {
            raw_response: "DISAGREE: missing X\n修正案: Y".to_string(),
            suggested_revision: Some("Y".to_string()),
        }
    );
}

#[test]
fn t_critic_max_uses_enforced() {
    let mut critic = CriticConfig {
        enabled: true,
        max_critic_uses: 2,
        ..CriticConfig::default()
    };
    let backend = SpyLlmBackend::new(vec!["AGREE: 1", "AGREE: 2", "AGREE: 3"]);
    assert!(matches!(
        run_critic(&mut critic, &backend, None),
        CriticOutcome::Agree { .. }
    ));
    assert!(matches!(
        run_critic(&mut critic, &backend, None),
        CriticOutcome::Agree { .. }
    ));
    assert_eq!(
        run_critic(&mut critic, &backend, None),
        CriticOutcome::Skipped { reason: "max_uses" }
    );
    assert_eq!(backend.call_count(), 2);
}

#[test]
fn t_critic_audit_log_emitted() {
    let mut critic = CriticConfig {
        enabled: true,
        ..CriticConfig::default()
    };
    let backend = SpyLlmBackend::new(vec!["AGREE: ok"]);
    let store = MemoryStore::in_memory().unwrap();
    let outcome = run_critic(&mut critic, &backend, Some(&store));
    assert!(matches!(outcome, CriticOutcome::Agree { .. }));
    let entries = AuditLog::new(store.conn()).recent(10).unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry.action_type == "critic_call"),
        "critic_call audit entry should be emitted"
    );
}

#[test]
#[should_panic(expected = "Phase 2")]
fn t_critic_separate_backend_phase1_panic() {
    force_separate_backend_panic();
}

// === Codex audit LOW findings カバレッジ追加 (項目 226) ===

/// DISAGREE 応答に `修正案:` 行が無い場合は `suggested_revision = None` で返却される。
/// 既存 t_critic_parses_disagree_with_revision (Some 経路) と pair で contract を明示。
#[test]
fn t_critic_parses_disagree_without_revision_returns_none() {
    let mut critic = CriticConfig {
        enabled: true,
        ..CriticConfig::default()
    };
    let backend = SpyLlmBackend::new(vec!["DISAGREE: missing X (no revision line)"]);
    let outcome = run_critic(&mut critic, &backend, None);
    assert_eq!(
        outcome,
        CriticOutcome::Disagree {
            raw_response: "DISAGREE: missing X (no revision line)".to_string(),
            suggested_revision: None,
        }
    );
}

/// backend が Err を返した場合 `CriticOutcome::BackendError` で graceful degrade し、
/// `critic_calls_used` を消費しないこと (max_uses 枯渇による副作用を防ぐ)。
#[test]
fn t_critic_backend_error_returns_backend_error_outcome() {
    let mut critic = CriticConfig {
        enabled: true,
        ..CriticConfig::default()
    };
    let backend = SpyLlmBackend::new(vec!["AGREE: unreached"]);
    let mut session = Session::new();
    let cancel = CancellationToken::new();
    cancel.cancel(); // 事前 cancel で SpyLlmBackend が anyhow::bail! を返す
    let outcome = inject_critic_review(
        &mut session,
        &mut critic,
        &backend,
        &InferenceParams::default(),
        "task",
        "answer",
        &cancel,
        None,
    );
    assert!(
        matches!(outcome, CriticOutcome::BackendError { .. }),
        "backend エラー時 BackendError 期待、got: {outcome:?}"
    );
    assert_eq!(
        critic.critic_calls_used, 0,
        "backend error は max_uses budget を消費してはならない"
    );
}

/// `mode = SeparateBackend` の production 経路は `Skipped { reason: "phase2_unimplemented" }`
/// を返す (test の `should_panic` ヘルパーとは別経路、production code path で panic しない)。
#[test]
fn t_critic_separate_backend_skipped_in_production_path() {
    let mut critic = CriticConfig {
        enabled: true,
        mode: CriticMode::SeparateBackend,
        ..CriticConfig::default()
    };
    let backend = SpyLlmBackend::new(vec!["AGREE: unreached"]);
    let outcome = run_critic(&mut critic, &backend, None);
    assert_eq!(
        outcome,
        CriticOutcome::Skipped {
            reason: "phase2_unimplemented"
        }
    );
    assert_eq!(
        backend.call_count(),
        0,
        "SeparateBackend mode では Phase 1 で backend を呼んではならない"
    );
}

/// `BONSAI_CRITIC_TEMPERATURE=inf` 等の非有限値は default (0.7) にフォールバック (Codex audit LOW)。
/// `is_finite()` フィルタにより JSON encode / persistence 経路が壊れない。
#[test]
fn t_critic_temperature_env_non_finite_falls_back_to_default() {
    let _g = CRITIC_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_critic_env();
    unsafe {
        std::env::set_var("BONSAI_CRITIC_ENABLED", "1");
    }
    for bad in ["inf", "Inf", "-inf", "NaN", "nan"] {
        unsafe {
            std::env::set_var("BONSAI_CRITIC_TEMPERATURE", bad);
        }
        let critic = CriticConfig::from_env();
        assert!(
            (critic.critic_temperature - 0.7).abs() < 1e-6,
            "非有限値 {bad} は default 0.7 にフォールバックすべき、got {}",
            critic.critic_temperature
        );
    }
    reset_critic_env();
}
