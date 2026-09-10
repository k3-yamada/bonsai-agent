use anyhow::Result;

use crate::agent::agent_loop::{AgentConfig, run_agent_loop};
use crate::agent::task::{TaskManager, TaskState};
use crate::agent::validate::PathGuard;
use crate::cancel::CancellationToken;
use crate::domain::llm::LlmBackend;
use crate::memory::store::MemoryStore;
use crate::observability::audit::{AuditAction, AuditLog};
use crate::observability::logger::{LogLevel, log_event};
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

/// サブエージェントの専門化ロール
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubAgentRole {
    /// 汎用サブエージェント（従来動作）
    #[default]
    General,
    /// 探索・調査特化（読み取りツール優先、コンテキスト消費最小化）
    Explorer,
    /// 実装・編集特化（変更・ファイル生成）
    Builder,
    /// 品質・検証特化（テスト・リント・構文検査）
    Verifier,
}

impl SubAgentRole {
    /// ロールに応じたデフォルトの許可ツール一覧
    ///
    /// ツール名は `src/tools/*.rs` の `TypedTool::NAME`（本番 `setup_tools()` で
    /// 実際に登録される名前）とのみ一致させること。H-1: 過去に Antigravity/Windsurf
    /// 系の語彙（`find_by_name` 等）が誤って混入したため、回帰防止テスト
    /// `test_default_tools_only_reference_registered_tools` で検証している。
    ///
    /// # ロール別権限方針（Issue #25）
    ///
    /// `Verifier` が `shell` を保持するのは意図的。ロールの本質が
    /// `cargo test` / `cargo clippy` の実行であり、`shell` を外すと役割が機能ゼロになる。
    /// `shell` の危険性は allowlist 層ではなく既存の多層防御
    /// （`DANGEROUS_PATTERNS` Block / `PathGuard` / 非 read-only ツールへの MAGI 合議
    ///  intercept / `Permission`・`DaemonPolicy`・`AutonomyLevel`）で抑える。
    /// read-only shell（コマンド allowlist）は現時点で存在せず、新規機能のため別 issue 扱い。
    pub fn default_tools(&self) -> Option<Vec<String>> {
        match self {
            SubAgentRole::General => None,
            SubAgentRole::Explorer => {
                Some(vec!["file_read".into(), "repo_map".into(), "recall".into()])
            }
            SubAgentRole::Builder => Some(vec![
                "file_read".into(),
                "file_write".into(),
                "multi_edit".into(),
            ]),
            SubAgentRole::Verifier => Some(vec!["file_read".into(), "shell".into()]),
        }
    }

    /// ロールに応じたシステムプロンプトの指示文
    pub fn prompt_instruction(&self) -> &'static str {
        match self {
            SubAgentRole::General => {
                "あなたはサブエージェントです。以下のタスクを完了してください。\n\
                 簡潔に作業し、完了したら結果を報告してください。"
            }
            SubAgentRole::Explorer => {
                "あなたは探索特化サブエージェントです。ファイルやコードの調査に専念してください。\n\
                 コードの修正は行わず、発見した情報と結論を3行以内で簡潔に報告してください。"
            }
            SubAgentRole::Builder => {
                "あなたは実装特化サブエージェントです。最小限の差分で的確に変更を行ってください。\n\
                 変更したファイルと理由を明確に報告してください。"
            }
            SubAgentRole::Verifier => {
                "あなたは検証特化サブエージェントです。テストや構文・リントの確認を行ってください。\n\
                 合否判定とエラー原因をピンポイントで報告してください。"
            }
        }
    }
}

/// サブエージェント設定
#[derive(Clone)]
pub struct SubAgentConfig {
    /// 現在の深度（0=ルート）
    pub depth: usize,
    /// サブエージェントの最大反復数（親の半分）
    pub max_iterations: usize,
    /// 許可するツール名（`None` なら全ツール、`Some(vec![])` なら全禁止）。
    /// Issue #25 以降、これは設定値ではなく実効的な強制ポリシーであり、
    /// `build_sub_config()` で `AgentConfig.allowed_tools` へ伝播し、提示フィルタ
    /// (`src/tools/mod.rs` の `ToolRegistry::select_relevant_split_semantic` /
    /// `select_relevant_split`、top-k切り詰め前) と、`agent_loop::step::execute_step`
    /// の dispatch ガードの2箇所で強制される。
    pub allowed_tools: Option<Vec<String>>,
    /// 親エージェントから引き継ぐ LLM context 予算 (項目 187 F2 ContextOverflowGuard)
    /// `None` なら legacy compaction 動作。サブエージェントが同じ backend を使う場合の
    /// silent fallback (max_context_tokens=14000) を防ぐため親から伝播必須。
    pub n_ctx_budget: Option<u32>,
    /// サブエージェントの専門化ロール
    pub role: SubAgentRole,
    /// ツール実行前の確認コールバック（Issue #22 B3-3）。親から `Arc` clone で継承する。
    /// `Some` の間は並列実行を選ばない（[`should_parallelize`] 参照、stdin 競合防止）。
    pub confirm_callback: Option<crate::agent::agent_loop::ConfirmCallback>,
    /// 自律レベル（Issue #22 B3-3）。子は親と同値を継承し、より広い値を設定する
    /// API は用意しない（単調性維持）。
    pub autonomy: crate::safety::autonomy::AutonomyLevel,
    /// デーモンモード（バックグラウンド無人実行）フラグ（Issue #22 B3-3）。
    pub is_daemon: bool,
    /// デーモン時のツール実行ポリシー（Issue #22 B3-3）。
    pub daemon_policy: crate::tools::permission::DaemonPolicy,
    /// 親から引き継ぐタスク単位のウォールクロックタイムアウト（Issue #22 B3-3）。
    /// 子の無制限実行を防ぐ。
    pub task_timeout: Option<std::time::Duration>,
    /// `with_tools()` により `allowed_tools` が明示指定されたかどうかの内部フラグ。
    /// `from_parent()` 由来（継承）の値と区別するために用いる。`true` の間は
    /// `with_role()` が積集合を取らず明示指定を無条件で優先する（Issue #25
    /// qa-reviewer 合意: 明示指定は単調性の保証対象外）。
    tools_explicit: bool,
}

impl std::fmt::Debug for SubAgentConfig {
    /// `confirm_callback` は `Arc<dyn Fn...>` のため自動 derive 不可（Issue #22 B3-3）。
    /// 中身は表示せず `.is_some()` のみ出す。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubAgentConfig")
            .field("depth", &self.depth)
            .field("max_iterations", &self.max_iterations)
            .field("allowed_tools", &self.allowed_tools)
            .field("n_ctx_budget", &self.n_ctx_budget)
            .field("role", &self.role)
            .field("confirm_callback", &self.confirm_callback.is_some())
            .field("autonomy", &self.autonomy)
            .field("is_daemon", &self.is_daemon)
            .field("daemon_policy", &self.daemon_policy)
            .field("task_timeout", &self.task_timeout)
            .field("tools_explicit", &self.tools_explicit)
            .finish()
    }
}

impl SubAgentConfig {
    /// デフォルト設定（深度0、親の設定から自動導出）
    ///
    /// # 権限の単調性（Issue #25 qa-reviewer MEDIUM-1）
    ///
    /// `allowed_tools` は親の allowlist をそのまま継承する。子が親より広い
    /// 権限を持つことは許されないため（monotonicity）、親が `None`（全許可）
    /// なら子も `None`、親が `Some(...)` で制限されていれば子も少なくとも
    /// 同等に制限される。
    ///
    /// この単調性の保証は `from_parent()` の継承についてのみ成立する。
    /// 後続で呼べるビルダーメソッドの扱いは以下のように異なる:
    /// - `.with_role(role)`: 親の allowlist とロール既定値の**積集合**を取るため、
    ///   単調性を維持する（親の制限をロールで上書き・拡大することはない）。
    /// - `.with_tools(tools)`: 明示指定として無条件に上書きする。単調性の
    ///   保証対象**外**。呼び出し元は同一プロセス内の信頼されたコードのみであり、
    ///   LLM出力が到達する経路は存在しないため、明示指定を無条件に優先する
    ///   設計としている（`with_tools()` 呼び出し後は `with_role()` を呼んでも
    ///   明示指定が保持される）。
    ///
    /// # 継承するフィールド（Issue #22 B3-3、明示列挙。増減時は本表と
    /// `build_sub_config` を同時更新すること）
    /// - `allowed_tools`=する(単調性) / `n_ctx_budget`=する / `confirm_callback`=する
    ///   (`Arc` clone、並列時は [`should_parallelize`] が抑止)
    /// - `autonomy`=する(子は親と同値、より広い値を設定するAPIは用意しない)
    /// - `is_daemon`/`daemon_policy`=する
    /// - `task_timeout`=する(子の無制限実行を防ぐ)
    /// - `max_iterations`=半減(親/2、最低3、従来どおり)
    /// - `auto_checkpoint`=しない(子は常にfalse) / `system_prompt`=しない(ロール指示+goalで置換)
    /// - `advisor`/`base_inference`/`soul_path`/`memory_blocks`/`max_tool_output_chars`
    ///   =しない(非ゴール、必要なら別Issue)
    pub fn from_parent(parent_config: &AgentConfig, depth: usize) -> Self {
        Self {
            depth,
            max_iterations: (parent_config.max_iterations / 2).max(3),
            allowed_tools: parent_config.allowed_tools.clone(),
            n_ctx_budget: parent_config.n_ctx_budget,
            role: SubAgentRole::General,
            confirm_callback: parent_config.confirm_callback.clone(),
            autonomy: parent_config.autonomy,
            is_daemon: parent_config.is_daemon,
            daemon_policy: parent_config.daemon_policy,
            task_timeout: parent_config.task_timeout,
            tools_explicit: false,
        }
    }

    /// ツール制限付き設定（明示指定、無条件上書き）
    pub fn with_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools = Some(tools);
        self.tools_explicit = true;
        self
    }

    /// ロール指定設定（ロールに応じたデフォルトツールも自動適用）
    ///
    /// # 積集合セマンティクス（Issue #25 qa-reviewer MEDIUM-1 再指摘）
    ///
    /// `with_tools()` による明示指定が既にある場合（`tools_explicit == true`）は
    /// 一切変更しない（明示指定を無条件で優先。上記 `from_parent()` doc 参照）。
    ///
    /// それ以外（`allowed_tools` が `from_parent()` 由来の継承値、または未設定）の
    /// 場合、親の allowlist（`self.allowed_tools`）とロール既定値
    /// （`role.default_tools()`）が両方 `Some` であれば両者の**積集合（交差）**
    /// を採用する。親が Builder（`file_write`/`multi_edit` 等含む）でロールが
    /// Explorer のような、親より狭いロールを指定した場合でも、ロール既定に
    /// 含まれないツールは確実に落ちる。逆に親が Explorer でロールが
    /// Verifier（`shell` 要求）の場合、親にない `shell` は積集合に含まれず、
    /// 「親を超えない」単調性は保たれる（ロールが機能しなくなる場合がある点は
    /// 呼び出し元が親の allowlist 設計時に考慮すべき責務）。
    ///
    /// ロール既定が `None`（`SubAgentRole::General`）の場合は親の allowlist を
    /// そのまま維持する。親が `None`（無制限）の場合はロール既定値をそのまま
    /// 適用する（従来どおり）。
    pub fn with_role(mut self, role: SubAgentRole) -> Self {
        self.role = role;
        if self.tools_explicit {
            return self;
        }
        self.allowed_tools = match (self.allowed_tools.take(), role.default_tools()) {
            (Some(parent), Some(role_default)) => Some(
                parent
                    .into_iter()
                    .filter(|t| role_default.contains(t))
                    .collect(),
            ),
            (None, Some(role_default)) => Some(role_default),
            (parent, None) => parent,
        };
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

        // Issue #25 Should: allowlist に live registry 未登録の名前が混ざっていたら1回だけ警告。
        if let Some(allowed) = &self.sub_config.allowed_tools {
            let unknown: Vec<&str> = allowed
                .iter()
                .filter(|n| !self.tools.has(n))
                .map(|s| s.as_str())
                .collect();
            if !unknown.is_empty() {
                log_event(
                    LogLevel::Warn,
                    "subagent",
                    &format!("allowed_tools に未登録のツール名: {}", unknown.join(", ")),
                );
            }
        }

        let independent = check_independence(subtask_goals);
        let store_clonable = self.store.map(|s| s.path().is_some()).unwrap_or(true);
        let has_confirm = self.sub_config.confirm_callback.is_some();
        let do_parallelize = should_parallelize(
            independent,
            store_clonable,
            subtask_goals.len(),
            has_confirm,
        );

        let results = if do_parallelize {
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
                    "順次実行モード: {}件 (独立={independent}, store_clonable={store_clonable}, confirm_cb={has_confirm})",
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
                log_event(
                    LogLevel::Warn,
                    "subagent",
                    "キャンセルにより残りのサブタスクをスキップ",
                );
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
                        let local_store: Option<MemoryStore> =
                            store_path.as_ref().and_then(|p| MemoryStore::open(p).ok());
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
        let instruction = self.sub_config.role.prompt_instruction();
        AgentConfig {
            max_iterations: self.sub_config.max_iterations,
            auto_checkpoint: false,
            system_prompt: format!("{instruction}\n\nタスク: {goal}"),
            // 項目 187 F2: 親から引き継いだ n_ctx_budget をサブエージェント実行にも適用
            n_ctx_budget: self.sub_config.n_ctx_budget,
            // Issue #25: ロール/明示指定の allowlist を AgentConfig へ伝播。
            // これが唯一の伝播経路であり、ここを落とすと enforcement が丸ごと無効になる。
            allowed_tools: self.sub_config.allowed_tools.clone(),
            // Issue #22 B3-3: 安全コンテキストの伝播。ここを落とすと子は常に
            // confirm_callback=None かつ autonomy=Supervised に戻り、
            // Permission::Confirm なツール（shell 含む）が無条件拒否される。
            confirm_callback: self.sub_config.confirm_callback.clone(),
            autonomy: self.sub_config.autonomy,
            is_daemon: self.sub_config.is_daemon,
            daemon_policy: self.sub_config.daemon_policy,
            task_timeout: self.sub_config.task_timeout,
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

/// 並列サブエージェント実行の可否（純関数、Issue #22 AC-3-5）。
/// `has_confirm_callback == true` のときは必ず `false` を返す
/// （`tool_exec::execute_read_batch_parallel` と同一理由: 確認プロンプトの
/// stdin 競合防止。複数スレッドが同時に確認コールバックを呼ぶと、どちらの
/// 確認がどちらのプロンプトへの応答か区別できなくなるため）。
pub(crate) fn should_parallelize(
    independent: bool,
    store_clonable: bool,
    goal_count: usize,
    has_confirm_callback: bool,
) -> bool {
    independent && store_clonable && goal_count >= 2 && !has_confirm_callback
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
    use crate::domain::llm::MockLlmBackend;
    use crate::tools::ToolRegistry;
    use crate::tools::arxiv::ArxivTool;
    use crate::tools::file::{FileReadTool, FileWriteTool, MultiEditTool};
    use crate::tools::git::GitTool;
    use crate::tools::memory::{RecallTool, RememberTool};
    use crate::tools::repomap::RepoMapTool;
    use crate::tools::shell::ShellTool;
    use crate::tools::typed::TypedTool;
    use crate::tools::web::{WebFetchTool, WebSearchTool};

    /// 本番 `setup_tools`（`src/main.rs`）で実際にレジストリへ登録される
    /// ツール名の一覧。H-1 回帰防止: `SubAgentRole::default_tools()` が
    /// 実在しないツール名（Antigravity/Windsurf系語彙の混入等）を返さないことを
    /// 検証するための一次情報として使う。
    fn known_tool_names() -> Vec<&'static str> {
        vec![
            <ShellTool as TypedTool>::NAME,
            <FileReadTool as TypedTool>::NAME,
            <FileWriteTool as TypedTool>::NAME,
            <MultiEditTool as TypedTool>::NAME,
            <GitTool as TypedTool>::NAME,
            <WebSearchTool as TypedTool>::NAME,
            <WebFetchTool as TypedTool>::NAME,
            <ArxivTool as TypedTool>::NAME,
            <RepoMapTool as TypedTool>::NAME,
            <RememberTool as TypedTool>::NAME,
            <RecallTool as TypedTool>::NAME,
        ]
    }

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
            n_ctx_budget: None,
            role: SubAgentRole::General,
            confirm_callback: None,
            autonomy: crate::safety::autonomy::AutonomyLevel::default(),
            is_daemon: false,
            daemon_policy: crate::tools::permission::DaemonPolicy::AutoOnly,
            task_timeout: None,
            tools_explicit: false,
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
        let backend = MockLlmBackend::new(vec!["サブタスク1完了".to_string()]);
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
            .execute("parent-1", &["task1".to_string(), "task2".to_string()])
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
        let parent = AgentConfig {
            max_iterations: 4,
            ..Default::default()
        };
        let sub = SubAgentConfig::from_parent(&parent, 0);
        assert_eq!(sub.max_iterations, 3);
    }

    /// Codex audit MEDIUM fix: 親 AgentConfig の n_ctx_budget が
    /// SubAgentConfig::from_parent でサブエージェントに引き継がれることを検証。
    /// 引き継ぎ忘れると silent fallback (max_context_tokens=14000) で
    /// 同 backend 経由のサブエージェントが F2 保護を失う退行を防ぐ回帰テスト。
    #[test]
    fn test_sub_config_inherits_n_ctx_budget() {
        let parent = AgentConfig {
            n_ctx_budget: Some(8192),
            ..Default::default()
        };
        let sub = SubAgentConfig::from_parent(&parent, 0);
        assert_eq!(sub.n_ctx_budget, Some(8192));

        // None も明示的に伝播 (legacy 動作の opt-out 経路)
        let parent_none = AgentConfig {
            n_ctx_budget: None,
            ..Default::default()
        };
        let sub_none = SubAgentConfig::from_parent(&parent_none, 0);
        assert_eq!(sub_none.n_ctx_budget, None);
    }

    /// qa-reviewer MEDIUM-1 fix: `from_parent()` は親の `allowed_tools` を
    /// そのまま継承すること（権限の単調性）。親が制限されているのに
    /// 子が `None`（全許可）へリセットされる非対称を防ぐ回帰テスト。
    #[test]
    fn test_sub_config_inherits_allowed_tools_when_parent_restricted() {
        let parent = AgentConfig {
            allowed_tools: Some(vec!["file_read".to_string()]),
            ..Default::default()
        };
        let sub = SubAgentConfig::from_parent(&parent, 0);
        assert_eq!(sub.allowed_tools, Some(vec!["file_read".to_string()]));
    }

    /// qa-reviewer MEDIUM-1 fix: 親が無制限（`None`）のときは子も `None`
    /// のままであること（従来動作を維持）。
    #[test]
    fn test_sub_config_inherits_none_allowed_tools_when_parent_unrestricted() {
        let parent = AgentConfig {
            allowed_tools: None,
            ..Default::default()
        };
        let sub = SubAgentConfig::from_parent(&parent, 0);
        assert_eq!(sub.allowed_tools, None);
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
            vec![
                "上記の内容を元にまとめる".to_string(),
                "別タスク".to_string(),
            ],
            vec!["最初にA".to_string(), "次にB".to_string()],
            vec!["ステップ1".to_string(), "ステップ2".to_string()],
            vec!["タスクα".to_string(), "その後にβ".to_string()],
        ];
        for goals in cases {
            assert!(!check_independence(&goals), "依存マーカー含有: {goals:?}");
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
            assert!(!check_independence(&goals), "dep marker: {goals:?}");
        }
    }

    #[test]
    fn test_parallel_execution_with_file_backed_store() {
        // file-backed store + 独立な複数タスク → 並列パス通過
        // MockLlmBackendは内部キューを共有するためスレッド間で応答数は合計で足りる
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let db_path = tmp.path().to_str().unwrap();
        let store = MemoryStore::open(db_path).unwrap();
        assert!(
            store.path().is_some(),
            "file-backed store should expose path"
        );

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

        let backend = MockLlmBackend::new(vec!["A完了".to_string(), "B完了".to_string()]);
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

    /// H-1 回帰防止テスト: 全ロールの `default_tools()` が返す名前は、
    /// 本番 `setup_tools()` で実際に登録されるツール名の集合の部分集合であること。
    /// Antigravity/Windsurf系の実在しないツール名（`find_by_name` 等）の
    /// 再混入を検出する。
    #[test]
    fn test_default_tools_only_reference_registered_tools() {
        let known = known_tool_names();
        for role in [
            SubAgentRole::General,
            SubAgentRole::Explorer,
            SubAgentRole::Builder,
            SubAgentRole::Verifier,
        ] {
            if let Some(tools) = role.default_tools() {
                for tool_name in &tools {
                    assert!(
                        known.contains(&tool_name.as_str()),
                        "{role:?}.default_tools() に実在しないツール名 '{tool_name}' が含まれている"
                    );
                }
            }
        }
    }

    #[test]
    fn test_sub_agent_role_default() {
        let parent = AgentConfig::default();
        let sub = SubAgentConfig::from_parent(&parent, 0);
        assert_eq!(sub.role, SubAgentRole::General);
        assert!(sub.allowed_tools.is_none());
        assert_eq!(sub.role.default_tools(), None);
    }

    #[test]
    fn test_sub_agent_role_explorer() {
        let parent = AgentConfig::default();
        let sub = SubAgentConfig::from_parent(&parent, 0).with_role(SubAgentRole::Explorer);
        assert_eq!(sub.role, SubAgentRole::Explorer);
        let tools = sub.allowed_tools.as_ref().unwrap();
        assert!(tools.contains(&"file_read".to_string()));
        assert!(tools.contains(&"repo_map".to_string()));
        assert!(tools.contains(&"recall".to_string()));
        assert!(!tools.contains(&"file_write".to_string()));

        let prompt = sub.role.prompt_instruction();
        assert!(prompt.contains("探索特化サブエージェント"));
    }

    #[test]
    fn test_sub_agent_role_builder() {
        let parent = AgentConfig::default();
        let sub = SubAgentConfig::from_parent(&parent, 0).with_role(SubAgentRole::Builder);
        assert_eq!(sub.role, SubAgentRole::Builder);
        let tools = sub.allowed_tools.as_ref().unwrap();
        assert!(tools.contains(&"file_write".to_string()));
        assert!(tools.contains(&"multi_edit".to_string()));
        assert!(!tools.contains(&"shell".to_string()));

        let prompt = sub.role.prompt_instruction();
        assert!(prompt.contains("実装特化サブエージェント"));
    }

    #[test]
    fn test_sub_agent_role_verifier() {
        let parent = AgentConfig::default();
        let sub = SubAgentConfig::from_parent(&parent, 0).with_role(SubAgentRole::Verifier);
        assert_eq!(sub.role, SubAgentRole::Verifier);
        let tools = sub.allowed_tools.as_ref().unwrap();
        assert!(tools.contains(&"shell".to_string()));
        assert!(tools.contains(&"file_read".to_string()));
        assert!(!tools.contains(&"multi_edit".to_string()));

        let prompt = sub.role.prompt_instruction();
        assert!(prompt.contains("検証特化サブエージェント"));
    }

    #[test]
    fn test_sub_agent_role_custom_tools_override() {
        let parent = AgentConfig::default();
        let sub = SubAgentConfig::from_parent(&parent, 0)
            .with_tools(vec!["custom_tool".to_string()])
            .with_role(SubAgentRole::Explorer);
        assert_eq!(sub.role, SubAgentRole::Explorer);
        let tools = sub.allowed_tools.as_ref().unwrap();
        assert_eq!(tools, &vec!["custom_tool".to_string()]);
    }

    /// qa-reviewer Changes Requested (Issue #25 再指摘) MEDIUM-1: 親が Builder
    /// (`file_read`/`file_write`/`multi_edit`) で `.with_role(Explorer)` を
    /// 適用した場合、積集合セマンティクスにより `file_write` が結果から
    /// 落ちること（Explorer = 読み取り専用というロールの本質を親の allowlist
    /// が黙って無効化しない）。
    #[test]
    fn test_with_role_intersects_builder_parent_with_explorer_role() {
        let parent = AgentConfig {
            allowed_tools: Some(vec![
                "file_read".to_string(),
                "file_write".to_string(),
                "multi_edit".to_string(),
            ]),
            ..Default::default()
        };
        let sub = SubAgentConfig::from_parent(&parent, 0).with_role(SubAgentRole::Explorer);
        let tools = sub.allowed_tools.as_ref().unwrap();
        assert!(!tools.contains(&"file_write".to_string()));
        assert!(!tools.contains(&"multi_edit".to_string()));
        assert!(tools.contains(&"file_read".to_string()));
    }

    /// qa-reviewer Changes Requested (Issue #25 再指摘) MEDIUM-1: 親が
    /// Explorer (`file_read`/`repo_map`/`recall`) で `.with_role(Verifier)`
    /// を適用した場合、積集合により結果が `file_read` のみとなること
    /// （`shell` は親の allowlist に含まれないため交差で消える）。
    #[test]
    fn test_with_role_intersects_explorer_parent_with_verifier_role() {
        let parent = AgentConfig {
            allowed_tools: Some(vec![
                "file_read".to_string(),
                "repo_map".to_string(),
                "recall".to_string(),
            ]),
            ..Default::default()
        };
        let sub = SubAgentConfig::from_parent(&parent, 0).with_role(SubAgentRole::Verifier);
        let tools = sub.allowed_tools.as_ref().unwrap();
        assert_eq!(tools, &vec!["file_read".to_string()]);
    }

    #[test]
    fn test_build_sub_config_role_prompt_injection() {
        let store = test_store();
        let backend = MockLlmBackend::new(vec![]);
        let tools = ToolRegistry::default();
        let path_guard = test_path_guard();
        let cancel = CancellationToken::new();
        let sub_config = SubAgentConfig::from_parent(&AgentConfig::default(), 0)
            .with_role(SubAgentRole::Explorer);

        let executor = SubAgentExecutor::new(
            &backend,
            &tools,
            &path_guard,
            &cancel,
            Some(&store),
            sub_config,
        );

        let config = executor.build_sub_config("コード探索タスク");
        assert!(config.system_prompt.contains("探索特化サブエージェント"));
        assert!(config.system_prompt.contains("タスク: コード探索タスク"));
    }

    // ===== Issue #25: allowed_tools 伝播 (build_sub_config) =====

    #[test]
    fn test_build_sub_config_propagates_allowed_tools_for_role() {
        let store = test_store();
        let backend = MockLlmBackend::new(vec![]);
        let tools = ToolRegistry::default();
        let path_guard = test_path_guard();
        let cancel = CancellationToken::new();
        let sub_config = SubAgentConfig::from_parent(&AgentConfig::default(), 0)
            .with_role(SubAgentRole::Explorer);

        let executor = SubAgentExecutor::new(
            &backend,
            &tools,
            &path_guard,
            &cancel,
            Some(&store),
            sub_config,
        );

        let config = executor.build_sub_config("g");
        assert_eq!(
            config.allowed_tools,
            Some(vec![
                "file_read".to_string(),
                "repo_map".to_string(),
                "recall".to_string()
            ])
        );
    }

    #[test]
    fn test_build_sub_config_propagates_none_for_general_role() {
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

        let config = executor.build_sub_config("g");
        assert_eq!(config.allowed_tools, None);
    }

    #[test]
    fn test_build_sub_config_propagates_explicit_with_tools() {
        let store = test_store();
        let backend = MockLlmBackend::new(vec![]);
        let tools = ToolRegistry::default();
        let path_guard = test_path_guard();
        let cancel = CancellationToken::new();
        let sub_config = SubAgentConfig::from_parent(&AgentConfig::default(), 0)
            .with_tools(vec!["echo".to_string()]);

        let executor = SubAgentExecutor::new(
            &backend,
            &tools,
            &path_guard,
            &cancel,
            Some(&store),
            sub_config,
        );

        let config = executor.build_sub_config("g");
        assert_eq!(config.allowed_tools, Some(vec!["echo".to_string()]));
    }

    #[test]
    fn test_build_sub_config_propagates_verifier_shell() {
        let store = test_store();
        let backend = MockLlmBackend::new(vec![]);
        let tools = ToolRegistry::default();
        let path_guard = test_path_guard();
        let cancel = CancellationToken::new();
        let sub_config = SubAgentConfig::from_parent(&AgentConfig::default(), 0)
            .with_role(SubAgentRole::Verifier);

        let executor = SubAgentExecutor::new(
            &backend,
            &tools,
            &path_guard,
            &cancel,
            Some(&store),
            sub_config,
        );

        let config = executor.build_sub_config("g");
        assert!(
            config
                .allowed_tools
                .as_ref()
                .unwrap()
                .contains(&"shell".to_string())
        );
    }

    // ===== Issue #22 B3-3: サブエージェント境界の安全コンテキスト継承 =====

    /// AC-3-1: `from_parent` が5フィールド（confirm_callback/autonomy/is_daemon/
    /// daemon_policy/task_timeout）を親からそのまま複製すること。
    #[test]
    fn t_from_parent_inherits_safety_context() {
        let cb: crate::agent::agent_loop::ConfirmCallback = std::sync::Arc::new(|_, _| true);
        let parent = AgentConfig {
            confirm_callback: Some(cb),
            autonomy: crate::safety::autonomy::AutonomyLevel::Full,
            is_daemon: true,
            daemon_policy: crate::tools::permission::DaemonPolicy::QueueForHuman,
            task_timeout: Some(std::time::Duration::from_secs(60)),
            ..Default::default()
        };
        let sub = SubAgentConfig::from_parent(&parent, 0);
        assert!(sub.confirm_callback.is_some());
        assert_eq!(sub.autonomy, crate::safety::autonomy::AutonomyLevel::Full);
        assert!(sub.is_daemon);
        assert_eq!(
            sub.daemon_policy,
            crate::tools::permission::DaemonPolicy::QueueForHuman
        );
        assert_eq!(sub.task_timeout, Some(std::time::Duration::from_secs(60)));
    }

    /// AC-3-1: `build_sub_config` が5フィールドを子 `AgentConfig` へ伝播すること。
    #[test]
    fn t_build_sub_config_propagates_safety_context() {
        let store = test_store();
        let backend = MockLlmBackend::new(vec![]);
        let tools = ToolRegistry::default();
        let path_guard = test_path_guard();
        let cancel = CancellationToken::new();

        let cb: crate::agent::agent_loop::ConfirmCallback = std::sync::Arc::new(|_, _| true);
        let parent = AgentConfig {
            confirm_callback: Some(cb),
            autonomy: crate::safety::autonomy::AutonomyLevel::Full,
            is_daemon: true,
            daemon_policy: crate::tools::permission::DaemonPolicy::QueueForHuman,
            task_timeout: Some(std::time::Duration::from_secs(45)),
            ..Default::default()
        };
        let sub_config = SubAgentConfig::from_parent(&parent, 0);

        let executor = SubAgentExecutor::new(
            &backend,
            &tools,
            &path_guard,
            &cancel,
            Some(&store),
            sub_config,
        );
        let config = executor.build_sub_config("g");
        assert!(config.confirm_callback.is_some());
        assert_eq!(
            config.autonomy,
            crate::safety::autonomy::AutonomyLevel::Full
        );
        assert!(config.is_daemon);
        assert_eq!(
            config.daemon_policy,
            crate::tools::permission::DaemonPolicy::QueueForHuman
        );
        assert_eq!(
            config.task_timeout,
            Some(std::time::Duration::from_secs(45))
        );
    }

    /// AC-3-3: 子 `AgentConfig.confirm_callback` が実際に呼び出せること
    /// （呼出回数を `AtomicUsize` で検証）。
    #[test]
    fn t_build_sub_config_confirm_callback_is_invoked() {
        let store = test_store();
        let backend = MockLlmBackend::new(vec![]);
        let tools = ToolRegistry::default();
        let path_guard = test_path_guard();
        let cancel = CancellationToken::new();

        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls_clone = calls.clone();
        let cb: crate::agent::agent_loop::ConfirmCallback =
            std::sync::Arc::new(move |_name: &str, _args: &str| {
                calls_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                true
            });

        let parent = AgentConfig {
            confirm_callback: Some(cb),
            ..Default::default()
        };
        let sub_config = SubAgentConfig::from_parent(&parent, 0);

        let executor = SubAgentExecutor::new(
            &backend,
            &tools,
            &path_guard,
            &cancel,
            Some(&store),
            sub_config,
        );
        let config = executor.build_sub_config("g");
        let callback = config
            .confirm_callback
            .as_ref()
            .expect("callback must propagate");
        assert!(callback("shell", "{}"));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// AC-3-3: `false` を返す confirm_callback を持つ子で、Confirm 権限ツール
    /// (`shell`) の実行が「確認拒否」文言で拒否されること
    /// (`tool_exec::execute_single_call_with_policy` の本番経路を直接検証)。
    #[test]
    fn t_confirm_callback_denying_blocks_confirm_tool() {
        use crate::agent::tool_exec::{ValidatedCall, execute_single_call_with_policy};
        use crate::tools::Tool;
        use crate::tools::shell::ShellTool;

        let shell = ShellTool::new();
        let deny_cb: crate::agent::agent_loop::ConfirmCallback =
            std::sync::Arc::new(|_name: &str, _args: &str| false);

        let parent = AgentConfig {
            autonomy: crate::safety::autonomy::AutonomyLevel::Supervised,
            confirm_callback: Some(deny_cb),
            ..Default::default()
        };
        let sub = SubAgentConfig::from_parent(&parent, 0);
        assert!(sub.confirm_callback.is_some());

        let call = ValidatedCall {
            name: "shell".to_string(),
            args_json: r#"{"command":"echo hi"}"#.to_string(),
            coerced_args: serde_json::json!({"command": "echo hi"}),
            tool: &shell as &dyn Tool,
            is_read_only: false,
        };

        let cb = sub.confirm_callback.as_deref().unwrap();
        let result = execute_single_call_with_policy(
            &call,
            sub.is_daemon,
            sub.daemon_policy,
            sub.autonomy,
            Some(cb),
        );
        assert!(!result.success);
        assert!(result.output.contains("確認拒否"));
    }

    /// AC-3-4: 親が Supervised + confirm_callback あり + Verifier ロールの場合、
    /// `shell` が allowlist に残り、かつ「確認コールバック未設定」エラーに
    /// ならないこと（伝播忘れによる機能不全の回帰防止）。
    #[test]
    fn t_verifier_role_shell_not_blocked_by_missing_callback() {
        use crate::agent::tool_exec::{ValidatedCall, execute_single_call_with_policy};
        use crate::tools::Tool;
        use crate::tools::shell::ShellTool;

        let shell = ShellTool::new();
        let allow_cb: crate::agent::agent_loop::ConfirmCallback =
            std::sync::Arc::new(|_name: &str, _args: &str| true);
        let parent = AgentConfig {
            autonomy: crate::safety::autonomy::AutonomyLevel::Supervised,
            confirm_callback: Some(allow_cb),
            ..Default::default()
        };
        let sub = SubAgentConfig::from_parent(&parent, 0).with_role(SubAgentRole::Verifier);
        assert!(
            sub.allowed_tools
                .as_ref()
                .unwrap()
                .contains(&"shell".to_string())
        );
        assert!(sub.confirm_callback.is_some());

        let call = ValidatedCall {
            name: "shell".to_string(),
            args_json: r#"{"command":"echo hi"}"#.to_string(),
            coerced_args: serde_json::json!({"command": "echo hi"}),
            tool: &shell as &dyn Tool,
            is_read_only: false,
        };
        let cb = sub.confirm_callback.as_deref().unwrap();
        let result = execute_single_call_with_policy(
            &call,
            sub.is_daemon,
            sub.daemon_policy,
            sub.autonomy,
            Some(cb),
        );
        assert!(!result.output.contains("確認コールバック未設定"));
    }

    /// AC-3-5: `should_parallelize` 純関数の4分岐検証。
    /// `has_confirm_callback == true` のときは必ず `false`。
    #[test]
    fn t_should_parallelize_false_when_confirm_callback() {
        assert!(should_parallelize(true, true, 2, false));
        assert!(!should_parallelize(true, true, 2, true));
        assert!(!should_parallelize(false, true, 2, false));
        assert!(!should_parallelize(true, false, 2, false));
        assert!(!should_parallelize(true, true, 1, false));
    }

    /// AC-3-6: 親の `task_timeout` が子 `SubAgentConfig`/`AgentConfig` へ
    /// 伝播すること。
    #[test]
    fn t_task_timeout_propagates_to_sub_config() {
        let parent = AgentConfig {
            task_timeout: Some(std::time::Duration::from_secs(30)),
            ..Default::default()
        };
        let sub = SubAgentConfig::from_parent(&parent, 0);
        assert_eq!(sub.task_timeout, Some(std::time::Duration::from_secs(30)));

        let store = test_store();
        let backend = MockLlmBackend::new(vec![]);
        let tools = ToolRegistry::default();
        let path_guard = test_path_guard();
        let cancel = CancellationToken::new();
        let executor =
            SubAgentExecutor::new(&backend, &tools, &path_guard, &cancel, Some(&store), sub);
        let config = executor.build_sub_config("g");
        assert_eq!(
            config.task_timeout,
            Some(std::time::Duration::from_secs(30))
        );
    }
}
