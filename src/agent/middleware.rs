//! エージェントループのミドルウェアチェーン（DeerFlow知見）
//!
//! 各関心事を独立したミドルウェアに分離し、テスト・追加・削除を容易にする。
//! before_step / after_step のフックポイントで、ループの前後処理をパイプライン化。

use crate::agent::agent_loop::{StallDetector, TokenBudgetTracker};
use crate::agent::compaction::{CompactionConfig, compact_if_needed};
use crate::agent::conversation::{Message, Session};
use crate::memory::store::MemoryStore;
use crate::observability::audit::{AuditAction, AuditLog};
use crate::observability::logger::{log_event, LogLevel};

/// ステップ実行結果のコンテキスト（ミドルウェアに渡す）
pub struct StepResult {
    pub outcome_type: &'static str,
    pub iteration: usize,
    pub duration_ms: u64,
    pub tools_used: Vec<String>,
    pub tools_succeeded: bool,
    pub output_hash: u64,
    pub consecutive_failures: usize,
}

/// ミドルウェアのアクション指示
pub enum MiddlewareSignal {
    Ok,
    Inject(String),
}

/// エージェントループのミドルウェアトレイト
pub trait Middleware {
    fn name(&self) -> &str;
    fn after_step(&mut self, session: &mut Session, result: &StepResult) -> MiddlewareSignal;
}

/// ミドルウェアチェーン — 登録順に実行
pub struct MiddlewareChain {
    middlewares: Vec<Box<dyn Middleware>>,
}

impl MiddlewareChain {
    pub fn new() -> Self {
        Self { middlewares: Vec::new() }
    }

    pub fn add(&mut self, mw: Box<dyn Middleware>) {
        self.middlewares.push(mw);
    }

    pub fn run_after_step(&mut self, session: &mut Session, result: &StepResult) {
        for mw in &mut self.middlewares {
            match mw.after_step(session, result) {
                MiddlewareSignal::Ok => {}
                MiddlewareSignal::Inject(msg) => {
                    session.add_message(Message::system(msg));
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.middlewares.len()
    }

    pub fn is_empty(&self) -> bool {
        self.middlewares.is_empty()
    }

    pub fn names(&self) -> Vec<&str> {
        self.middlewares.iter().map(|m| m.name()).collect()
    }
}

impl Default for MiddlewareChain {
    fn default() -> Self {
        Self::new()
    }
}

// --- 具象ミドルウェア ---

/// 1. 監査ログミドルウェア
pub struct AuditMiddleware {
    session_id: String,
    store: Option<*const MemoryStore>,
}

impl AuditMiddleware {
    /// # Safety
    /// store のライフタイムは MiddlewareChain より長くなければならない。
    pub unsafe fn new(session_id: String, store: Option<&MemoryStore>) -> Self {
        Self {
            session_id,
            store: store.map(|s| s as *const MemoryStore),
        }
    }
}

impl Middleware for AuditMiddleware {
    fn name(&self) -> &str { "audit" }

    fn after_step(&mut self, _session: &mut Session, result: &StepResult) -> MiddlewareSignal {
        if let Some(ptr) = self.store {
            let store = unsafe { &*ptr };
            let audit = AuditLog::new(store.conn());
            let _ = audit.log(
                Some(&self.session_id),
                &AuditAction::StepOutcome {
                    step_index: result.iteration,
                    outcome: result.outcome_type.to_string(),
                    duration_ms: result.duration_ms,
                    tools_used: result.tools_used.clone(),
                    consecutive_failures: result.consecutive_failures,
                },
            );
        }
        MiddlewareSignal::Ok
    }
}

/// 2. ツール追跡ミドルウェア
pub struct ToolTrackingMiddleware {
    pub all_tools: Vec<String>,
}

impl ToolTrackingMiddleware {
    pub fn new() -> Self { Self { all_tools: Vec::new() } }
}

impl Default for ToolTrackingMiddleware {
    fn default() -> Self { Self::new() }
}

impl Middleware for ToolTrackingMiddleware {
    fn name(&self) -> &str { "tool_tracking" }

    fn after_step(&mut self, _session: &mut Session, result: &StepResult) -> MiddlewareSignal {
        self.all_tools.extend(result.tools_used.clone());
        MiddlewareSignal::Ok
    }
}

/// 3. 停滞検出ミドルウェア
pub struct StallMiddleware {
    detector: StallDetector,
}

impl StallMiddleware {
    pub fn new(threshold: usize) -> Self {
        Self { detector: StallDetector::new(threshold) }
    }
}

impl Default for StallMiddleware {
    fn default() -> Self { Self::new(3) }
}

impl Middleware for StallMiddleware {
    fn name(&self) -> &str { "stall" }

    fn after_step(&mut self, _session: &mut Session, result: &StepResult) -> MiddlewareSignal {
        if result.outcome_type != "continue" {
            return MiddlewareSignal::Ok;
        }
        if self.detector.record_step(result.tools_succeeded, result.output_hash) {
            self.detector.reset();
            log_event(LogLevel::Warn, "middleware:stall", "停滞検出 → 再計画促進");
            MiddlewareSignal::Inject(
                "【停滞検出】進捗がありません。現在の問題を分析し、別のアプローチを試してください。\n\
                 1. 何が妨げているか特定\n\
                 2. 別のツールまたはパラメータを検討\n\
                 3. タスクを小さく分割".to_string(),
            )
        } else {
            MiddlewareSignal::Ok
        }
    }
}

/// 4. コンパクションミドルウェア
pub struct CompactionMiddleware {
    config: CompactionConfig,
}

impl CompactionMiddleware {
    pub fn new(config: CompactionConfig) -> Self { Self { config } }
}

impl Default for CompactionMiddleware {
    fn default() -> Self { Self::new(CompactionConfig::default()) }
}

impl Middleware for CompactionMiddleware {
    fn name(&self) -> &str { "compaction" }

    fn after_step(&mut self, session: &mut Session, result: &StepResult) -> MiddlewareSignal {
        if result.outcome_type != "continue" {
            return MiddlewareSignal::Ok;
        }
        let (lv, _offloaded) = compact_if_needed(&mut session.messages, &self.config);
        if lv > 0 {
            log_event(
                LogLevel::Debug,
                "middleware:compact",
                &format!("level {lv} applied (iter {})", result.iteration),
            );
        }
        MiddlewareSignal::Ok
    }
}

/// 5. トークン予算ミドルウェア
pub struct TokenBudgetMiddleware {
    tracker: TokenBudgetTracker,
}

impl TokenBudgetMiddleware {
    pub fn new(budget: usize) -> Self {
        Self { tracker: TokenBudgetTracker::new(budget) }
    }
}

impl Default for TokenBudgetMiddleware {
    fn default() -> Self { Self::new(8000) }
}

impl Middleware for TokenBudgetMiddleware {
    fn name(&self) -> &str { "token_budget" }

    fn after_step(&mut self, _session: &mut Session, result: &StepResult) -> MiddlewareSignal {
        if result.outcome_type != "continue" {
            return MiddlewareSignal::Ok;
        }
        let approx_tokens = result.tools_used.len() * 200 + 100;
        self.tracker.record(approx_tokens);
        match self.tracker.check() {
            Some(msg) => MiddlewareSignal::Inject(msg.to_string()),
            None => MiddlewareSignal::Ok,
        }
    }
}

/// デフォルト5段ミドルウェアチェーンを構築
///
/// # Safety
/// store のライフタイムは返却される MiddlewareChain より長くなければならない。
pub unsafe fn build_default_chain(
    session_id: &str,
    store: Option<&MemoryStore>,
) -> MiddlewareChain {
    let mut chain = MiddlewareChain::new();
    chain.add(Box::new(unsafe { AuditMiddleware::new(session_id.to_string(), store) }));
    chain.add(Box::new(ToolTrackingMiddleware::new()));
    // StallMiddleware は除外: Advisor連携付きの inject_replan_on_stall() が上位互換
    chain.add(Box::new(CompactionMiddleware::default()));
    chain.add(Box::new(TokenBudgetMiddleware::default()));
    chain
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::conversation::Session;

    fn make_continue_result(iteration: usize, tools: Vec<String>) -> StepResult {
        StepResult {
            outcome_type: "continue",
            iteration,
            duration_ms: 100,
            tools_used: tools,
            tools_succeeded: true,
            output_hash: iteration as u64,
            consecutive_failures: 0,
        }
    }

    fn make_final_result() -> StepResult {
        StepResult {
            outcome_type: "final_answer",
            iteration: 0,
            duration_ms: 50,
            tools_used: vec![],
            tools_succeeded: true,
            output_hash: 0,
            consecutive_failures: 0,
        }
    }

    #[test]
    fn test_chain_empty() {
        let mut chain = MiddlewareChain::new();
        let mut session = Session::new();
        let result = make_continue_result(0, vec!["echo".to_string()]);
        chain.run_after_step(&mut session, &result);
        assert!(chain.is_empty());
    }

    #[test]
    fn test_chain_ordering() {
        let mut chain = MiddlewareChain::new();
        chain.add(Box::new(ToolTrackingMiddleware::new()));
        chain.add(Box::new(StallMiddleware::default()));
        chain.add(Box::new(CompactionMiddleware::default()));
        assert_eq!(chain.len(), 3);
        assert_eq!(chain.names(), vec!["tool_tracking", "stall", "compaction"]);
    }

    #[test]
    fn test_tool_tracking_accumulates() {
        let mut mw = ToolTrackingMiddleware::new();
        let mut session = Session::new();
        let r1 = make_continue_result(0, vec!["shell".to_string()]);
        let r2 = make_continue_result(1, vec!["file_read".to_string(), "git".to_string()]);
        mw.after_step(&mut session, &r1);
        mw.after_step(&mut session, &r2);
        assert_eq!(mw.all_tools, vec!["shell", "file_read", "git"]);
    }

    #[test]
    fn test_stall_detects_after_threshold() {
        let mut mw = StallMiddleware::new(3);
        let mut session = Session::new();
        let r = StepResult {
            outcome_type: "continue",
            iteration: 0,
            duration_ms: 100,
            tools_used: vec![],
            tools_succeeded: false,
            output_hash: 42,
            consecutive_failures: 1,
        };
        assert!(matches!(mw.after_step(&mut session, &r), MiddlewareSignal::Ok));
        assert!(matches!(mw.after_step(&mut session, &r), MiddlewareSignal::Ok));
        assert!(matches!(mw.after_step(&mut session, &r), MiddlewareSignal::Inject(_)));
    }

    #[test]
    fn test_stall_skips_non_continue() {
        let mut mw = StallMiddleware::new(1);
        let mut session = Session::new();
        let r = make_final_result();
        assert!(matches!(mw.after_step(&mut session, &r), MiddlewareSignal::Ok));
    }

    #[test]
    fn test_compaction_runs_without_panic() {
        let mut mw = CompactionMiddleware::default();
        let mut session = Session::new();
        let r = make_continue_result(0, vec![]);
        assert!(matches!(mw.after_step(&mut session, &r), MiddlewareSignal::Ok));
    }

    #[test]
    fn test_token_budget_ok_initially() {
        let mut mw = TokenBudgetMiddleware::new(100_000);
        let mut session = Session::new();
        let r = make_continue_result(0, vec!["echo".to_string()]);
        assert!(matches!(mw.after_step(&mut session, &r), MiddlewareSignal::Ok));
    }

    #[test]
    fn test_token_budget_warns_near_limit() {
        let mut mw = TokenBudgetMiddleware::new(500);
        let mut session = Session::new();
        for i in 0..10 {
            let r = make_continue_result(i, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
            let _ = mw.after_step(&mut session, &r);
        }
        let r = make_continue_result(10, vec!["d".to_string()]);
        let signal = mw.after_step(&mut session, &r);
        assert!(matches!(signal, MiddlewareSignal::Inject(_)));
    }

    #[test]
    fn test_audit_no_store_no_panic() {
        let mut mw = unsafe { AuditMiddleware::new("test-session".to_string(), None) };
        let mut session = Session::new();
        let r = make_continue_result(0, vec![]);
        assert!(matches!(mw.after_step(&mut session, &r), MiddlewareSignal::Ok));
    }

    #[test]
    fn test_audit_with_store() {
        let store = MemoryStore::in_memory().unwrap();
        let mut mw = unsafe { AuditMiddleware::new("test-session".to_string(), Some(&store)) };
        let mut session = Session::new();
        let r = make_continue_result(0, vec!["shell".to_string()]);
        mw.after_step(&mut session, &r);
        let audit = AuditLog::new(store.conn());
        let entries = audit.for_session("test-session").unwrap();
        assert!(entries.iter().any(|e| e.action_type == "step_outcome"));
    }

    #[test]
    fn test_build_default_chain_has_5_middlewares() {
        let chain = unsafe { build_default_chain("test", None) };
        assert_eq!(chain.len(), 4);
        assert_eq!(chain.names(), vec!["audit", "tool_tracking", "compaction", "token_budget"]);
    }


    #[test]
    fn test_stall_default_threshold_is_3() {
        // StallMiddleware::default() の閾値が3であることを検証
        let mut mw = StallMiddleware::default();
        let mut session = Session::new();
        let r = StepResult {
            outcome_type: "continue",
            iteration: 0,
            duration_ms: 100,
            tools_used: vec![],
            tools_succeeded: false,
            output_hash: 42,
            consecutive_failures: 1,
        };
        // 1回目・2回目はOk
        assert!(matches!(mw.after_step(&mut session, &r), MiddlewareSignal::Ok));
        assert!(matches!(mw.after_step(&mut session, &r), MiddlewareSignal::Ok));
        // 3回目でInject（閾値3）
        assert!(matches!(mw.after_step(&mut session, &r), MiddlewareSignal::Inject(_)));
    }

    #[test]
    fn test_stall_resets_after_detection() {
        // 停滞検出後にリセットされ、再度閾値まで蓄積可能
        let mut mw = StallMiddleware::new(2);
        let mut session = Session::new();
        let r = StepResult {
            outcome_type: "continue",
            iteration: 0,
            duration_ms: 100,
            tools_used: vec![],
            tools_succeeded: false,
            output_hash: 42,
            consecutive_failures: 1,
        };
        // 2回で検出
        assert!(matches!(mw.after_step(&mut session, &r), MiddlewareSignal::Ok));
        assert!(matches!(mw.after_step(&mut session, &r), MiddlewareSignal::Inject(_)));
        // リセット後、再度2回必要
        assert!(matches!(mw.after_step(&mut session, &r), MiddlewareSignal::Ok));
        assert!(matches!(mw.after_step(&mut session, &r), MiddlewareSignal::Inject(_)));
    }
    #[test]
    fn test_chain_integration_run() {
        let mut chain = MiddlewareChain::new();
        chain.add(Box::new(ToolTrackingMiddleware::new()));
        chain.add(Box::new(StallMiddleware::default()));
        chain.add(Box::new(CompactionMiddleware::default()));
        chain.add(Box::new(TokenBudgetMiddleware::default()));
        let mut session = Session::new();
        for i in 0..5 {
            let r = make_continue_result(i, vec![format!("tool_{i}")]);
            chain.run_after_step(&mut session, &r);
        }
    }

    #[test]
    fn test_send_safe_middlewares() {
        fn _assert_send<T: Send>() {}
        _assert_send::<ToolTrackingMiddleware>();
        _assert_send::<StallMiddleware>();
        _assert_send::<CompactionMiddleware>();
        _assert_send::<TokenBudgetMiddleware>();
    }

    #[test]
    fn test_independent_chains_in_parallel_threads() {
        std::thread::scope(|s| {
            let handles: Vec<_> = (0..4)
                .map(|i| {
                    s.spawn(move || {
                        let mut chain = MiddlewareChain::new();
                        chain.add(Box::new(ToolTrackingMiddleware::new()));
                        chain.add(Box::new(StallMiddleware::default()));
                        let mut session = Session::new();
                        let r = make_continue_result(i, vec![format!("tool_{i}")]);
                        chain.run_after_step(&mut session, &r);
                        chain.len()
                    })
                })
                .collect();
            let results: Vec<usize> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            assert!(results.iter().all(|&len| len == 2));
        });
    }
}
