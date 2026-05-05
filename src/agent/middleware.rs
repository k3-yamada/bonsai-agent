//! エージェントループのミドルウェアチェーン（DeerFlow知見）
//!
//! 各関心事を独立したミドルウェアに分離し、テスト・追加・削除を容易にする。
//! before_step / after_step のフックポイントで、ループの前後処理をパイプライン化。

use crate::agent::agent_loop::{StallDetector, TokenBudgetTracker};
use crate::agent::compaction::{
    CompactionConfig, compact_if_needed, compact_level3, estimate_message_tokens, estimate_tokens,
};
use crate::agent::conversation::{Message, Role, Session};
use crate::memory::store::MemoryStore;
use crate::observability::audit::{AuditAction, AuditLog};
use crate::observability::logger::{LogLevel, log_event};

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
    /// ループ中断（NAT before_step知見: LLM呼出前に安全停止）
    Abort(String),
}

/// エージェントループのミドルウェアトレイト
pub trait Middleware {
    fn name(&self) -> &str;
    fn after_step(&mut self, session: &mut Session, result: &StepResult) -> MiddlewareSignal;
    /// LLM呼出前のフック（NAT知見: プリスクリーン/プロンプト修正/安全ガード）
    ///
    /// Phase 2a Red: 引数を `&Session` → `&mut Session` に変更
    /// (ContextOverflowGuard が level3 圧縮で session.messages を変更するため)
    fn before_step(&mut self, _session: &mut Session, _iteration: usize) -> MiddlewareSignal {
        MiddlewareSignal::Ok
    }
}

/// ミドルウェアチェーン — 登録順に実行
pub struct MiddlewareChain<'a> {
    middlewares: Vec<Box<dyn Middleware + 'a>>,
}

impl<'a> MiddlewareChain<'a> {
    pub fn new() -> Self {
        Self {
            middlewares: Vec::new(),
        }
    }

    pub fn add(&mut self, mw: Box<dyn Middleware + 'a>) {
        self.middlewares.push(mw);
    }

    pub fn run_after_step(&mut self, session: &mut Session, result: &StepResult) {
        for mw in &mut self.middlewares {
            match mw.after_step(session, result) {
                MiddlewareSignal::Ok | MiddlewareSignal::Abort(_) => {}
                MiddlewareSignal::Inject(msg) => {
                    session.add_message(Message::system(msg));
                }
            }
        }
    }

    /// LLM呼出前にミドルウェアチェーンを実行（NAT before_step知見）
    ///
    /// Abort返却時はループ中断理由を返す。Inject時はセッションにメッセージ追加。
    pub fn run_before_step(&mut self, session: &mut Session, iteration: usize) -> Option<String> {
        for mw in &mut self.middlewares {
            match mw.before_step(session, iteration) {
                MiddlewareSignal::Ok => {}
                MiddlewareSignal::Inject(msg) => {
                    session.add_message(Message::system(msg));
                }
                MiddlewareSignal::Abort(reason) => {
                    log_event(
                        LogLevel::Warn,
                        "middleware",
                        &format!("{} before_step abort: {}", mw.name(), reason),
                    );
                    return Some(reason);
                }
            }
        }
        None
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

impl<'a> Default for MiddlewareChain<'a> {
    fn default() -> Self {
        Self::new()
    }
}

// --- 具象ミドルウェア ---

/// 1. 監査ログミドルウェア
pub struct AuditMiddleware<'a> {
    session_id: String,
    store: Option<&'a MemoryStore>,
}

impl<'a> AuditMiddleware<'a> {
    pub fn new(session_id: String, store: Option<&'a MemoryStore>) -> Self {
        Self { session_id, store }
    }
}

impl Middleware for AuditMiddleware<'_> {
    fn name(&self) -> &str {
        "audit"
    }

    fn after_step(&mut self, _session: &mut Session, result: &StepResult) -> MiddlewareSignal {
        if let Some(store) = self.store {
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
    pub fn new() -> Self {
        Self {
            all_tools: Vec::new(),
        }
    }
}

impl Default for ToolTrackingMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

impl Middleware for ToolTrackingMiddleware {
    fn name(&self) -> &str {
        "tool_tracking"
    }

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
        Self {
            detector: StallDetector::new(threshold),
        }
    }
}

impl Default for StallMiddleware {
    fn default() -> Self {
        Self::new(3)
    }
}

impl Middleware for StallMiddleware {
    fn name(&self) -> &str {
        "stall"
    }

    fn after_step(&mut self, _session: &mut Session, result: &StepResult) -> MiddlewareSignal {
        if result.outcome_type != "continue" {
            return MiddlewareSignal::Ok;
        }
        if self
            .detector
            .record_step(result.tools_succeeded, result.output_hash)
        {
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
    pub fn new(config: CompactionConfig) -> Self {
        Self { config }
    }

    /// LLM context 予算から ContextOverflowGuard 用 middleware を構築。
    /// `None` なら legacy default、`Some(n)` で `n * 0.7` 派生 budget を使用。
    pub fn with_n_ctx_budget(n_ctx_budget: Option<u32>) -> Self {
        Self::new(CompactionConfig::from_n_ctx_budget(n_ctx_budget))
    }
}

impl Default for CompactionMiddleware {
    fn default() -> Self {
        Self::new(CompactionConfig::default())
    }
}

impl Middleware for CompactionMiddleware {
    fn name(&self) -> &str {
        "compaction"
    }

    /// LLM 呼出前 ContextOverflowGuard (項目 186 H6 CONTEXT_OVERFLOW 対策)。
    ///
    /// 1. 推定 tokens が `max_context_tokens` 未満なら no-op
    /// 2. 超過時は `compact_level3` を強制発火 (handoff summary + emergency_keep)
    /// 3. 圧縮後も超過 or 縮小不可なら `Abort` で graceful 停止 → LLM 呼出を防ぎ HTTP 400 を回避
    ///
    /// 再帰圧縮はしない (level3 後の再削減は handoff summary 破壊リスク)。
    fn before_step(&mut self, session: &mut Session, iteration: usize) -> MiddlewareSignal {
        let before = estimate_tokens(&session.messages);
        // `<=` で境界一致時も no-op (Codex audit LOW fix: before==budget で
        // 過保守 abort を起こさない)
        if before <= self.config.max_context_tokens {
            return MiddlewareSignal::Ok;
        }

        compact_level3(&mut session.messages, &self.config);
        let after = estimate_tokens(&session.messages);
        log_event(
            LogLevel::Warn,
            "middleware:context_guard",
            &format!(
                "LLM呼出前コンテキスト圧縮 iter={iteration} tokens_before={before} tokens_after={after} budget={}",
                self.config.max_context_tokens
            ),
        );

        if after > self.config.max_context_tokens {
            return MiddlewareSignal::Abort(format!(
                "context overflow remains after emergency compaction: tokens={after}, budget={}",
                self.config.max_context_tokens
            ));
        }

        // 注: `after == before` ケース (level3 が no-op だが after も budget 以下) は
        // 上の budget 判定で fall-through し Ok 扱い (Codex audit LOW fix で
        // 不要な Abort branch を削除)。
        MiddlewareSignal::Ok
    }

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

/// 4b. F3 RequestSizeGuard — 単発 message size > threshold で末尾 truncate
///
/// 項目 190: 単発 burst (file_write の長 new_text、巨大 tool 出力等) で
/// llama-server n_ctx を瞬時に超過し HTTP 400 を引き起こすケースを抑制。
/// F2 ContextOverflowGuard (累積保護、項目 187) と相補的。
///
/// **保護タグ (Codex HIGH 1 / Gemini MUST 2)**: Assistant content に
/// `<tool_call>` `</tool_call>` `<start_function_call>` `<end_function_call>`
/// のいずれかを含む場合は skip (JSON tag 中切による parse 失敗を回避、F2 累積
/// 保護に委譲)。
///
/// **Tool role**: 全件対象 (data 部のみで JSON 中切リスクが低い)。
///
/// **Idempotent (Gemini MUST 1)**: 既に suffix で終わる message は skip
/// (累積無限ループ防止)。
///
/// **Token estimator (Codex MEDIUM 1)**: F2 と同じ hybrid estimator
/// (`max(chars/3, bytes*0.4)`) を message 単位で使用。char count 単純比は
/// 日本語混在で実 BPE token を 50% しか見積らないため不採用。
pub struct RequestSizeGuard<'a> {
    /// 0 = disabled (legacy 互換)。`>0` で n token を上限。
    pub max_message_tokens: u32,
    /// truncate 末尾に付与する marker 文字列 (idempotent 判定にも使用)。
    pub truncate_suffix: String,
    session_id: Option<String>,
    store: Option<&'a MemoryStore>,
}

impl<'a> RequestSizeGuard<'a> {
    /// 監査ログなしの構築 (テスト + store 不在時)。
    pub fn new(max_message_tokens: u32) -> Self {
        Self {
            max_message_tokens,
            truncate_suffix: "\n[truncated by F3 size_guard]".to_string(),
            session_id: None,
            store: None,
        }
    }

    /// 監査ログ付き構築 (Gemini SHOULD 1: AuditAction::F3SizeGuard で SQLite 永続化)。
    pub fn with_audit(max_message_tokens: u32, session_id: String, store: &'a MemoryStore) -> Self {
        Self {
            max_message_tokens,
            truncate_suffix: "\n[truncated by F3 size_guard]".to_string(),
            session_id: Some(session_id),
            store: Some(store),
        }
    }

    pub fn disabled() -> Self {
        Self::new(0)
    }

    /// Assistant content に tool_call 系の保護タグが含まれるか判定。
    /// 含まれる場合 F3 は skip (HIGH 1 / MUST 2 反映)。
    pub fn has_protected_tags(content: &str) -> bool {
        const TAGS: &[&str] = &[
            "<tool_call>",
            "</tool_call>",
            "<start_function_call>",
            "<end_function_call>",
        ];
        TAGS.iter().any(|t| content.contains(t))
    }
}

impl Middleware for RequestSizeGuard<'_> {
    fn name(&self) -> &str {
        "request_size_guard"
    }

    fn after_step(&mut self, _session: &mut Session, _result: &StepResult) -> MiddlewareSignal {
        MiddlewareSignal::Ok
    }

    /// LLM 呼出前に session.messages を走査し、size > threshold の
    /// Assistant/Tool message を末尾切捨。tool_call tag を含む Assistant は skip。
    fn before_step(&mut self, session: &mut Session, _iteration: usize) -> MiddlewareSignal {
        if self.max_message_tokens == 0 {
            return MiddlewareSignal::Ok; // disabled
        }
        let max_tokens = self.max_message_tokens as usize;
        let suffix_tokens = estimate_message_tokens(&self.truncate_suffix);
        if suffix_tokens >= max_tokens {
            // 極端 config 防護: suffix が threshold 以上なら no-op
            return MiddlewareSignal::Ok;
        }
        let cutoff_tokens = max_tokens - suffix_tokens;
        let mut truncated_count = 0_usize;

        for (idx, msg) in session.messages.iter_mut().enumerate() {
            // User/System は task 指示で保護
            if !matches!(msg.role, Role::Assistant | Role::Tool) {
                continue;
            }
            // Idempotent: 既に suffix で終わる message は skip (Gemini MUST 1)
            if msg.content.ends_with(&self.truncate_suffix) {
                continue;
            }
            // tool_call tag 保護 (Codex HIGH 1 / Gemini MUST 2)
            if matches!(msg.role, Role::Assistant) && Self::has_protected_tags(&msg.content) {
                continue;
            }

            let cur_tokens = estimate_message_tokens(&msg.content);
            if cur_tokens <= max_tokens {
                continue;
            }

            // 言語に応じた初期 target_chars: 線形スケール (cutoff/cur 比) +
            // 5% 安全マージンで while loop 反復を最小化
            let cur_chars = msg.content.chars().count();
            let scale = cutoff_tokens as f64 / cur_tokens as f64;
            let initial = ((cur_chars as f64) * scale * 0.95) as usize;
            let target_chars = initial.max(1);

            let prefix: String = msg.content.chars().take(target_chars).collect();
            // 必要なら微調整 (Japanese で chars*1.2 倍トークンとなり初期推定超過時)
            let mut chars: Vec<char> = prefix.chars().collect();
            while !chars.is_empty()
                && estimate_message_tokens(&chars.iter().collect::<String>()) > cutoff_tokens
            {
                chars.truncate(chars.len().saturating_sub(64));
            }
            let truncated_text: String = chars.into_iter().collect();
            let original_size = msg.content.len();
            msg.content = format!("{}{}", truncated_text, self.truncate_suffix);
            let new_size = msg.content.len();
            truncated_count += 1;

            // LOW (Codex): role/index/sizes を log
            log_event(
                LogLevel::Info,
                "middleware:f3_size_guard",
                &format!(
                    "truncated role={:?} idx={} original_size={} new_size={} threshold_tokens={}",
                    msg.role, idx, original_size, new_size, self.max_message_tokens
                ),
            );

            // SHOULD 1 (Gemini): SQLite audit 永続化
            if let (Some(store), Some(session_id)) = (self.store, self.session_id.as_deref()) {
                let audit = AuditLog::new(store.conn());
                let role_str = format!("{:?}", msg.role).to_lowercase();
                let _ = audit.log(
                    Some(session_id),
                    &AuditAction::F3SizeGuard {
                        role: role_str,
                        message_index: idx,
                        original_size: original_size as u64,
                        new_size: new_size as u64,
                        threshold_tokens: self.max_message_tokens,
                    },
                );
            }
        }

        if truncated_count > 0 {
            log_event(
                LogLevel::Info,
                "middleware:f3_size_guard",
                &format!(
                    "step truncated {} messages (threshold_tokens={})",
                    truncated_count, self.max_message_tokens
                ),
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
        Self {
            tracker: TokenBudgetTracker::new(budget),
        }
    }
}

impl Default for TokenBudgetMiddleware {
    fn default() -> Self {
        Self::new(8000)
    }
}

impl Middleware for TokenBudgetMiddleware {
    fn name(&self) -> &str {
        "token_budget"
    }

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
/// 順序: `[Audit, ToolTrack, RequestSizeGuard (F3), Compaction (F2), TokenBudget]`
///
/// - `n_ctx_budget` が `Some(value)` で `CompactionMiddleware` が ContextOverflowGuard 動作 (F2 累積保護)。
///   `None` で legacy 動作 (max_context_tokens=14000)。
/// - `f3_max_message_tokens` が `>0` で `RequestSizeGuard` が単発 message size 保護 (F3)。
///   `0` で F3 disabled (legacy 互換)。F2 の前に配置することで単発 burst を抑えてから累積保護に渡す。
pub fn build_default_chain<'a>(
    session_id: &str,
    store: Option<&'a MemoryStore>,
    n_ctx_budget: Option<u32>,
    f3_max_message_tokens: u32,
) -> MiddlewareChain<'a> {
    let mut chain = MiddlewareChain::new();
    chain.add(Box::new(AuditMiddleware::new(
        session_id.to_string(),
        store,
    )));
    chain.add(Box::new(ToolTrackingMiddleware::new()));
    // F3 RequestSizeGuard (項目 190): 単発 message burst 防護。F2 の前に挿入。
    // store が利用可能なら audit 永続化付き (SHOULD 1 反映)、不在なら log_event のみ。
    let f3 = match store {
        Some(s) => RequestSizeGuard::with_audit(f3_max_message_tokens, session_id.to_string(), s),
        None => RequestSizeGuard::new(f3_max_message_tokens),
    };
    chain.add(Box::new(f3));
    // StallMiddleware は除外: Advisor連携付きの inject_replan_on_stall() が上位互換
    chain.add(Box::new(CompactionMiddleware::with_n_ctx_budget(
        n_ctx_budget,
    )));
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
        assert!(matches!(
            mw.after_step(&mut session, &r),
            MiddlewareSignal::Ok
        ));
        assert!(matches!(
            mw.after_step(&mut session, &r),
            MiddlewareSignal::Ok
        ));
        assert!(matches!(
            mw.after_step(&mut session, &r),
            MiddlewareSignal::Inject(_)
        ));
    }

    #[test]
    fn test_stall_skips_non_continue() {
        let mut mw = StallMiddleware::new(1);
        let mut session = Session::new();
        let r = make_final_result();
        assert!(matches!(
            mw.after_step(&mut session, &r),
            MiddlewareSignal::Ok
        ));
    }

    #[test]
    fn test_compaction_runs_without_panic() {
        let mut mw = CompactionMiddleware::default();
        let mut session = Session::new();
        let r = make_continue_result(0, vec![]);
        assert!(matches!(
            mw.after_step(&mut session, &r),
            MiddlewareSignal::Ok
        ));
    }

    // --- Phase 2a Red: ContextOverflowGuard tests (F2 plan) ---
    // Red phase では `before_step` が default Ok を返すため、
    // session 縮約を期待する test は fail (期待動作は Green commit で実装)。

    /// Phase 2a Red: stub では session 不変、Green で `compact_level3` 強制発火に置換すると pass。
    #[test]
    fn t_context_overflow_guard_compacts_before_llm_call() {
        use crate::agent::compaction::estimate_tokens;
        let mut mw = CompactionMiddleware::with_n_ctx_budget(Some(8192));
        let mut session = Session::new();
        session.add_message(Message::system("s"));

        for i in 0..12 {
            session.add_message(Message::user(format!("q{i}")));
            session.add_message(Message::assistant("あ".repeat(700)));
            session.add_message(Message::tool("い".repeat(700), format!("tool-{i}")));
        }

        assert!(estimate_tokens(&session.messages) > 6000);
        let signal = mw.before_step(&mut session, 0);

        assert!(matches!(signal, MiddlewareSignal::Ok));
        assert!(estimate_tokens(&session.messages) < 6000);
        assert!(
            session.messages.len() <= 6,
            "system+handoff+emergency_keep=最大6"
        );
    }

    /// Phase 2a: 短いセッションは Ok 返し、session 不変 (stub も pass、Green も pass = behavior 一致)。
    #[test]
    fn t_context_overflow_guard_no_op_below_threshold() {
        use crate::agent::compaction::estimate_tokens;
        let mut mw = CompactionMiddleware::with_n_ctx_budget(Some(8192));
        let mut session = Session::new();
        session.add_message(Message::system("s"));
        session.add_message(Message::user("hello"));

        let before_len = session.messages.len();
        let before_tokens = estimate_tokens(&session.messages);
        let signal = mw.before_step(&mut session, 0);

        assert!(matches!(signal, MiddlewareSignal::Ok));
        assert_eq!(session.messages.len(), before_len);
        assert_eq!(estimate_tokens(&session.messages), before_tokens);
    }

    /// Phase 2a Red: stub では Abort 発火せず Ok → 期待 Abort で fail。
    /// Green で level3 後も超過時 Abort に置換すると pass。
    #[test]
    fn t_context_overflow_guard_aborts_when_unrecoverable() {
        let mut mw = CompactionMiddleware::with_n_ctx_budget(Some(16));
        let mut session = Session::new();
        session.add_message(Message::system("システム".repeat(100)));
        session.add_message(Message::user("q0"));
        session.add_message(Message::assistant("a0".repeat(100)));
        session.add_message(Message::tool("t0".repeat(100), "tool-0"));
        session.add_message(Message::user("q1"));
        session.add_message(Message::assistant("a1".repeat(100)));
        session.add_message(Message::tool("t1".repeat(100), "tool-1"));

        let signal = mw.before_step(&mut session, 0);

        match signal {
            MiddlewareSignal::Abort(reason) => {
                assert!(reason.contains("context overflow"));
            }
            _ => panic!("unrecoverable overflow must abort before LLM call"),
        }
    }

    #[test]
    fn test_token_budget_ok_initially() {
        let mut mw = TokenBudgetMiddleware::new(100_000);
        let mut session = Session::new();
        let r = make_continue_result(0, vec!["echo".to_string()]);
        assert!(matches!(
            mw.after_step(&mut session, &r),
            MiddlewareSignal::Ok
        ));
    }

    #[test]
    fn test_token_budget_warns_near_limit() {
        let mut mw = TokenBudgetMiddleware::new(500);
        let mut session = Session::new();
        for i in 0..10 {
            let r =
                make_continue_result(i, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
            let _ = mw.after_step(&mut session, &r);
        }
        let r = make_continue_result(10, vec!["d".to_string()]);
        let signal = mw.after_step(&mut session, &r);
        assert!(matches!(signal, MiddlewareSignal::Inject(_)));
    }

    #[test]
    fn test_audit_no_store_no_panic() {
        let mut mw = AuditMiddleware::new("test-session".to_string(), None);
        let mut session = Session::new();
        let r = make_continue_result(0, vec![]);
        assert!(matches!(
            mw.after_step(&mut session, &r),
            MiddlewareSignal::Ok
        ));
    }

    #[test]
    fn test_audit_with_store() {
        let store = MemoryStore::in_memory().unwrap();
        let mut mw = AuditMiddleware::new("test-session".to_string(), Some(&store));
        let mut session = Session::new();
        let r = make_continue_result(0, vec!["shell".to_string()]);
        mw.after_step(&mut session, &r);
        let audit = AuditLog::new(store.conn());
        let entries = audit.for_session("test-session").unwrap();
        assert!(entries.iter().any(|e| e.action_type == "step_outcome"));
    }

    #[test]
    fn test_build_default_chain_has_5_middlewares() {
        // 項目 190 F3: chain は 5 段 (Audit, ToolTrack, F3, Compaction, TokenBudget)
        let chain = build_default_chain("test", None, None, 0);
        assert_eq!(chain.len(), 5);
        assert_eq!(
            chain.names(),
            vec![
                "audit",
                "tool_tracking",
                "request_size_guard",
                "compaction",
                "token_budget"
            ]
        );
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
        assert!(matches!(
            mw.after_step(&mut session, &r),
            MiddlewareSignal::Ok
        ));
        assert!(matches!(
            mw.after_step(&mut session, &r),
            MiddlewareSignal::Ok
        ));
        // 3回目でInject（閾値3）
        assert!(matches!(
            mw.after_step(&mut session, &r),
            MiddlewareSignal::Inject(_)
        ));
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
        assert!(matches!(
            mw.after_step(&mut session, &r),
            MiddlewareSignal::Ok
        ));
        assert!(matches!(
            mw.after_step(&mut session, &r),
            MiddlewareSignal::Inject(_)
        ));
        // リセット後、再度2回必要
        assert!(matches!(
            mw.after_step(&mut session, &r),
            MiddlewareSignal::Ok
        ));
        assert!(matches!(
            mw.after_step(&mut session, &r),
            MiddlewareSignal::Inject(_)
        ));
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

    // --- Phase 5: before_step テスト（NAT知見） ---

    struct BeforeInjectMw;
    impl Middleware for BeforeInjectMw {
        fn name(&self) -> &str {
            "before_inject"
        }
        fn after_step(&mut self, _s: &mut Session, _r: &StepResult) -> MiddlewareSignal {
            MiddlewareSignal::Ok
        }
        fn before_step(&mut self, _s: &mut Session, _iter: usize) -> MiddlewareSignal {
            MiddlewareSignal::Inject("pre-step context".to_string())
        }
    }

    struct BeforeAbortMw;
    impl Middleware for BeforeAbortMw {
        fn name(&self) -> &str {
            "before_abort"
        }
        fn after_step(&mut self, _s: &mut Session, _r: &StepResult) -> MiddlewareSignal {
            MiddlewareSignal::Ok
        }
        fn before_step(&mut self, _s: &mut Session, _iter: usize) -> MiddlewareSignal {
            MiddlewareSignal::Abort("safety limit".to_string())
        }
    }

    #[test]
    fn t_before_step_inject() {
        let mut chain = MiddlewareChain::new();
        chain.add(Box::new(BeforeInjectMw));
        let mut session = Session::new();
        let result = chain.run_before_step(&mut session, 0);
        assert!(result.is_none(), "Inject does not abort");
        assert!(
            session
                .messages
                .iter()
                .any(|m| m.content.contains("pre-step context"))
        );
    }

    #[test]
    fn t_before_step_abort() {
        let mut chain = MiddlewareChain::new();
        chain.add(Box::new(BeforeAbortMw));
        let mut session = Session::new();
        let result = chain.run_before_step(&mut session, 0);
        assert_eq!(result, Some("safety limit".to_string()));
    }

    struct DefaultBeforeMw;
    impl Middleware for DefaultBeforeMw {
        fn name(&self) -> &str {
            "default_before"
        }
        fn after_step(&mut self, _s: &mut Session, _r: &StepResult) -> MiddlewareSignal {
            MiddlewareSignal::Ok
        }
        // before_step uses default impl -> Ok
    }

    #[test]
    fn t_before_step_default_ok() {
        let mut chain = MiddlewareChain::new();
        chain.add(Box::new(DefaultBeforeMw));
        let mut session = Session::new();
        let result = chain.run_before_step(&mut session, 0);
        assert!(result.is_none(), "default before_step returns Ok");
        assert!(session.messages.is_empty());
    }

    #[test]
    fn t_before_step_abort_stops_chain() {
        let mut chain = MiddlewareChain::new();
        chain.add(Box::new(BeforeAbortMw));
        chain.add(Box::new(BeforeInjectMw));
        let mut session = Session::new();
        let result = chain.run_before_step(&mut session, 0);
        assert!(result.is_some(), "abort stops chain");
        assert!(
            session.messages.is_empty(),
            "inject after abort not reached"
        );
    }

    // --- 項目 190 F3 RequestSizeGuard tests (Phase 1 Red, CCG review v2 反映) ---

    /// F3 のテストで使う threshold は token 単位 (chars/3 と bytes*0.4 の max)。
    /// `repeat(N, 'x')` は N chars = N bytes なので tokens = max(N/3, N*0.4) ≈ N*0.4。
    /// → threshold=1000 tokens は約 2500 chars (ASCII) 相当。
    /// 安全マージンで oversized は threshold * 5 chars 程度を使う。
    fn make_session_with(messages: Vec<Message>) -> Session {
        let mut s = Session::new();
        for m in messages {
            s.messages.push(m);
        }
        s
    }

    #[test]
    fn t_f3_truncates_oversized_assistant_message() {
        // 純粋 text の Assistant message (tool_call tag なし) を truncate する
        let big = "x".repeat(10000); // ≈ 4000 tokens
        let original_len = big.len();
        let mut session = make_session_with(vec![Message::assistant(big)]);
        let mut mw = RequestSizeGuard::new(500); // threshold 500 tokens
        let signal = mw.before_step(&mut session, 0);
        assert!(matches!(signal, MiddlewareSignal::Ok));
        let new_content = &session.messages[0].content;
        assert!(
            new_content.len() < original_len,
            "Assistant message must be truncated (was {} chars)",
            original_len
        );
        assert!(
            new_content.contains("[truncated by F3 size_guard]"),
            "Expected suffix marker"
        );
    }

    #[test]
    fn t_f3_truncates_oversized_tool_message() {
        let big = "y".repeat(10000);
        let mut session = make_session_with(vec![Message::tool(big.clone(), "call_001")]);
        let mut mw = RequestSizeGuard::new(500);
        mw.before_step(&mut session, 0);
        let new_content = &session.messages[0].content;
        assert!(new_content.len() < big.len());
        assert!(new_content.contains("[truncated by F3 size_guard]"));
    }

    #[test]
    fn t_f3_preserves_under_threshold_messages() {
        // 100 chars ≈ 40 tokens、threshold 1000 を超えない
        let small = "a".repeat(100);
        let mut session = make_session_with(vec![Message::assistant(small.clone())]);
        let mut mw = RequestSizeGuard::new(1000);
        mw.before_step(&mut session, 0);
        assert_eq!(
            session.messages[0].content, small,
            "under-threshold message must not be truncated"
        );
    }

    #[test]
    fn t_f3_disabled_when_threshold_zero() {
        let big = "z".repeat(10000);
        let mut session = make_session_with(vec![Message::assistant(big.clone())]);
        let mut mw = RequestSizeGuard::new(0); // disabled
        mw.before_step(&mut session, 0);
        assert_eq!(
            session.messages[0].content, big,
            "threshold=0 must be no-op"
        );
    }

    #[test]
    fn t_f3_skips_user_and_system_messages() {
        // User と System は task 指示で保護対象外
        let big_user = "u".repeat(10000);
        let big_sys = "s".repeat(10000);
        let mut session = make_session_with(vec![
            Message::system(big_sys.clone()),
            Message::user(big_user.clone()),
        ]);
        let mut mw = RequestSizeGuard::new(500);
        mw.before_step(&mut session, 0);
        assert_eq!(session.messages[0].content, big_sys, "System unchanged");
        assert_eq!(session.messages[1].content, big_user, "User unchanged");
    }

    #[test]
    fn t_f3_skips_assistant_with_tool_call_tag() {
        // Codex HIGH 1 / Gemini MUST 2: tool_call tag を含む Assistant は skip
        let big = format!(
            "{}<tool_call>{{\"name\":\"shell\",\"arguments\":{{}}}}</tool_call>",
            "x".repeat(10000)
        );
        let mut session = make_session_with(vec![Message::assistant(big.clone())]);
        let mut mw = RequestSizeGuard::new(500);
        mw.before_step(&mut session, 0);
        assert_eq!(
            session.messages[0].content, big,
            "Assistant with <tool_call> must NOT be truncated"
        );
    }

    #[test]
    fn t_f3_skips_assistant_with_function_call_tag() {
        // Codex HIGH 1: <start_function_call>...<end_function_call> も保護
        let big = format!(
            "{}<start_function_call>call:shell{{cmd:date}}<end_function_call>",
            "y".repeat(10000)
        );
        let mut session = make_session_with(vec![Message::assistant(big.clone())]);
        let mut mw = RequestSizeGuard::new(500);
        mw.before_step(&mut session, 0);
        assert_eq!(
            session.messages[0].content, big,
            "Assistant with function_call tag must NOT be truncated"
        );
    }

    #[test]
    fn t_f3_idempotent_does_not_accumulate_suffix() {
        // Gemini MUST 1: 2 回 before_step を通しても suffix は 1 回のみ
        let big = "p".repeat(10000);
        let mut session = make_session_with(vec![Message::assistant(big)]);
        let mut mw = RequestSizeGuard::new(500);
        mw.before_step(&mut session, 0);
        let after_first = session.messages[0].content.clone();
        mw.before_step(&mut session, 1);
        assert_eq!(
            session.messages[0].content, after_first,
            "Second pass must not modify already-truncated message"
        );
        // suffix が 1 回しか出現しないことを確認
        let suffix = "[truncated by F3 size_guard]";
        let count = session.messages[0].content.matches(suffix).count();
        assert_eq!(count, 1, "Suffix appears exactly once (idempotent)");
    }

    #[test]
    fn t_f3_token_estimator_handles_japanese() {
        // Codex MEDIUM 1: 日本語混在で hybrid estimator が char 単純比と異なる挙動
        // "あ" (1 char, UTF-8 3 bytes) → estimate ≈ max(1/3, 3*0.4) = 1.2 tokens
        // つまり日本語の方が char 数比でトークンが大きく見積もられる。
        let jp = "あ".repeat(5000); // 5000 chars = 15000 bytes ≈ 6000 tokens
        let mut session = make_session_with(vec![Message::tool(jp, "call_jp")]);
        let mut mw = RequestSizeGuard::new(500); // 500 tokens threshold
        mw.before_step(&mut session, 0);
        let new_content = &session.messages[0].content;
        assert!(
            new_content.contains("[truncated by F3 size_guard]"),
            "Japanese-heavy content over threshold must be truncated"
        );
        // 結果の token 数は threshold 以下のはず (suffix 込みで近似)
        let new_tokens = estimate_message_tokens(new_content);
        assert!(
            new_tokens <= 500,
            "After truncation tokens={} must be <= 500",
            new_tokens
        );
    }

    #[test]
    fn t_build_default_chain_includes_f3() {
        // Codex MEDIUM 2: chain.len()==5、names に "request_size_guard" を含む
        let chain = build_default_chain("test", None, None, 4915);
        assert_eq!(chain.len(), 5, "Chain has 5 middlewares with F3");
        let names = chain.names();
        assert!(
            names.contains(&"request_size_guard"),
            "Chain must include request_size_guard, got: {:?}",
            names
        );
        // F3 が ToolTrack の後、Compaction の前に配置されることを確認
        let f3_idx = names
            .iter()
            .position(|n| *n == "request_size_guard")
            .unwrap();
        let compaction_idx = names.iter().position(|n| *n == "compaction").unwrap();
        let tooltrack_idx = names.iter().position(|n| *n == "tool_tracking").unwrap();
        assert!(
            tooltrack_idx < f3_idx && f3_idx < compaction_idx,
            "F3 order must be: tool_tracking < request_size_guard < compaction"
        );
    }

    #[test]
    fn t_f3_has_protected_tags_helper() {
        // helper の検出網羅性を確認 (4 種類すべて)
        assert!(RequestSizeGuard::has_protected_tags("foo<tool_call>bar"));
        assert!(RequestSizeGuard::has_protected_tags("foo</tool_call>"));
        assert!(RequestSizeGuard::has_protected_tags(
            "<start_function_call>"
        ));
        assert!(RequestSizeGuard::has_protected_tags("<end_function_call>"));
        assert!(!RequestSizeGuard::has_protected_tags("plain text"));
        assert!(!RequestSizeGuard::has_protected_tags("<think>foo</think>"));
    }
}
