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
    loop {
        if io.cancel.is_cancelled() {
            break;
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
