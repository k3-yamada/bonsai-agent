use crate::memory::store::MemoryStore;
use crate::observability::logger::{LogLevel, log_event};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
/// APIリクエストを処理し、(HTTPステータス, JSONレスポンスボディ) を返す
pub fn handle_api_request(
    path: &str,
    auth_header: Option<&str>,
    effective_key: &str,
    store: &MemoryStore,
) -> (&'static str, String) {
    if path == "/health" {
        return ("200 OK", r#"{"status":"ok"}"#.to_string());
    }

    let is_authorized = auth_header
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(|token| token.trim() == effective_key)
        .unwrap_or(false);

    if !is_authorized {
        return (
            "401 Unauthorized",
            r#"{"error":"Unauthorized: valid Bearer token required"}"#.to_string(),
        );
    }

    match path {
        "/api/memories" => {
            let m = store.all_memories().unwrap_or_default();
            let j: Vec<serde_json::Value> = m
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "id": m.id,
                        "content": m.content,
                        "category": m.category
                    })
                })
                .collect();
            (
                "200 OK",
                serde_json::to_string(&j).unwrap_or_else(|e| format!(r#"{{"error":"{}"}}"#, e)),
            )
        }
        "/api/skills" => {
            let s = crate::memory::skill::SkillStore::new(store.conn());
            let sk = s.list_all().unwrap_or_default();
            let j: Vec<serde_json::Value> = sk
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "name": s.name,
                        "success_count": s.success_count
                    })
                })
                .collect();
            (
                "200 OK",
                serde_json::to_string(&j).unwrap_or_else(|e| format!(r#"{{"error":"{}"}}"#, e)),
            )
        }
        "/api/sessions" => {
            let ss = store.list_sessions(50).unwrap_or_default();
            let j: Vec<serde_json::Value> = ss
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "id": s.id,
                        "created_at": s.created_at
                    })
                })
                .collect();
            (
                "200 OK",
                serde_json::to_string(&j).unwrap_or_else(|e| format!(r#"{{"error":"{}"}}"#, e)),
            )
        }
        "/api/arxiv" => {
            let m = store.search_memories("arxiv", 50).unwrap_or_default();
            let j: Vec<serde_json::Value> = m
                .iter()
                .map(|m| serde_json::json!({"content": m.content}))
                .collect();
            (
                "200 OK",
                serde_json::to_string(&j).unwrap_or_else(|e| format!(r#"{{"error":"{}"}}"#, e)),
            )
        }
        "/api/vault" => {
            let vp = dirs::data_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("bonsai-agent")
                .join("vault");
            let mut map = serde_json::Map::new();
            for c in &[
                "decisions",
                "facts",
                "preferences",
                "insights",
                "todos",
                "patterns",
            ] {
                map.insert(
                    c.to_string(),
                    serde_json::Value::String(
                        std::fs::read_to_string(vp.join(format!("{c}.md"))).unwrap_or_default(),
                    ),
                );
            }
            (
                "200 OK",
                serde_json::to_string(&map).unwrap_or_else(|e| format!(r#"{{"error":"{}"}}"#, e)),
            )
        }
        _ => (
            "404 Not Found",
            r#"{"endpoints":["/api/memories","/api/skills","/api/sessions","/api/arxiv","/api/vault"]}"#
                .to_string(),
        ),
    }
}

pub fn start_api_server(store: &MemoryStore, port: u16, api_key: Option<&str>) {
    let generated = if api_key.is_none() {
        Some(uuid::Uuid::new_v4().to_string())
    } else {
        None
    };
    let effective_key = api_key.unwrap_or_else(|| generated.as_deref().unwrap());

    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).expect("API起動失敗");
    log_event(
        LogLevel::Info,
        "server",
        &format!("API: http://127.0.0.1:{port}"),
    );
    if generated.is_some() {
        log_event(
            LogLevel::Info,
            "server",
            &format!("Auto-generated API key: {effective_key}"),
        );
        println!("Auto-generated API key: {effective_key}");
    }

    for stream in listener.incoming().flatten() {
        let mut reader = BufReader::new(&stream);
        let mut req = String::new();
        if reader.read_line(&mut req).is_err() {
            continue;
        }
        let path = req.split_whitespace().nth(1).unwrap_or("/");
        let mut auth_header: Option<String> = None;
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':')
                && k.trim().eq_ignore_ascii_case("authorization")
            {
                auth_header = Some(v.trim().to_string());
            }
        }
        let (st, body) = handle_api_request(path, auth_header.as_deref(), effective_key, store);
        let resp = format!(
            "HTTP/1.1 {st}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let mut w = stream;
        let _ = w.write_all(resp.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_no_auth_required() {
        let store = MemoryStore::in_memory().unwrap();
        let (st, body) = handle_api_request("/health", None, "test-key", &store);
        assert_eq!(st, "200 OK");
        assert!(body.contains("status"));
    }

    #[test]
    fn test_memories_unauthorized_without_token() {
        let store = MemoryStore::in_memory().unwrap();
        let (st, body) = handle_api_request("/api/memories", None, "test-key", &store);
        assert_eq!(st, "401 Unauthorized");
        assert!(body.contains("Unauthorized"));
    }

    #[test]
    fn test_memories_unauthorized_with_wrong_token() {
        let store = MemoryStore::in_memory().unwrap();
        let (st, body) = handle_api_request(
            "/api/memories",
            Some("Bearer wrong-key"),
            "test-key",
            &store,
        );
        assert_eq!(st, "401 Unauthorized");
        assert!(body.contains("Unauthorized"));
    }

    #[test]
    fn test_memories_authorized_with_correct_token() {
        let store = MemoryStore::in_memory().unwrap();
        let (st, body) =
            handle_api_request("/api/memories", Some("Bearer test-key"), "test-key", &store);
        assert_eq!(st, "200 OK");
        assert_eq!(body, "[]");
    }

    #[test]
    fn test_not_found_authorized() {
        let store = MemoryStore::in_memory().unwrap();
        let (st, body) =
            handle_api_request("/api/unknown", Some("Bearer test-key"), "test-key", &store);
        assert_eq!(st, "404 Not Found");
        assert!(body.contains("endpoints"));
    }
}
