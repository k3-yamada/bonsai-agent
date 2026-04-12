use std::io::{self, BufRead, Write};
use crate::memory::store::MemoryStore;
pub fn run_mcp_server(store: &MemoryStore) {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines().flatten() {
        let req: serde_json::Value = match serde_json::from_str(&line) { Ok(v) => v, Err(_) => continue };
        let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
        let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let resp = match method {
            "initialize" => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"bonsai-agent","version":"0.1.0"}}}),
            "notifications/initialized" => continue,
            "tools/list" => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"tools":[
                {"name":"search_memories","description":"メモリ検索","inputSchema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}},
                {"name":"list_skills","description":"スキル一覧","inputSchema":{"type":"object","properties":{}}},
                {"name":"search_arxiv","description":"arxiv知識検索","inputSchema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}},
                {"name":"get_vault","description":"Vault取得","inputSchema":{"type":"object","properties":{"category":{"type":"string"}}}},
            ]}}),
            "tools/call" => {
                let p = req.get("params").cloned().unwrap_or_default();
                let tn = p.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let a = p.get("arguments").cloned().unwrap_or_default();
                let text = match tn {
                    "search_memories" => { let q = a.get("query").and_then(|v|v.as_str()).unwrap_or(""); store.search_memories(q,10).unwrap_or_default().iter().map(|m|format!("- {}",m.content)).collect::<Vec<_>>().join("\n") }
                    "list_skills" => { crate::memory::skill::SkillStore::new(store.conn()).list_all().unwrap_or_default().iter().map(|s|format!("- {} ({}x)",s.name,s.success_count)).collect::<Vec<_>>().join("\n") }
                    "search_arxiv" => { let q = a.get("query").and_then(|v|v.as_str()).unwrap_or("arxiv"); store.search_memories(q,20).unwrap_or_default().iter().filter(|m|m.content.contains("arxiv")).map(|m|format!("- {}",m.content)).collect::<Vec<_>>().join("\n") }
                    "get_vault" => { let vp = dirs::data_dir().unwrap_or_else(||std::path::PathBuf::from(".")).join("bonsai-agent").join("vault"); let cat = a.get("category").and_then(|v|v.as_str()).unwrap_or("decisions"); std::fs::read_to_string(vp.join(format!("{cat}.md"))).unwrap_or_default() }
                    _ => "unknown".into(),
                };
                serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":text}]}})
            }
            _ => serde_json::json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"not found"}}),
        };
        let _ = writeln!(stdout, "{}", serde_json::to_string(&resp).unwrap_or_default());
        let _ = stdout.flush();
    }
}
