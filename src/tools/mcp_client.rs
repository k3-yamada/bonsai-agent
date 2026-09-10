use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::tools::permission::Permission;
use crate::tools::{Tool, ToolResult};

/// MCP サーバー設定（TOML定義）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    /// HTTP transportを使用する場合のURL（設定時はcommand/argsを無視）
    #[serde(default)]
    pub url: Option<String>,
}

/// JSON-RPC リクエスト
#[derive(Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

/// JSON-RPC レスポンス
#[derive(Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    id: u64,
    result: Option<serde_json::Value>,
    error: Option<serde_json::Value>,
}

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// MCP通信トランスポート
enum McpTransport {
    /// 子プロセスstdio通信
    Stdio {
        child: Child,
        stdin: ChildStdin,
        reader: BufReader<ChildStdout>,
    },
    /// HTTP JSON-RPC通信
    Http {
        client: reqwest::blocking::Client,
        url: String,
    },
}

/// MCPサーバーとの接続
pub struct McpConnection {
    transport: McpTransport,
    config: McpServerConfig,
}

/// stdio transport用の子プロセスCommandを構築する（Issue #33）。
/// `process_group(0)` で子プロセス自身をグループリーダーにし、`Drop for McpConnection` の
/// `libc::kill(-pid, ..)` でグループ全体（`npx` → `node` 等の孫プロセス含む）を殺せるようにする。
///
/// 既知の制約（F-2、本Issueのスコープ外）: `process_group(0)` により子プロセスは親の
/// 制御端末の前景プロセスグループから外れる。そのため端末クローズ等でのSIGHUP配送や
/// tty由来のシグナル伝播の挙動が親プロセスと異なり、bonsai-agentプロセス自体が
/// シグナル等で強制終了しDropを経由しなかった場合、子プロセスグループが孤児化しうる。
/// フル対応（前景グループ管理・シグナルハンドラでの明示cleanup等）は別Issueで検討する。
///
/// 追記（Issue #33 Q-5）: `process_group(0)` 導入以前は子プロセスが親と同じ
/// プロセスグループに属していたため、tty由来のSIGHUP等は子（とその孫）にも
/// カーネルが自動配送していた。`process_group(0)` 導入後は子が独立グループの
/// リーダーになるため、この自動配送経路が失われ、Drop非経由の終了（親プロセスの
/// SIGKILL/SIGSEGV等でDropが走らないケースやtty切断）では #33 以前より孤児化
/// リスクが悪化している。グループ化によるkillpgの確実性向上（Q-1参照）とのトレード
/// オフであり、フル対応は上記の別Issue検討事項に含まれる。
fn build_stdio_command(config: &McpServerConfig) -> Command {
    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    cmd
}

/// stdio子プロセスのpeek結果（Issue #33 Q-1）。reapを一切伴わずに終了状態を判定する。
///
/// `Alive` / `ExitedNotReaped` の2つは「pidがまだOSに存在しreapされていない」
/// という点で共通しており、これが killpg の安全性（pid再利用が起きていないこと）
/// を担保する条件そのものである。両者を区別しているのは `is_alive()`（再接続要否の
/// 判定にはプロセスが実際に動いているかが必要）のためであり、`Drop` 側の
/// killpg 可否判定では両方とも「安全」として扱う（実測確認済み: ゾンビ化した
/// グループリーダーに対する `kill(-pgid, ..)` も孫プロセスへ正しく伝播する）。
#[cfg(unix)]
#[derive(Debug, PartialEq, Eq)]
enum ChildPeekState {
    /// まだ生存中（終了しておらず、当然reapもされていない）。
    Alive,
    /// 終了済みだが未reap（ゾンビ）。pidはまだOS上でこのプロセス専用に予約されており
    /// 再利用されていないため、この状態からの killpg は安全。
    ExitedNotReaped,
    /// 既にreap済み（`waitid` が `ECHILD` を返した）。pidがOSに再利用されている
    /// 可能性があるため killpg は不可。
    AlreadyReaped,
    /// 判定不能（`waitid` が `ECHILD` 以外のエラーを返した）。安全側に倒し未確定として扱う。
    Unknown,
}

/// `libc::waitid(P_PID, .., WEXITED | WNOHANG | WNOWAIT)` で子プロセスの終了状態を
/// **reapせずにpeekする**（Issue #33 Q-1）。
///
/// `std::process::Child::try_wait()` は死亡していれば即座にreapしOSにstatusを
/// キャッシュしてしまうため使えない。`is_alive()` は `McpToolWrapper::call` から
/// 毎回呼ばれるので、これを使うとMCPサーバー自然死直後の自動再接続処理の中でreapが
/// 起きてしまい、旧 `McpConnection` が drop される際に `Drop` 側の「reap済みか
/// 再確認」ガード（F-1）が必ず「既にreap済み」と判定して killpg を一度も発行しなく
/// なる。結果、`npx` の孫 `node` プロセスが再接続のたびにリークする
/// （本Issueが解決すべき典型シナリオの再発）。
///
/// `WNOWAIT` はプロセスをゾンビのまま残す（reapしない）ため、「reapを行うのは
/// `Drop` のみ」という不変条件を保ちながら生死判定できる。これによりF-1の保証
/// （pid再利用ゼロ）を一切損なわずに孫プロセスの確実な後始末を両立する。
#[cfg(unix)]
fn peek_child_state(child: &Child) -> ChildPeekState {
    // si_pid を0クリアしてから呼ぶ: WNOHANG指定時に報告できる子が無い場合、
    // siginfo_t の内容はPOSIX上未規定。呼び出し前に0クリアしておき、呼び出し後の
    // 非0を「終了済み」の判定根拠にするのがglibc/Linux含め既知の回避策。
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let ret = unsafe {
        libc::waitid(
            libc::P_PID,
            child.id(),
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if ret != 0 {
        let is_echild = std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD);
        return if is_echild {
            // 該当pidの子がOS側に存在しない＝既にreap済み。不変条件が保たれていれば
            // Drop以外では本来起きないはずだが、防御的にAlreadyReaped扱いする。
            ChildPeekState::AlreadyReaped
        } else {
            ChildPeekState::Unknown
        };
    }
    // SAFETY: waitid が 0 を返した直後であり、info は初期化済みのsiginfo_t。
    // si_pid は WEXITED|WNOWAIT で終了済みの子が実際にあった場合のみ非0になる
    // （呼び出し前に0クリア済みのため誤検知しない）。
    if unsafe { info.si_pid() } != 0 {
        ChildPeekState::ExitedNotReaped
    } else {
        ChildPeekState::Alive
    }
}

/// stdio子プロセスの生存チェック本体。unixではreapしないpeekで判定する（Q-1）。
/// `is_alive()` の意味論上、ゾンビ（`ExitedNotReaped`）は「動いていない」= false。
#[cfg(unix)]
fn stdio_is_alive(child: &Child) -> bool {
    matches!(peek_child_state(child), ChildPeekState::Alive)
}

/// 非unix環境向けフォールバック。プロセスグループkillを行わないため、本Issueの
/// group-killリーク対策（グループを跨いだreapタイミング調整）は対象外であり、
/// 従来通りtry_wait()で判定する。
#[cfg(not(unix))]
fn stdio_is_alive(child: &mut Child) -> bool {
    matches!(child.try_wait(), Ok(None))
}

impl McpConnection {
    /// MCPサーバーへ接続（url設定時はHTTP、未設定時はstdioプロセス起動）
    pub fn spawn(config: &McpServerConfig) -> Result<Self> {
        if let Some(ref url) = config.url {
            // HTTP transportで接続
            let client = reqwest::blocking::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .map_err(|e| anyhow::anyhow!("HTTPクライアント作成失敗: {e}"))?;

            let mut conn = Self {
                transport: McpTransport::Http {
                    client,
                    url: url.clone(),
                },
                config: config.clone(),
            };

            // initialize
            conn.send_request(
                "initialize",
                Some(serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "bonsai-agent", "version": "0.1.0" }
                })),
            )?;

            // initialized通知
            conn.send_notification("notifications/initialized")?;

            Ok(conn)
        } else {
            // Stdio transportで起動
            let mut child = build_stdio_command(config)
                .spawn()
                .map_err(|e| anyhow::anyhow!("MCPサーバー起動失敗 '{}': {e}", config.command))?;

            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| anyhow::anyhow!("MCPサーバーのstdin取得失敗"))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| anyhow::anyhow!("MCPサーバーのstdout取得失敗"))?;
            let reader = BufReader::new(stdout);

            let mut conn = Self {
                transport: McpTransport::Stdio {
                    child,
                    stdin,
                    reader,
                },
                config: config.clone(),
            };

            // initialize
            conn.send_request(
                "initialize",
                Some(serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "bonsai-agent", "version": "0.1.0" }
                })),
            )?;

            // initialized通知
            conn.send_notification("notifications/initialized")?;

            Ok(conn)
        }
    }

    /// JSON-RPCリクエストを送信してレスポンスを受け取る
    fn send_request(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let id = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };

        match &mut self.transport {
            McpTransport::Stdio { stdin, reader, .. } => {
                let request_json = serde_json::to_string(&request)?;
                writeln!(stdin, "{request_json}")?;
                stdin.flush()?;

                let mut line = String::new();
                reader.read_line(&mut line)?;

                let response: JsonRpcResponse = serde_json::from_str(line.trim())?;

                if let Some(error) = response.error {
                    anyhow::bail!("MCPエラー: {error}");
                }

                Ok(response.result.unwrap_or(serde_json::Value::Null))
            }
            McpTransport::Http { client, url } => {
                let body = serde_json::to_string(&request)?;
                let resp = client
                    .post(url.as_str())
                    .header("Content-Type", "application/json")
                    .body(body)
                    .send()
                    .map_err(|e| anyhow::anyhow!("MCP HTTP送信失敗: {e}"))?;

                let status = resp.status();
                if !status.is_success() {
                    let err_body = resp.text().unwrap_or_default();
                    anyhow::bail!("MCP HTTPエラー: ステータス {status}, ボディ: {err_body}");
                }

                let text = resp
                    .text()
                    .map_err(|e| anyhow::anyhow!("MCP HTTPレスポンス読取失敗: {e}"))?;

                let response: JsonRpcResponse = serde_json::from_str(&text).map_err(|e| {
                    anyhow::anyhow!("MCP HTTPレスポンスパース失敗: {e}, ボディ: {text}")
                })?;

                if let Some(error) = response.error {
                    anyhow::bail!("MCPエラー: {error}");
                }

                Ok(response.result.unwrap_or(serde_json::Value::Null))
            }
        }
    }

    /// 通知を送信（レスポンスなし）
    fn send_notification(&mut self, method: &str) -> Result<()> {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
        });

        match &mut self.transport {
            McpTransport::Stdio { stdin, .. } => {
                let json = serde_json::to_string(&request)?;
                writeln!(stdin, "{json}")?;
                stdin.flush()?;
            }
            McpTransport::Http { client, url } => {
                let body = serde_json::to_string(&request)?;
                let resp = client
                    .post(url.as_str())
                    .header("Content-Type", "application/json")
                    .body(body)
                    .send()
                    .map_err(|e| anyhow::anyhow!("MCP HTTP通知送信失敗: {e}"))?;
                // ボディを消費してKeep-Alive接続を正しく解放
                let _ = resp.text();
            }
        }
        Ok(())
    }

    /// ツール一覧を取得
    pub fn list_tools(&mut self) -> Result<Vec<McpToolInfo>> {
        let result = self.send_request("tools/list", None)?;
        let tools = result
            .get("tools")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| {
                        Some(McpToolInfo {
                            name: t.get("name")?.as_str()?.to_string(),
                            description: t
                                .get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            input_schema: t
                                .get("inputSchema")
                                .cloned()
                                .unwrap_or(serde_json::json!({})),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(tools)
    }

    /// ツールを呼び出す
    pub fn call_tool(&mut self, name: &str, arguments: serde_json::Value) -> Result<String> {
        let result = self.send_request(
            "tools/call",
            Some(serde_json::json!({
                "name": name,
                "arguments": arguments,
            })),
        )?;

        // MCP tool/call レスポンス: { content: [{ type: "text", text: "..." }] }
        let text = result
            .get("content")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if text.is_empty() {
            Ok(serde_json::to_string_pretty(&result)?)
        } else {
            Ok(text.to_string())
        }
    }
}

impl McpConnection {
    /// MCPサーバーの生存チェック
    pub fn is_alive(&mut self) -> bool {
        match &mut self.transport {
            McpTransport::Stdio { child, .. } => stdio_is_alive(child),
            McpTransport::Http { client, url } => {
                // HTTPサーバーの死活をtools/listで軽量チェック（タイムアウト5秒）
                let req = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 0,
                    "method": "tools/list",
                });
                client
                    .post(url.as_str())
                    .header("Content-Type", "application/json")
                    .timeout(std::time::Duration::from_secs(5))
                    .body(req.to_string())
                    .send()
                    .map(|r| r.status().is_success())
                    .unwrap_or(false)
            }
        }
    }
}

impl Drop for McpConnection {
    fn drop(&mut self) {
        if let McpTransport::Stdio { child, .. } = &mut self.transport {
            // グループkill（Issue #33）。build_stdio_command で process_group(0) 済みのため
            // `-pid` でプロセスグループ全体（npx → node 等の孫プロセス含む）へ SIGKILL が届く。
            //
            // 「reapを行うのは Drop のみ」という不変条件（Issue #33 Q-1）。is_alive() /
            // stdio_is_alive() は `libc::waitid(.., WNOWAIT)` でreapしないpeek専用実装の
            // ため、この Drop の中で初めて子プロセスの終了状態を判定し、reapより前に
            // killpgを行う。これにより自動再接続経路でis_alive()が呼ばれていても
            // killpgが必ず発行される（Q-1）一方、F-1のpid再利用ガード（生存を確認できた
            // 場合のみグループ全体へSIGKILLする）も維持する。この不変条件は
            // `sandbox/direct.rs` の `kill_group_and_reap`（reap済み経路からは呼ばれない）
            // と揃えている。
            #[cfg(unix)]
            match peek_child_state(child) {
                ChildPeekState::Alive | ChildPeekState::ExitedNotReaped => {
                    // pidはまだOS上に存在しreapされていない（生存中、または未reapの
                    // ゾンビ）→ pid再利用の懸念なし。グループ全体へSIGKILL。
                    // SAFETY: 直前の peek（waitid, WNOWAITでreapしない）でpidがまだ
                    // OS上に存在することを確認済み。この後 child.wait() まで reap は
                    // 一切行われないため、pid の再利用は発生していない。ゾンビ化した
                    // グループリーダーに対する `kill(-pgid, ..)` もプロセスグループ自体は
                    // 存命な限り（孫プロセスがメンバーとして残っている限り）孫へ正しく
                    // 伝播する（実測確認済み）。
                    unsafe {
                        libc::kill(-(child.id() as i32), libc::SIGKILL);
                    }
                }
                ChildPeekState::AlreadyReaped => {
                    // 既にreap済み。pidがOSに再利用されている可能性があるため
                    // グループkillはスキップする（F-1対策）。
                }
                ChildPeekState::Unknown => {
                    // 生死判定不能。安全側に倒し、プロセスグループには触れない。
                }
            }

            // child自身の後始末。ここで初めてreapする（不変条件: reapはDropのみ）。
            // `child.kill()` はプロセスグループではなく生pid単体へSIGKILLを送るのみで、
            // 既に終了していれば何もせずErrを返す（reap自体は伴わない。reapするのは
            // 直後の `wait()` のみ）。
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// MCPツール情報
#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// MCPツールをTool traitにラップ（接続参照を保持し、実際のツール呼び出しを委譲）
pub struct McpToolWrapper {
    info: McpToolInfo,
    #[allow(dead_code)]
    server_name: String,
    /// ネームスペース付き表示名（"server:tool"形式）
    display_name: String,
    connection: Arc<Mutex<McpConnection>>,
}

impl McpToolWrapper {
    pub fn new(
        info: McpToolInfo,
        server_name: &str,
        connection: Arc<Mutex<McpConnection>>,
    ) -> Self {
        let display_name = format!("{}:{}", server_name, info.name);
        Self {
            info,
            server_name: server_name.to_string(),
            display_name,
            connection,
        }
    }
}

impl Tool for McpToolWrapper {
    fn name(&self) -> &str {
        &self.display_name
    }

    fn description(&self) -> &str {
        &self.info.description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.info.input_schema.clone()
    }

    fn permission(&self) -> Permission {
        Permission::Confirm // MCPツールはデフォルトConfirm
    }

    fn call(&self, args: serde_json::Value) -> Result<ToolResult> {
        let mut conn = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("MCP接続ロック取得失敗"))?;
        // 自動復旧: stdioプロセス死亡時に再接続
        // HTTP transportはステートレスのため再接続不要（send_request内でリトライなし、呼出側で対応）
        if !conn.is_alive() && matches!(conn.transport, McpTransport::Stdio { .. }) {
            // clone必須: *conn = new_conn で参照先が上書きされるため、事前にconfigをコピー
            match McpConnection::spawn(&conn.config.clone()) {
                Ok(new_conn) => {
                    *conn = new_conn;
                    crate::observability::logger::log_event(
                        crate::observability::logger::LogLevel::Info,
                        "mcp",
                        &format!("MCPサーバー '{}' 自動再接続成功", self.server_name),
                    );
                }
                Err(e) => {
                    return Ok(ToolResult {
                        output: format!("MCP再接続失敗: {e}"),
                        success: false,
                        ..Default::default()
                    });
                }
            }
        }
        match conn.call_tool(&self.info.name, args) {
            Ok(output) => Ok(ToolResult {
                output,
                success: true,
                ..Default::default()
            }),
            Err(e) => Ok(ToolResult {
                output: format!("MCPツールエラー: {e}"),
                success: false,
                ..Default::default()
            }),
        }
    }
}

#[cfg(test)]
mod tests;
