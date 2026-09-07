//! MAGI型三重合議監視モジュール
//!
//! Judge A (Goodhart), Judge B (VALUES.md), Judge C (Consistency) の合議により、
//! エージェントの暴走や指標形骸化を防ぎ、必要に応じて停止（Halt）権限を行使する。

pub mod judges;
pub mod panel;

pub use judges::{ConsistencyJudge, GoodhartJudge, ValuesJudge};
pub use panel::{DecisionOutcome, Judge, JudgeVerdict, MagiPanel, ResponseContext};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::consistency::MetricConsistencyChecker;
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_magi_panel_all_clear() {
        let checker = Arc::new(Mutex::new(MetricConsistencyChecker::new(5)));
        let panel = MagiPanel::new(vec![
            Box::new(GoodhartJudge::new(checker)),
            Box::new(ValuesJudge::new()),
            Box::new(ConsistencyJudge::new()),
        ]);

        let trajectory = vec!["shell: ls".to_string()];
        let ctx = ResponseContext::new(
            "ファイル一覧の取得",
            "ファイル一覧を取得しました: Cargo.toml, src/",
        )
        .with_user_input("ls を実行して")
        .with_trajectory(&trajectory);

        assert_eq!(panel.decide(&ctx), DecisionOutcome::Proceed);
    }

    #[test]
    fn test_magi_panel_values_destructive_block() {
        let checker = Arc::new(Mutex::new(MetricConsistencyChecker::new(5)));
        let panel = MagiPanel::new(vec![
            Box::new(GoodhartJudge::new(checker)),
            Box::new(ValuesJudge::new()),
            Box::new(ConsistencyJudge::new()),
        ]);

        let ctx = ResponseContext::new("ディスクの初期化", "承知しました。rm -rf / を実行します。")
            .with_user_input("rm -rf /");

        // 1体がBlock、他がClear ➔ 単一BlockはWarn（合議の多数決安全閾値）
        match panel.decide(&ctx) {
            DecisionOutcome::Proceed => panic!("破壊的操作はProceedしてはならない"),
            DecisionOutcome::Warn(_) => {} // 1体ブロックは警告
            DecisionOutcome::Halt(_) => {} // 全員一致ならHalt
        }
    }

    #[test]
    fn test_magi_panel_unanimous_halt() {
        // 全JudgeがBlockを返すモック構成
        struct AlwaysBlockJudge;
        impl Judge for AlwaysBlockJudge {
            fn name(&self) -> &'static str {
                "AlwaysBlock"
            }
            fn evaluate(&self, _ctx: &ResponseContext) -> JudgeVerdict {
                JudgeVerdict::Block("Blocked".to_string())
            }
        }

        let panel = MagiPanel::new(vec![
            Box::new(AlwaysBlockJudge),
            Box::new(AlwaysBlockJudge),
            Box::new(AlwaysBlockJudge),
        ]);

        let ctx = ResponseContext::new("test", "test").with_user_input("test");

        assert!(matches!(panel.decide(&ctx), DecisionOutcome::Halt(_)));
    }
}
