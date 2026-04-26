use anyhow::Result;

use crate::agent::agent_loop::{AgentConfig, run_agent_loop};
use crate::agent::task::{TaskManager, TaskState};
use crate::agent::validate::PathGuard;
use crate::cancel::CancellationToken;
use crate::memory::store::MemoryStore;
use crate::observability::audit::{AuditAction, AuditLog};
use crate::observability::logger::{LogLevel, log_event};
use crate::runtime::inference::LlmBackend;
use crate::tools::ToolRegistry;

/// サブエージェント実行の最大深度（2階層まで）
const MAX_DEPTH: usize = 2;

/// サブタスクの実行結果
#[derive(Debug, Clone)]
pub struct SubTaskResult {
    pub task_id: String,
    pub goal: String,
    pub answer: String,
    pub iterations_used: usize,
    pub success: bool,
}

/// サブエージェント委任の全体結果
#[derive(Debug, Clone)]
pub struct DelegationResult {
    pub results: Vec<SubTaskResult>,
    pub summary: String,
}

impl DelegationResult {
    /// 全サブタスクが成功したか
    pub fn all_succeeded(&self) -> bool {
        self.results.iter().all(|r| r.success)
    }

    /// 成功率
    pub fn success_rate(&self) -> f64 {
        if self.results.is_empty() {
            return 0.0;
        }
        let ok = self.results.iter().filter(|r| r.success).count();
        ok as f64 / self.results.len() as f64
    }
}

/// サブエージェント設定
#[derive(Debug, Clone)]
pub struct SubAgentConfig {
    /// 現在の深度（0=ルート）
    pub depth: usize,
    /// サブエージェントの最大反復数（親の半分）
    pub max_iterations: usize,
    /// 許可するツール名（Noneなら全ツール）
    pub allowed_tools: Option<Vec<String>>,
}

impl SubAgentConfig {
    /// デフォルト設定（深度0、親の設定から自動導出）
    pub fn from_parent(parent_config: &AgentConfig, depth: usize) -> Self {
        Self {
            depth,
            max_iterations: (parent_config.max_iterations / 2).max(3),
            allowed_tools: None,
        }
    }

    /// ツール制限付き設定
    pub fn with_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools = Some(tools);
        self
    }

    /// 深度チェック — MAX_DEPTH以上なら委任不可
    pub fn can_delegate(&self) -> bool {
        self.depth < MAX_DEPTH
    }
}

/// サブエージェントエグゼキュータ
///
/// 複雑タスクをサブタスクに分割し、各サブタスクを独立した
/// エージェントループで実行する。
///
/// # ADK Workflow primitive 対応（項目166: Phase D 評価結果）
///
/// Google ADK 2.0 の 3 種 Workflow primitive と本実装の対応:
///
/// - **`SequentialAgent`** ⇄ `execute_sequential()`: サブタスクを順次実行し、
///   前段の出力を後段のコンテキストへ伝搬。`check_independence` で日本語/英語の
///   依存マーカー（"前の"/"上記"/"previous"/"then "等 20 種）を検出した場合、
///   または in-memory store のためスレッド非可搬な場合、自動的にこちらを選択。
///
/// - **`ParallelAgent`** ⇄ `execute_parallel()`: サブタスク間に依存がなく
///   かつ file-backed store を使う場合に `std::thread::scope` で並列実行。
///   各スレッドは `MemoryStore::open()` で独立 Connection を確保。
///
/// - **`LoopAgent`** ⇄ サポートなし: 終了条件付きの繰り返しは
///   `run_agent_loop_with_session` 自体の `for iteration in 0..max_iterations`
///   で代替済み。プリミティブとしての trait 化は YAGNI 判定で見送り
///   （`.claude/plan/phase-d-evaluation.md`）。
///
/// `execute()` がディスパッチャとして上記 2 系統を自動選択する。
pub struct SubAgentExecutor<'a> {
    backend: &'a dyn LlmBackend,
    tools: &'a ToolRegistry,
    path_guard: &'a PathGuard,
    cancel: &'a CancellationToken,
    store: Option<&'a MemoryStore>,
    sub_config: SubAgentConfig,
}

impl<'a> SubAgentExecutor<'a> {
    pub fn new(
        backend: &'a dyn LlmBackend,
        tools: &'a ToolRegistry,
        path_guard: &'a PathGuard,
        cancel: &'a CancellationToken,
        store: Option<&'a MemoryStore>,
        sub_config: SubAgentConfig,
    ) -> Self {
        Self {
            backend,
            tools,
            path_guard,
            cancel,
            store,
            sub_config,
        }
    }

    /// サブタスク群を実行し、結果をまとめて返す。
    /// 独立性検出＋store condition で並列 or 順次を自動選択。
    pub fn execute(
        &self,
        parent_task_id: &str,
        subtask_goals: &[String],
    ) -> Result<DelegationResult> {
        if !self.sub_config.can_delegate() {
            return Ok(DelegationResult {
                results: vec![],
                summary: format!(
                    "委任深度上限({MAX_DEPTH})に達したため、サブタスクは実行されませんでした"
                ),
            });
        }

        if subtask_goals.is_empty() {
            return Ok(DelegationResult {
                results: vec![],
                summary: "サブタスクがありません".to_string(),
            });
        }

        log_event(
            LogLevel::Info,
            "subagent",
            &format!(
                "サブエージェント委任開始: {}件のサブタスク (深度{})",
                subtask_goals.len(),
                self.sub_config.depth
            ),
        );

        let independent = check_independence(subtask_goals);
        let store_clonable = self.store.map(|s| s.path().is_some()).unwrap_or(true);
        let should_parallelize = independent && store_clonable && subtask_goals.len() >= 2;

        let results = if should_parallelize {
            log_event(
                LogLevel::Info,
                "subagent",
                &format!("並列実行モード: {}件（独立性検出）", subtask_goals.len()),
            );
            self.execute_parallel(parent_task_id, subtask_goals)
        } else {
            log_event(
                LogLevel::Info,
                "subagent",
                &format!(
                    "順次実行モード: {}件 (独立={independent}, store_clonable={store_clonable})",
                    subtask_goals.len(),
                ),
            );
            self.execute_sequential(parent_task_id, subtask_goals)
        };

        let summary = self.build_summary(&results);

        log_event(
            LogLevel::Info,
            "subagent",
            &format!(
                "サブエージェント委任完了: {}/{}成功",
                results.iter().filter(|r| r.success).count(),
                results.len()
            ),
        );

        Ok(DelegationResult { results, summary })
    }

    /// 順次実行（従来動作、依存サブタスクや in-memory store で使用）
    fn execute_sequential(
        &self,
        parent_task_id: &str,
        subtask_goals: &[String],
    ) -> Vec<SubTaskResult> {
        let mut results = Vec::with_capacity(subtask_goals.len());
        for (i, goal) in subtask_goals.iter().enumerate() {
            if self.cancel.is_cancelled() {
                log_event(LogLevel::Warn, "subagent", "キャンセルにより残りのサブタスクをスキップ");
                break;
            }

            log_event(
                LogLevel::Info,
                "subagent",
                &format!("サブタスク {}/{}: {}", i + 1, subtask_goals.len(), goal),
            );

            let sub_agent_config = self.build_sub_config(goal);
            let result = execute_single_subtask(
                self.backend,
                self.tools,
                self.path_guard,
                self.cancel,
                self.store,
                parent_task_id,
                i,
                goal,
                &sub_agent_config,
            );
            results.push(result);
        }
        results
    }

    /// 並列実行（std::thread::scope + file-backed store のスレッド毎Connection）
    fn execute_parallel(
        &self,
        parent_task_id: &str,
        subtask_goals: &[String],
    ) -> Vec<SubTaskResult> {
        let backend = self.backend;
        let tools = self.tools;
        let path_guard = self.path_guard;
        let cancel = self.cancel;
        let store_path: Option<String> = self.store.and_then(|s| s.path().map(String::from));
        let parent_id = parent_task_id.to_string();
        let sub_configs: Vec<AgentConfig> = subtask_goals
            .iter()
            .map(|g| self.build_sub_config(g))
            .collect();
        let total = subtask_goals.len();

        std::thread::scope(|scope| {
            let handles: Vec<_> = subtask_goals
                .iter()
                .enumerate()
                .map(|(i, goal)| {
                    let store_path = store_path.clone();
                    let parent_id = parent_id.clone();
                    let goal = goal.clone();
                    let sub_agent_config = sub_configs[i].clone();
                    scope.spawn(move || {
                        if cancel.is_cancelled() {
                            return SubTaskResult {
                                task_id: format!("sub-{i}"),
                                goal,
                                answer: "キャンセル".to_string(),
                                iterations_used: 0,
                                success: false,
                            };
                        }
                        log_event(
                            LogLevel::Info,
                            "subagent",
                            &format!("並列サブタスク {}/{}: {}", i + 1, total, goal),
                        );
                        let local_store: Option<MemoryStore> = store_path
                            .as_ref()
                            .and_then(|p| MemoryStore::open(p).ok());
                        execute_single_subtask(
                            backend,
                            tools,
                            path_guard,
                            cancel,
                            local_store.as_ref(),
                            &parent_id,
                            i,
                            &goal,
                            &sub_agent_config,
                        )
                    })
                })
                .collect();

            handles
                .into_iter()
                .map(|h| {
                    h.join().unwrap_or_else(|_| SubTaskResult {
                        task_id: "panic".into(),
                        goal: "panic".into(),
                        answer: "スレッドがパニック".into(),
                        iterations_used: 0,
                        success: false,
                    })
                })
                .collect()
        })
    }

    /// サブエージェント用のAgentConfigを構築
    fn build_sub_config(&self, goal: &str) -> AgentConfig {
        AgentConfig {
            max_iterations: self.sub_config.max_iterations,
            auto_checkpoint: false,
            system_prompt: format!(
                "あなたはサブエージェントです。以下のタスクを完了してください。\n\
                 簡潔に作業し、完了したら結果を報告してください。\n\n\
                 タスク: {goal}"
            ),
            ..Default::default()
        }
    }

    /// 結果のサマリーを生成
    fn build_summary(&self, results: &[SubTaskResult]) -> String {
        let total = results.len();
        let succeeded = results.iter().filter(|r| r.success).count();

        let mut summary = format!("## サブタスク実行結果 ({succeeded}/{total}成功)\n\n");

        for (i, r) in results.iter().enumerate() {
            let status = if r.success { "OK" } else { "NG" };
            summary.push_str(&format!(
                "{}. [{}] {} ({}ステップ)\n",
                i + 1,
                status,
                r.goal,
                r.iterations_used
            ));
            let preview: String = r.answer.chars().take(100).collect();
            summary.push_str(&format!("   -> {preview}\n\n"));
        }

        summary
    }
}

/// サブタスク結果をセッションメッセージとしてフォーマット
pub fn format_delegation_for_context(result: &DelegationResult) -> String {
    format!(
        "<context type=\"subtask-results\">\n{}\n</context>",
        result.summary
    )
}

/// サブタスク群が相互に独立かをヒューリスティックで判定。
/// 依存マーカー（"前の"/"上記"/"次に"/"previous"など）を含むgoalが
/// 1つでもあれば非独立と判定する。保守的に倒すため、誤判定時は順次実行にフォールバック。
pub fn check_independence(goals: &[String]) -> bool {
    if goals.len() < 2 {
        return false;
    }
    // 日本語/英語の依存関係マーカー
    const DEP_MARKERS: &[&str] = &[
        "前の",
        "先の",
        "上記",
        "下記",
        "さっき",
        "その後",
        "次に",
        "続いて",
        "それから",
        "最初に",
        "最後に",
        "ステップ",
        "previous",
        "above",
        "below",
        "then ",
        "after ",
        "subsequently",
        "finally",
        "first,",
        "last,",
    ];
    !goals
        .iter()
        .any(|g| DEP_MARKERS.iter().any(|m| g.contains(m)))
}

/// 単一サブタスクを実行するコアロジック（順次/並列共用）。
/// エラー境界でラップし、TaskManager/AuditLog更新も担う。
#[allow(clippy::too_many_arguments)]
pub fn execute_single_subtask(
    backend: &dyn LlmBackend,
    tools: &ToolRegistry,
    path_guard: &PathGuard,
    cancel: &CancellationToken,
    store: Option<&MemoryStore>,
    parent_task_id: &str,
    index: usize,
    goal: &str,
    sub_agent_config: &AgentConfig,
) -> SubTaskResult {
    // TaskManagerでサブタスク登録
    let task_id = if let Some(s) = store {
        let mgr = TaskManager::new(s.conn());
        match mgr.create(goal, Some(parent_task_id)) {
            Ok(id) => {
                let _ = mgr.update_state(&id, TaskState::InProgress);
                Some(id)
            }
            Err(e) => {
                log_event(
                    LogLevel::Warn,
                    "subagent",
                    &format!("タスク登録失敗（続行）: {e}"),
                );
                None
            }
        }
    } else {
        None
    };

    let task_id_str = task_id.clone().unwrap_or_else(|| format!("sub-{index}"));

    match run_agent_loop(
        goal,
        backend,
        tools,
        path_guard,
        sub_agent_config,
        cancel,
        store,
    ) {
        Ok(loop_result) => {
            if let (Some(s), Some(tid)) = (store, &task_id) {
                let mgr = TaskManager::new(s.conn());
                let _ = mgr.update_state(tid, TaskState::Completed);
                let _ = mgr.add_step(tid, "完了", &loop_result.answer);
            }

            if let Some(s) = store {
                let _ = AuditLog::new(s.conn()).log(
                    None,
                    &AuditAction::TaskComplete {
                        task_summary: goal.chars().take(100).collect::<String>(),
                        total_steps: loop_result.iterations_used,
                        tool_success_rate: 1.0,
                        duration_ms: 0,
                    },
                );
            }

            SubTaskResult {
                task_id: task_id_str,
                goal: goal.to_string(),
                answer: loop_result.answer,
                iterations_used: loop_result.iterations_used,
                success: true,
            }
        }
        Err(e) => {
            log_event(
                LogLevel::Warn,
                "subagent",
                &format!("サブタスク失敗（続行）: {e}"),
            );

            if let (Some(s), Some(tid)) = (store, &task_id) {
                let mgr = TaskManager::new(s.conn());
                let _ = mgr.set_error(tid, &e.to_string());
            }

            SubTaskResult {
                task_id: task_id_str,
                goal: goal.to_string(),
                answer: format!("エラー: {e}"),
                iterations_used: 0,
                success: false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancel::CancellationToken;
    use crate::runtime::inference::MockLlmBackend;
    use crate::tools::ToolRegistry;

    fn test_store() -> MemoryStore {
        MemoryStore::in_memory().unwrap()
    }

    fn test_path_guard() -> PathGuard {
        PathGuard::new(vec![std::env::temp_dir().to_string_lossy().to_string()])
    }

    #[test]
    fn test_sub_agent_config_from_parent() {
        let parent = AgentConfig::default();
        let sub = SubAgentConfig::from_parent(&parent, 0);
        assert_eq!(sub.depth, 0);
        assert_eq!(sub.max_iterations, 5);
        assert!(sub.can_delegate());
    }

    #[test]
    fn test_sub_agent_config_depth_limit() {
        let parent = AgentConfig::default();
        let sub = SubAgentConfig::from_parent(&parent, MAX_DEPTH);
        assert!(!sub.can_delegate());
    }

    #[test]
    fn test_sub_agent_config_with_tools() {
        let parent = AgentConfig::default();
        let sub = SubAgentConfig::from_parent(&parent, 0)
            .with_tools(vec!["shell".to_string(), "file_read".to_string()]);
        assert_eq!(sub.allowed_tools.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_delegation_result_all_succeeded() {
        let result = DelegationResult {
            results: vec![
                SubTaskResult {
                    task_id: "1".into(),
                    goal: "a".into(),
                    answer: "done".into(),
                    iterations_used: 2,
                    success: true,
                },
                SubTaskResult {
                    task_id: "2".into(),
                    goal: "b".into(),
                    answer: "done".into(),
                    iterations_used: 3,
                    success: true,
                },
            ],
            summary: "test".into(),
        };
        assert!(result.all_succeeded());
        assert!((result.success_rate() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_delegation_result_partial_failure() {
        let result = DelegationResult {
            results: vec![
                SubTaskResult {
                    task_id: "1".into(),
                    goal: "a".into(),
                    answer: "done".into(),
                    iterations_used: 2,
                    success: true,
                },
                SubTaskResult {
                    task_id: "2".into(),
                    goal: "b".into(),
                    answer: "error".into(),
                    iterations_used: 0,
                    success: false,
                },
            ],
            summary: "test".into(),
        };
        assert!(!result.all_succeeded());
        assert!((result.success_rate() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_delegation_result_empty() {
        let result = DelegationResult {
            results: vec![],
            summary: "empty".into(),
        };
        assert!(result.all_succeeded());
        assert!((result.success_rate() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_depth_limit_blocks_delegation() {
        let store = test_store();
        let backend = MockLlmBackend::new(vec![]);
        let tools = ToolRegistry::default();
        let path_guard = test_path_guard();
        let cancel = CancellationToken::new();

        let sub_config = SubAgentConfig {
            depth: MAX_DEPTH,
            max_iterations: 5,
            allowed_tools: None,
        };

        let executor = SubAgentExecutor::new(
            &backend,
            &tools,
            &path_guard,
            &cancel,
            Some(&store),
            sub_config,
        );

        let result = executor
            .execute("parent-1", &["task1".to_string()])
            .unwrap();
        assert!(result.results.is_empty());
        assert!(result.summary.contains("深度上限"));
    }

    #[test]
    fn test_empty_subtasks() {
        let store = test_store();
        let backend = MockLlmBackend::new(vec![]);
        let tools = ToolRegistry::default();
        let path_guard = test_path_guard();
        let cancel = CancellationToken::new();

        let sub_config = SubAgentConfig::from_parent(&AgentConfig::default(), 0);

        let executor = SubAgentExecutor::new(
            &backend,
            &tools,
            &path_guard,
            &cancel,
            Some(&store),
            sub_config,
        );

        let result = executor.execute("parent-1", &[]).unwrap();
        assert!(result.results.is_empty());
    }

    #[test]
    fn test_cancellation_stops_subtasks() {
        let store = test_store();
        let backend = MockLlmBackend::new(vec![
            "サブタスク1完了".to_string(),
        ]);
        let tools = ToolRegistry::default();
        let path_guard = test_path_guard();
        let cancel = CancellationToken::new();
        cancel.cancel();

        let sub_config = SubAgentConfig::from_parent(&AgentConfig::default(), 0);

        let executor = SubAgentExecutor::new(
            &backend,
            &tools,
            &path_guard,
            &cancel,
            Some(&store),
            sub_config,
        );

        let result = executor
            .execute(
                "parent-1",
                &["task1".to_string(), "task2".to_string()],
            )
            .unwrap();
        assert!(result.results.is_empty());
    }

    #[test]
    fn test_format_delegation_for_context() {
        let result = DelegationResult {
            results: vec![SubTaskResult {
                task_id: "1".into(),
                goal: "テスト実行".into(),
                answer: "テスト通過".into(),
                iterations_used: 2,
                success: true,
            }],
            summary: "1/1成功".into(),
        };
        let ctx = format_delegation_for_context(&result);
        assert!(ctx.contains("<context type=\"subtask-results\">"));
        assert!(ctx.contains("1/1成功"));
    }

    #[test]
    fn test_build_summary_format() {
        let store = test_store();
        let backend = MockLlmBackend::new(vec![]);
        let tools = ToolRegistry::default();
        let path_guard = test_path_guard();
        let cancel = CancellationToken::new();
        let sub_config = SubAgentConfig::from_parent(&AgentConfig::default(), 0);

        let executor = SubAgentExecutor::new(
            &backend,
            &tools,
            &path_guard,
            &cancel,
            Some(&store),
            sub_config,
        );

        let results = vec![
            SubTaskResult {
                task_id: "1".into(),
                goal: "ファイル読み取り".into(),
                answer: "内容を取得しました".into(),
                iterations_used: 1,
                success: true,
            },
            SubTaskResult {
                task_id: "2".into(),
                goal: "分析".into(),
                answer: "エラー: タイムアウト".into(),
                iterations_used: 0,
                success: false,
            },
        ];

        let summary = executor.build_summary(&results);
        assert!(summary.contains("1/2成功"));
        assert!(summary.contains("[OK]"));
        assert!(summary.contains("[NG]"));
    }

    #[test]
    fn test_sub_config_min_iterations() {
        let mut parent = AgentConfig::default();
        parent.max_iterations = 4;
        let sub = SubAgentConfig::from_parent(&parent, 0);
        assert_eq!(sub.max_iterations, 3);
    }

    #[test]
    fn test_check_independence_empty() {
        // 空配列および単一goalはfalse（並列化不要）
        assert!(!check_independence(&[]));
        assert!(!check_independence(&["単独タスク".to_string()]));
    }

    #[test]
    fn test_check_independence_truly_independent() {
        // 依存マーカーなし → 独立（true）
        let goals = vec![
            "README.mdを読む".to_string(),
            "Cargo.tomlの内容を確認".to_string(),
        ];
        assert!(check_independence(&goals));
    }

    #[test]
    fn test_check_independence_with_japanese_markers() {
        // 日本語依存マーカー検出 → false
        let cases = vec![
            vec!["タスクA".to_string(), "前の結果を使ってタスクB".to_string()],
            vec!["上記の内容を元にまとめる".to_string(), "別タスク".to_string()],
            vec!["最初にA".to_string(), "次にB".to_string()],
            vec!["ステップ1".to_string(), "ステップ2".to_string()],
            vec!["タスクα".to_string(), "その後にβ".to_string()],
        ];
        for goals in cases {
            assert!(
                !check_independence(&goals),
                "依存マーカー含有: {goals:?}"
            );
        }
    }

    #[test]
    fn test_check_independence_with_english_markers() {
        // 英語依存マーカー検出 → false
        let cases = vec![
            vec!["task A".to_string(), "use previous output".to_string()],
            vec!["do X".to_string(), "then apply Y".to_string()],
            vec!["first, compile".to_string(), "run tests".to_string()],
        ];
        for goals in cases {
            assert!(
                !check_independence(&goals),
                "dep marker: {goals:?}"
            );
        }
    }

    #[test]
    fn test_parallel_execution_with_file_backed_store() {
        // file-backed store + 独立な複数タスク → 並列パス通過
        // MockLlmBackendは内部キューを共有するためスレッド間で応答数は合計で足りる
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let db_path = tmp.path().to_str().unwrap();
        let store = MemoryStore::open(db_path).unwrap();
        assert!(store.path().is_some(), "file-backed store should expose path");

        // 並列タスク2つ、それぞれの応答を事前準備
        let backend = MockLlmBackend::new(vec![
            "タスクAの結果".to_string(),
            "タスクBの結果".to_string(),
        ]);
        let tools = ToolRegistry::default();
        let path_guard = test_path_guard();
        let cancel = CancellationToken::new();
        let sub_config = SubAgentConfig::from_parent(&AgentConfig::default(), 0);

        let executor = SubAgentExecutor::new(
            &backend,
            &tools,
            &path_guard,
            &cancel,
            Some(&store),
            sub_config,
        );

        let mgr = TaskManager::new(store.conn());
        let parent_id = mgr.create("親タスク", None).unwrap();

        // 独立なgoals（依存マーカーなし）で並列パス発火
        let result = executor
            .execute(
                &parent_id,
                &[
                    "README.mdを読む".to_string(),
                    "Cargo.tomlの内容を確認".to_string(),
                ],
            )
            .unwrap();

        assert_eq!(result.results.len(), 2);
        // 並列/順次どちらでも最終的に両サブタスクが登録される
        let subs = mgr.subtasks(&parent_id).unwrap();
        assert_eq!(subs.len(), 2);
    }

    #[test]
    fn test_sequential_fallback_for_in_memory_store() {
        // in-memory store (path=None) は順次実行にフォールバック
        let store = test_store();
        assert!(store.path().is_none());

        let backend = MockLlmBackend::new(vec![
            "A完了".to_string(),
            "B完了".to_string(),
        ]);
        let tools = ToolRegistry::default();
        let path_guard = test_path_guard();
        let cancel = CancellationToken::new();
        let sub_config = SubAgentConfig::from_parent(&AgentConfig::default(), 0);

        let executor = SubAgentExecutor::new(
            &backend,
            &tools,
            &path_guard,
            &cancel,
            Some(&store),
            sub_config,
        );
        let mgr = TaskManager::new(store.conn());
        let parent_id = mgr.create("親", None).unwrap();

        // 独立判定されても in-memory なら順次パス
        let result = executor
            .execute(
                &parent_id,
                &["独立タスクA".to_string(), "独立タスクB".to_string()],
            )
            .unwrap();
        assert_eq!(result.results.len(), 2);
    }

    #[test]
    fn test_successful_subtask_execution() {
        let store = test_store();
        let backend = MockLlmBackend::new(vec![
            "サブタスク1の回答です".to_string(),
            "サブタスク2の回答です".to_string(),
        ]);
        let tools = ToolRegistry::default();
        let path_guard = test_path_guard();
        let cancel = CancellationToken::new();
        let sub_config = SubAgentConfig::from_parent(&AgentConfig::default(), 0);

        let executor = SubAgentExecutor::new(
            &backend,
            &tools,
            &path_guard,
            &cancel,
            Some(&store),
            sub_config,
        );

        let mgr = TaskManager::new(store.conn());
        let parent_id = mgr.create("親タスク", None).unwrap();

        let result = executor
            .execute(
                &parent_id,
                &["サブタスク1".to_string(), "サブタスク2".to_string()],
            )
            .unwrap();

        assert_eq!(result.results.len(), 2);
        assert!(result.all_succeeded());

        let subs = mgr.subtasks(&parent_id).unwrap();
        assert_eq!(subs.len(), 2);
    }
}
