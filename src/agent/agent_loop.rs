use anyhow::Result;

use crate::agent::conversation::{Message, ParsedOutput, Session};
use crate::agent::error_recovery::{
    decide_recovery, CircuitBreaker, FailureMode, LoopDetector, ParseErrorDetail,
    RecoveryAction,
};
use crate::agent::parse::parse_assistant_output;
use crate::agent::validate::{validate_tool_call, PathGuard, Severity};
use crate::cancel::CancellationToken;
use crate::memory::experience::{ExperienceStore, ExperienceType, RecordParams};
use crate::memory::search::HybridSearch;
use crate::memory::skill::SkillStore;
use crate::memory::store::MemoryStore;
use crate::observability::audit::{AuditAction, AuditLog};
use crate::runtime::embedder::create_embedder;
use crate::runtime::inference::LlmBackend;
use crate::safety::secrets::SecretsFilter;
use crate::tools::ToolRegistry;

/// エージェント設定
pub struct AgentConfig {
    pub max_iterations: usize,
    pub max_retries: usize,
    pub system_prompt: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_iterations: 10,
            max_retries: 3,
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
        }
    }
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
"#;

/// エージェントのステップ結果
#[derive(Debug)]
pub enum StepOutcome {
    /// 最終回答（ループ終了）
    FinalAnswer(String),
    /// ツール実行後、ループ継続
    Continue,
    /// エラーで中断
    Aborted(String),
}

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

    let selected_tools = ctx.tools.select_relevant(last_user_msg, 5);
    let tool_schemas: Vec<_> = selected_tools.iter().map(|t| t.schema()).collect();

    // 2. LLM呼び出し（ストリーミング対応）
    let in_think = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let in_think_clone = in_think.clone();

    let result = ctx.backend.generate(
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
                    Ok(StepOutcome::Continue)
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

    // 5. 各ツール呼び出しを実行
    let assistant_text = result.text.clone();
    session.add_message(Message::assistant(&assistant_text));

    for tool_call in &parsed.tool_calls {
        // ループ検出
        let action_key = format!("{}:{}", tool_call.name, tool_call.arguments);
        if loop_detector.record_and_check(&action_key) {
            let mode = FailureMode::LoopDetected;
            let action = decide_recovery(&mode, attempt, ctx.config.max_retries);
            if let RecoveryAction::Abort(msg) = action {
                return Ok(StepOutcome::Aborted(msg));
            }
        }

        // サーキットブレーカーチェック
        if !circuit_breaker.is_available(&tool_call.name) {
            session.add_message(Message::tool(
                format!("ツール '{}' は連続失敗のため一時停止中です。別の方法を試してください。", tool_call.name),
                &tool_call.name,
            ));
            continue;
        }

        // バリデーション
        let known = ctx.tools.known_names();
        let validation = validate_tool_call(tool_call, &known, ctx.path_guard, None);

        if !validation.is_valid {
            let block_issues: Vec<_> = validation
                .issues
                .iter()
                .filter(|i| i.severity == Severity::Block)
                .map(|i| i.message.as_str())
                .collect();
            session.add_message(Message::tool(
                format!("バリデーションエラー: {}", block_issues.join(", ")),
                &tool_call.name,
            ));
            continue;
        }

        // ツール実行
        let tool = match ctx.tools.get(&tool_call.name) {
            Some(t) => t,
            None => continue,
        };

        match tool.call(tool_call.arguments.clone()) {
            Ok(tool_result) => {
                circuit_breaker.record_success(&tool_call.name);
                // 秘密情報をマスク
                let redacted_output = ctx.secrets_filter.redact(&tool_result.output);
                // 監査ログ記録
                if let Some(s) = ctx.store {
                    let audit = AuditLog::new(s.conn());
                    let _ = audit.log(
                        Some(&session.id),
                        &AuditAction::ToolCall {
                            tool_name: tool_call.name.clone(),
                            args: serde_json::to_string(&tool_call.arguments).unwrap_or_default(),
                            success: tool_result.success,
                            output_preview: redacted_output.chars().take(200).collect(),
                        },
                    );
                }
                session.add_message(Message::tool(&redacted_output, &tool_call.name));
            }
            Err(e) => {
                circuit_breaker.record_failure(&tool_call.name);
                let error_msg = format!("ツール実行エラー: {e}");
                // 失敗も監査ログに記録
                if let Some(s) = ctx.store {
                    let audit = AuditLog::new(s.conn());
                    let _ = audit.log(
                        Some(&session.id),
                        &AuditAction::ToolCall {
                            tool_name: tool_call.name.clone(),
                            args: serde_json::to_string(&tool_call.arguments).unwrap_or_default(),
                            success: false,
                            output_preview: error_msg.clone(),
                        },
                    );
                }
                session.add_message(Message::tool(&error_msg, &tool_call.name));
            }
        }
    }

    Ok(StepOutcome::Continue)
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
) -> Result<String> {
    let mut session = Session::new();
    session.add_message(Message::system(&config.system_prompt));
    session.add_message(Message::user(input));

    run_agent_loop_with_session(&mut session, backend, tools, path_guard, config, cancel, store)
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
) -> Result<String> {
    // 経験記録用にユーザー入力を取得
    let task_context: String = session
        .messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, crate::agent::conversation::Role::User))
        .map(|m| m.content.clone())
        .unwrap_or_default();

    let secrets_filter = SecretsFilter::default();

    let vault_path = dirs::data_dir().unwrap_or_else(|| std::path::PathBuf::from(".")).join("bonsai-agent").join("vault");
    let vault = crate::knowledge::vault::Vault::new(&vault_path).ok();
    if let Some(ref v) = vault {
        let stocks = crate::knowledge::extractor::extract_stock(&task_context, &session.id);
        let _ = v.append_all(&stocks);
    }
    let embedder = create_embedder();

    // ハイブリッド検索: 関連メモリをシステムプロンプトに注入
    if let Some(s) = store {
        let search = HybridSearch::new(s, embedder.as_ref());
        let memories = search.search(&task_context, 3).unwrap_or_default();
        if !memories.is_empty() {
            let memory_context: String = memories
                .iter()
                .map(|r| format!("- {}", r.memory.content))
                .collect::<Vec<_>>()
                .join("\n");
            session.add_message(Message::system(format!(
                "関連する過去の記憶:\n{memory_context}"
            )));
        }

        // 類似経験を検索して注入
        let exp = ExperienceStore::new(s.conn());
        let past = exp.find_similar(&task_context, 3).unwrap_or_default();
        if !past.is_empty() {
            let exp_context: String = past
                .iter()
                .map(|e| {
                    let prefix = match e.exp_type {
                        ExperienceType::Success => "成功",
                        ExperienceType::Failure => "失敗（避けよ）",
                        ExperienceType::Insight => "学び",
                    };
                    format!("- [{prefix}] {}: {}", e.action, e.outcome)
                })
                .collect::<Vec<_>>()
                .join("\n");
            session.add_message(Message::system(format!(
                "過去の経験:\n{exp_context}"
            )));
        }
    }

    let mut circuit_breaker = CircuitBreaker::default();
    let mut loop_detector = LoopDetector::default();

    let ctx = StepContext {
        backend,
        tools,
        path_guard,
        config,
        cancel,
        secrets_filter: &secrets_filter,
        store,
    };

    for iteration in 0..config.max_iterations {
        let outcome = execute_step(
            session,
            &ctx,
            &mut circuit_breaker,
            &mut loop_detector,
            iteration,
        )?;

        match outcome {
            StepOutcome::FinalAnswer(answer) => {
                // セッション保存 + 経験記録（成功）
                if let Some(s) = store {
                    let _ = s.save_session(session);
                    let exp = ExperienceStore::new(s.conn());
                    let _ = exp.record(&RecordParams {
                        exp_type: ExperienceType::Success,
                        task_context: &task_context,
                        action: &answer,
                        outcome: "completed",
                        lesson: None,
                        tool_name: None,
                        error_type: None,
                        error_detail: None,
                    });
                    // スキル昇格チェック（3回成功で昇格）
                    let skills = SkillStore::new(s.conn());
                    let _ = skills.promote_from_experiences(s.conn(), 3);
                    let evo = crate::memory::evolution::EvolutionEngine::new(s);
                    let _ = evo.auto_collect();
                }
                return Ok(answer);
            }
            StepOutcome::Aborted(reason) => {
                // セッション保存 + 経験記録（失敗）
                if let Some(s) = store {
                    let _ = s.save_session(session);
                    let exp = ExperienceStore::new(s.conn());
                    let _ = exp.record(&RecordParams {
                        exp_type: ExperienceType::Insight,
                        task_context: &task_context,
                        action: "aborted",
                        outcome: &reason,
                        lesson: Some(&reason),
                        tool_name: None,
                        error_type: Some("Aborted"),
                        error_detail: None,
                    });
                }
                return Ok(format!("[中断] {reason}"));
            }
            StepOutcome::Continue => continue,
        }
    }

    let timeout_msg = format!(
        "最大イテレーション数({})に到達しました。タスクを完了できませんでした。",
        config.max_iterations
    );
    Ok(format!("[中断] {timeout_msg}"))
}

/// ParsedOutputから回答テキストを構築
fn build_answer(parsed: &ParsedOutput) -> String {
    let raw = parsed.text.clone().unwrap_or_else(|| "(回答なし)".to_string());
    clean_response(&raw)
}

fn clean_response(text: &str) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    lines.dedup();
    let joined = lines.join("\n");
    let len = joined.len();
    if len > 100 {
        let half = len / 2;
        let first = &joined[..half];
        let second = &joined[half..];
        let check = &first[..50.min(first.len())];
        if second.contains(check) { return first.trim_end().to_string(); }
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::MemoryStore;
    use crate::runtime::inference::MockLlmBackend;
    use crate::tools::permission::Permission;
    use crate::tools::{Tool, ToolResult};

    /// テスト用のエコーツール
    struct EchoTool;
    impl Tool for EchoTool {
        fn name(&self) -> &str { "echo" }
        fn description(&self) -> &str { "入力をそのまま返す" }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}})
        }
        fn permission(&self) -> Permission { Permission::Auto }
        fn call(&self, args: serde_json::Value) -> Result<ToolResult> {
            let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("(empty)");
            Ok(ToolResult { output: text.to_string(), success: true })
        }
    }

    /// テスト用の失敗ツール
    struct FailTool;
    impl Tool for FailTool {
        fn name(&self) -> &str { "fail" }
        fn description(&self) -> &str { "常に失敗する" }
        fn parameters_schema(&self) -> serde_json::Value { serde_json::json!({}) }
        fn permission(&self) -> Permission { Permission::Auto }
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

        let result = run_agent_loop("天気は？", &mock, &tools, &guard, &config, &cancel, None::<&MemoryStore>).unwrap();
        assert!(result.contains("晴れ"));
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

        let result = run_agent_loop("echo test", &mock, &tools, &guard, &config, &cancel, None::<&MemoryStore>).unwrap();
        assert!(result.contains("hello"));
    }

    // テスト3: 最大イテレーション到達
    #[test]
    fn test_max_iterations() {
        // 常にツール呼び出しを返すモック（終了しない）
        let responses: Vec<String> = (0..15)
            .map(|i| format!(r#"<tool_call>{{"name":"echo","arguments":{{"text":"iter{}"}}}}</tool_call>"#, i))
            .collect();
        let mock = MockLlmBackend::new(responses);
        let tools = test_registry();
        let guard = PathGuard::default_deny_list();
        let config = AgentConfig { max_iterations: 3, ..Default::default() };
        let cancel = CancellationToken::new();

        let result = run_agent_loop("loop", &mock, &tools, &guard, &config, &cancel, None::<&MemoryStore>).unwrap();
        assert!(result.contains("中断"));
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

        let result = run_agent_loop("test", &mock, &tools, &guard, &config, &cancel, None::<&MemoryStore>);
        // MockLlmBackend::generateがキャンセルエラーを返す
        assert!(result.is_err() || result.unwrap().contains("キャンセル"));
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

        let result = run_agent_loop("hack", &mock, &tools, &guard, &config, &cancel, None::<&MemoryStore>).unwrap();
        assert!(result.contains("了解"));
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

        let result = run_agent_loop("fail", &mock, &tools, &guard, &config, &cancel, None::<&MemoryStore>).unwrap();
        assert!(result.contains("エラー"));
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

        run_agent_loop("test query", &mock, &tools, &guard, &config, &cancel, Some(&store)).unwrap();

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
        let config = AgentConfig { max_iterations: 10, ..Default::default() };
        let cancel = CancellationToken::new();

        let result = run_agent_loop("loop", &mock, &tools, &guard, &config, &cancel, None::<&MemoryStore>).unwrap();
        assert!(result.contains("中断"));
    }
}
