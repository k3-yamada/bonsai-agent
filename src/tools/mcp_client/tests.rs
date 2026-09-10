use super::*;

#[test]
fn test_mcp_server_config_deserialize() {
    let toml_str = r#"
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
"#;
    let config: McpServerConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.name, "filesystem");
    assert_eq!(config.command, "npx");
    assert_eq!(config.args.len(), 3);
    assert!(config.url.is_none()); // url未設定時はNone
}

#[test]
fn test_mcp_tool_info() {
    let info = McpToolInfo {
        name: "read_file".to_string(),
        description: "ファイルを読む".to_string(),
        input_schema: serde_json::json!({"type": "object"}),
    };
    assert_eq!(info.name, "read_file");
    assert_eq!(info.description, "ファイルを読む");
}

#[test]
fn test_mcp_tool_info_schema() {
    let info = McpToolInfo {
        name: "test".to_string(),
        description: "desc".to_string(),
        input_schema: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
    };
    assert!(info.input_schema["properties"]["path"].is_object());
}

#[test]
fn test_json_rpc_request_serialize() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0",
        id: 1,
        method: "tools/list".to_string(),
        params: None,
    };
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains("tools/list"));
    assert!(!json.contains("params")); // skip_serializing_if
}

#[test]
fn test_json_rpc_request_with_params() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0",
        id: 2,
        method: "tools/call".to_string(),
        params: Some(serde_json::json!({"name": "test"})),
    };
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains("params"));
}

#[test]
fn test_mcp_tool_wrapper_display_name_format() {
    // display_nameのフォーマット検証（McpConnection不要）
    let display = format!("{}:{}", "filesystem", "read_file");
    assert_eq!(display, "filesystem:read_file");
    let display2 = format!("{}:{}", "git", "status");
    assert_eq!(display2, "git:status");
}

#[test]
fn test_mcp_multiple_servers_toml() {
    let toml_str = r#"
[[servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]

[[servers]]
name = "git"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-git"]
"#;
    let config: crate::config::McpConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.servers.len(), 2);
    assert_eq!(config.servers[0].name, "filesystem");
    assert_eq!(config.servers[1].name, "git");
}

#[test]
fn test_mcp_server_config_with_url() {
    // url設定時のデシリアライズ検証
    let toml_str = r#"
name = "remote-mcp"
command = "unused"
args = []
url = "http://localhost:8080/mcp"
"#;
    let config: McpServerConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.name, "remote-mcp");
    assert_eq!(config.url.as_deref(), Some("http://localhost:8080/mcp"));
    // command/argsはHTTP時は無視されるが、フィールドとして保持
    assert_eq!(config.command, "unused");
}

#[test]
fn test_mcp_server_config_without_url() {
    // url未設定時の後方互換性検証
    let toml_str = r#"
name = "stdio-server"
command = "npx"
args = ["-y", "some-mcp-server"]
"#;
    let config: McpServerConfig = toml::from_str(toml_str).unwrap();
    assert!(config.url.is_none());
    assert_eq!(config.command, "npx");
}

#[test]
fn test_mcp_server_config_http_toml() {
    // 複数サーバー混在（stdio + HTTP）のTOML検証
    let toml_str = r#"
[[servers]]
name = "local"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]

[[servers]]
name = "remote"
command = "unused"
args = []
url = "https://mcp.example.com/rpc"
"#;
    let config: crate::config::McpConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.servers.len(), 2);
    assert!(config.servers[0].url.is_none()); // stdioサーバー
    assert_eq!(
        config.servers[1].url.as_deref(),
        Some("https://mcp.example.com/rpc")
    );
}

#[test]
fn test_http_transport_request_serialization() {
    // HTTP transport用JSON-RPCリクエストのフォーマット検証
    let id = 42u64;
    let request = JsonRpcRequest {
        jsonrpc: "2.0",
        id,
        method: "tools/call".to_string(),
        params: Some(serde_json::json!({
            "name": "read_file",
            "arguments": {"path": "/tmp/test.txt"}
        })),
    };
    let json = serde_json::to_string(&request).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    // JSON-RPC 2.0準拠のフィールド検証
    assert_eq!(parsed["jsonrpc"], "2.0");
    assert_eq!(parsed["id"], 42);
    assert_eq!(parsed["method"], "tools/call");
    assert_eq!(parsed["params"]["name"], "read_file");
    assert_eq!(parsed["params"]["arguments"]["path"], "/tmp/test.txt");
}

#[test]
fn test_http_notification_format() {
    let notification = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
    });
    let json = serde_json::to_string(&notification).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["jsonrpc"], "2.0");
    assert_eq!(parsed["method"], "notifications/initialized");
    assert!(parsed.get("id").is_none());
}

#[test]
fn test_http_error_response_parsing() {
    let error_json =
        r#"{"id": 1, "result": null, "error": {"code": -32601, "message": "Method not found"}}"#;
    let response: JsonRpcResponse = serde_json::from_str(error_json).unwrap();
    assert!(response.error.is_some());
    let err = response.error.unwrap();
    assert_eq!(err["code"], -32601);
}

#[test]
fn test_http_tool_call_result_empty_content() {
    let result = serde_json::json!({"content": []});
    let text = result
        .get("content")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|c| c.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(text.is_empty());
}

#[test]
fn test_http_tool_call_result_with_text() {
    let result = serde_json::json!({
        "content": [{"type": "text", "text": "ファイル内容です"}]
    });
    let text = result
        .get("content")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|c| c.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(text, "ファイル内容です");
}

#[test]
fn test_http_config_url_overrides_command() {
    let toml_str = r#"
name = "http-server"
command = "should-not-run"
args = ["--invalid"]
url = "http://localhost:9090/mcp"
"#;
    let config: McpServerConfig = toml::from_str(toml_str).unwrap();
    assert!(config.url.is_some());
    assert_eq!(config.command, "should-not-run");
}

#[test]
fn test_http_initialize_request_format() {
    let init_params = serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": { "name": "bonsai-agent", "version": "0.1.0" }
    });
    assert_eq!(init_params["protocolVersion"], "2024-11-05");
    assert_eq!(init_params["clientInfo"]["name"], "bonsai-agent");
}

#[test]
fn test_http_tool_call_result_no_content_field() {
    let result = serde_json::json!({"status": "ok"});
    let text = result
        .get("content")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|c| c.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(text.is_empty());
}

#[test]
fn test_http_tool_call_result_non_text_content() {
    let result = serde_json::json!({
        "content": [{"type": "image", "data": "base64..."}]
    });
    let text = result
        .get("content")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|c| c.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(text.is_empty());
}

#[test]
fn test_json_rpc_response_both_result_and_error() {
    let json = r#"{"id": 1, "result": {"tools": []}, "error": {"code": -1, "message": "partial"}}"#;
    let response: JsonRpcResponse = serde_json::from_str(json).unwrap();
    assert!(response.error.is_some());
    assert!(response.result.is_some());
}
// --- Issue #33: stdio子プロセスのプロセスグループ化とグループkill ---
#[cfg(unix)]
fn sh_config(script: &str) -> McpServerConfig {
    McpServerConfig {
        name: "pgtest".to_string(),
        command: "sh".to_string(),
        args: vec!["-c".to_string(), script.to_string()],
        url: None,
    }
}

#[cfg(unix)]
#[test]
fn test_build_stdio_command_sets_process_group_leader() {
    // process_group(0) で子プロセス自身が新グループのリーダー（pgid == pid）になること。
    let mut child = build_stdio_command(&sh_config("sleep 5")).spawn().unwrap();
    let pid = child.id() as i32;
    let pgid = unsafe { libc::getpgid(pid) }; // SAFETY: 直前取得の自プロセスの子pid
    assert_eq!(pgid, pid); // process_group(0)で自身がグループリーダーになるべき
    let _ = child.kill();
    let _ = child.wait();
}
#[cfg(unix)]
#[test]
fn test_drop_kills_grandchild_process_via_group_kill() {
    // sh が孫(sleep)をバックグラウンド起動、pidをechoしwait。Dropでグループkillされ孫も消える。
    let config = sh_config("sleep 30 & echo $!; wait");
    let mut child = build_stdio_command(&config).spawn().unwrap();
    let stdin = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());

    let mut line = String::new();
    reader.read_line(&mut line).expect("孫pidの読み取り");
    let gpid: i32 = line.trim().parse().expect("孫pidのパース");

    // F-3: アサートより先にMcpConnectionへ包む。こうすることで、もしアサートが
    // 失敗（panic）してもスタック巻き戻し時にconnのカスタムDropが走り、
    // プロセスグループがkillされる（sh + 孫sleepのリークを防止）。
    let conn = McpConnection {
        transport: McpTransport::Stdio {
            child,
            stdin,
            reader,
        },
        config,
    };

    // SAFETY: kill(pid, 0) はシグナル送信なしの生存確認。
    assert_eq!(unsafe { libc::kill(gpid, 0) }, 0, "孫は生存中のはず");

    drop(conn);

    // グループkill伝播待ち（プロセス終了はOSスケジューリング依存のため猶予を持たせる）。
    // F-3: ゾンビ化までの実測が1.3-1.6秒とばらつくため、CI環境の揺らぎも見込み5秒に設定。
    //
    // 既知の環境依存の弱さ（Issue #33 Q-3）: `kill(pid, 0) == 0` はプロセスが
    // ゾンビ化した直後（親に未reap）でも成立するため、厳密には「終了した」ではなく
    // 「reapされ、プロセステーブルから消えた」ことの確認になっている。ゾンビの
    // reapはOS（このテストではsleepの新しい親、通常initまたはsubreaper）が行う
    // ため即時ではない点に依存する。`/proc/<pid>/stat` のstate('Z')を見れば
    // ゾンビ化した時点で判定できるが、本プロジェクトの主要ターゲットはmacOS
    // （M2 Mac、`/proc` が存在しない）であるため、あえてLinux専用の実装は
    // 使わず、5秒のポーリング猶予で吸収する設計を維持する。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut alive = true;
    while std::time::Instant::now() < deadline {
        if unsafe { libc::kill(gpid, 0) } != 0 {
            alive = false;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(!alive, "Drop後はグループkillで孫プロセスも消えているべき");
}

#[cfg(unix)]
#[test]
fn test_is_alive_then_drop_still_kills_grandchild_process() {
    // Issue #33 Q-1 回帰テスト: is_alive() を呼んだ後に drop しても孫プロセスが
    // 正しく死ぬこと。
    //
    // 背景: `try_wait()` は死亡していれば即座にreapする。`is_alive()` は
    // `McpToolWrapper::call` から毎回呼ばれるため、これをそのまま使うと
    // MCPサーバー自然死直後の自動再接続処理の中でreapが起き、その後
    // `McpConnection` が drop される際に Drop 側の「reap済みか再確認」ガード
    // (F-1) が必ず「既にreap済み」と判定して killpg が一度も発行されなくなる。
    // 結果、`npx` → `node` のような孫プロセスが再接続のたびにリークする。
    //
    // このテストは `is_alive()` 呼び出し → `drop()` という順序を再現する。
    // 修正前の実装（is_alive() が `child.try_wait()` でreapしてしまう版）に
    // 戻すと必ず FAIL することを確認済み（RED確認）。
    let config = sh_config("sleep 30 & echo $!; exit 0");
    let mut child = build_stdio_command(&config).spawn().unwrap();
    let stdin = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());

    let mut line = String::new();
    reader.read_line(&mut line).expect("孫pidの読み取り");
    let gpid: i32 = line.trim().parse().expect("孫pidのパース");

    let mut conn = McpConnection {
        transport: McpTransport::Stdio {
            child,
            stdin,
            reader,
        },
        config,
    };

    // 親(sh)は `exit 0` で即死する。反応が返るまで軽くポーリングしてから
    // is_alive() を呼ぶ（環境によって sh の終了に多少の揺らぎがあるため）。
    let sh_dead_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < sh_dead_deadline {
        // is_alive() を毎回呼んでも reap されない（peek専用）ことも同時に検証する。
        if !conn.is_alive() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(!conn.is_alive(), "親(sh)は exit 0 で終了しているはず");

    // 孫(sleep)はまだ生存しているはず。
    assert_eq!(unsafe { libc::kill(gpid, 0) }, 0, "孫は生存中のはず");

    drop(conn);

    // グループkill伝播待ち（プロセス終了はOSスケジューリング依存のため猶予を持たせる）。
    // kill(pid, 0)によるゾンビ検知の既知の環境依存の弱さは
    // `test_drop_kills_grandchild_process_via_group_kill` のコメント（Q-3）参照。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut alive = true;
    while std::time::Instant::now() < deadline {
        if unsafe { libc::kill(gpid, 0) } != 0 {
            alive = false;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        !alive,
        "is_alive() 呼び出し後に drop しても孫プロセスはグループkillで消えているべき \
         (Q-1: reapとkillpgの順序が正しければ is_alive() が先に reap してしまうことはない)"
    );
}

// 実MCPサーバーとの統合テスト
#[test]
#[ignore]
fn test_mcp_echo_server() {
    // echo的なMCPサーバーが必要
    let config = McpServerConfig {
        name: "test".to_string(),
        command: "npx".to_string(),
        args: vec![
            "-y".to_string(),
            "@modelcontextprotocol/server-filesystem".to_string(),
            "/tmp".to_string(),
        ],
        url: None,
    };
    let mut conn = McpConnection::spawn(&config).unwrap();
    let tools = conn.list_tools().unwrap();
    assert!(!tools.is_empty());
}
