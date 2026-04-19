use crate::memory::store::MemoryStore;
use crate::observability::logger::{LogLevel, log_event};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
pub fn start_api_server(store: &MemoryStore, port: u16) {
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).expect("API起動失敗");
    log_event(LogLevel::Info, "server", "API: http://127.0.0.1:{port}");
    for stream in listener.incoming().flatten() {
        let mut reader = BufReader::new(&stream);
        let mut req = String::new();
        if reader.read_line(&mut req).is_err() {
            continue;
        }
        let path = req.split_whitespace().nth(1).unwrap_or("/");
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                break;
            }
        }
        let (st, body) = match path {
            "/api/memories" => { let m = store.all_memories().unwrap_or_default(); let j: Vec<serde_json::Value> = m.iter().map(|m| serde_json::json!({"id":m.id,"content":m.content,"category":m.category})).collect(); ("200 OK", serde_json::to_string(&j).unwrap()) }
            "/api/skills" => { let s = crate::memory::skill::SkillStore::new(store.conn()); let sk = s.list_all().unwrap_or_default(); let j: Vec<serde_json::Value> = sk.iter().map(|s| serde_json::json!({"name":s.name,"success_count":s.success_count})).collect(); ("200 OK", serde_json::to_string(&j).unwrap()) }
            "/api/sessions" => { let ss = store.list_sessions(50).unwrap_or_default(); let j: Vec<serde_json::Value> = ss.iter().map(|s| serde_json::json!({"id":s.id,"created_at":s.created_at})).collect(); ("200 OK", serde_json::to_string(&j).unwrap()) }
            "/api/arxiv" => { let m = store.search_memories("arxiv", 50).unwrap_or_default(); let j: Vec<serde_json::Value> = m.iter().map(|m| serde_json::json!({"content":m.content})).collect(); ("200 OK", serde_json::to_string(&j).unwrap()) }
            "/api/vault" => { let vp = dirs::data_dir().unwrap_or_else(||std::path::PathBuf::from(".")).join("bonsai-agent").join("vault"); let mut map = serde_json::Map::new(); for c in &["decisions","facts","preferences","insights","todos","patterns"] { map.insert(c.to_string(), serde_json::Value::String(std::fs::read_to_string(vp.join(format!("{c}.md"))).unwrap_or_default())); } ("200 OK", serde_json::to_string(&map).unwrap()) }
            "/health" => ("200 OK", r#"{"status":"ok"}"#.to_string()),
            _ => ("404 Not Found", r#"{"endpoints":["/api/memories","/api/skills","/api/sessions","/api/arxiv","/api/vault"]}"#.to_string()),
        };
        let resp = format!(
            "HTTP/1.1 {st}\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let mut w = stream;
        let _ = w.write_all(resp.as_bytes());
    }
}
