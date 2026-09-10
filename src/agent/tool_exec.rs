//! ツール実行関連の構造体・関数群
//!
//! agent_loop.rs からの抽出モジュール。
//! ツール呼び出しの実行・並列化・結果反映を担う。

use crate::agent::error_recovery::{
    CircuitBreaker, FileStuckAction, FileStuckGuard, MultiFileEditCycleDetector, TrialSummary,
};
use crate::agent::tool_spill;
use crate::domain::conversation::{Message, Session};
use crate::domain::event::EventType;
use crate::memory::graph::KnowledgeGraph;
use crate::memory::store::MemoryStore;
use crate::observability::audit::{AuditAction, AuditLog};
use crate::observability::logger::{LogLevel, log_event};
use crate::safety::secrets::SecretsFilter;
use crate::tools::{ToolResult, ToolResultCache};

pub(crate) type ConfirmCheckRef<'a> = &'a (dyn Fn(&str, &str) -> bool + Send + Sync);

/// バリデーション済みツール呼び出し（並列実行の単位）
pub(crate) struct ValidatedCall<'a> {
    pub name: String,
    pub args_json: String,
    pub coerced_args: serde_json::Value,
    pub tool: &'a dyn crate::tools::Tool,
    pub is_read_only: bool,
}

/// ツール実行結果（並列実行からの収集用）
pub(crate) struct ToolExecResult {
    pub name: String,
    pub args_json: String,
    pub output: String,
    pub success: bool,
    pub is_error: bool,
    /// ユーザー操作（Ctrl+C等）による取消で失敗したか（Issue #22 qa-reviewer MEDIUM-2）。
    /// `apply_tool_result` はこれが true の場合、circuit_breaker / trial_summary /
    /// KnowledgeGraph への学習記録をスキップする（ユーザー取消を品質シグナル化しない）。
    pub cancelled: bool,
}

/// ツール出力をmax_chars以内に切り詰め（OpenCode知見: スピルオーバー保存）
///
/// スピルオーバー保存先は `tool_spill::SpillStore::process_default()`
/// （プロセス単位ディレクトリ、件数/サイズ上限・ライフサイクル管理付き、Issue #22 B3-1）。
pub(crate) fn truncate_tool_output(output: &str, max_chars: usize) -> String {
    truncate_tool_output_with(
        output,
        max_chars,
        Some(tool_spill::SpillStore::process_default()),
    )
}

/// `truncate_tool_output` の本体。`spill` を注入可能にしテストが `/tmp` を汚さないようにする。
///
/// 切り詰め位置計算・hash算出ロジックは既存から1文字も変えていない。
pub(crate) fn truncate_tool_output_with(
    output: &str,
    max_chars: usize,
    spill: Option<&tool_spill::SpillStore>,
) -> String {
    if max_chars == 0 || output.len() <= max_chars {
        return output.to_string();
    }
    let safe_end = output
        .char_indices()
        .take_while(|(i, _)| *i < max_chars)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    let truncated = &output[..safe_end];
    let hash = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        output.len().hash(&mut h);
        let hash_end = output.floor_char_boundary(safe_end.min(200));
        output[..hash_end].hash(&mut h);
        h.finish()
    };
    match spill.and_then(|s| s.write(hash, output)) {
        Some(path) => format!(
            "{}...\n[全文保存: {} ({}文字、表示{}文字)]",
            truncated,
            path.display(),
            output.len(),
            safe_end
        ),
        None => format!(
            "{}...\n[全文省略 ({}文字、表示{}文字)]",
            truncated,
            output.len(),
            safe_end
        ),
    }
}

/// 単一ツール呼び出しを実行（デフォルトポリシー、テスト用）
#[cfg(test)]
pub(crate) fn execute_single_call(call: &ValidatedCall<'_>) -> ToolExecResult {
    execute_single_call_with_policy(
        call,
        false,
        crate::tools::permission::DaemonPolicy::AutoOnly,
        crate::safety::autonomy::AutonomyLevel::Supervised,
        None,
    )
}

/// 単一ツール呼び出しを権限・自律ポリシーを適用して実行
pub(crate) fn execute_single_call_with_policy(
    call: &ValidatedCall<'_>,
    is_daemon: bool,
    daemon_policy: crate::tools::permission::DaemonPolicy,
    autonomy: crate::safety::autonomy::AutonomyLevel,
    confirm_callback: Option<ConfirmCheckRef<'_>>,
) -> ToolExecResult {
    // 1. 自律レベルによる書き込み制限 (ReadOnly モードでの write 禁止)
    if !call.is_read_only && !autonomy.can_write() {
        return ToolExecResult {
            name: call.name.clone(),
            args_json: call.args_json.clone(),
            output: format!(
                "権限エラー: 現在の自律レベル ({:?}) では書き込み系ツール '{}' の実行は拒否されています",
                autonomy, call.name
            ),
            success: false,
            is_error: true,
            cancelled: false,
        };
    }

    // 2. ツールの権限レベルチェック (check_permission)
    let perm = call.tool.permission();
    let decision = crate::tools::permission::check_permission(perm, is_daemon, daemon_policy);
    match decision {
        crate::tools::permission::PermissionDecision::Allow => {}
        crate::tools::permission::PermissionDecision::Denied => {
            return ToolExecResult {
                name: call.name.clone(),
                args_json: call.args_json.clone(),
                output: format!(
                    "権限エラー: ツール '{}' の実行権限が拒否されています (permission: {:?})",
                    call.name, perm
                ),
                success: false,
                is_error: true,
                cancelled: false,
            };
        }
        crate::tools::permission::PermissionDecision::QueueForLater => {
            return ToolExecResult {
                name: call.name.clone(),
                args_json: call.args_json.clone(),
                output: format!(
                    "保留: ツール '{}' の実行には確認が必要です（キューに保留されました）",
                    call.name
                ),
                success: false,
                is_error: true,
                cancelled: false,
            };
        }
        crate::tools::permission::PermissionDecision::NeedConfirmation => match autonomy {
            crate::safety::autonomy::AutonomyLevel::Full => {}
            crate::safety::autonomy::AutonomyLevel::Supervised => {
                if let Some(cb) = confirm_callback {
                    if !cb(&call.name, &call.args_json) {
                        return ToolExecResult {
                            name: call.name.clone(),
                            args_json: call.args_json.clone(),
                            output: format!(
                                "確認拒否: ツール '{}' の実行がユーザーにより拒否されました",
                                call.name
                            ),
                            success: false,
                            is_error: true,
                            cancelled: false,
                        };
                    }
                } else {
                    return ToolExecResult {
                        name: call.name.clone(),
                        args_json: call.args_json.clone(),
                        output: format!(
                            "確認エラー: ツール '{}' (権限: {:?}) の実行には確認が必要です。非対話または確認コールバック未設定のため実行は拒否されました (自律レベル: {:?})",
                            call.name, perm, autonomy
                        ),
                        success: false,
                        is_error: true,
                        cancelled: false,
                    };
                }
            }
            crate::safety::autonomy::AutonomyLevel::ReadOnly => {
                return ToolExecResult {
                    name: call.name.clone(),
                    args_json: call.args_json.clone(),
                    output: format!(
                        "確認エラー: ツール '{}' (権限: {:?}) の実行には確認が必要です。現在の自律レベル ({:?}) では未確認実行は拒否されました",
                        call.name, perm, autonomy
                    ),
                    success: false,
                    is_error: true,
                    cancelled: false,
                };
            }
        },
    }

    match call.tool.call(call.coerced_args.clone()) {
        Ok(tool_result) => ToolExecResult {
            name: call.name.clone(),
            args_json: call.args_json.clone(),
            output: tool_result.output,
            success: tool_result.success,
            is_error: false,
            cancelled: tool_result.cancelled,
        },
        Err(e) => ToolExecResult {
            name: call.name.clone(),
            args_json: call.args_json.clone(),
            output: format!("ツール実行エラー: {e}"),
            success: false,
            is_error: true,
            cancelled: false,
        },
    }
}

/// 読取専用ツールをstd::thread::scopeで並列実行
///
/// ただし、Confirm 権限を要するツールが含まれ対話コールバックが存在する場合は、
/// stdin / ターミナル入力の競合（プロンプト交錯）を防ぐため逐次（直列）実行する。
pub(crate) fn execute_read_batch_parallel(
    batch: &[ValidatedCall<'_>],
    is_daemon: bool,
    daemon_policy: crate::tools::permission::DaemonPolicy,
    autonomy: crate::safety::autonomy::AutonomyLevel,
    confirm_callback: Option<ConfirmCheckRef<'_>>,
) -> Vec<ToolExecResult> {
    let has_confirm = batch
        .iter()
        .any(|call| call.tool.permission() == crate::tools::permission::Permission::Confirm);
    if has_confirm && confirm_callback.is_some() {
        log_event(
            LogLevel::Debug,
            "sequential",
            &format!(
                "Confirm要件を含む読取ツール{}件を直列実行（stdin競合回避）",
                batch.len()
            ),
        );
        return batch
            .iter()
            .map(|call| {
                execute_single_call_with_policy(
                    call,
                    is_daemon,
                    daemon_policy,
                    autonomy,
                    confirm_callback,
                )
            })
            .collect();
    }

    log_event(
        LogLevel::Debug,
        "parallel",
        &format!("読取ツール{}件を並列実行", batch.len()),
    );
    std::thread::scope(|s| {
        let handles: Vec<_> = batch
            .iter()
            .map(|call| {
                s.spawn(move || {
                    execute_single_call_with_policy(
                        call,
                        is_daemon,
                        daemon_policy,
                        autonomy,
                        confirm_callback,
                    )
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    })
}

/// ツール実行結果をセッション・サーキットブレーカー・監査ログ・試行サマリーに反映
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_tool_result(
    r: &ToolExecResult,
    session: &mut Session,
    circuit_breaker: &mut CircuitBreaker,
    trial_summary: &mut TrialSummary,
    file_stuck_guard: &mut FileStuckGuard,
    iteration: usize,
    secrets_filter: &SecretsFilter,
    store: Option<&MemoryStore>,
    max_output_chars: usize,
) {
    // 冒頭で output と args_json を両方 redact (H4 秘密漏洩防止: error/audit/session/cache 単一ゲート)
    let redacted_output = secrets_filter.redact(&r.output);
    let redacted_args = secrets_filter.redact(&r.args_json);

    let file_path = serde_json::from_str::<serde_json::Value>(&redacted_args)
        .ok()
        .and_then(|v| {
            v.get("path")
                .or_else(|| v.get("file_path"))
                .and_then(|p| p.as_str().map(String::from))
        });

    let is_failed = r.is_error || !r.success;

    // EventStore: ToolCallStart + ToolCallEnd emit (項目162: P1 Step 5 ランタイム統合)
    crate::agent::agent_loop::emit_event(
        store,
        &session.id,
        &EventType::ToolCallStart,
        &serde_json::json!({ "tool": r.name }).to_string(),
        None,
    );
    crate::agent::agent_loop::emit_event(
        store,
        &session.id,
        &EventType::ToolCallEnd,
        &serde_json::json!({ "tool": r.name, "success": !is_failed }).to_string(),
        None,
    );

    if is_failed {
        // Issue #22 qa-reviewer MEDIUM-2: ユーザー取消 (Ctrl+C) はタスク未完了として
        // is_failed=true を維持する（エージェントループの継続判断に必要）が、
        // ツール品質の学習信号 (circuit_breaker/trial_summary/KnowledgeGraph) には
        // 記録しない。ユーザーの取消操作をツール失敗として永続学習するのは
        // docs/VALUES.md V1/V4 (フィードバックシグナルの純度) に反するため
        // (オーナー承認済み)。
        if !r.cancelled {
            circuit_breaker.record_failure(&r.name);
            trial_summary.record_failure(&r.name, &redacted_args, &redacted_output, iteration);
        }
        if let Some(ref fp) = file_path {
            file_stuck_guard.record_file_failure(fp);
            if let Some(action) = file_stuck_guard.check_stuck(fp) {
                let msg = match action {
                    FileStuckAction::Nudge(m) => m,
                    FileStuckAction::GiveUp(m) => m,
                };
                session.add_message(Message::system(msg));
            }
        }
        if let Some(s) = store {
            let audit = AuditLog::new(s.conn());
            let _ = audit.log(
                Some(&session.id),
                &AuditAction::ToolCall {
                    tool_name: r.name.clone(),
                    args: redacted_args.clone(),
                    success: false,
                    output_preview: redacted_output.chars().take(200).collect(),
                },
            );
            if !r.cancelled {
                let graph = KnowledgeGraph::new(s.conn());
                let path = file_path.as_deref().unwrap_or("unknown");
                let _ = graph.record_error_pattern("tool_error", path, &r.name);
            }
        }
        session.add_message(Message::tool(&redacted_output, &r.name));
    } else {
        circuit_breaker.record_success(&r.name);
        if let Some(ref fp) = file_path {
            file_stuck_guard.record_file_success(fp);
        }
        let truncated = truncate_tool_output(&redacted_output, max_output_chars);
        if let Some(s) = store {
            let audit = AuditLog::new(s.conn());
            let _ = audit.log(
                Some(&session.id),
                &AuditAction::ToolCall {
                    tool_name: r.name.clone(),
                    args: redacted_args.clone(),
                    success: true,
                    output_preview: truncated.chars().take(200).collect(),
                },
            );
            if let Some(ref fp) = file_path {
                let graph = KnowledgeGraph::new(s.conn());
                let _ = graph.record_tool_usage(&r.name, fp);
            }
        }
        session.add_message(Message::tool(&truncated, &r.name));
    }
}

/// 編集ツール呼出から path を抽出
///
/// `file_write`/`multi_edit` のみ対象。`args["path"]` または後方互換用 `args["file_path"]` を返す。
/// MultiEdit のように複数 path を扱うツールは args 単一値前提。
fn extract_edit_path(name: &str, args: &serde_json::Value) -> Option<String> {
    if name != "file_write" && name != "multi_edit" {
        return None;
    }
    args.get("path")
        .or_else(|| args.get("file_path"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// 成功した編集ツール呼出から path を抽出して cycle_detector に記録、
/// 検出時 nudge を session に system message として追加する
///
/// `store` 指定時は AuditAction::MultiFileNudge も記録（Phase 1.1 観測用）。
fn record_edit_and_nudge(
    name: &str,
    args: &serde_json::Value,
    cycle_detector: &mut MultiFileEditCycleDetector,
    session: &mut Session,
    store: Option<&MemoryStore>,
) {
    let Some(path) = extract_edit_path(name, args) else {
        return;
    };
    if let Some(nudge) = cycle_detector.record_and_check(&path) {
        log_event(LogLevel::Warn, "edit_cycle", &nudge);
        let files = parse_nudge_files(&nudge);
        let nudge_len = nudge.len();
        session.add_message(Message::system(&nudge));
        if let Some(s) = store {
            let audit = AuditLog::new(s.conn());
            audit
                .log(
                    Some(&session.id),
                    &AuditAction::MultiFileNudge {
                        files,
                        fire_count: cycle_detector.nudge_fire_count(),
                        nudge_len,
                    },
                )
                .ok();
        }
    }
}

/// nudge 文字列の "ファイル <names> を交互に編集" から basename リストを抽出
fn parse_nudge_files(nudge: &str) -> Vec<String> {
    let prefix = "ファイル ";
    let suffix = " を交互に";
    let Some(start) = nudge.find(prefix) else {
        return Vec::new();
    };
    let after = &nudge[start + prefix.len()..];
    let Some(end) = after.find(suffix) else {
        return Vec::new();
    };
    after[..end]
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// バリデーション済みツール呼び出しを実行（読取専用は並列、書き込みは逐次）
///
/// 戻り値: `(実行ツール名一覧, 全ツール成功フラグ)`
#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_validated_calls(
    calls: &[ValidatedCall<'_>],
    session: &mut Session,
    circuit_breaker: &mut CircuitBreaker,
    trial_summary: &mut TrialSummary,
    file_stuck_guard: &mut FileStuckGuard,
    iteration: usize,
    secrets_filter: &SecretsFilter,
    store: Option<&MemoryStore>,
    cache: &mut ToolResultCache,
    cycle_detector: &mut MultiFileEditCycleDetector,
    config: &crate::agent::agent_loop::AgentConfig,
) -> (Vec<String>, bool) {
    let confirm_cb = config.confirm_callback.as_deref();
    let mut step_tools: Vec<String> = Vec::new();
    let mut all_succeeded = true;
    let mut i = 0;
    while i < calls.len() {
        let batch_start = i;
        while i < calls.len() && calls[i].is_read_only {
            i += 1;
        }
        let read_batch = &calls[batch_start..i];
        if read_batch.len() >= 2 {
            let results = execute_read_batch_parallel(
                read_batch,
                config.is_daemon,
                config.daemon_policy,
                config.autonomy,
                confirm_cb,
            );
            for r in results {
                let is_failed = r.is_error || !r.success;
                if is_failed {
                    all_succeeded = false;
                } else if let Ok(args) = serde_json::from_str::<serde_json::Value>(&r.args_json) {
                    // HIGH-3: キャッシュ格納前に平文シークレットをマスク
                    cache.put(
                        &r.name,
                        &args,
                        ToolResult {
                            output: secrets_filter.redact(&r.output),
                            success: r.success,
                            ..Default::default()
                        },
                    );
                }
                apply_tool_result(
                    &r,
                    session,
                    circuit_breaker,
                    trial_summary,
                    file_stuck_guard,
                    iteration,
                    secrets_filter,
                    store,
                    4000,
                );
                step_tools.push(r.name);
            }
        } else {
            for call in read_batch {
                if let Some(cached) = cache.get(&call.name, &call.coerced_args) {
                    // HIGH-3: キャッシュ復元時にも二重防護でマスク適用
                    let redacted = secrets_filter.redact(&cached.output);
                    session.add_message(Message::tool(&redacted, &call.name));
                    step_tools.push(call.name.clone());
                    // EventStore: キャッシュヒット時もトラジェクトリに記録（項目162）
                    crate::agent::agent_loop::emit_event(
                        store,
                        &session.id,
                        &EventType::ToolCallStart,
                        &serde_json::json!({ "tool": call.name }).to_string(),
                        None,
                    );
                    crate::agent::agent_loop::emit_event(
                        store,
                        &session.id,
                        &EventType::ToolCallEnd,
                        &serde_json::json!({ "tool": call.name, "success": cached.success })
                            .to_string(),
                        None,
                    );
                    log_event(
                        LogLevel::Debug,
                        "cache",
                        &format!("キャッシュヒット: {}", call.name),
                    );
                    continue;
                }
                let r = execute_single_call_with_policy(
                    call,
                    config.is_daemon,
                    config.daemon_policy,
                    config.autonomy,
                    confirm_cb,
                );
                let is_failed = r.is_error || !r.success;
                if is_failed {
                    all_succeeded = false;
                } else if let Ok(args) = serde_json::from_str::<serde_json::Value>(&r.args_json) {
                    // HIGH-3: キャッシュ格納前に平文シークレットをマスク
                    cache.put(
                        &r.name,
                        &args,
                        ToolResult {
                            output: secrets_filter.redact(&r.output),
                            success: r.success,
                            ..Default::default()
                        },
                    );
                }
                apply_tool_result(
                    &r,
                    session,
                    circuit_breaker,
                    trial_summary,
                    file_stuck_guard,
                    iteration,
                    secrets_filter,
                    store,
                    4000,
                );
                step_tools.push(r.name);
            }
        }
        if i < calls.len() && !calls[i].is_read_only {
            let write_call = &calls[i];
            let r = execute_single_call_with_policy(
                write_call,
                config.is_daemon,
                config.daemon_policy,
                config.autonomy,
                confirm_cb,
            );
            let is_failed = r.is_error || !r.success;
            if is_failed {
                all_succeeded = false;
            } else {
                // Step 11: 書き込み成功時に cycle 検出
                record_edit_and_nudge(
                    &r.name,
                    &write_call.coerced_args,
                    cycle_detector,
                    session,
                    store,
                );
            }
            apply_tool_result(
                &r,
                session,
                circuit_breaker,
                trial_summary,
                file_stuck_guard,
                iteration,
                secrets_filter,
                store,
                4000,
            );
            step_tools.push(r.name);
            cache.invalidate("file_read");
            cache.invalidate("repo_map");
            i += 1;
        }
    }
    (step_tools, all_succeeded)
}

#[cfg(test)]
mod tests;
