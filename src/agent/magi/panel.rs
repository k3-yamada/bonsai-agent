//! MAGI型三重合議監視パネル
//!
//! 単一チェッカーが持つ盲点・被ゲーム耐性の弱さを、独立した判定基準を持つ
//! 複数チェッカーの合議（多数決）で補い、明確な停止（Halt）権限を持つ。

use crate::memory::store::MemoryStore;

/// 判定対象の応答コンテキスト
#[derive(Clone)]
pub struct ResponseContext<'a> {
    pub task_description: &'a str,
    pub user_input: &'a str,
    pub proposed_response: &'a str,
    pub trajectory: &'a [String],
    pub store: Option<&'a MemoryStore>,
}

impl<'a> std::fmt::Debug for ResponseContext<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponseContext")
            .field("task_description", &self.task_description)
            .field("user_input", &self.user_input)
            .field("proposed_response", &self.proposed_response)
            .field("trajectory", &self.trajectory)
            .field("has_store", &self.store.is_some())
            .finish()
    }
}

impl<'a> ResponseContext<'a> {
    pub fn new(task_description: &'a str, proposed_response: &'a str) -> Self {
        Self {
            task_description,
            user_input: "",
            proposed_response,
            trajectory: &[],
            store: None,
        }
    }

    pub fn with_user_input(mut self, user_input: &'a str) -> Self {
        self.user_input = user_input;
        self
    }

    pub fn with_trajectory(mut self, trajectory: &'a [String]) -> Self {
        self.trajectory = trajectory;
        self
    }

    pub fn with_store(mut self, store: Option<&'a MemoryStore>) -> Self {
        self.store = store;
        self
    }
}

/// 各Judgeによる判定結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JudgeVerdict {
    /// 懸念なし
    Clear,
    /// 懸念あり（警告レベル）
    Concern(String),
    /// 停止要求（ブロックレベル）
    Block(String),
}

/// 合議パネルの最終決定
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionOutcome {
    /// 進行許可
    Proceed,
    /// 警告（1体以上のJudgeがConcern）
    Warn(Vec<JudgeVerdict>),
    /// 停止（1体以上のJudgeがBlock: C5合議契約統一）
    Halt(Vec<JudgeVerdict>),
}

/// 判定主体（Judge）トレイト
pub trait Judge: Send + Sync {
    /// Judgeの識別名
    fn name(&self) -> &'static str;

    /// 応答コンテキストを評価
    fn evaluate(&self, ctx: &ResponseContext) -> JudgeVerdict;
}

/// MAGI合議パネル
pub struct MagiPanel {
    judges: Vec<Box<dyn Judge>>,
}

impl MagiPanel {
    pub fn new(judges: Vec<Box<dyn Judge>>) -> Self {
        Self { judges }
    }

    /// 全Judgeの判定を集計し、最終決定と各Judgeの個別判定を返す。
    ///
    /// - 1体以上が Block ➔ Halt（安全停止: C5合議契約統一）
    /// - 1体以上が Concern ➔ Warn（警告: 懸念事項の検知）
    /// - それ以外（全員一致で Clear） ➔ Proceed（承認）
    pub fn decide_with_verdicts(
        &self,
        ctx: &ResponseContext,
    ) -> (DecisionOutcome, Vec<JudgeVerdict>) {
        if self.judges.is_empty() {
            return (DecisionOutcome::Proceed, Vec::new());
        }

        let verdicts: Vec<JudgeVerdict> = self.judges.iter().map(|j| j.evaluate(ctx)).collect();

        let blocks = verdicts
            .iter()
            .filter(|v| matches!(v, JudgeVerdict::Block(_)))
            .count();
        let concerns_or_blocks = verdicts
            .iter()
            .filter(|v| !matches!(v, JudgeVerdict::Clear))
            .count();

        let outcome = if blocks >= 1 {
            DecisionOutcome::Halt(verdicts.clone())
        } else if concerns_or_blocks >= 1 {
            DecisionOutcome::Warn(verdicts.clone())
        } else {
            DecisionOutcome::Proceed
        };

        (outcome, verdicts)
    }

    /// 全Judgeの判定を集計し、多数決で最終決定を下す。
    pub fn decide(&self, ctx: &ResponseContext) -> DecisionOutcome {
        self.decide_with_verdicts(ctx).0
    }

    /// デフォルトのMAGIパネルを生成
    pub fn default_panel() -> Self {
        use crate::agent::magi::judges::{ConsistencyJudge, GoodhartJudge, ValuesJudge};
        use crate::memory::consistency::MetricConsistencyChecker;
        use std::sync::{Arc, Mutex};

        let checker = Arc::new(Mutex::new(MetricConsistencyChecker::new(5)));
        Self::new(vec![
            Box::new(GoodhartJudge::new(checker)),
            Box::new(ValuesJudge::new()),
            Box::new(ConsistencyJudge::new()),
        ])
    }

    /// ツール呼び出し（特に破壊的ツール）の安全性を合議評価する。
    pub fn evaluate_tool_safety(&self, tool_name: &str, args_json: &str) -> DecisionOutcome {
        let proposed = format!("{tool_name}: {args_json}");
        let ctx = ResponseContext {
            task_description: "tool invocation safety check",
            user_input: "",
            proposed_response: &proposed,
            trajectory: &[],
            store: None,
        };
        self.decide(&ctx)
    }

    /// 最終回答（FinalAnswer）の品質・安全性を合議評価する（VALUES.md 照合および記憶整合性）。
    pub fn evaluate_final_answer(
        &self,
        proposed_answer: &str,
        task_description: &str,
    ) -> (DecisionOutcome, Vec<JudgeVerdict>) {
        self.evaluate_final_answer_with_store(proposed_answer, task_description, None)
    }

    /// 四層記憶ストアを参照して最終回答の事実整合性を合議評価する。
    pub fn evaluate_final_answer_with_store(
        &self,
        proposed_answer: &str,
        task_description: &str,
        store: Option<&MemoryStore>,
    ) -> (DecisionOutcome, Vec<JudgeVerdict>) {
        self.evaluate_final_answer_full(proposed_answer, task_description, &[], store)
    }

    /// 四層記憶ストアと実行軌跡を参照して最終回答の事実整合性を合議評価する。
    pub fn evaluate_final_answer_full(
        &self,
        proposed_answer: &str,
        task_description: &str,
        trajectory: &[String],
        store: Option<&MemoryStore>,
    ) -> (DecisionOutcome, Vec<JudgeVerdict>) {
        let ctx = ResponseContext::new(task_description, proposed_answer)
            .with_user_input(task_description)
            .with_trajectory(trajectory)
            .with_store(store);
        self.decide_with_verdicts(&ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::magi::judges::{ConsistencyJudge, ValuesJudge};

    #[test]
    fn test_evaluate_final_answer_detects_overconfidence() {
        let panel = MagiPanel::new(vec![Box::new(ValuesJudge::new())]);
        let (outcome, verdicts) = panel.evaluate_final_answer(
            "この操作は100%確実に成功することを保証します。",
            "システム更新",
        );
        // 1体のJudgeがConcernの場合、Warn outcomeとなり個別verdictにもConcernが含まれる
        assert!(matches!(outcome, DecisionOutcome::Warn(_)));
        assert!(
            verdicts
                .iter()
                .any(|v| matches!(v, JudgeVerdict::Concern(_)))
        );
    }

    #[test]
    fn test_evaluate_final_answer_halt_on_destructive_action() {
        let panel = MagiPanel::new(vec![Box::new(ValuesJudge::new())]);
        let (outcome, verdicts) = panel.evaluate_final_answer(
            "完了しました。念のため rm -rf / を実行してください。",
            "ディスク整理",
        );
        // Blockを返したためHaltになる
        assert!(matches!(outcome, DecisionOutcome::Halt(_)));
        assert!(verdicts.iter().any(|v| matches!(v, JudgeVerdict::Block(_))));
    }

    #[test]
    fn test_evaluate_final_answer_halt_on_single_block_among_multiple_judges() {
        let panel = MagiPanel::new(vec![
            Box::new(ValuesJudge::new()),
            Box::new(ConsistencyJudge::new()),
        ]);
        let (outcome, verdicts) = panel.evaluate_final_answer("rm -rf / を実行します", "タスク");
        // 複数Judge中1体でもBlockがあればHalt（C5契約統一）
        assert!(matches!(outcome, DecisionOutcome::Halt(_)));
        assert!(verdicts.iter().any(|v| matches!(v, JudgeVerdict::Block(_))));
    }
}
