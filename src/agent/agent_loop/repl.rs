//! 会話継続型 REPL ループ (I/O 抽象化により単体テスト可能)。
//!
//! `handle_repl_mode` が毎ターン独立実行 (`run_agent_loop`) して会話履歴を
//! 失う UX バグを解消するため、単一 `Session` をターン間でスレッドする
//! ロジックを lib 側へ切り出す。stdin/stdout は `BufRead`/`Write` で抽象化し、
//! テストでは `Cursor`/`Vec<u8>` を注入する。

use std::io::{BufRead, Write};

use anyhow::Result;

use super::AgentConfig;
use super::core::run_agent_loop_with_session;
use crate::agent::validate::PathGuard;
use crate::cancel::CancellationToken;
use crate::domain::conversation::{Message, Session};
use crate::domain::llm::LlmBackend;
use crate::memory::store::MemoryStore;
use crate::tools::ToolRegistry;

/// REPL ループの実行に必要な依存をまとめた borrow バンドル。
pub struct ReplIo<'a> {
    pub backend: &'a dyn LlmBackend,
    pub tools: &'a ToolRegistry,
    pub path_guard: &'a PathGuard,
    pub config: &'a AgentConfig,
    pub cancel: &'a CancellationToken,
    pub store: Option<&'a MemoryStore>,
    pub is_busy: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub dmn_inbox: Option<std::sync::Arc<std::sync::Mutex<Vec<String>>>>,
    pub dmn_notify: Option<bool>,
}

impl<'a> ReplIo<'a> {
    pub fn new(
        backend: &'a dyn LlmBackend,
        tools: &'a ToolRegistry,
        path_guard: &'a PathGuard,
        config: &'a AgentConfig,
        cancel: &'a CancellationToken,
    ) -> Self {
        Self {
            backend,
            tools,
            path_guard,
            config,
            cancel,
            store: None,
            is_busy: None,
            dmn_inbox: None,
            dmn_notify: None,
        }
    }

    pub fn with_store(mut self, store: Option<&'a MemoryStore>) -> Self {
        self.store = store;
        self
    }

    pub fn with_is_busy(
        mut self,
        is_busy: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> Self {
        self.is_busy = is_busy;
        self
    }

    pub fn with_dmn_inbox(
        mut self,
        inbox: Option<std::sync::Arc<std::sync::Mutex<Vec<String>>>>,
    ) -> Self {
        self.dmn_inbox = inbox;
        self
    }

    pub fn with_dmn_notify(mut self, notify: bool) -> Self {
        self.dmn_notify = Some(notify);
        self
    }
}

/// `reader` から 1 行ずつ読み、各行を **同一 `session`** 上のエージェント
/// ターンとして実行する。これによりターン間で会話履歴が保持される。
///
/// EOF / 空行スキップ / "exit"・"quit" 終了 / キャンセル監視は本関数が担う。
/// プロンプト表示と結果フレーミングは `writer` へ書く。エージェントの
/// ストリーミング応答自体は `run_agent_loop_with_session` 内部で stdout へ
/// 出力される (既存挙動を保持)。
pub fn run_repl<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    session: &mut Session,
    io: &ReplIo,
) -> Result<()> {
    let mut fast_path = crate::agent::fast_path::FastPathDispatcher::with_default_rules();

    loop {
        if io.cancel.is_cancelled() {
            break;
        }
        // ユーザー入力待ち中はビジー状態を解除（DMN 等が動作可能）
        if let Some(ref busy) = io.is_busy {
            busy.store(false, std::sync::atomic::Ordering::Relaxed);
        }

        // DMN からの自発的内省（未読通知）があればプロンプト直前に控えめに表示
        let notify_enabled = io.dmn_notify.unwrap_or_else(|| {
            std::env::var("BONSAI_DMN_NOTIFY")
                .map(|v| v != "0")
                .unwrap_or(true)
        });

        if notify_enabled
            && let Some(ref inbox) = io.dmn_inbox
            && let Ok(mut msgs) = inbox.lock()
        {
            for msg in msgs.drain(..) {
                writeln!(writer, "\n💡 [DMN]: {msg}")?;
            }
        }

        write!(writer, "bonsai> ")?;
        writer.flush()?;

        let mut input = String::new();
        if reader.read_line(&mut input)? == 0 {
            break; // EOF
        }
        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        if input == "exit" || input == "quit" {
            break;
        }

        // 高速ファストパス（反射層代替）: 定型挨拶・ping等をLLM推論バイパスで即答
        if let Some(fast_resp) = fast_path.try_handle(input) {
            session.add_message(Message::user(input));
            session.add_message(Message::assistant(&fast_resp));
            writeln!(writer, "{fast_resp}\n")?;
            continue;
        }

        // 内省・知能スラッシュコマンド (/help, /dmn, /dream, /magi, /vault)
        if input.starts_with('/') {
            handle_slash_command(input, writer, io)?;
            continue;
        }

        // 推論・ツール実行中はビジー状態を設定（DMN 等の競合を防止）
        if let Some(ref busy) = io.is_busy {
            busy.store(true, std::sync::atomic::Ordering::Relaxed);
        }

        session.add_message(Message::user(input));
        let result = run_agent_loop_with_session(
            session,
            io.backend,
            io.tools,
            io.path_guard,
            io.config,
            io.cancel,
            io.store,
        );

        if let Some(ref busy) = io.is_busy {
            busy.store(false, std::sync::atomic::Ordering::Relaxed);
        }

        match result {
            Ok(loop_result) => {
                let answer = &loop_result.answer;
                if answer.starts_with("[中断]") {
                    writeln!(writer, "\n{answer}\n")?;
                } else {
                    writeln!(writer)?;
                }
            }
            Err(e) => {
                writeln!(writer, "\nエラー: {e}\n")?;
            }
        }
    }
    Ok(())
}

/// REPL スラッシュコマンド（内省・知能・メタ認知の対話的操作）
fn handle_slash_command<W: Write>(cmd: &str, writer: &mut W, io: &ReplIo) -> Result<()> {
    match cmd {
        "/help" => {
            writeln!(writer, "\n💡 利用可能な内省・知能コマンド:")?;
            writeln!(
                writer,
                "  /dmn   - DMN（自発思考ループ）の直近の内省履歴を表示"
            )?;
            writeln!(
                writer,
                "  /dream - 即座に Deep Dreaming をキックし、メタ認知レポートを表示"
            )?;
            writeln!(
                writer,
                "  /magi  - MAGI 三重監視の判定ログ・ブロック履歴を表示"
            )?;
            writeln!(
                writer,
                "  /vault - ナレッジ Vault に蓄積されたストック知見を一覧表示"
            )?;
            writeln!(
                writer,
                "  /graph - 記憶・ナレッジグラフの力学可視化HTMLを生成 (/graph open でブラウザ表示)"
            )?;
            writeln!(writer, "  /help  - このヘルプを表示")?;
            writeln!(writer, "  exit   - セッションを終了\n")?;
        }
        "/dmn" => {
            writeln!(writer, "\n🧠 [DMN 直近内省ログ]:")?;
            if let Some(store) = io.store {
                let exp_store = crate::memory::experience::ExperienceStore::new(store.conn());
                if let Ok(exps) = exp_store.find_similar("DMN自発思考ループ", 5) {
                    if exps.is_empty() {
                        writeln!(writer, "  （まだ内省レコードはありません）")?;
                    } else {
                        for (i, exp) in exps.iter().enumerate() {
                            writeln!(
                                writer,
                                "  {}. {} (結果: {})",
                                i + 1,
                                exp.action,
                                exp.outcome
                            )?;
                        }
                    }
                } else {
                    writeln!(writer, "  内省レコードの取得に失敗しました")?;
                }
            } else {
                writeln!(writer, "  （記憶ストアが未初期化です）")?;
            }
            writeln!(writer)?;
        }
        "/dream" => {
            writeln!(writer, "\n🌙 [Deep Dreaming メタ認知レポート]:")?;
            if let Some(store) = io.store {
                let dreamer = crate::memory::dreams::Dreamer::new(store.conn());
                match dreamer.generate_report(7) {
                    Ok(report) => {
                        writeln!(writer, "  成功率: {:.1}%", report.success_rate * 100.0)?;
                        if !report.tool_usage.is_empty() {
                            writeln!(writer, "  ツール使用統計:")?;
                            for (t, c) in report.tool_usage.iter().take(5) {
                                writeln!(writer, "    - {}: {}回", t, c)?;
                            }
                        }
                        if !report.failure_patterns.is_empty() {
                            writeln!(writer, "  失敗傾向:")?;
                            for (f, c) in report.failure_patterns.iter().take(5) {
                                writeln!(writer, "    - {}: {}回", f, c)?;
                            }
                        }
                        if !report.insights.is_empty() {
                            writeln!(writer, "  発見された洞察:")?;
                            for ins in &report.insights {
                                writeln!(writer, "    💡 {}", ins)?;
                            }
                        }
                        if !report.skill_promotions.is_empty() {
                            writeln!(writer, "  スキル昇格推薦:")?;
                            for s in &report.skill_promotions {
                                writeln!(writer, "    ⭐ {}", s)?;
                            }
                        }
                    }
                    Err(e) => {
                        writeln!(writer, "  Dreaming レポート生成エラー: {}", e)?;
                    }
                }
            } else {
                writeln!(writer, "  （記憶ストアが未初期化です）")?;
            }
            writeln!(writer)?;
        }
        "/magi" => {
            writeln!(writer, "\n⚖️ [MAGI 三重監視ステータス]:")?;
            if let Some(store) = io.store {
                let conn = store.conn();
                let audit_count: i64 = conn
                    .query_row("SELECT COUNT(*) FROM audit_log", [], |r| r.get(0))
                    .unwrap_or(0);
                let blocked_count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM events WHERE event_type = 'magi_halt'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                let warned_count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM events WHERE event_type = 'magi_warn'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);

                writeln!(writer, "  総監査件数: {} 件", audit_count)?;
                writeln!(writer, "  遮断・是正 (Block): {} 件", blocked_count)?;
                writeln!(writer, "  警告・指導 (Warn): {} 件", warned_count)?;
                writeln!(
                    writer,
                    "  ValuesJudge (思想・安全) / ConsistencyJudge (整合性) / GoodhartJudge (指標健全性) 稼働中"
                )?;
            } else {
                writeln!(writer, "  （監査ログストアが未初期化です）")?;
            }
            writeln!(writer)?;
        }
        "/vault" => {
            writeln!(writer, "\n📚 [ナレッジVault 最新ストック知見]:")?;
            let default_vault = dirs::data_dir().map(|d| d.join("bonsai-agent").join("vault"));
            if let Some(ref vp) = default_vault
                && vp.exists()
            {
                let insights_file = vp.join("insights.md");
                if insights_file.exists()
                    && let Ok(content) = std::fs::read_to_string(&insights_file)
                {
                    let recent_lines: Vec<&str> =
                        content.lines().filter(|l| l.starts_with("- ")).collect();
                    writeln!(writer, "  ナレッジVault: {}", vp.display())?;
                    writeln!(
                        writer,
                        "  直近の洞察エントリ (insights.md): {} 件",
                        recent_lines.len()
                    )?;
                    for line in recent_lines.iter().rev().take(5) {
                        writeln!(writer, "  {}", line)?;
                    }
                } else {
                    writeln!(writer, "  ナレッジVault: {} (エントリなし)", vp.display())?;
                }
            } else {
                writeln!(writer, "  ナレッジVaultディレクトリが未初期化です")?;
            }
            writeln!(writer)?;
        }
        cmd if cmd == "/graph" || cmd.starts_with("/graph ") => {
            writeln!(writer, "\n📊 [記憶・知識グラフ HTML 出力]:")?;
            if let Some(store) = io.store {
                let parts: Vec<&str> = cmd.split_whitespace().collect();
                let open_browser = parts.iter().any(|&p| p == "open" || p == "--open");
                let output_path = parts
                    .iter()
                    .skip(1)
                    .find(|&&p| p != "open" && p != "--open")
                    .copied()
                    .unwrap_or("memory_graph.html");

                match crate::memory::html_viewer::export_and_render_html(store, true) {
                    Ok(html) => {
                        if let Err(e) = std::fs::write(output_path, html) {
                            writeln!(writer, "  ❌ ファイル書き込みに失敗しました: {e}")?;
                        } else {
                            writeln!(
                                writer,
                                "  ✅ 力学グラフビューアを出力しました: {output_path}"
                            )?;
                            if open_browser {
                                writeln!(writer, "  🌐 ブラウザで開いています: {output_path}")?;
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
                        }
                    }
                    Err(e) => {
                        writeln!(writer, "  ❌ グラフ生成エラー: {e}")?;
                    }
                }
            } else {
                writeln!(writer, "  （記憶ストアが未初期化です）")?;
            }
            writeln!(writer)?;
        }
        _ => {
            writeln!(
                writer,
                "\n不明なコマンドです: '{}'。'/help' で一覧を表示できます。\n",
                cmd
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::conversation::Role;
    use crate::domain::llm::MockLlmBackend;
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_repl_flushes_dmn_inbox_before_prompt() {
        let mock = MockLlmBackend::single("OK");
        let tools = ToolRegistry::new();
        let guard = PathGuard::default_deny_list();
        let config = AgentConfig::default();
        let cancel = CancellationToken::new();

        let mut session = Session::new();
        let inbox = Arc::new(Mutex::new(vec![
            "直近の環境変化を内省: Xcode。作業文脈に応じた支援準備を整えました。".to_string(),
        ]));

        let input = b"exit\n";
        let mut reader = std::io::Cursor::new(&input[..]);
        let mut writer: Vec<u8> = Vec::new();

        let io = ReplIo::new(&mock, &tools, &guard, &config, &cancel)
            .with_dmn_inbox(Some(inbox.clone()))
            .with_dmn_notify(true);

        run_repl(&mut reader, &mut writer, &mut session, &io).unwrap();

        let output = String::from_utf8(writer).unwrap();
        // 1. プロンプト前に 💡 [DMN] バナーが出力されていること
        assert!(output.contains("💡 [DMN]: 直近の環境変化を内省: Xcode"));
        assert!(output.contains("bonsai> "));

        // 2. インボックスが消費されて空になっていること
        assert!(inbox.lock().unwrap().is_empty());
    }

    #[test]
    fn test_repl_dmn_notify_disabled() {
        let mock = MockLlmBackend::single("OK");
        let tools = ToolRegistry::new();
        let guard = PathGuard::default_deny_list();
        let config = AgentConfig::default();
        let cancel = CancellationToken::new();

        let mut session = Session::new();
        let inbox = Arc::new(Mutex::new(vec!["通知すべき洞察".to_string()]));

        let input = b"exit\n";
        let mut reader = std::io::Cursor::new(&input[..]);
        let mut writer: Vec<u8> = Vec::new();

        let io = ReplIo::new(&mock, &tools, &guard, &config, &cancel)
            .with_dmn_inbox(Some(inbox.clone()))
            .with_dmn_notify(false);

        run_repl(&mut reader, &mut writer, &mut session, &io).unwrap();

        let output = String::from_utf8(writer).unwrap();
        // dmn_notify=false なのでプロンプト前にバナーが出ないこと
        assert!(!output.contains("💡 [DMN]"));
    }

    #[test]
    fn test_repl_slash_help() {
        let mock = MockLlmBackend::single("OK");
        let tools = ToolRegistry::new();
        let guard = PathGuard::default_deny_list();
        let config = AgentConfig::default();
        let cancel = CancellationToken::new();

        let mut session = Session::new();
        let input = b"/help\nexit\n";
        let mut reader = std::io::Cursor::new(&input[..]);
        let mut writer: Vec<u8> = Vec::new();

        let io = ReplIo::new(&mock, &tools, &guard, &config, &cancel);
        run_repl(&mut reader, &mut writer, &mut session, &io).unwrap();

        let output = String::from_utf8(writer).unwrap();
        assert!(output.contains("利用可能な内省・知能コマンド"));
        assert!(output.contains("/dmn"));
        assert!(output.contains("/dream"));
        assert!(output.contains("/magi"));
        assert!(output.contains("/vault"));
    }

    #[test]
    fn test_repl_slash_dream() {
        let mock = MockLlmBackend::single("OK");
        let tools = ToolRegistry::new();
        let guard = PathGuard::default_deny_list();
        let config = AgentConfig::default();
        let cancel = CancellationToken::new();

        let store = MemoryStore::in_memory().unwrap();
        let mut session = Session::new();
        let input = b"/dream\nexit\n";
        let mut reader = std::io::Cursor::new(&input[..]);
        let mut writer: Vec<u8> = Vec::new();

        let io = ReplIo::new(&mock, &tools, &guard, &config, &cancel).with_store(Some(&store));
        run_repl(&mut reader, &mut writer, &mut session, &io).unwrap();

        let output = String::from_utf8(writer).unwrap();
        assert!(output.contains("Deep Dreaming メタ認知レポート"));
    }

    #[test]
    fn test_repl_slash_dmn() {
        let mock = MockLlmBackend::single("OK");
        let tools = ToolRegistry::new();
        let guard = PathGuard::default_deny_list();
        let config = AgentConfig::default();
        let cancel = CancellationToken::new();

        let store = MemoryStore::in_memory().unwrap();
        let exp_store = crate::memory::experience::ExperienceStore::new(store.conn());
        let _ = exp_store.record(&crate::memory::experience::RecordParams {
            exp_type: crate::memory::experience::ExperienceType::Insight,
            task_context: "DMN自発思考ループ",
            action: "internal_reflection",
            outcome: "テスト用DMN洞察",
            lesson: None,
            tool_name: None,
            error_type: None,
            error_detail: None,
        });

        let mut session = Session::new();
        let input = b"/dmn\nexit\n";
        let mut reader = std::io::Cursor::new(&input[..]);
        let mut writer: Vec<u8> = Vec::new();

        let io = ReplIo::new(&mock, &tools, &guard, &config, &cancel).with_store(Some(&store));
        run_repl(&mut reader, &mut writer, &mut session, &io).unwrap();

        let output = String::from_utf8(writer).unwrap();
        assert!(output.contains("[DMN 直近内省ログ]"));
        assert!(output.contains("テスト用DMN洞察"));
    }

    #[test]
    fn test_repl_slash_magi() {
        let mock = MockLlmBackend::single("OK");
        let tools = ToolRegistry::new();
        let guard = PathGuard::default_deny_list();
        let config = AgentConfig::default();
        let cancel = CancellationToken::new();

        let store = MemoryStore::in_memory().unwrap();
        let mut session = Session::new();
        let input = b"/magi\nexit\n";
        let mut reader = std::io::Cursor::new(&input[..]);
        let mut writer: Vec<u8> = Vec::new();

        let io = ReplIo::new(&mock, &tools, &guard, &config, &cancel).with_store(Some(&store));
        run_repl(&mut reader, &mut writer, &mut session, &io).unwrap();

        let output = String::from_utf8(writer).unwrap();
        assert!(output.contains("[MAGI 三重監視ステータス]"));
        assert!(output.contains("ValuesJudge"));
    }

    #[test]
    fn test_repl_slash_graph() {
        let mock = MockLlmBackend::single("OK");
        let tools = ToolRegistry::new();
        let guard = PathGuard::default_deny_list();
        let config = AgentConfig::default();
        let cancel = CancellationToken::new();

        let store = MemoryStore::in_memory().unwrap();
        store
            .save_memory("Test graph memory in REPL", "fact", &["graph".to_string()])
            .unwrap();

        let temp_html = format!("/tmp/bonsai_test_graph_{}.html", std::process::id());
        let input_cmd = format!("/graph {}\nexit\n", temp_html);

        let mut session = Session::new();
        let mut reader = std::io::Cursor::new(input_cmd.into_bytes());
        let mut writer: Vec<u8> = Vec::new();

        let io = ReplIo::new(&mock, &tools, &guard, &config, &cancel).with_store(Some(&store));
        run_repl(&mut reader, &mut writer, &mut session, &io).unwrap();

        let output = String::from_utf8(writer).unwrap();
        assert!(output.contains("[記憶・知識グラフ HTML 出力]"));
        assert!(output.contains("力学グラフビューアを出力しました"));

        // ファイルが実際に生成されているか確認
        let html_content = std::fs::read_to_string(&temp_html).expect("read generated html");
        assert!(html_content.contains("Test graph memory in REPL"));
        let _ = std::fs::remove_file(&temp_html);
    }

    #[test]
    fn test_repl_multi_turn_conversation_retention() {
        let mock = MockLlmBackend::new(vec![
            "リンゴですね、覚えました！".to_string(),
            "あなたが好きなのはリンゴです。".to_string(),
        ]);
        let tools = ToolRegistry::new();
        let guard = PathGuard::default_deny_list();
        let config = AgentConfig::default();
        let cancel = CancellationToken::new();
        let store = MemoryStore::in_memory().unwrap();

        let mut session = Session::new();
        let input = "私の好きな果物はリンゴです\n私の好きな果物は何でしたか？\nexit\n";
        let mut reader = std::io::Cursor::new(input.as_bytes());
        let mut writer: Vec<u8> = Vec::new();

        let io = ReplIo::new(&mock, &tools, &guard, &config, &cancel).with_store(Some(&store));
        run_repl(&mut reader, &mut writer, &mut session, &io).unwrap();

        // 複数ターンにわたる会話履歴が同一 session に蓄積されていることを検証
        let user_msgs: Vec<_> = session
            .messages
            .iter()
            .filter(|m| matches!(m.role, Role::User))
            .map(|m| m.content.as_str())
            .collect();
        assert_eq!(
            user_msgs,
            vec!["私の好きな果物はリンゴです", "私の好きな果物は何でしたか？"]
        );

        let assistant_msgs: Vec<_> = session
            .messages
            .iter()
            .filter(|m| matches!(m.role, Role::Assistant))
            .map(|m| m.content.as_str())
            .collect();
        assert_eq!(
            assistant_msgs,
            vec![
                "リンゴですね、覚えました！",
                "あなたが好きなのはリンゴです。"
            ]
        );

        // SQLite にもターンごとの会話履歴が永続化されていることを検証
        let saved_session = store.load_session(&session.id).unwrap().unwrap();
        assert_eq!(saved_session.messages.len(), session.messages.len());
    }
}
