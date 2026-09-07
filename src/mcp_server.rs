use crate::memory::store::MemoryStore;
use crate::safety::path_guard::PathGuard;
use std::io::{self, BufRead, Write};

pub const ALLOWED_VAULT_CATEGORIES: &[&str] = &[
    "decisions",
    "facts",
    "preferences",
    "insights",
    "todos",
    "patterns",
];

pub fn handle_mcp_request(
    req: &serde_json::Value,
    store: &MemoryStore,
    path_guard: &PathGuard,
) -> Option<serde_json::Value> {
    let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
    let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let resp = match method {
        "initialize" => {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "bonsai-agent", "version": "0.1.0"}
                }
            })
        }
        "notifications/initialized" => return None,
        "tools/list" => serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "tools": [
                    {"name": "search_memories", "description": "メモリ検索", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}},
                    {"name": "list_skills", "description": "スキル一覧", "inputSchema": {"type": "object", "properties": {}}},
                    {"name": "search_arxiv", "description": "arxiv知識検索", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}},
                    {"name": "get_vault", "description": "Vault取得", "inputSchema": {"type": "object", "properties": {"category": {"type": "string"}}}},
                ]
            }
        }),
        "tools/call" => {
            let p = req.get("params").cloned().unwrap_or_default();
            let tn = p.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let a = p.get("arguments").cloned().unwrap_or_default();
            let text = match tn {
                "search_memories" => {
                    let q = a.get("query").and_then(|v| v.as_str()).unwrap_or("");
                    store
                        .search_memories(q, 10)
                        .unwrap_or_default()
                        .iter()
                        .map(|m| format!("- {}", m.content))
                        .collect::<Vec<_>>()
                        .join("\n")
                }
                "list_skills" => crate::memory::skill::SkillStore::new(store.conn())
                    .list_all()
                    .unwrap_or_default()
                    .iter()
                    .map(|s| format!("- {} ({}x)", s.name, s.success_count))
                    .collect::<Vec<_>>()
                    .join("\n"),
                "search_arxiv" => {
                    let q = a.get("query").and_then(|v| v.as_str()).unwrap_or("arxiv");
                    store
                        .search_memories(q, 20)
                        .unwrap_or_default()
                        .iter()
                        .filter(|m| m.content.contains("arxiv"))
                        .map(|m| format!("- {}", m.content))
                        .collect::<Vec<_>>()
                        .join("\n")
                }
                "get_vault" => {
                    let cat = a
                        .get("category")
                        .and_then(|v| v.as_str())
                        .unwrap_or("decisions");
                    if !ALLOWED_VAULT_CATEGORIES.contains(&cat) {
                        format!(
                            "Error: Invalid category '{cat}'. Allowed categories: {}",
                            ALLOWED_VAULT_CATEGORIES.join(", ")
                        )
                    } else {
                        let vp = dirs::data_dir()
                            .unwrap_or_else(|| std::path::PathBuf::from("."))
                            .join("bonsai-agent")
                            .join("vault");
                        let file_path = vp.join(format!("{cat}.md"));
                        let path_str = file_path.to_string_lossy();
                        if path_guard.is_denied(&path_str) {
                            "Error: Access denied by PathGuard".to_string()
                        } else {
                            std::fs::read_to_string(file_path).unwrap_or_default()
                        }
                    }
                }
                _ => "unknown".into(),
            };
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"content": [{"type": "text", "text": text}]}
            })
        }
        _ => {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": "not found"}
            })
        }
    };
    Some(resp)
}

pub fn run_mcp_server(store: &MemoryStore) {
    let path_guard = PathGuard::default_deny_list();
    run_mcp_server_with_guard(store, &path_guard);
}

pub fn run_mcp_server_with_guard(store: &MemoryStore, path_guard: &PathGuard) {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines().map_while(|r| r.ok()) {
        let req: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(resp) = handle_mcp_request(&req, store, path_guard) {
            let _ = writeln!(
                stdout,
                "{}",
                serde_json::to_string(&resp).unwrap_or_default()
            );
            let _ = stdout.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initialize() {
        let store = MemoryStore::in_memory().unwrap();
        let guard = PathGuard::default_deny_list();
        let req = serde_json::json!({"id": 1, "method": "initialize"});
        let resp = handle_mcp_request(&req, &store, &guard).unwrap();
        assert_eq!(resp["result"]["serverInfo"]["name"], "bonsai-agent");
    }

    #[test]
    fn test_get_vault_invalid_category_rejected() {
        let store = MemoryStore::in_memory().unwrap();
        let guard = PathGuard::default_deny_list();
        let req = serde_json::json!({
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "get_vault",
                "arguments": {"category": "../../etc/passwd"}
            }
        });
        let resp = handle_mcp_request(&req, &store, &guard).unwrap();
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Invalid category"));
    }

    #[test]
    fn test_get_vault_valid_category_allowed() {
        let store = MemoryStore::in_memory().unwrap();
        let guard = PathGuard::default_deny_list();
        let req = serde_json::json!({
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "get_vault",
                "arguments": {"category": "decisions"}
            }
        });
        let resp = handle_mcp_request(&req, &store, &guard).unwrap();
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(!text.contains("Invalid category"));
        assert!(!text.contains("Error: Access denied"));
    }

    #[test]
    fn test_get_vault_denied_by_path_guard() {
        let store = MemoryStore::in_memory().unwrap();
        let guard = PathGuard::new(vec!["decisions.md".to_string()]);
        let req = serde_json::json!({
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "get_vault",
                "arguments": {"category": "decisions"}
            }
        });
        let resp = handle_mcp_request(&req, &store, &guard).unwrap();
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Access denied by PathGuard"));
    }
}
