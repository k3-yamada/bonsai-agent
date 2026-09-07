//! 管理系コマンドハンドラ (ダッシュボード、セッション、タスク、Vault、スキル、チェックポイント、監査)

use anyhow::Result;
use bonsai_agent::agent::checkpoint::CheckpointManager;
use bonsai_agent::memory::store::MemoryStore;

pub fn handle_vault_mode() -> Result<()> {
    let vp = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("bonsai-agent")
        .join("vault");
    if let Ok(v) = bonsai_agent::knowledge::vault::Vault::new(&vp) {
        println!("{}", v.summary().unwrap_or_default());
    }
    Ok(())
}

pub fn handle_skills_export_mode(store: &MemoryStore) -> Result<()> {
    use bonsai_agent::memory::skill::SkillStore;

    let skills = SkillStore::new(store.conn());
    let path = std::path::Path::new("SKILLS.md");
    skills.export_to_file(path)?;
    println!("スキルをエクスポートしました: {}", path.display());
    Ok(())
}

pub fn handle_sessions_mode(store: &MemoryStore) -> Result<()> {
    let sessions = store.list_sessions(20)?;
    if sessions.is_empty() {
        println!("セッションはありません。");
    } else {
        println!("{:<38} {:<22} 内容", "ID", "日時");
        println!("{}", "-".repeat(80));
        for s in &sessions {
            let preview: String = s
                .first_user_message
                .as_deref()
                .unwrap_or("(空)")
                .chars()
                .take(30)
                .collect();
            let date = if s.created_at.len() >= 19 {
                &s.created_at[..19]
            } else {
                &s.created_at
            };
            println!("{id:<38} {date:<22} {preview}", id = s.id);
        }
    }
    Ok(())
}

pub fn handle_tasks_mode(store: &MemoryStore) -> Result<()> {
    let mgr = bonsai_agent::agent::task::TaskManager::new(store.conn());
    let tasks = mgr.list_incomplete()?;
    if tasks.is_empty() {
        println!("未完了タスクはありません。");
    } else {
        for t in &tasks {
            let state = match t.state {
                bonsai_agent::agent::task::TaskState::Pending => "待機",
                bonsai_agent::agent::task::TaskState::InProgress => "実行中",
                bonsai_agent::agent::task::TaskState::WaitingForHuman => "確認待ち",
                bonsai_agent::agent::task::TaskState::Completed => "完了",
                bonsai_agent::agent::task::TaskState::Failed => "失敗",
            };
            println!("[{state}] {} (ステップ: {})", t.goal, t.step_log.len());
            let id_prefix = &t.id[..8.min(t.id.len())];
            println!("  ID: {id_prefix}");
        }
    }
    Ok(())
}

pub fn handle_dashboard_mode(store: &MemoryStore) -> Result<()> {
    use bonsai_agent::agent::experiment_log::ExperimentLog;
    use bonsai_agent::observability::audit::AuditLog;

    println!("╔══════════════════════════════════════════╗");
    println!("║         bonsai-agent ダッシュボード        ║");
    println!("╚══════════════════════════════════════════╝");

    // --- Advisor 統計 ---
    let audit = AuditLog::new(store.conn());
    let advisor = audit.advisor_stats(None)?;
    println!("\n📊 Advisor 統計:");
    if advisor.total_calls == 0 {
        println!("  (呼出なし)");
    } else {
        println!(
            "  総呼出: {}  検証: {}  再計画: {}",
            advisor.total_calls, advisor.verification_calls, advisor.replan_calls
        );
        println!(
            "  remote: {}  local: {}",
            advisor.remote_calls, advisor.local_calls
        );
        println!(
            "  平均プロンプト長: {} 文字  平均remote所要: {} ms",
            advisor.avg_prompt_len, advisor.avg_remote_duration_ms
        );
    }

    // --- Checkpoint 統計 ---
    let cp_stats = CheckpointManager::stats(store.conn(), None)?;
    println!("\n💾 Checkpoint 統計:");
    if cp_stats.total == 0 {
        println!("  (チェックポイントなし)");
    } else {
        println!(
            "  総CP: {}  ロールバック済: {} ({:.0}%)  git保存: {} ({:.0}%)",
            cp_stats.total,
            cp_stats.rolled_back,
            cp_stats.rollback_rate() * 100.0,
            cp_stats.with_git_ref,
            cp_stats.git_capture_rate() * 100.0,
        );
    }

    // --- 実験（Lab）統計 ---
    let experiments = ExperimentLog::recent_experiments(store.conn(), 20)?;
    println!("\n🧪 Lab 実験 (直近{}件):", experiments.len());
    if experiments.is_empty() {
        println!("  (実験なし)");
    } else {
        let accepted = experiments.iter().filter(|e| e.accepted).count();
        let rejected = experiments.len() - accepted;
        let best_delta = experiments
            .iter()
            .map(|e| e.delta)
            .fold(f64::NEG_INFINITY, f64::max);
        let worst_delta = experiments
            .iter()
            .map(|e| e.delta)
            .fold(f64::INFINITY, f64::min);
        println!(
            "  承認: {} / 却下: {} (承認率 {:.0}%)",
            accepted,
            rejected,
            accepted as f64 / experiments.len() as f64 * 100.0
        );
        println!(
            "  最良delta: {:+.4}  最悪delta: {:+.4}",
            best_delta, worst_delta
        );
        // 直近3件を表示
        println!("  ─── 直近3件 ───");
        for exp in experiments.iter().take(3) {
            let status = if exp.accepted { "✓" } else { "✗" };
            println!(
                "  {} {:+.4} | {} | {}",
                status,
                exp.delta,
                exp.mutation_detail.chars().take(40).collect::<String>(),
                exp.experiment_id.chars().take(8).collect::<String>()
            );
        }
    }

    // --- タスク完了統計 ---
    let task_stats = audit.task_complete_stats(None)?;
    println!("\n✅ タスク完了統計:");
    if task_stats.total_completed == 0 {
        println!("  (完了タスクなし)");
    } else {
        println!(
            "  完了数: {}  平均ステップ: {:.1}  平均成功率: {:.0}%  平均所要: {:.0} ms",
            task_stats.total_completed,
            task_stats.avg_steps,
            task_stats.avg_tool_success_rate * 100.0,
            task_stats.avg_duration_ms,
        );
        if !task_stats.recent_summaries.is_empty() {
            println!("  ─── 直近タスク ───");
            for summary in &task_stats.recent_summaries {
                println!("  ・{}", summary.chars().take(60).collect::<String>());
            }
        }
    }

    // --- 監査ログ概要 ---
    let audit_count = audit.count()?;
    println!("\n📋 監査ログ: {} 件", audit_count);

    Ok(())
}

pub fn handle_checkpoints_mode(store: &MemoryStore) -> Result<()> {
    let all = CheckpointManager::load_persisted(store.conn(), None)?;
    if all.is_empty() {
        println!("チェックポイントなし");
        return Ok(());
    }
    let stats = CheckpointManager::stats(store.conn(), None)?;
    println!(
        "=== チェックポイント一覧 ({} 件, ロールバック率 {:.0}%) ===",
        stats.total,
        stats.rollback_rate() * 100.0
    );
    for cp in &all {
        let rb = cp.rolled_back_at.as_deref().unwrap_or("-");
        let git = cp.git_ref.as_deref().unwrap_or("(変更なし)");
        println!(
            "  [{}] {} | git:{} | rb:{} | {}",
            cp.id, cp.description, git, rb, cp.timestamp
        );
    }
    Ok(())
}

pub fn handle_rollback_mode(store: &MemoryStore, cp_id: i64) -> Result<()> {
    let all = CheckpointManager::load_persisted(store.conn(), None)?;
    let cp = all
        .iter()
        .find(|c| c.id == cp_id)
        .ok_or_else(|| anyhow::anyhow!("チェックポイント {} が見つかりません", cp_id))?;
    if cp.git_ref.is_none() {
        println!(
            "チェックポイント {} にはgit stashがありません（変更なしでした）",
            cp_id
        );
        return Ok(());
    }
    // DB+gitで直接ロールバック実行
    let git_ref = cp.git_ref.as_deref().expect("git_ref確認済み");
    if let Err(e) = std::process::Command::new("git")
        .args(["checkout", "."])
        .output()
    {
        println!("git checkout失敗: {e}");
    }
    let success = std::process::Command::new("git")
        .args(["stash", "apply", git_ref])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    // DB 記録
    let now = chrono::Utc::now().to_rfc3339();
    store.conn().execute(
        "UPDATE checkpoints SET rolled_back_at = ?1 WHERE id = ?2",
        rusqlite::params![&now, cp_id],
    )?;
    if success {
        println!(
            "チェックポイント {} にロールバックしました: {}",
            cp_id, cp.description
        );
    } else {
        println!("ロールバック失敗: git stash apply {} がエラー", git_ref);
    }
    Ok(())
}

pub fn handle_audit_mode(store: &MemoryStore) -> Result<()> {
    let audit = bonsai_agent::observability::audit::AuditLog::new(store.conn());
    let entries = audit.recent(50)?;
    if entries.is_empty() {
        println!("監査ログはありません。");
    } else {
        for entry in entries.iter().rev() {
            let ts = if entry.timestamp.len() >= 19 {
                &entry.timestamp[..19]
            } else {
                &entry.timestamp
            };
            let sid = entry.session_id.as_deref().unwrap_or("-");
            println!(
                "{ts}  [{typ}]  session={sid}",
                typ = entry.action_type,
                sid = &sid[..8.min(sid.len())],
            );
            println!("  {}", entry.action_data);
        }
    }
    Ok(())
}

pub fn handle_visualize_mode(
    store: &MemoryStore,
    output_path: &str,
    open_browser: bool,
    filter_privacy: bool,
) -> Result<()> {
    use bonsai_agent::memory::html_viewer::export_and_render_html;
    use std::fs;

    println!("🌱 記憶・ナレッジグラフの可視化HTMLを生成中...");
    let html = export_and_render_html(store, filter_privacy)?;

    fs::write(output_path, html)?;
    println!("✅ 力学グラフビューアを出力しました: {}", output_path);

    if open_browser {
        println!("🌐 デフォルトブラウザで開いています: {}", output_path);
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("open").arg(output_path).spawn();

        #[cfg(target_os = "linux")]
        let _ = std::process::Command::new("xdg-open")
            .arg(output_path)
            .spawn();

        #[cfg(target_os = "windows")]
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", output_path])
            .spawn();
    }

    Ok(())
}
