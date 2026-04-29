//! agent_loop の Advisor 連携＋ステップ注入モジュール（refactor 5/8）
//!
//! Advisor 応答解決（remote/claude-code/local 階層フォールバック）、停滞検出時の
//! 再計画注入、完了前自己検証注入、複雑タスク検出時の計画プレステップ注入を集約。

use crate::agent::conversation::{Message, Session};
use crate::agent::error_recovery::{StructuredFeedback, TrialSummary};
use crate::memory::store::MemoryStore;
use crate::observability::audit::{AuditAction, AuditLog};
use crate::observability::logger::{LogLevel, log_event};
use crate::runtime::model_router::{AdvisorConfig, AdvisorRole};

use super::outcome::detect_task_complexity;
use super::state::StallDetector;

/// アドバイザー応答解決の戻り値
pub(super) struct AdvisorResolution {
    pub(super) prompt: String,
    pub(super) source: &'static str, // "remote" or "local"
    pub(super) duration_ms: u64,
}

/// アドバイザー応答を解決（remote優先→ローカルフォールバック、共通ヘルパー）
pub(super) fn resolve_advisor_prompt(
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
pub(super) fn log_advisor_call(
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
pub(super) fn inject_replan_on_stall(
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
pub(super) fn inject_verification_step(
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
            checklist.push_str(
                "

",
            );
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
pub(super) fn inject_planning_step(session: &mut Session, task_context: &str) {
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
