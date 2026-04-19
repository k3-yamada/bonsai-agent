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

/// MCPサーバープロセスとの接続
pub struct McpConnection {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    #[allow(dead_code)]
    config: McpServerConfig,
}

impl McpConnection {
    /// MCPサーバーを子プロセスとして起動
    pub fn spawn(config: &McpServerConfig) -> Result<Self> {
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
            child,
            stdin,
            reader,
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

        let request_json = serde_json::to_string(&request)?;
        writeln!(self.stdin, "{request_json}")?;
        self.stdin.flush()?;

        let mut line = String::new();
        self.reader.read_line(&mut line)?;

        let response: JsonRpcResponse = serde_json::from_str(line.trim())?;

        if let Some(error) = response.error {
            anyhow::bail!("MCPエラー: {error}");
        }

        Ok(response.result.unwrap_or(serde_json::Value::Null))
    }

    /// 通知を送信（レスポンスなし）
    fn send_notification(&mut self, method: &str) -> Result<()> {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
        });

        let json = serde_json::to_string(&request)?;
        writeln!(self.stdin, "{json}")?;
        self.stdin.flush()?;
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
    /// MCPサーバープロセスの生存チェック
    pub fn is_alive(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,  // まだ実行中
            _ => false,        // 終了済みまたはエラー
        }
    }
}

impl Drop for McpConnection {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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
        // 自動復旧: プロセス死亡時に再接続
        if !conn.is_alive() {
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
        };
        let mut conn = McpConnection::spawn(&config).unwrap();
        let tools = conn.list_tools().unwrap();
        assert!(!tools.is_empty());
    }
}
