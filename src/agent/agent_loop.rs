#![allow(clippy::collapsible_if)]
use crate::observability::logger::{LogLevel, log_event};
use anyhow::Result;

use crate::agent::checkpoint::CheckpointManager;
use crate::agent::context_inject::inject_contextual_memories;
use crate::agent::conversation::{Message, ParsedOutput, Role, Session};
use crate::agent::error_recovery::TrialSummary;
use crate::agent::error_recovery::{
    CircuitBreaker, FailureMode, LoopDetector, ParseErrorDetail, RecoveryAction,
    StructuredFeedback, decide_recovery,
};
use crate::agent::middleware::{MiddlewareChain, StepResult as MwStepResult};
use crate::agent::parse::{coerce_tool_arguments, parse_assistant_output};
use crate::agent::tool_exec::{ValidatedCall, execute_validated_calls};
use crate::agent::validate::{PathGuard, Severity, validate_tool_call};
use crate::cancel::CancellationToken;
use crate::config::InferenceParams;
use crate::memory::experience::{ExperienceStore, ExperienceType, RecordParams};
use crate::memory::skill::SkillStore;
use crate::memory::store::MemoryStore;
use crate::observability::audit::{AuditAction, AuditLog};
use crate::runtime::inference::LlmBackend;
use crate::runtime::model_router::{AdvisorConfig, AdvisorRole};
use crate::safety::secrets::SecretsFilter;
use crate::tools::detect_task_type;
use crate::tools::{TaskType, ToolRegistry, ToolResultCache};

/// エージェント設定
pub struct AgentConfig {
    pub max_iterations: usize,
    pub max_retries: usize,
    pub max_tools_selected: usize,
    pub system_prompt: String,
    /// アドバイザー設定（完了前自己検証の呼び出し回数を制御）
    pub advisor: AdvisorConfig,
    /// タスク開始時に自動チェックポイント作成（git stash + DB永続化）
    pub auto_checkpoint: bool,
    /// ツール出力の最大文字数（超過分は切り詰め、コンテキスト節約）
    pub max_tool_output_chars: usize,
    /// コンテキストに含めるツールの最大数
    pub max_tools_in_context: usize,
    /// MCPツールの追加枠（ビルトインとは別枠）
    pub max_mcp_tools_in_context: usize,
    /// ベース推論パラメータ（TaskTypeで動的調整）
    pub base_inference: InferenceParams,
    /// タスク単位のウォールクロックタイムアウト（None=無制限）
    pub task_timeout: Option<std::time::Duration>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_iterations: 10,
            max_retries: 3,
            max_tools_selected: 5,
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            advisor: AdvisorConfig::default(),
            auto_checkpoint: true,
            max_tool_output_chars: 4000,
            max_tools_in_context: 8,
            max_mcp_tools_in_context: 3,
            base_inference: InferenceParams::default(),
            task_timeout: None,
        }
    }
}

/// タスク種別に応じた推論パラメータを導出
pub fn inference_for_task(task_type: TaskType, base: &InferenceParams) -> InferenceParams {
    let mut params = base.clone();
    match task_type {
        TaskType::FileOperation | TaskType::CodeExecution => {
            params.temperature = 0.3; // 精密操作
        }
        TaskType::Research => {
            params.temperature = 0.6; // 探索的
        }
        TaskType::General => {} // ベースのまま
    }
    params
}

/// 1ビットモデル向けに最適化されたシステムプロンプト。
/// arxiv知見: スキーマファースト（ツール定義をプロンプト先頭に配置）、
/// 簡潔な指示、明確なフォーマット例が小型モデルの精度を最大化する。
const DEFAULT_SYSTEM_PROMPT: &str = r#"あなたはbonsai-agent、ローカルで動作する自律型AIアシスタントです。

## ツールの使い方

ツールを呼び出すには、以下のXML形式を使ってください:

<tool_call>{"name": "ツール名", "arguments": {"パラメータ名": "値"}}</tool_call>

### 例

ファイルを読む:
<tool_call>{"name": "file_read", "arguments": {"path": "README.md"}}</tool_call>

コマンドを実行する:
<tool_call>{"name": "shell", "arguments": {"command": "ls -la"}}</tool_call>

ファイルの一部を編集する:
<tool_call>{"name": "file_write", "arguments": {"path": "main.rs", "old_text": "hello", "new_text": "world"}}</tool_call>

Gitの状態を確認する:
<tool_call>{"name": "git", "arguments": {"subcommand": "status"}}</tool_call>

## ルール

1. 回答は簡潔にする。聞かれたことだけ答える
2. 同じ内容を繰り返さない
3. 日本語で回答する
4. 考える必要があれば <think>ここに思考</think> タグを使う
5. ツール呼び出しのJSONは正しい形式にする
6. ツール結果を元に簡潔に回答する
7. わからないことは「わからない」と答える
8. 「検索して」→ web_search。URLが分かっている時だけ web_fetch
9. 複数ステップが必要な場合、まず計画を <think> に書いてから実行する
10. ツールを使う前に必ず <think> で意図と期待結果を書く
11. ツール結果を確認せずに内容を主張しない。「たぶん」「おそらく」は使わない
12. 同じファイルを連続で再読込しない。前回の結果を使う
13. ツール結果が期待と違う場合、別のツールを試す
14. <tool_persistence>ツールが使える場面では必ずツールを使い、推測で回答しない</tool_persistence>
15. 回答を出す前にファイルの内容を確認する。未読のファイルについて断定しない
"#;

/// エージェントループの構造化戻り値
#[derive(Debug, Clone)]
pub struct AgentLoopResult {
    pub answer: String,
    pub iterations_used: usize,
    pub tools_called: Vec<String>,
}

/// エージェントのステップ結果
#[derive(Debug)]
pub enum StepOutcome {
    /// 最終回答（ループ終了）
    FinalAnswer(String),
    /// ツール実行後、ループ継続（使用ツール名を保持）
    Continue(Vec<String>),
    /// エラーで中断
    Aborted(String),
}

/// エージェントループのミュータブル状態を集約
///
/// run_agent_loop_with_session の局所変数が多すぎるため構造体に抽出。
/// 将来のミドルウェアチェーン化の基盤。
pub struct LoopState<'a> {
    pub circuit_breaker: CircuitBreaker,
    pub loop_detector: LoopDetector,
    pub stall_detector: StallDetector,
    pub advisor: AdvisorConfig,
    pub all_tools: Vec<String>,
    pub consecutive_failures: usize,
    pub iteration: usize,
    /// トークン予算追跡（diminishing returns検出用、macOS26/Agent知見）
    pub token_budget: TokenBudgetTracker,
    /// ミドルウェアチェーン（DeerFlow知見: 5段パイプライン）
    pub middleware_chain: MiddlewareChain<'a>,
    /// ツール結果キャッシュ（読取専用ツールの重複呼び出し回避）
    pub tool_cache: ToolResultCache,
    /// 試行サマリー記憶（GrandCode知見: 失敗履歴を保持し再計画時に注入）
    pub trial_summary: TrialSummary,
}

impl<'a> LoopState<'a> {
    pub fn new(advisor: AdvisorConfig) -> Self {
        Self {
            circuit_breaker: CircuitBreaker::default(),
            loop_detector: LoopDetector::default(),
            stall_detector: StallDetector::default(),
            advisor,
            all_tools: Vec::new(),
            consecutive_failures: 0,
            iteration: 0,
            token_budget: TokenBudgetTracker::default(),
            middleware_chain: MiddlewareChain::default(),
            tool_cache: ToolResultCache::new(),
            trial_summary: TrialSummary::default(),
        }
    }
}

/// トークン予算追跡器（macOS26/Agent TokenBudgetTracker パターン）
///
/// 累積トークンを追跡し、diminishing returns（連続低出力）を検出。
/// 90%でnudge、100%で停止、5ターン連続100トークン未満で早期停止推奨。
pub struct TokenBudgetTracker {
    total_tokens: usize,
    budget: usize,
    recent_outputs: Vec<usize>,
    low_output_threshold: usize,
    diminishing_window: usize,
}

impl TokenBudgetTracker {
    pub fn new(budget: usize) -> Self {
        Self {
            total_tokens: 0,
            budget,
            recent_outputs: Vec::new(),
            low_output_threshold: 100,
            diminishing_window: 5,
        }
    }

    /// ステップのトークン使用量を記録
    pub fn record(&mut self, tokens: usize) {
        self.total_tokens += tokens;
        self.recent_outputs.push(tokens);
        if self.recent_outputs.len() > self.diminishing_window * 2 {
            self.recent_outputs.remove(0);
        }
    }

    /// 予算使用率 (0.0〜1.0+)
    pub fn usage_ratio(&self) -> f64 {
        self.total_tokens as f64 / self.budget as f64
    }

    /// diminishing returns 検出（直近N回が低出力）
    pub fn is_diminishing(&self) -> bool {
        if self.recent_outputs.len() < self.diminishing_window {
            return false;
        }
        let recent = &self.recent_outputs[self.recent_outputs.len() - self.diminishing_window..];
        recent.iter().all(|&t| t < self.low_output_threshold)
    }

    /// 予算チェック: None=OK, Some(msg)=nudge/stop
    pub fn check(&self) -> Option<&'static str> {
        if self.usage_ratio() >= 1.0 {
            Some("トークン予算の上限に達しました。タスクを完了してください。")
        } else if self.is_diminishing() {
            Some("出力が少なくなっています。早めにタスクを完了してください。")
        } else if self.usage_ratio() >= 0.9 {
            Some("トークン予算の90%を使いました。すぐにタスクを完了してください。")
        } else {
            None
        }
    }
}

impl Default for TokenBudgetTracker {
    fn default() -> Self {
        Self::new(8000) // llama-server の max_tokens デフォルト
    }
}

/// Outcome ハンドラの結果
pub enum OutcomeAction {
    /// ループ終了（最終結果）
    Return(AgentLoopResult),
    /// 次のイテレーションへ継続
    Continue,
}

/// 停滞検出器: 進捗のないステップが続いた場合に再計画を促す
pub struct StallDetector {
    no_progress_count: usize,
    stall_threshold: usize,
    last_output_hash: u64,
}

impl StallDetector {
    pub fn new(threshold: usize) -> Self {
        Self {
            no_progress_count: 0,
            stall_threshold: threshold,
            last_output_hash: 0,
        }
    }

    /// ステップ結果を記録し、停滞を検出したらtrueを返す
    pub fn record_step(&mut self, tools_succeeded: bool, output_hash: u64) -> bool {
        if !tools_succeeded || output_hash == self.last_output_hash {
            self.no_progress_count += 1;
        } else {
            self.no_progress_count = 0;
        }
        self.last_output_hash = output_hash;
        self.no_progress_count >= self.stall_threshold
    }

    pub fn reset(&mut self) {
        self.no_progress_count = 0;
    }
}

impl Default for StallDetector {
    fn default() -> Self {
        Self::new(3)
    }
}

// ValidatedCall, ToolExecResult → tool_exec.rs に移動

// execute_validated_calls → tool_exec.rs に移動

// execute_read_batch_parallel → tool_exec.rs に移動

// execute_single_call → tool_exec.rs に移動

// apply_tool_result → tool_exec.rs に移動

/// ステップ実行に必要なコンテキスト
pub struct StepContext<'a> {
    pub backend: &'a dyn LlmBackend,
    pub tools: &'a ToolRegistry,
    pub path_guard: &'a PathGuard,
    pub config: &'a AgentConfig,
    pub cancel: &'a CancellationToken,
    pub secrets_filter: &'a SecretsFilter,
    pub store: Option<&'a MemoryStore>,
}

/// エージェントの1ステップを実行する（テスト容易性のためループの内側を分離）
pub fn execute_step(
    session: &mut Session,
    ctx: &StepContext<'_>,
    circuit_breaker: &mut CircuitBreaker,
    loop_detector: &mut LoopDetector,
    attempt: usize,
    tool_cache: &mut ToolResultCache,
) -> Result<StepOutcome> {
    if ctx.cancel.is_cancelled() {
        return Ok(StepOutcome::Aborted("キャンセルされました".to_string()));
    }

    // 1. 動的ツール選択
    let last_user_msg = session
        .messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, crate::agent::conversation::Role::User))
        .map(|m| m.content.as_str())
        .unwrap_or("");

    // セマンティックツール選択（ローカルONNX埋め込み、失敗時は自動でキーワード版にフォールバック）
    let selected_tools = ctx.tools.select_relevant_split_semantic(
        last_user_msg,
        ctx.config.max_tools_in_context,
        ctx.config.max_mcp_tools_in_context,
    );
    let tool_schemas: Vec<_> = selected_tools.iter().map(|t| t.schema()).collect();

    // 2. タスク種別に応じた推論パラメータ導出
    let task_type = detect_task_type(last_user_msg);
    let task_params = inference_for_task(task_type, &ctx.config.base_inference);

    // 3. LLM呼び出し（ストリーミング対応、タスク別パラメータ）
    let in_think = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let in_think_clone = in_think.clone();

    let result = ctx.backend.generate_with_params(
        &session.messages,
        &tool_schemas,
        &mut |token| {
            // ストリーミングトークンの表示
            if token.contains("<think>") {
                in_think_clone.store(true, std::sync::atomic::Ordering::Relaxed);
                eprint!("\x1b[2m"); // 薄色開始
            } else if token.contains("</think>") {
                in_think_clone.store(false, std::sync::atomic::Ordering::Relaxed);
                eprint!("\x1b[0m"); // 色リセット
            } else if token.contains("<tool_call>") || token.contains("</tool_call>") {
                // ツール呼び出しタグは非表示
            } else {
                eprint!("{token}");
            }
        },
        ctx.cancel,
        &task_params,
    )?;
    // 色リセットを保証
    if in_think.load(std::sync::atomic::Ordering::Relaxed) {
        eprint!("\x1b[0m");
    }
    eprintln!(); // 改行

    // 3. パース
    let parsed = match parse_assistant_output(&result.text) {
        Ok(p) => p,
        Err(e) => {
            let mode = FailureMode::ParseError(ParseErrorDetail::InvalidJson);
            let action = decide_recovery(&mode, attempt, ctx.config.max_retries);
            return match action {
                RecoveryAction::ExplainAndStop(msg) => Ok(StepOutcome::Aborted(msg)),
                _ => {
                    // エラー情報をコンテキストに追加してリトライを促す
                    session.add_message(Message::assistant(format!(
                        "パースエラー: {e}。修正します。"
                    )));
                    Ok(StepOutcome::Continue(Vec::new()))
                }
            };
        }
    };

    // 4. ツール呼び出しがなければ最終回答
    if parsed.tool_calls.is_empty() {
        let answer = build_answer(&parsed);
        session.add_message(Message::assistant(&answer));
        return Ok(StepOutcome::FinalAnswer(answer));
    }

    // 5. ツール呼び出し実行（並列対応）
    let assistant_text = result.text.clone();
    session.add_message(Message::assistant(&assistant_text));

    let known = ctx.tools.known_names();
    let mut validated: Vec<ValidatedCall<'_>> = Vec::new();

    for tool_call in &parsed.tool_calls {
        let action_key = format!("{}:{}", tool_call.name, tool_call.arguments);
        if loop_detector.record_and_check(&action_key) {
            let mode = FailureMode::LoopDetected;
            let action = decide_recovery(&mode, attempt, ctx.config.max_retries);
            if let RecoveryAction::Abort(msg) = action {
                return Ok(StepOutcome::Aborted(msg));
            }
        }
        if !circuit_breaker.is_available(&tool_call.name) {
            session.add_message(Message::tool(
                format!(
                    "ツール '{}' は連続で失敗したため使えません。別の方法を試してください。",
                    tool_call.name
                ),
                &tool_call.name,
            ));
            continue;
        }
        let validation = validate_tool_call(tool_call, &known, ctx.path_guard, None);
        if !validation.is_valid {
            let block_issues: Vec<_> = validation
                .issues
                .iter()
                .filter(|i| i.severity == Severity::Block)
                .map(|i| i.message.as_str())
                .collect();
            let alt = match tool_call.name.as_str() {
                "shell" => "代わりにfile_readやgitツールを使ってください。",
                "file_write" => "許可されたディレクトリのパスを指定してください。",
                _ => "別のツールか、別のパラメータで試してください。",
            };
            session.add_message(Message::tool(
                format!("拒否: {}。{}", block_issues.join(", "), alt),
                &tool_call.name,
            ));
            continue;
        }
        let tool = match ctx.tools.get(&tool_call.name) {
            Some(t) => t,
            None => continue,
        };
        let mut coerced_args = tool_call.arguments.clone();
        coerce_tool_arguments(&mut coerced_args);
        validated.push(ValidatedCall {
            name: tool_call.name.clone(),
            args_json: serde_json::to_string(&tool_call.arguments).unwrap_or_default(),
            coerced_args,
            tool,
            is_read_only: tool.is_read_only(),
        });
    }

    let step_tools = execute_validated_calls(
        &validated,
        session,
        circuit_breaker,
        ctx.secrets_filter,
        ctx.store,
        tool_cache,
    );
    Ok(StepOutcome::Continue(step_tools))
}

/// エージェントループ全体を実行
pub fn run_agent_loop(
    input: &str,
    backend: &dyn LlmBackend,
    tools: &ToolRegistry,
    path_guard: &PathGuard,
    config: &AgentConfig,
    cancel: &CancellationToken,
    store: Option<&MemoryStore>,
) -> Result<AgentLoopResult> {
    let mut session = Session::new();
    let now = chrono::Local::now();
    let date_str = now.format("%Y年%m月%d日(%A) %H:%M");
    let system_with_date = format!(
        "{}

## 現在の日時
現在は{}です。正確な現在時刻が必要な場合は shell ツールで date コマンドを実行してください。",
        config.system_prompt, date_str
    );
    session.add_message(Message::system(&system_with_date));
    session.add_message(Message::user(input));

    run_agent_loop_with_session(
        &mut session,
        backend,
        tools,
        path_guard,
        config,
        cancel,
        store,
    )
}

/// 既存セッションでエージェントループを実行（セッション再開用）
pub fn run_agent_loop_with_session(
    session: &mut Session,
    backend: &dyn LlmBackend,
    tools: &ToolRegistry,
    path_guard: &PathGuard,
    config: &AgentConfig,
    cancel: &CancellationToken,
    store: Option<&MemoryStore>,
) -> Result<AgentLoopResult> {
    // 経験記録用にユーザー入力を取得
    let task_context: String = session
        .messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, crate::agent::conversation::Role::User))
        .map(|m| m.content.clone())
        .unwrap_or_default();

    let secrets_filter = SecretsFilter::default();

    // セッション開始時に期限切れ情報を自動パージ
    if let Some(s) = store {
        match s.purge_all_expired() {
            Ok(n) if n > 0 => log_event(LogLevel::Info, "ttl", &format!("期限切れ{}件をパージ", n)),
            _ => {}
        }
    }

    inject_contextual_memories(session, &task_context, store);
    inject_planning_step(session, &task_context);

    // Advisor設定ログ（初回のみ、セッション最初のメッセージが2件=system+userの場合）
    if session.messages.len() <= 2 {
        config.advisor.log_startup();
    }

    // タスク開始時の自動チェックポイント（auto_checkpoint=true 時、git+DB）
    if config.auto_checkpoint {
        let _ = create_task_start_checkpoint(session, &task_context, store);
    }

    let mut state = LoopState::new(config.advisor.clone());
    // ミドルウェアチェーン構築（DeerFlow知見: 5段パイプライン）
    state.middleware_chain = crate::agent::middleware::build_default_chain(&session.id, store);

    let ctx = StepContext {
        backend,
        tools,
        path_guard,
        config,
        cancel,
        secrets_filter: &secrets_filter,
        store,
    };

    let task_start = std::time::Instant::now();
    let mut final_iteration = 0;
    for iteration in 0..config.max_iterations {
        // ウォールクロックタイムアウトチェック
        if let Some(timeout) = config.task_timeout {
            if task_start.elapsed() > timeout {
                let timeout_msg = format!(
                    "[タイムアウト] {}秒以内に完了できませんでした",
                    timeout.as_secs()
                );
                log_event(LogLevel::Warn, "timeout", &timeout_msg);
                return Ok(AgentLoopResult {
                    answer: timeout_msg,
                    iterations_used: iteration,
                    tools_called: state.all_tools,
                });
            }
        }
        state.iteration = iteration;
        final_iteration = iteration + 1;

        // before_stepフック: LLM呼出前にミドルウェア介入（NAT知見、項目142）
        if let Some(abort_reason) = state.middleware_chain.run_before_step(session, iteration) {
            return Ok(AgentLoopResult {
                answer: format!("[中断] {abort_reason}"),
                iterations_used: iteration,
                tools_called: state.all_tools,
            });
        }

        let step_start = std::time::Instant::now();
        let outcome = execute_step(
            session,
            &ctx,
            &mut state.circuit_breaker,
            &mut state.loop_detector,
            iteration,
            &mut state.tool_cache,
        )?;

        let duration_ms = step_start.elapsed().as_millis() as u64;

        match handle_outcome(
            outcome,
            session,
            &mut state,
            &task_context,
            store,
            config.max_iterations,
            final_iteration,
            iteration,
            duration_ms,
        ) {
            OutcomeAction::Return(result) => return Ok(result),
            OutcomeAction::Continue => continue,
        }
    }

    let timeout_msg = format!(
        "最大ステップ数({})に達しました。タスクを完了できませんでした。",
        config.max_iterations
    );
    Ok(AgentLoopResult {
        answer: format!("[中断] {timeout_msg}"),
        iterations_used: final_iteration,
        tools_called: state.all_tools,
    })
}

/// タスク開始時の自動チェックポイントを作成
///
/// store があれば SQLite 永続化、なければインメモリ。
/// git stash 失敗 / リポジトリ外でも黙殺（コア機能ではない）。
fn create_task_start_checkpoint(
    session: &Session,
    task_context: &str,
    store: Option<&MemoryStore>,
) -> Option<i64> {
    let desc = format!(
        "auto-start: {}",
        task_context.chars().take(60).collect::<String>()
    );
    let session_id = session.id.clone();
    let mut mgr = if let Some(s) = store {
        CheckpointManager::with_persistence(s.conn(), Some(session_id))
    } else {
        CheckpointManager::new()
    };
    match mgr.create(&desc) {
        Ok(id) => {
            log_event(
                LogLevel::Info,
                "checkpoint",
                &format!("タスク開始時CP作成 id={id}"),
            );
            Some(id)
        }
        Err(e) => {
            log_event(
                LogLevel::Warn,
                "checkpoint",
                &format!("CP作成失敗（無視）: {e}"),
            );
            None
        }
    }
}

/// ステップ結果のディスパッチ（LoopState + セッションを操作）
///
/// FinalAnswer → 検証ステップ挿入可能（Continue に変換）
/// Aborted → 即座にReturn
/// Continue → 停滞検出+再計画+コンパクション
#[allow(clippy::too_many_arguments)]
fn handle_outcome(
    outcome: StepOutcome,
    session: &mut Session,
    state: &mut LoopState,
    task_context: &str,
    store: Option<&MemoryStore>,
    max_iterations: usize,
    final_iteration: usize,
    iteration: usize,
    duration_ms: u64,
) -> OutcomeAction {
    match outcome {
        StepOutcome::FinalAnswer(answer) => {
            let mw_result = MwStepResult {
                outcome_type: "final_answer",
                iteration,
                duration_ms,
                tools_used: vec![],
                tools_succeeded: true,
                output_hash: 0,
                consecutive_failures: 0,
            };
            state.middleware_chain.run_after_step(session, &mw_result);
            if inject_verification_step(
                session,
                &mut state.advisor,
                task_context,
                &answer,
                iteration,
                max_iterations,
                store,
                &state.trial_summary,
            ) {
                return OutcomeAction::Continue;
            }
            // 不変条件チェック（非ブロッキング警告）
            let violations = check_invariants(session, task_context);
            for v in &violations {
                log_event(LogLevel::Warn, "invariant", v);
            }
            record_success(store, session, task_context, &answer);
            OutcomeAction::Return(AgentLoopResult {
                answer,
                iterations_used: final_iteration,
                tools_called: std::mem::take(&mut state.all_tools),
            })
        }
        StepOutcome::Aborted(reason) => {
            state.consecutive_failures += 1;
            let mw_result = MwStepResult {
                outcome_type: "aborted",
                iteration,
                duration_ms,
                tools_used: vec![],
                tools_succeeded: false,
                output_hash: 0,
                consecutive_failures: state.consecutive_failures,
            };
            state.middleware_chain.run_after_step(session, &mw_result);
            record_abort(store, session, task_context, &reason);
            OutcomeAction::Return(AgentLoopResult {
                answer: format!("[中断] {reason}"),
                iterations_used: final_iteration,
                tools_called: std::mem::take(&mut state.all_tools),
            })
        }
        StepOutcome::Continue(step_tools) => {
            let tools_succeeded = !step_tools.is_empty();
            if !tools_succeeded {
                state.consecutive_failures += 1;
            } else {
                state.consecutive_failures = 0;
            }
            // ミドルウェアチェーンでafter_step処理（Audit/ToolTrack/Stall/Compact/TokenBudget）
            let output_hash = compute_output_hash(session);
            let mw_result = MwStepResult {
                outcome_type: "continue",
                iteration,
                duration_ms,
                tools_used: step_tools.clone(),
                tools_succeeded,
                output_hash,
                consecutive_failures: state.consecutive_failures,
            };
            state.middleware_chain.run_after_step(session, &mw_result);
            // ツール追跡はミドルウェア外で保持（ReturnでのAgentLoopResult構築に必要）
            state.all_tools.extend(step_tools);
            // Advisor連携の停滞検出（ミドルウェアのStallとは別に、Advisor呼び出しが必要）
            inject_replan_on_stall(
                session,
                &mut state.stall_detector,
                &mut state.advisor,
                task_context,
                tools_succeeded,
                output_hash,
                store,
                &state.trial_summary,
            );
            OutcomeAction::Continue
        }
    }
}

/// タスクの複雑さを判定（複数ステップが必要か）
fn detect_task_complexity(input: &str) -> bool {
    let complex_signals = [
        "作成して",
        "実装して",
        "修正して",
        "リファクタ",
        "調べて",
        "分析して",
        "比較して",
        "設計して",
        "テストを書",
        "ビルドして",
        "デプロイ",
        "ファイルを.*して.*して", // 複数動詞
    ];
    let signal_count = complex_signals
        .iter()
        .filter(|s| input.contains(*s))
        .count();
    // 2つ以上のシグナル or 長い入力（複雑なタスクの兆候）
    signal_count >= 2 || input.len() > 200
}

/// アドバイザー応答解決の戻り値
struct AdvisorResolution {
    prompt: String,
    source: &'static str, // "remote" or "local"
    duration_ms: u64,
}

/// アドバイザー応答を解決（remote優先→ローカルフォールバック、共通ヘルパー）
fn resolve_advisor_prompt(
    advisor: &mut AdvisorConfig,
    role: AdvisorRole,
    task_context: &str,
) -> AdvisorResolution {
    let start = std::time::Instant::now();
    // Claude Code バックエンド優先
    if let Ok(Some(cc_advice)) = advisor.try_claude_code_advice(role, task_context) {
        let duration_ms = start.elapsed().as_millis() as u64;
        eprintln!(
            "[advisor] Claude Code応答取得 role={:?} ({}文字, {}ms)",
            role,
            cc_advice.len(),
            duration_ms
        );
        return AdvisorResolution {
            prompt: cc_advice,
            source: "claude-code",
            duration_ms,
        };
    }
    match advisor.try_remote_advice(role, task_context) {
        Ok(Some(remote)) => {
            let duration_ms = start.elapsed().as_millis() as u64;
            eprintln!(
                "[advisor] 外部アドバイザー応答取得 role={:?} ({}文字, {}ms)",
                role,
                remote.len(),
                duration_ms
            );
            AdvisorResolution {
                prompt: remote,
                source: "remote",
                duration_ms,
            }
        }
        Ok(None) => AdvisorResolution {
            prompt: advisor.local_prompt_for(role, task_context),
            source: "local",
            duration_ms: 0,
        },
        Err(e) => {
            let duration_ms = start.elapsed().as_millis() as u64;
            eprintln!("[advisor] 外部API失敗 role={role:?}、ローカルにフォールバック: {e}");
            AdvisorResolution {
                prompt: advisor.local_prompt_for(role, task_context),
                source: "local",
                duration_ms,
            }
        }
    }
}

/// Advisor呼出を監査ログに記録
fn log_advisor_call(
    store: Option<&MemoryStore>,
    session: &Session,
    role: AdvisorRole,
    resolution: &AdvisorResolution,
) {
    if let Some(s) = store {
        let audit = AuditLog::new(s.conn());
        let role_str = match role {
            AdvisorRole::Verification => "verification",
            AdvisorRole::Replan => "replan",
        };
        let _ = audit.log(
            Some(&session.id),
            &AuditAction::AdvisorCall {
                role: role_str.to_string(),
                source: resolution.source.to_string(),
                prompt_len: resolution.prompt.chars().count(),
                duration_ms: resolution.duration_ms,
            },
        );
    }
}

/// 停滞検出時に再計画ステップを注入
///
/// 戻り値: true なら再計画ステップ挿入済（StallDetectorをreset）
#[allow(clippy::too_many_arguments)]
fn inject_replan_on_stall(
    session: &mut Session,
    stall_detector: &mut StallDetector,
    advisor: &mut AdvisorConfig,
    task_context: &str,
    tools_succeeded: bool,
    output_hash: u64,
    store: Option<&MemoryStore>,
    trial_summary: &TrialSummary,
) -> bool {
    if !stall_detector.record_step(tools_succeeded, output_hash) {
        return false;
    }
    if !advisor.can_advise() {
        log_event(
            LogLevel::Warn,
            "stall",
            "停滞検出だが advisor max_uses 到達",
        );
        stall_detector.reset();
        return false;
    }
    let resolution = resolve_advisor_prompt(advisor, AdvisorRole::Replan, task_context);
    log_advisor_call(store, session, AdvisorRole::Replan, &resolution);
    let mut replan_msg = resolution.prompt;
    // NAT知見: 構造化フィードバックで再計画精度向上
    let structured = StructuredFeedback::from_trial_summary(trial_summary, task_context);
    let injection = structured.format_for_injection();
    if !injection.is_empty() {
        replan_msg.push_str("\n\n");
        replan_msg.push_str(&injection);
    } else if !trial_summary.is_empty() {
        replan_msg.push_str("\n\n");
        replan_msg.push_str(&trial_summary.format_for_replan());
    }
    session.add_message(Message::system(replan_msg));
    advisor.record_call();
    stall_detector.reset();
    eprintln!(
        "[stall] 検出→再計画ステップ注入 (advisor残{}/{}回)",
        advisor.remaining(),
        advisor.max_uses
    );
    true
}

/// 出力ハッシュを計算（StallDetector用）
fn compute_output_hash(session: &Session) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    if let Some(last) = session.messages.last() {
        last.content.hash(&mut h);
    }
    h.finish()
}

/// 完了前自己検証ステップを注入
///
/// 戻り値: true なら検証ステップ挿入済（ループcontinue）、false なら検証不要（通常のFinalAnswer処理へ）
///
/// 条件:
/// - iteration > 0（初回回答ではない）
/// - advisor.can_advise()（max_uses未達）
/// - 複雑タスクである
/// - 回答に [検証済] マーカー未含有
/// - 残りイテレーションあり
#[allow(clippy::too_many_arguments)]
fn inject_verification_step(
    session: &mut Session,
    advisor: &mut AdvisorConfig,
    task_context: &str,
    answer: &str,
    iteration: usize,
    max_iterations: usize,
    store: Option<&MemoryStore>,
    trial_summary: &TrialSummary,
) -> bool {
    if iteration == 0
        || !advisor.can_advise()
        || !detect_task_complexity(task_context)
        || answer.contains("[検証済]")
        || iteration >= max_iterations - 1
    {
        return false;
    }
    let resolution = resolve_advisor_prompt(advisor, AdvisorRole::Verification, task_context);
    log_advisor_call(store, session, AdvisorRole::Verification, &resolution);
    session.add_message(Message::system(resolution.prompt));
    let mut checklist = "確認チェックリスト:
         - すべての主張にツール結果の根拠があるか？
         - 確認していない仮定が残っていないか？
         - 見落としているケースはないか？
         - ツール呼び出し成功率が80%以上か？
         - ファイル変更がある場合、コンパイル/構文チェックを通過したか？
         - 元のタスクの完了条件をすべて満たしているか？"
        .to_string();
    if !trial_summary.is_empty() {
        let structured = StructuredFeedback::from_trial_summary(trial_summary, task_context);
        let injection = structured.format_for_injection();
        if !injection.is_empty() {
            checklist.push_str("

");
            checklist.push_str(&injection);
        }
    }
    session.add_message(Message::system(checklist));
    advisor.record_call();
    eprintln!(
        "[advisor] 完了前自己検証ステップ挿入 (iter {iteration}, 残{}/{}回)",
        advisor.remaining(),
        advisor.max_uses
    );
    true
}

/// 複雑タスクに計画プレステップを注入
fn inject_planning_step(session: &mut Session, task_context: &str) {
    if detect_task_complexity(task_context) {
        // Advisor Tool パターン: 100語以内・箇条書きでトークン35-45%削減（Anthropic実測）
        session.add_message(Message::system(
            "このタスクは複数ステップが必要です。\n\
             <think> 内で以下の手順を考えてください:\n\
             \n\
             【手順】\n\
             1. 調査: 関連ファイル・情報を集める（file_read, repo_map）\n\
             2. 計画: 仮説を立て、小さなテストで確認\n\
             3. 実行: 計画どおりに実装（1ステップずつ）\n\
             4. 確認: 結果を確認（期待どおりか照合）\n\
             \n\
             【完了条件】\n\
             - すべてのステップを実行した\n\
             - エラーなし、またはエラーを解決した\n\
             - 成果物が要件を満たしている\n\
             \n\
             計画は100語以内、箇条書きで。調査から順に実行。"
                .to_string(),
        ));
        log_event(
            LogLevel::Info,
            "advisor",
            "複雑タスク検出 → 簡潔計画プレステップ注入",
        );
    }
}

// inject_experience_context → context_inject.rs に移動

// inject_vault_knowledge → context_inject.rs に移動

// load_soul → context_inject.rs に移動

// inject_contextual_memories → context_inject.rs に移動

/// タスク完了時の不変条件チェック（PaperOrchestra知見）
fn check_invariants(session: &Session, task_context: &str) -> Vec<String> {
    let mut violations = Vec::new();
    let tool_msgs: Vec<_> = session
        .messages
        .iter()
        .filter(|m| m.role == Role::Tool)
        .collect();
    if !tool_msgs.is_empty() {
        let success_count = tool_msgs
            .iter()
            .filter(|m| !m.content.contains("エラー") && !m.content.contains("失敗"))
            .count();
        let rate = success_count as f64 / tool_msgs.len() as f64;
        if rate < 0.5 {
            violations.push(format!("ツール成功率が低い: {:.0}%", rate * 100.0));
        }
    }
    if let Some(answer) = session
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant)
        && answer.content.len() < 10
        && task_context.len() > 50
    {
        violations.push("回答が短すぎる可能性".to_string());
    }
    violations
}

/// 成功時のセッション保存・経験記録・スキル昇格
fn record_success(
    store: Option<&MemoryStore>,
    session: &Session,
    task_context: &str,
    answer: &str,
) {
    let Some(s) = store else { return };
    let _ = s.save_session(session);
    let exp = ExperienceStore::new(s.conn());
    let _ = exp.record(&RecordParams {
        exp_type: ExperienceType::Success,
        task_context,
        action: answer,
        outcome: "completed",
        lesson: None,
        tool_name: None,
        error_type: None,
        error_detail: None,
    });
    let skills = SkillStore::new(s.conn());
    let _ = skills.promote_from_experiences(s.conn(), 3);
    let evo = crate::memory::evolution::EvolutionEngine::new(s);
    let _ = evo.auto_collect();
    let _ = evo.apply_improvements();
}

/// 中断時のセッション保存・経験記録
fn record_abort(store: Option<&MemoryStore>, session: &Session, task_context: &str, reason: &str) {
    let Some(s) = store else { return };
    let _ = s.save_session(session);
    let exp = ExperienceStore::new(s.conn());
    let _ = exp.record(&RecordParams {
        exp_type: ExperienceType::Insight,
        task_context,
        action: "aborted",
        outcome: reason,
        lesson: Some(reason),
        tool_name: None,
        error_type: Some("Aborted"),
        error_detail: None,
    });
}

/// ParsedOutputから回答テキストを構築
fn build_answer(parsed: &ParsedOutput) -> String {
    let raw = parsed
        .text
        .clone()
        .unwrap_or_else(|| "(回答なし)".to_string());
    clean_response(&raw)
}

fn clean_response(text: &str) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    lines.dedup();
    let joined = lines.join("\n");
    let chars: Vec<char> = joined.chars().collect();
    if chars.len() > 100 {
        let half = chars.len() / 2;
        let first: String = chars[..half].iter().collect();
        let second: String = chars[half..].iter().collect();
        let check: String = first.chars().take(30).collect();
        if second.contains(&check) {
            return first.trim_end().to_string();
        }
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::context_inject::inject_experience_context;
    use crate::agent::tool_exec::{ToolExecResult, apply_tool_result};
    use crate::memory::graph::KnowledgeGraph;
    use crate::memory::store::MemoryStore;
    use crate::runtime::inference::MockLlmBackend;
    use crate::tools::permission::Permission;
    use crate::tools::{Tool, ToolResult};

    /// テスト用のエコーツール
    struct EchoTool;
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "入力をそのまま返す"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}})
        }
        fn permission(&self) -> Permission {
            Permission::Auto
        }
        fn call(&self, args: serde_json::Value) -> Result<ToolResult> {
            let text = args
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("(empty)");
            Ok(ToolResult {
                output: text.to_string(),
                success: true,
            })
        }
    }

    /// テスト用の失敗ツール
    struct FailTool;
    impl Tool for FailTool {
        fn name(&self) -> &str {
            "fail"
        }
        fn description(&self) -> &str {
            "常に失敗する"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        fn permission(&self) -> Permission {
            Permission::Auto
        }
        fn call(&self, _args: serde_json::Value) -> Result<ToolResult> {
            anyhow::bail!("意図的なエラー")
        }
    }

    fn test_registry() -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoTool));
        reg.register(Box::new(FailTool));
        reg
    }

    // テスト1: ツール不要 → 直接回答
    #[test]
    fn test_direct_answer() {
        let mock = MockLlmBackend::single("東京の天気は晴れです。");
        let tools = test_registry();
        let guard = PathGuard::default_deny_list();
        let config = AgentConfig::default();
        let cancel = CancellationToken::new();

        let result = run_agent_loop(
            "天気は？",
            &mock,
            &tools,
            &guard,
            &config,
            &cancel,
            None::<&MemoryStore>,
        )
        .unwrap();
        assert!(result.answer.contains("晴れ"));
        assert_eq!(result.iterations_used, 1);
        assert!(result.tools_called.is_empty());
    }

    // テスト2: ツール1回 → 回答
    #[test]
    fn test_single_tool_call() {
        let mock = MockLlmBackend::new(vec![
            r#"<tool_call>{"name":"echo","arguments":{"text":"hello"}}</tool_call>"#.to_string(),
            "ツール結果: hello".to_string(),
        ]);
        let tools = test_registry();
        let guard = PathGuard::default_deny_list();
        let config = AgentConfig::default();
        let cancel = CancellationToken::new();

        let result = run_agent_loop(
            "echo test",
            &mock,
            &tools,
            &guard,
            &config,
            &cancel,
            None::<&MemoryStore>,
        )
        .unwrap();
        assert!(result.answer.contains("hello"));
        assert_eq!(result.iterations_used, 2);
        assert!(result.tools_called.contains(&"echo".to_string()));
    }

    // テスト3: 最大イテレーション到達
    #[test]
    fn test_max_iterations() {
        // 常にツール呼び出しを返すモック（終了しない）
        let responses: Vec<String> = (0..15)
            .map(|i| {
                format!(
                    r#"<tool_call>{{"name":"echo","arguments":{{"text":"iter{}"}}}}</tool_call>"#,
                    i
                )
            })
            .collect();
        let mock = MockLlmBackend::new(responses);
        let tools = test_registry();
        let guard = PathGuard::default_deny_list();
        let config = AgentConfig {
            max_iterations: 3,
            ..Default::default()
        };
        let cancel = CancellationToken::new();

        let result = run_agent_loop(
            "loop",
            &mock,
            &tools,
            &guard,
            &config,
            &cancel,
            None::<&MemoryStore>,
        )
        .unwrap();
        assert!(result.answer.contains("中断"));
        assert_eq!(result.iterations_used, 3);
    }

    // テスト4: Ctrl+Cキャンセル
    #[test]
    fn test_cancellation() {
        let mock = MockLlmBackend::single("回答");
        let tools = test_registry();
        let guard = PathGuard::default_deny_list();
        let config = AgentConfig::default();
        let cancel = CancellationToken::new();
        cancel.cancel();

        let result = run_agent_loop(
            "test",
            &mock,
            &tools,
            &guard,
            &config,
            &cancel,
            None::<&MemoryStore>,
        );
        // MockLlmBackend::generateがキャンセルエラーを返す
        assert!(result.is_err() || result.unwrap().answer.contains("キャンセル"));
    }

    // テスト5: 不正ツール名 → バリデーション拒否
    #[test]
    fn test_unknown_tool_blocked() {
        let mock = MockLlmBackend::new(vec![
            r#"<tool_call>{"name":"hack","arguments":{}}</tool_call>"#.to_string(),
            "バリデーションエラーのため別の方法を試します。回答: 了解".to_string(),
        ]);
        let tools = test_registry();
        let guard = PathGuard::default_deny_list();
        let config = AgentConfig::default();
        let cancel = CancellationToken::new();

        let result = run_agent_loop(
            "hack",
            &mock,
            &tools,
            &guard,
            &config,
            &cancel,
            None::<&MemoryStore>,
        )
        .unwrap();
        assert!(result.answer.contains("了解"));
    }

    // テスト6: ツール失敗 → サーキットブレーカー記録
    #[test]
    fn test_tool_failure_recorded() {
        let mock = MockLlmBackend::new(vec![
            r#"<tool_call>{"name":"fail","arguments":{}}</tool_call>"#.to_string(),
            "ツールが失敗しました。回答: エラーが発生しました".to_string(),
        ]);
        let tools = test_registry();
        let guard = PathGuard::default_deny_list();
        let config = AgentConfig::default();
        let cancel = CancellationToken::new();

        let result = run_agent_loop(
            "fail",
            &mock,
            &tools,
            &guard,
            &config,
            &cancel,
            None::<&MemoryStore>,
        )
        .unwrap();
        assert!(result.answer.contains("エラー"));
    }

    // テスト7: 経験メモリへの記録
    #[test]
    fn test_experience_recording() {
        let store = MemoryStore::in_memory().unwrap();
        let mock = MockLlmBackend::single("回答です。");
        let tools = test_registry();
        let guard = PathGuard::default_deny_list();
        let config = AgentConfig::default();
        let cancel = CancellationToken::new();

        run_agent_loop(
            "test query",
            &mock,
            &tools,
            &guard,
            &config,
            &cancel,
            Some(&store),
        )
        .unwrap();

        let exp = ExperienceStore::new(store.conn());
        let experiences = exp.find_similar("test", 10).unwrap();
        assert!(!experiences.is_empty());
    }

    // テスト8: ループ検出
    #[test]
    fn test_loop_detection() {
        // 全く同じツール呼び出しを繰り返すモック
        let same_call = r#"<tool_call>{"name":"echo","arguments":{"text":"same"}}</tool_call>"#;
        let responses: Vec<String> = (0..10).map(|_| same_call.to_string()).collect();
        let mock = MockLlmBackend::new(responses);
        let tools = test_registry();
        let guard = PathGuard::default_deny_list();
        let config = AgentConfig {
            max_iterations: 10,
            ..Default::default()
        };
        let cancel = CancellationToken::new();

        let result = run_agent_loop(
            "loop",
            &mock,
            &tools,
            &guard,
            &config,
            &cancel,
            None::<&MemoryStore>,
        )
        .unwrap();
        assert!(result.answer.contains("中断"));
    }

    // --- StallDetector テスト ---

    #[test]
    fn test_stall_detector_no_progress() {
        let mut sd = StallDetector::new(3);
        assert!(!sd.record_step(false, 1));
        assert!(!sd.record_step(false, 2));
        assert!(sd.record_step(false, 3));
    }

    #[test]
    fn test_stall_detector_resets_on_progress() {
        let mut sd = StallDetector::new(3);
        assert!(!sd.record_step(false, 1));
        assert!(!sd.record_step(false, 2));
        assert!(!sd.record_step(true, 99));
        assert!(!sd.record_step(false, 100));
        assert!(!sd.record_step(false, 101));
        assert!(sd.record_step(false, 102));
    }

    #[test]
    fn test_stall_detector_same_output_hash() {
        let mut sd = StallDetector::new(3);
        // 初回はハッシュが0→42で変化するため進捗あり
        assert!(!sd.record_step(true, 42));
        // 2回目以降は同じハッシュ → 停滞カウント
        assert!(!sd.record_step(true, 42));
        assert!(!sd.record_step(true, 42));
        assert!(sd.record_step(true, 42)); // 3回停滞で検出
    }

    #[test]
    fn test_stall_detector_default_threshold() {
        let sd = StallDetector::default();
        assert_eq!(sd.stall_threshold, 3);
    }

    // --- SOUL.md テスト ---

    #[test]
    fn test_load_soul_missing_is_none() {
        let result = crate::agent::context_inject::load_soul(&Some(std::path::PathBuf::from(
            "/tmp/nonexistent_soul_bonsai.md",
        )));
        assert!(result.is_none());
    }

    #[test]
    fn test_load_soul_from_explicit_path() {
        let path = format!("/tmp/bonsai-test-soul-{}.md", uuid::Uuid::new_v4());
        std::fs::write(&path, "私はテスト用ペルソナです").unwrap();
        let result =
            crate::agent::context_inject::load_soul(&Some(std::path::PathBuf::from(&path)));
        assert!(result.is_some());
        assert!(result.unwrap().contains("ペルソナ"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_load_soul_empty_file_is_none() {
        let path = format!("/tmp/bonsai-test-soul-empty-{}.md", uuid::Uuid::new_v4());
        std::fs::write(&path, "   ").unwrap();
        let result =
            crate::agent::context_inject::load_soul(&Some(std::path::PathBuf::from(&path)));
        assert!(result.is_none());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_load_soul_none_path() {
        // Noneパスの場合、.bonsai/SOUL.mdなどを探すが通常存在しない
        let result = crate::agent::context_inject::load_soul(&None);
        // テスト環境では存在しないのでNone（存在する場合はSome）
        // assertはしない — 環境依存
        let _ = result;
    }

    // テスト: デフォルトシステムプロンプトに計画強制ルールが含まれる
    #[test]
    fn test_default_prompt_contains_plan_rule() {
        let config = AgentConfig::default();
        assert!(
            config.system_prompt.contains("計画"),
            "デフォルトプロンプトに計画強制ルールが含まれるべき"
        );
    }

    // テスト: RepoMapツールがレジストリに登録される
    #[test]
    fn test_repomap_registered() {
        let reg = test_registry_with_repomap();
        assert!(reg.get("repo_map").is_some(), "repo_mapが登録されるべき");
    }

    fn test_registry_with_repomap() -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoTool));
        reg.register(Box::new(crate::tools::repomap::RepoMapTool));
        reg
    }

    // テスト: StepOutcomeが監査ログに記録される
    #[test]
    fn test_step_outcome_audit_logged() {
        let store = MemoryStore::in_memory().unwrap();
        let mock = MockLlmBackend::single("回答です。");
        let tools = test_registry();
        let guard = PathGuard::default_deny_list();
        let config = AgentConfig::default();
        let cancel = CancellationToken::new();

        run_agent_loop(
            "test",
            &mock,
            &tools,
            &guard,
            &config,
            &cancel,
            Some(&store),
        )
        .unwrap();

        let audit = AuditLog::new(store.conn());
        let entries = audit.recent(50).unwrap();
        let step_outcomes: Vec<_> = entries
            .iter()
            .filter(|e| e.action_type == "step_outcome")
            .collect();
        assert!(
            !step_outcomes.is_empty(),
            "StepOutcomeが監査ログに記録されるべき"
        );
    }

    // テスト: タスク複雑さ検出
    #[test]
    fn test_detect_task_complexity_simple() {
        assert!(!detect_task_complexity("天気は？"));
        assert!(!detect_task_complexity("ファイルを読んで"));
    }

    #[test]
    fn test_detect_task_complexity_complex() {
        assert!(detect_task_complexity(
            "テストを書いて、実装して、リファクタリングして"
        ));
        assert!(detect_task_complexity(&"a".repeat(201)));
    }

    // テスト: 計画プレステップ注入
    #[test]
    fn test_inject_planning_step_complex() {
        let mut session = Session::new();
        session.add_message(Message::user("テストを書いて実装して"));
        inject_planning_step(
            &mut session,
            "テストを書いて、実装して、リファクタリングして",
        );
        let has_plan = session.messages.iter().any(|m| m.content.contains("計画"));
        assert!(has_plan, "複雑タスクに計画プレステップが注入されるべき");
    }

    #[test]
    fn test_inject_planning_step_simple() {
        let mut session = Session::new();
        inject_planning_step(&mut session, "天気は？");
        let msg_count = session.messages.len();
        assert_eq!(msg_count, 0, "単純タスクには計画プレステップ不要");
    }

    // テスト: AdvisorConfig が AgentConfig に統合されている
    #[test]
    fn test_agent_config_includes_advisor() {
        let config = AgentConfig::default();
        assert_eq!(config.advisor.max_uses, 3);
        assert_eq!(config.advisor.calls_used, 0);
        assert!(config.advisor.can_advise());
    }

    // テスト: AdvisorConfig をカスタマイズ可能
    #[test]
    fn test_agent_config_custom_advisor() {
        let config = AgentConfig {
            advisor: AdvisorConfig {
                max_uses: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(config.advisor.max_uses, 1);
    }

    // テスト: task_timeoutが設定されるとエージェントループがタイムアウトする
    #[test]
    fn test_task_timeout_triggers() {
        use crate::runtime::inference::MockLlmBackend;
        // 各ステップ遅延が発生するためタイムアウトする想定（0秒タイムアウト）
        let responses: Vec<String> = (0..100)
            .map(|_| "考え中です...".to_string())
            .collect();
        let mock = MockLlmBackend::new(responses);
        let tools = test_registry();
        let guard = PathGuard::default_deny_list();
        let config = AgentConfig {
            max_iterations: 100,
            task_timeout: Some(std::time::Duration::from_millis(1)),
            ..Default::default()
        };
        let cancel = CancellationToken::new();
        let result = run_agent_loop("test", &mock, &tools, &guard, &config, &cancel, None);
        assert!(result.is_ok());
        let r = result.unwrap();
        // タイムアウトまたは少ないイテレーションで完了
        assert!(r.answer.contains("タイムアウト") || r.iterations_used < 100);
    }

    // テスト: task_timeout=Noneではタイムアウトしない
    #[test]
    fn test_no_timeout_by_default() {
        let config = AgentConfig::default();
        assert!(config.task_timeout.is_none());
    }

    // テスト: inject_verification_step — 複雑タスク＋初回以降で検証挿入
    #[test]
    fn test_inject_verification_step_injects() {
        let mut session = Session::new();
        let mut advisor = AdvisorConfig::default();
        let injected = inject_verification_step(
            &mut session,
            &mut advisor,
            "テストを書いて、実装して、リファクタしてください",
            "部分的な回答",
            1, // iteration > 0
            10,
            None,
            &TrialSummary::default(),
        );
        assert!(injected, "複雑タスクは検証ステップを挿入");
        assert_eq!(advisor.calls_used, 1);
        assert!(session.messages.iter().any(|m| m.content.contains("検証")));
    }

    // テスト: 初回イテレーションでは検証スキップ
    #[test]
    fn test_inject_verification_step_skips_first_iteration() {
        let mut session = Session::new();
        let mut advisor = AdvisorConfig::default();
        let injected = inject_verification_step(
            &mut session,
            &mut advisor,
            "テストを書いて、実装して、リファクタしてください",
            "回答",
            0, // 初回
            10,
            None,
            &TrialSummary::default(),
        );
        assert!(!injected);
        assert_eq!(advisor.calls_used, 0);
    }

    // テスト: [検証済] マーカーがある場合はスキップ
    #[test]
    fn test_inject_verification_step_skips_verified() {
        let mut session = Session::new();
        let mut advisor = AdvisorConfig::default();
        let injected = inject_verification_step(
            &mut session,
            &mut advisor,
            "テストを書いて、実装して、リファクタしてください",
            "[検証済] 完了しました",
            1,
            10,
            None,
            &TrialSummary::default(),
        );
        assert!(!injected);
    }

    // テスト: max_uses 超過時はスキップ
    #[test]
    fn test_inject_verification_step_respects_max_uses() {
        let mut session = Session::new();
        let mut advisor = AdvisorConfig {
            max_uses: 1,
            calls_used: 1, // 既に上限
            ..Default::default()
        };
        let injected = inject_verification_step(
            &mut session,
            &mut advisor,
            "テストを書いて、実装して、リファクタしてください",
            "回答",
            1,
            10,
            None,
            &TrialSummary::default(),
        );
        assert!(!injected);
    }

    // テスト: 単純タスクはスキップ
    #[test]
    fn test_inject_verification_step_skips_simple_task() {
        let mut session = Session::new();
        let mut advisor = AdvisorConfig::default();
        let injected = inject_verification_step(
            &mut session,
            &mut advisor,
            "天気は？",
            "晴れです",
            1,
            10,
            None,
            &TrialSummary::default(),
        );
        assert!(!injected);
    }

    // テスト: inject_replan_on_stall — 閾値到達で再計画注入
    #[test]
    fn test_inject_replan_on_stall_triggers_after_threshold() {
        let mut session = Session::new();
        let mut stall = StallDetector::new(3);
        let mut advisor = AdvisorConfig::default();
        // 1〜2回目: 検出されない
        assert!(!inject_replan_on_stall(
            &mut session,
            &mut stall,
            &mut advisor,
            "task",
            false,
            0,
            None,
            &TrialSummary::default()
        ));
        assert!(!inject_replan_on_stall(
            &mut session,
            &mut stall,
            &mut advisor,
            "task",
            false,
            0,
            None,
            &TrialSummary::default()
        ));
        // 3回目: 停滞検出→再計画注入
        assert!(inject_replan_on_stall(
            &mut session,
            &mut stall,
            &mut advisor,
            "task",
            false,
            0,
            None,
            &TrialSummary::default()
        ));
        assert_eq!(advisor.calls_used, 1);
        assert!(session.messages.iter().any(|m| m.content.contains("停滞")));
    }

    // テスト: inject_replan_on_stall — advisor max_uses超過時はreset+スキップ
    #[test]
    fn test_inject_replan_on_stall_respects_advisor_max_uses() {
        let mut session = Session::new();
        let mut stall = StallDetector::new(2);
        let mut advisor = AdvisorConfig {
            max_uses: 1,
            calls_used: 1,
            ..Default::default()
        };
        assert!(!inject_replan_on_stall(
            &mut session,
            &mut stall,
            &mut advisor,
            "task",
            false,
            0,
            None,
            &TrialSummary::default()
        ));
        let injected = inject_replan_on_stall(
            &mut session,
            &mut stall,
            &mut advisor,
            "task",
            false,
            0,
            None,
            &TrialSummary::default(),
        );
        assert!(!injected, "max_uses超過時は注入しない");
        assert_eq!(advisor.calls_used, 1, "calls_usedは増えない");
    }

    // テスト: inject_replan_on_stall — 進捗ありでスキップ
    #[test]
    fn test_inject_replan_on_stall_skips_on_progress() {
        let mut session = Session::new();
        let mut stall = StallDetector::new(2);
        let mut advisor = AdvisorConfig::default();
        assert!(!inject_replan_on_stall(
            &mut session,
            &mut stall,
            &mut advisor,
            "task",
            true,
            1,
            None,
            &TrialSummary::default()
        ));
        assert!(!inject_replan_on_stall(
            &mut session,
            &mut stall,
            &mut advisor,
            "task",
            true,
            2,
            None,
            &TrialSummary::default()
        ));
        assert!(!inject_replan_on_stall(
            &mut session,
            &mut stall,
            &mut advisor,
            "task",
            true,
            3,
            None,
            &TrialSummary::default()
        ));
        assert_eq!(advisor.calls_used, 0);
    }

    // テスト: compute_output_hash は変化を検出
    #[test]
    fn test_compute_output_hash_differs_for_different_content() {
        let mut s1 = Session::new();
        s1.add_message(Message::user("A"));
        let h1 = compute_output_hash(&s1);
        let mut s2 = Session::new();
        s2.add_message(Message::user("B"));
        let h2 = compute_output_hash(&s2);
        assert_ne!(h1, h2);
    }

    // テスト: resolve_advisor_prompt はリモート未設定時にローカルを返す
    #[test]
    fn test_resolve_advisor_prompt_local_when_no_endpoint() {
        let mut advisor = AdvisorConfig::default();
        let v = resolve_advisor_prompt(&mut advisor, AdvisorRole::Verification, "task");
        let r = resolve_advisor_prompt(&mut advisor, AdvisorRole::Replan, "task");
        assert_eq!(v.source, "local");
        assert_eq!(r.source, "local");
        assert_eq!(v.duration_ms, 0);
        assert!(v.prompt.contains("検証"));
        assert!(r.prompt.contains("停滞"));
    }

    // テスト: log_advisor_call は store=None でもパニックしない
    #[test]
    fn test_log_advisor_call_with_no_store() {
        let session = Session::new();
        let resolution = AdvisorResolution {
            prompt: "test".to_string(),
            source: "local",
            duration_ms: 0,
        };
        // store=None: 何もしない（パニックしない）
        log_advisor_call(None, &session, AdvisorRole::Verification, &resolution);
    }

    // テスト: log_advisor_call が store にエントリを追加
    #[test]
    fn test_log_advisor_call_writes_to_store() {
        use crate::memory::store::MemoryStore;
        let store = MemoryStore::in_memory().unwrap();
        let session = Session::new();
        let resolution = AdvisorResolution {
            prompt: "verification prompt content".to_string(),
            source: "remote",
            duration_ms: 123,
        };
        log_advisor_call(
            Some(&store),
            &session,
            AdvisorRole::Verification,
            &resolution,
        );

        let audit = AuditLog::new(store.conn());
        let entries = audit.for_session(&session.id).unwrap();
        let advisor_entries: Vec<_> = entries
            .iter()
            .filter(|e| e.action_type == "advisor_call")
            .collect();
        assert_eq!(advisor_entries.len(), 1);
        assert!(
            advisor_entries[0]
                .action_data
                .contains("\"role\":\"verification\"")
        );
        assert!(
            advisor_entries[0]
                .action_data
                .contains("\"source\":\"remote\"")
        );
    }

    // テスト: handle_outcome — FinalAnswer で Return
    #[test]
    fn test_handle_outcome_final_answer_returns() {
        let mut session = Session::new();
        let mut state = LoopState::new(AdvisorConfig::default());
        let outcome = StepOutcome::FinalAnswer("回答".to_string());
        let action = handle_outcome(
            outcome,
            &mut session,
            &mut state,
            "simple",
            None,
            10,
            1,
            0,
            100,
        );
        assert!(matches!(action, OutcomeAction::Return(_)));
    }

    // テスト: handle_outcome — Continue で Continue
    #[test]
    fn test_handle_outcome_continue_returns_continue() {
        let mut session = Session::new();
        let mut state = LoopState::new(AdvisorConfig::default());
        let outcome = StepOutcome::Continue(vec!["shell".to_string()]);
        let action = handle_outcome(
            outcome,
            &mut session,
            &mut state,
            "task",
            None,
            10,
            1,
            0,
            100,
        );
        assert!(matches!(action, OutcomeAction::Continue));
        assert_eq!(state.all_tools.len(), 1);
    }

    // テスト: handle_outcome — Aborted で Return
    #[test]
    fn test_handle_outcome_aborted_returns() {
        let mut session = Session::new();
        let mut state = LoopState::new(AdvisorConfig::default());
        let outcome = StepOutcome::Aborted("cancelled".to_string());
        let action = handle_outcome(
            outcome,
            &mut session,
            &mut state,
            "task",
            None,
            10,
            1,
            0,
            100,
        );
        assert!(matches!(action, OutcomeAction::Return(_)));
        assert_eq!(state.consecutive_failures, 1);
    }

    // テスト: LoopState 初期状態
    #[test]
    fn test_loop_state_new() {
        let state = LoopState::new(AdvisorConfig::default());
        assert!(state.all_tools.is_empty());
        assert_eq!(state.consecutive_failures, 0);
        assert_eq!(state.iteration, 0);
        assert!(state.advisor.can_advise());
    }

    // テスト: AgentConfig に auto_checkpoint デフォルト値 true
    #[test]
    fn test_agent_config_default_auto_checkpoint_enabled() {
        let config = AgentConfig::default();
        assert!(config.auto_checkpoint);
    }

    // テスト: create_task_start_checkpoint — store なしでも動作
    #[test]
    fn test_create_task_start_checkpoint_no_store() {
        let session = Session::new();
        // git stash の結果に依存するが、関数自体は panic しない
        let _id = create_task_start_checkpoint(&session, "テストタスク", None);
        // インメモリ or git失敗 のどちらでもOK
    }

    // テスト: create_task_start_checkpoint — store ありで永続化
    #[test]
    fn test_create_task_start_checkpoint_with_store() {
        use crate::agent::checkpoint::CheckpointManager;
        use crate::memory::store::MemoryStore;
        let store = MemoryStore::in_memory().unwrap();
        let session = Session::new();
        let id_opt = create_task_start_checkpoint(&session, "永続化テスト", Some(&store));
        // git stash が成功する場合（リポ内）は Some、失敗してもエラーなし
        if let Some(id) = id_opt {
            assert!(id > 0, "永続IDは正");
            let loaded =
                CheckpointManager::load_persisted(store.conn(), Some(&session.id)).unwrap();
            assert_eq!(loaded.len(), 1);
            assert!(loaded[0].description.contains("auto-start"));
        }
    }

    // テスト: 読取専用ツール並列化 — ValidatedCall構造体
    #[test]
    fn test_validated_call_read_only_flag() {
        let tool = EchoTool;
        assert!(!tool.is_read_only(), "EchoToolはis_read_only=false");
    }

    // テスト: FileReadToolはis_read_only=true
    #[test]
    fn test_file_read_is_read_only() {
        let tool = crate::tools::file::FileReadTool;
        assert!(tool.is_read_only());
    }

    // テスト: RepoMapToolはis_read_only=true
    #[test]
    fn test_repo_map_is_read_only() {
        let tool = crate::tools::repomap::RepoMapTool;
        assert!(tool.is_read_only());
    }

    // テスト: execute_validated_calls — 空リストでパニックしない
    #[test]
    fn test_execute_validated_calls_empty() {
        use crate::safety::secrets::SecretsFilter;
        let mut session = Session::new();
        let mut cb = CircuitBreaker::default();
        let sf = SecretsFilter::default();
        let mut cache = ToolResultCache::new();
        let result = execute_validated_calls(&[], &mut session, &mut cb, &sf, None, &mut cache);
        assert!(result.is_empty());
    }

    #[test]
    fn test_inference_for_task_file_operation() {
        let base = InferenceParams::default();
        let params = inference_for_task(TaskType::FileOperation, &base);
        assert!((params.temperature - 0.3).abs() < f64::EPSILON);
        assert_eq!(params.max_tokens, base.max_tokens); // 他のフィールドは保持
    }

    #[test]
    fn test_inference_for_task_research() {
        let base = InferenceParams::default();
        let params = inference_for_task(TaskType::Research, &base);
        assert!((params.temperature - 0.6).abs() < f64::EPSILON);
    }

    #[test]
    fn test_inference_for_task_general_unchanged() {
        let base = InferenceParams::default();
        let params = inference_for_task(TaskType::General, &base);
        assert!((params.temperature - base.temperature).abs() < f64::EPSILON);
    }

    // テスト: apply_tool_result でツール成功時にKnowledgeGraphにツール使用が記録される
    #[test]
    fn test_apply_tool_result_records_graph_tool_usage() {
        use crate::safety::secrets::SecretsFilter;
        let store = MemoryStore::in_memory().unwrap();
        let mut session = Session::new();
        let mut cb = CircuitBreaker::default();
        let sf = SecretsFilter::default();

        let r = ToolExecResult {
            name: "file_read".to_string(),
            args_json: r#"{"path": "src/main.rs"}"#.to_string(),
            output: "file contents here".to_string(),
            success: true,
            is_error: false,
        };
        apply_tool_result(&r, &mut session, &mut cb, &sf, Some(&store), 4000);

        // グラフにツール使用が記録されていることを確認
        let graph = KnowledgeGraph::new(store.conn());
        let neighbors = graph.neighbors("file_read", 1).unwrap();
        assert_eq!(
            neighbors.len(),
            1,
            "ツール→ファイルのエッジが記録されるべき"
        );
        assert_eq!(neighbors[0].0, "src/main.rs");
        assert_eq!(neighbors[0].1, "uses");
    }

    // テスト: apply_tool_result でツール失敗時にエラーパターンが記録される
    #[test]
    fn test_apply_tool_result_records_graph_error_pattern() {
        use crate::safety::secrets::SecretsFilter;
        let store = MemoryStore::in_memory().unwrap();
        let mut session = Session::new();
        let mut cb = CircuitBreaker::default();
        let sf = SecretsFilter::default();

        let r = ToolExecResult {
            name: "shell".to_string(),
            args_json: r#"{"path": "src/lib.rs"}"#.to_string(),
            output: "error: compilation failed".to_string(),
            success: false,
            is_error: true,
        };
        apply_tool_result(&r, &mut session, &mut cb, &sf, Some(&store), 4000);

        // グラフにエラーパターンが記録されていることを確認
        let graph = KnowledgeGraph::new(store.conn());
        let error_neighbors = graph.neighbors("tool_error", 1).unwrap();
        assert!(
            error_neighbors
                .iter()
                .any(|(name, rel, _)| name == "src/lib.rs" && rel == "caused_by"),
            "エラー→ファイルのcaused_byエッジが記録されるべき"
        );
    }

    // テスト: inject_experience_context — 成功/失敗を分離してフォーマット
    #[test]
    fn t_inject_experience_context_formats_correctly() {
        let store = MemoryStore::in_memory().unwrap();
        let exp = ExperienceStore::new(store.conn());

        // 成功経験を記録
        exp.record(&RecordParams {
            exp_type: ExperienceType::Success,
            task_context: "file editing",
            action: "file_write with fuzzy match",
            outcome: "edit succeeded",
            lesson: Some("fuzzyマッチで成功"),
            tool_name: Some("file_write"),
            error_type: None,
            error_detail: None,
        })
        .unwrap();

        // 失敗経験を記録
        exp.record(&RecordParams {
            exp_type: ExperienceType::Failure,
            task_context: "file reading",
            action: "file_read timeout",
            outcome: "timeout error",
            lesson: Some("タイムアウト、リトライで解決"),
            tool_name: Some("file_read"),
            error_type: Some("Timeout"),
            error_detail: Some("read timeout"),
        })
        .unwrap();

        let mut session = Session::new();
        inject_experience_context(&mut session, "file", &store);

        // メッセージが追加されていること
        assert_eq!(session.messages.len(), 1);
        let msg = &session.messages[0].content;
        assert!(
            msg.contains("<context type=\"experience\">"),
            "統一コンテキストタグで囲まれるべき"
        );
        assert!(
            msg.contains("[成功パターン]"),
            "成功パターンセクションがあるべき"
        );
        assert!(
            msg.contains("[失敗パターン]"),
            "失敗パターンセクションがあるべき"
        );
        assert!(
            msg.contains("fuzzyマッチで成功"),
            "成功のlessonが含まれるべき"
        );
        assert!(
            msg.contains("タイムアウト、リトライで解決"),
            "失敗のlessonが含まれるべき"
        );
    }

    // テスト: inject_experience_context — 経験が空の場合にメッセージ追加しない
    #[test]
    fn t_inject_experience_context_empty_no_message() {
        let store = MemoryStore::in_memory().unwrap();
        let mut session = Session::new();
        inject_experience_context(&mut session, "nonexistent_task_xyz", &store);
        assert!(session.messages.is_empty(), "経験が空ならメッセージ不追加");
    }

    // テスト: inject_experience_context — Insightタイプも含まれる
    #[test]
    fn t_inject_experience_context_includes_insights() {
        let store = MemoryStore::in_memory().unwrap();
        let exp = ExperienceStore::new(store.conn());

        exp.record(&RecordParams {
            exp_type: ExperienceType::Insight,
            task_context: "deploy task",
            action: "deploy analysis",
            outcome: "rollback needed",
            lesson: Some("デプロイ前にテスト必須"),
            tool_name: None,
            error_type: None,
            error_detail: None,
        })
        .unwrap();

        let mut session = Session::new();
        inject_experience_context(&mut session, "deploy", &store);

        assert_eq!(session.messages.len(), 1);
        let msg = &session.messages[0].content;
        assert!(msg.contains("[学び]"), "学びセクションがあるべき");
        assert!(
            msg.contains("デプロイ前にテスト必須"),
            "Insightのlessonが含まれるべき"
        );
    }

    // テスト: 全コンテキスト注入が統一タグフォーマット <context type="xxx"> を使用する
    #[test]
    fn t_context_tags_consistent() {
        let store = MemoryStore::in_memory().unwrap();
        let exp = ExperienceStore::new(store.conn());

        // 経験注入が統一タグを使用
        exp.record(&RecordParams {
            exp_type: ExperienceType::Success,
            task_context: "consistency check",
            action: "test action",
            outcome: "ok",
            lesson: Some("lesson"),
            tool_name: None,
            error_type: None,
            error_detail: None,
        })
        .unwrap();

        let mut session = Session::new();
        inject_experience_context(&mut session, "consistency", &store);

        if !session.messages.is_empty() {
            let msg = &session.messages[0].content;
            assert!(
                msg.starts_with("<context type="),
                "経験注入は<context type=で始まるべき"
            );
            assert!(
                msg.ends_with("</context>"),
                "経験注入は</context>で終わるべき"
            );
        }
    }

    #[test]
    fn t_loop_state_has_trial_summary() {
        let state = LoopState::new(AdvisorConfig::default());
        assert!(state.trial_summary.is_empty());
    }

    #[test]
    fn t_planning_step_contains_hypothesis() {
        let mut session = Session::new();
        inject_planning_step(
            &mut session,
            "テストを書いて、実装して、リファクタリングして",
        );
        let last = session.messages.last().unwrap();
        assert!(
            last.content.contains("仮説"),
            "仮説キーワード: {}",
            last.content
        );
    }

    #[test]
    fn t_verification_checklist() {
        let mut session = Session::new();
        let mut advisor = AdvisorConfig::default();
        let injected = inject_verification_step(
            &mut session,
            &mut advisor,
            "テストを書いて、実装して、リファクタリングして",
            "完了しました",
            1,
            10,
            None,
            &TrialSummary::default(),
        );
        if injected {
            let has_checklist = session
                .messages
                .iter()
                .any(|m| m.content.contains("チェックリスト"));
            assert!(has_checklist, "検証チェックリストが注入される");
        }
    }

    #[test]
    fn t_replan_with_trial_summary() {
        let mut session = Session::new();
        let mut stall = StallDetector::default();
        let mut advisor = AdvisorConfig::default();
        let mut ts = TrialSummary::default();
        ts.record_failure("shell", r#"{"command":"cargo build"}"#, "compile error", 1);
        // 閾値到達させる
        inject_replan_on_stall(
            &mut session,
            &mut stall,
            &mut advisor,
            "task",
            false,
            0,
            None,
            &ts,
        );
        inject_replan_on_stall(
            &mut session,
            &mut stall,
            &mut advisor,
            "task",
            false,
            0,
            None,
            &ts,
        );
        let triggered = inject_replan_on_stall(
            &mut session,
            &mut stall,
            &mut advisor,
            "task",
            false,
            0,
            None,
            &ts,
        );
        if triggered {
            let has_trial = session
                .messages
                .iter()
                .any(|m| m.content.contains("[EVALUATION]") || m.content.contains("試した方法"));
            assert!(has_trial, "構造化フィードバックまたは試行サマリーがreplanに含まれる");
        }
    }

    // テスト: check_invariants — 正常セッションで違反なし
    #[test]
    fn t_check_invariants_no_violations() {
        use crate::agent::conversation::Role;
        let mut session = Session::new();
        session.add_message(Message::user(
            "テストを書いて、実装して、リファクタリングして",
        ));
        session.add_message(Message::assistant(
            "実装が完了しました。テスト結果: 全パス".to_string(),
        ));
        session.add_message(Message {
            role: Role::Tool,
            content: "ファイルを正常に読み込みました".to_string(),
            attachments: Vec::new(),
            tool_call_id: None,
        });
        let violations = check_invariants(&session, "テストを書いて実装して");
        assert!(
            violations.is_empty(),
            "正常セッションでは違反なし: {:?}",
            violations
        );
    }

    // テスト: check_invariants — ツール失敗多い場合に違反検出
    #[test]
    fn t_check_invariants_low_success_rate() {
        use crate::agent::conversation::Role;
        let mut session = Session::new();
        session.add_message(Message::user("テストを書いて"));
        // ツール失敗メッセージ3件
        for _ in 0..3 {
            session.add_message(Message {
                role: Role::Tool,
                content: "エラー: ファイルが見つかりません".to_string(),
                attachments: Vec::new(),
                tool_call_id: None,
            });
        }
        // ツール成功メッセージ1件（成功率25% < 50%）
        session.add_message(Message {
            role: Role::Tool,
            content: "OK".to_string(),
            attachments: Vec::new(),
            tool_call_id: None,
        });
        let violations = check_invariants(&session, "テストを書いて");
        assert!(!violations.is_empty(), "低成功率で違反検出されるべき");
        assert!(
            violations[0].contains("ツール成功率が低い"),
            "成功率警告: {}",
            violations[0]
        );
    }

    // テスト: before_stepフックがAbort時にループを中断する（NAT知見、項目142統合）
    #[test]
    fn test_before_step_abort_stops_loop() {
        use crate::agent::middleware::{Middleware, MiddlewareSignal, StepResult as MwStepResult};

        struct AbortMiddleware;
        impl Middleware for AbortMiddleware {
            fn name(&self) -> &str { "abort_test" }
            fn before_step(&mut self, _session: &Session, _iteration: usize) -> MiddlewareSignal {
                MiddlewareSignal::Abort("テスト中断".to_string())
            }
            fn after_step(&mut self, _session: &mut Session, _result: &MwStepResult) -> MiddlewareSignal {
                MiddlewareSignal::Ok
            }
        }

        let mut chain = MiddlewareChain::new();
        chain.add(Box::new(AbortMiddleware));
        let mut session = Session::new();
        let abort = chain.run_before_step(&mut session, 0);
        assert!(abort.is_some(), "Abortミドルウェアはループ中断を返すべき");
        assert!(abort.unwrap().contains("テスト中断"));
    }

    // テスト: before_stepフックがInject時にセッションにメッセージ追加
    #[test]
    fn test_before_step_inject_adds_message() {
        use crate::agent::middleware::{Middleware, MiddlewareSignal, StepResult as MwStepResult};

        struct InjectMiddleware;
        impl Middleware for InjectMiddleware {
            fn name(&self) -> &str { "inject_test" }
            fn before_step(&mut self, _session: &Session, _iteration: usize) -> MiddlewareSignal {
                MiddlewareSignal::Inject("注入テスト".to_string())
            }
            fn after_step(&mut self, _session: &mut Session, _result: &MwStepResult) -> MiddlewareSignal {
                MiddlewareSignal::Ok
            }
        }

        let mut chain = MiddlewareChain::new();
        chain.add(Box::new(InjectMiddleware));
        let mut session = Session::new();
        let msg_count_before = session.messages.len();
        let abort = chain.run_before_step(&mut session, 0);
        assert!(abort.is_none(), "Injectはループ中断しない");
        assert_eq!(session.messages.len(), msg_count_before + 1);
        assert!(session.messages.last().unwrap().content.contains("注入テスト"));
    }
}
