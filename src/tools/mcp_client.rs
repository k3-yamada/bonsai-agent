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
            let mut child = Command::new(&config.command)
                .args(&config.args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
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
            McpTransport::Stdio { child, .. } => {
                matches!(child.try_wait(), Ok(None))
            }
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
                    });
                }
            }
        }
        match conn.call_tool(&self.info.name, args) {
            Ok(output) => Ok(ToolResult {
                output,
                success: true,
            }),
            Err(e) => Ok(ToolResult {
                output: format!("MCPツールエラー: {e}"),
                success: false,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
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
        let error_json = r#"{"id": 1, "result": null, "error": {"code": -32601, "message": "Method not found"}}"#;
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
        let json =
            r#"{"id": 1, "result": {"tools": []}, "error": {"code": -1, "message": "partial"}}"#;
        let response: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert!(response.error.is_some());
        assert!(response.result.is_some());
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
}
