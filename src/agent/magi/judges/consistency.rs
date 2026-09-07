//! Judge C: 記憶・事実整合性 Judge
//!
//! 直近の記憶・軌跡やタスク前提と明らかに矛盾する発言・自己矛盾を検査する。

use crate::agent::magi::panel::{Judge, JudgeVerdict, ResponseContext};

pub struct ConsistencyJudge;

impl ConsistencyJudge {
    pub fn new() -> Self {
        Self
    }

    /// 四層記憶（MemoryStore / ExperienceStore）との事実整合性照合
    fn check_memory_contradictions(
        store: &crate::memory::store::MemoryStore,
        ctx: &ResponseContext,
    ) -> anyhow::Result<Option<String>> {
        let exp_store = crate::memory::experience::ExperienceStore::new(store.conn());
        let query = if !ctx.task_description.is_empty() {
            ctx.task_description
        } else {
            ctx.user_input
        };

        if query.is_empty() {
            return Ok(None);
        }

        let similar_exps = exp_store.find_similar(query, 5)?;
        let has_recent_failure = similar_exps.iter().any(|e| {
            matches!(
                e.exp_type,
                crate::memory::experience::ExperienceType::Failure
            )
        });

        let claims_all_success = ctx.proposed_response.contains("すべて正常に完了しました")
            || ctx.proposed_response.contains("エラーはありませんでした")
            || ctx.proposed_response.contains("問題なく成功しました")
            || ctx
                .proposed_response
                .contains("何の問題も発生しませんでした");

        if has_recent_failure && claims_all_success {
            return Ok(Some(
                "記憶ストア内に直近のタスク失敗記録が存在するにもかかわらず、全成功と主張しています"
                    .to_string(),
            ));
        }

        Ok(None)
    }
}

impl Default for ConsistencyJudge {
    fn default() -> Self {
        Self::new()
    }
}

impl Judge for ConsistencyJudge {
    fn name(&self) -> &'static str {
        "Judge C (Memory & Fact Consistency)"
    }

    fn evaluate(&self, ctx: &ResponseContext) -> JudgeVerdict {
        // 1. 空応答や無意味な応答
        if ctx.proposed_response.trim().is_empty() {
            return JudgeVerdict::Block("空の応答が生成されました".to_string());
        }

        // 2. 過去の軌跡で失敗したツールを理由なく「成功した」と主張する虚偽・ハルシネーションの検知
        let has_failed_tool = ctx.trajectory.iter().any(|t| {
            let lower = t.to_lowercase();
            lower.contains("fail")
                || lower.contains("error")
                || t.contains("エラー")
                || t.contains("失敗")
        });
        let claims_all_success = ctx.proposed_response.contains("すべて正常に完了しました")
            || ctx.proposed_response.contains("エラーはありませんでした")
            || ctx.proposed_response.contains("問題なく成功しました");

        if has_failed_tool && claims_all_success {
            return JudgeVerdict::Concern(
                "過去のツール実行で失敗が記録されているにもかかわらず、全成功と主張しています"
                    .to_string(),
            );
        }

        // 3. 四層記憶（MemoryStore）との事実照合
        if let Some(concern_msg) = ctx
            .store
            .and_then(|store| Self::check_memory_contradictions(store, ctx).ok().flatten())
        {
            return JudgeVerdict::Concern(concern_msg);
        }

        JudgeVerdict::Clear
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::experience::{ExperienceStore, RecordParams};
    use crate::memory::store::MemoryStore;

    #[test]
    fn test_consistency_judge_blocks_empty() {
        let judge = ConsistencyJudge::new();
        let ctx = ResponseContext::new("task", "   ");
        assert!(matches!(judge.evaluate(&ctx), JudgeVerdict::Block(_)));
    }

    #[test]
    fn test_consistency_judge_detects_trajectory_contradiction() {
        let judge = ConsistencyJudge::new();
        let trajectory = vec!["tool execution failed: permission denied".to_string()];
        let ctx =
            ResponseContext::new("deploy", "すべて正常に完了しました").with_trajectory(&trajectory);

        let verdict = judge.evaluate(&ctx);
        assert!(matches!(verdict, JudgeVerdict::Concern(_)));
        if let JudgeVerdict::Concern(msg) = verdict {
            assert!(msg.contains("失敗が記録されているにもかかわらず"));
        }
    }

    #[test]
    fn test_consistency_judge_detects_store_contradiction() {
        let judge = ConsistencyJudge::new();
        let store = MemoryStore::in_memory().unwrap();
        let exp_store = ExperienceStore::new(store.conn());

        // 失敗経験を記録
        exp_store
            .record(&RecordParams {
                exp_type: crate::memory::experience::ExperienceType::Failure,
                task_context: "deploy_service",
                action: "docker run",
                outcome: "port already in use",
                lesson: Some("check port binding"),
                tool_name: Some("docker"),
                error_type: Some("BindError"),
                error_detail: Some("port 8080 busy"),
            })
            .unwrap();

        // 記憶内に失敗があるのに「すべて正常に完了しました」と回答
        let ctx = ResponseContext::new("deploy_service", "すべて正常に完了しました")
            .with_store(Some(&store));

        let verdict = judge.evaluate(&ctx);
        assert!(matches!(verdict, JudgeVerdict::Concern(_)));
        if let JudgeVerdict::Concern(msg) = verdict {
            assert!(msg.contains("記憶ストア内に直近のタスク失敗記録が存在"));
        }
    }

    #[test]
    fn test_consistency_judge_clears_honest_response() {
        let judge = ConsistencyJudge::new();
        let store = MemoryStore::in_memory().unwrap();
        let exp_store = ExperienceStore::new(store.conn());

        exp_store
            .record(&RecordParams {
                exp_type: crate::memory::experience::ExperienceType::Failure,
                task_context: "deploy_service",
                action: "docker run",
                outcome: "port already in use",
                lesson: Some("check port binding"),
                tool_name: Some("docker"),
                error_type: Some("BindError"),
                error_detail: Some("port 8080 busy"),
            })
            .unwrap();

        // 誠実に失敗を報告する回答
        let ctx = ResponseContext::new(
            "deploy_service",
            "デプロイを実行しましたが、ポート競合により失敗しました。ポート設定を確認してください。",
        )
        .with_store(Some(&store));

        assert_eq!(judge.evaluate(&ctx), JudgeVerdict::Clear);
    }
}
