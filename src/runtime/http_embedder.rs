//! ローカル HTTP 埋め込み adapter（MLX sidecar 等の OpenAI 互換 `/v1/embeddings`）。
//!
//! `domain::embedder::Embedder` port の runtime 実装。HTTP は runtime 関心事のため、
//! `LlamaServerBackend`（`domain::llm::LlmBackend` の runtime 実装）と同様にここに置く
//! （DEP-001 / ADR-010 クリーンアーキテクチャ準拠 — domain は port のみ）。
//!
//! `embeddings` feature（fastembed/ONNX）に依存しないため、`--no-default-features`
//! （= ort バイナリの build-time DL 不要）のオフライン/Linux ビルドでも実埋め込みを
//! 利用できる。

use anyhow::Result;

use crate::domain::embedder::{
    DEFAULT_EMBEDDING_DIM, Embedder, SimpleEmbedder, hash_embed, l2_normalize,
};
use crate::runtime::http_agent::shared_agent;

/// OpenAI 互換 `/v1/embeddings` の request body を組む（純粋・テスト可能）。
fn build_embeddings_request(model: &str, texts: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "input": texts,
    })
}

/// `/v1/embeddings` レスポンスから埋め込み配列を抽出し `dim` に整形する（純粋・テスト可能）。
///
/// - `data[].index` があれば昇順に並べ替える（順序保証のため）。
/// - 各ベクトルは先頭 `dim` に切詰め（短ければ 0 padding）→ L2 正規化。
///   既存の sqlite-vec `float[256]` テーブルと SimpleEmbedder の次元に揃える。
fn parse_embeddings_response(v: &serde_json::Value, dim: usize) -> Result<Vec<Vec<f32>>> {
    let data = v
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| anyhow::anyhow!("embeddings レスポンスに data 配列がありません"))?;

    let mut indexed: Vec<(u64, Vec<f32>)> = Vec::with_capacity(data.len());
    for (pos, item) in data.iter().enumerate() {
        let arr = item
            .get("embedding")
            .and_then(|e| e.as_array())
            .ok_or_else(|| anyhow::anyhow!("embeddings item に embedding 配列がありません"))?;
        let mut vec = vec![0.0f32; dim];
        for (i, x) in arr.iter().take(dim).enumerate() {
            vec[i] = x.as_f64().unwrap_or(0.0) as f32;
        }
        l2_normalize(&mut vec);
        let idx = item
            .get("index")
            .and_then(|i| i.as_u64())
            .unwrap_or(pos as u64);
        indexed.push((idx, vec));
    }
    indexed.sort_by_key(|(idx, _)| *idx);
    Ok(indexed.into_iter().map(|(_, v)| v).collect())
}

/// MLX sidecar 等の OpenAI 互換 `/v1/embeddings` を叩くローカル HTTP 埋め込み。
///
/// `BONSAI_EMBED_URL` 設定時のみ有効化され、`create_embedder()` が fastembed より
/// 優先して採用する。
pub struct HttpEmbedder {
    base_url: String,
    model: String,
    dim: usize,
}

impl HttpEmbedder {
    /// 明示的に構築する。`base_url` は末尾スラッシュを除去して保持。
    pub fn new(base_url: impl Into<String>, model: impl Into<String>, dim: usize) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Self {
            base_url,
            model: model.into(),
            dim,
        }
    }

    /// env から構築する。`BONSAI_EMBED_URL` 未設定/空なら `None`。
    ///
    /// - `BONSAI_EMBED_URL`   例: `http://localhost:8888`（MLX sidecar）
    /// - `BONSAI_EMBED_MODEL` 既定 `bonsai-embed`
    pub fn from_env() -> Option<Self> {
        let raw = std::env::var("BONSAI_EMBED_URL").ok()?;
        let base_url = raw.trim().trim_end_matches('/').to_string();
        if base_url.is_empty() {
            return None;
        }
        let model = std::env::var("BONSAI_EMBED_MODEL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "bonsai-embed".to_string());
        Some(Self::new(base_url, model, DEFAULT_EMBEDDING_DIM))
    }

    /// リモート `/v1/embeddings` を呼ぶ。ネットワーク/パースエラーは `Err` を返す。
    fn embed_remote(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let url = format!("{}/v1/embeddings", self.base_url);
        let body = build_embeddings_request(&self.model, texts);
        let resp: serde_json::Value = shared_agent()
            .post(&url)
            .header("Content-Type", "application/json")
            .send_json(&body)?
            .body_mut()
            .read_json()?;
        parse_embeddings_response(&resp, self.dim)
    }
}

impl Embedder for HttpEmbedder {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        // リモート失敗 or 件数不一致時はハッシュ埋め込みに graceful fallback。
        // 全 Embedder impl は「出力数 == 入力数」を保証する契約（呼び出し側の
        // `query_vec[0]` 等の index 前提を守る）。HTTP-200 でも data 配列が空/不足だと
        // 契約違反になり downstream で index out-of-bounds panic するため、件数を検査して
        // 不一致なら fallback で正しい件数・次元を返す。
        let fallback = |reason: String| -> Vec<Vec<f32>> {
            eprintln!("[warn] HttpEmbedder {reason}、ハッシュ埋め込みにフォールバック");
            texts.iter().map(|t| hash_embed(t, self.dim)).collect()
        };
        match self.embed_remote(texts) {
            Ok(v) if v.len() == texts.len() => Ok(v),
            Ok(v) => Ok(fallback(format!(
                "が埋め込み件数不一致 ({} != 入力 {})",
                v.len(),
                texts.len()
            ))),
            Err(e) => Ok(fallback(format!("リモート失敗 ({e})"))),
        }
    }

    fn dim(&self) -> usize {
        self.dim
    }
}

/// 最適な Embedder を作成する（合成ルート）。優先順位は以下の通り:
///
/// 1. `HttpEmbedder`（`BONSAI_EMBED_URL` 設定時）— MLX sidecar 等のローカル
///    `/v1/embeddings` 経由。`embeddings` feature 非依存なので、ort バイナリDLを
///    避けたオフライン/Linux ビルドでも実埋め込みが使える。
/// 2. `FastEmbedder`（`embeddings` feature ON 時）— fastembed/ONNX。
/// 3. `SimpleEmbedder`（フォールバック）— ハッシュベース・ゼロ依存。
pub fn create_embedder() -> Box<dyn Embedder> {
    if let Some(e) = HttpEmbedder::from_env() {
        return Box::new(e);
    }
    #[cfg(feature = "embeddings")]
    {
        match crate::domain::embedder::FastEmbedder::new() {
            Ok(e) => return Box::new(e),
            Err(err) => {
                eprintln!("FastEmbedder初期化失敗、SimpleEmbedderにフォールバック: {err}");
            }
        }
    }
    Box::new(SimpleEmbedder::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// env mutation race を避けるため module-local Mutex で serialize する。
    static EMBED_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn t_build_embeddings_request_shape() {
        let body = build_embeddings_request("my-model", &["a", "b"]);
        assert_eq!(body["model"], "my-model");
        assert_eq!(body["input"][0], "a");
        assert_eq!(body["input"][1], "b");
    }

    #[test]
    fn t_parse_embeddings_response_conforms_and_normalizes() {
        // 384 次元 → 先頭 256 に切詰め + L2 正規化。
        let emb: Vec<f32> = (0..384).map(|i| (i % 7) as f32 + 1.0).collect();
        let json = serde_json::json!({
            "data": [ { "index": 0, "embedding": emb } ]
        });
        let out = parse_embeddings_response(&json, DEFAULT_EMBEDDING_DIM).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), DEFAULT_EMBEDDING_DIM);
        let norm: f32 = out[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "L2 正規化されているべき: {norm}");
    }

    #[test]
    fn t_parse_embeddings_response_pads_short_vector() {
        // dim より短いベクトルは 0 padding。
        let json = serde_json::json!({
            "data": [ { "index": 0, "embedding": [3.0, 4.0] } ]
        });
        let out = parse_embeddings_response(&json, DEFAULT_EMBEDDING_DIM).unwrap();
        assert_eq!(out[0].len(), DEFAULT_EMBEDDING_DIM);
        // [3,4] を正規化 → [0.6, 0.8]、残りは 0。
        assert!((out[0][0] - 0.6).abs() < 1e-4);
        assert!((out[0][1] - 0.8).abs() < 1e-4);
        assert_eq!(out[0][2], 0.0);
    }

    #[test]
    fn t_parse_embeddings_response_respects_index_order() {
        // index が逆順で返っても昇順に並べ替える。
        let json = serde_json::json!({
            "data": [
                { "index": 1, "embedding": [0.0, 1.0] },
                { "index": 0, "embedding": [1.0, 0.0] }
            ]
        });
        let out = parse_embeddings_response(&json, 2).unwrap();
        assert!((out[0][0] - 1.0).abs() < 1e-4, "index 0 が先頭に来るべき");
        assert!((out[1][1] - 1.0).abs() < 1e-4, "index 1 が末尾に来るべき");
    }

    #[test]
    fn t_parse_embeddings_response_missing_data_errors() {
        let json = serde_json::json!({ "object": "list" });
        assert!(parse_embeddings_response(&json, 4).is_err());
    }

    #[test]
    fn t_http_embedder_from_env_none_when_unset() {
        let _guard = EMBED_ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var("BONSAI_EMBED_URL");
        }
        assert!(HttpEmbedder::from_env().is_none());
    }

    #[test]
    fn t_http_embedder_from_env_parses_url_and_model() {
        let _guard = EMBED_ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("BONSAI_EMBED_URL", "http://localhost:8888/");
            std::env::set_var("BONSAI_EMBED_MODEL", "embed-xyz");
        }
        let e = HttpEmbedder::from_env().expect("env 設定時は Some");
        assert_eq!(e.base_url, "http://localhost:8888"); // 末尾スラッシュ除去
        assert_eq!(e.model, "embed-xyz");
        assert_eq!(e.dim(), DEFAULT_EMBEDDING_DIM);
        unsafe {
            std::env::remove_var("BONSAI_EMBED_URL");
            std::env::remove_var("BONSAI_EMBED_MODEL");
        }
    }

    #[test]
    fn t_http_embedder_falls_back_on_unreachable_server() {
        // 到達不能ポート → embed() はハッシュ埋め込みに graceful fallback し、
        // dim を保ったまま Err を返さない。
        let e = HttpEmbedder::new("http://127.0.0.1:1", "m", DEFAULT_EMBEDDING_DIM);
        let out = e
            .embed(&["hello", "world"])
            .expect("fallback で Ok を返すべき");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].len(), DEFAULT_EMBEDDING_DIM);
    }

    #[test]
    fn t_http_embedder_roundtrip_against_local_server() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let handle = std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let payload = serde_json::json!({
                    "object": "list",
                    "data": [ { "index": 0, "object": "embedding", "embedding": [1.0, 0.0, 0.0] } ]
                })
                .to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    payload.len(),
                    payload
                );
                let _ = s.write_all(resp.as_bytes());
                let _ = s.flush();
            }
        });

        let e = HttpEmbedder::new(
            format!("http://127.0.0.1:{port}"),
            "m",
            DEFAULT_EMBEDDING_DIM,
        );
        let out = e.embed(&["hi"]).expect("ローカルサーバから Ok");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), DEFAULT_EMBEDDING_DIM);
        assert!((out[0][0] - 1.0).abs() < 1e-4, "[1,0,0] 正規化で先頭=1");
        let _ = handle.join();
    }

    #[test]
    fn t_http_embedder_falls_back_on_count_mismatch() {
        // HTTP 200 でも data 配列が空（入力数と不一致）→ ハッシュ fallback で入力数を維持。
        // downstream の `query_vec[0]` index out-of-bounds panic を防ぐ契約の回帰テスト。
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let payload = r#"{"object":"list","data":[]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    payload.len(),
                    payload
                );
                let _ = s.write_all(resp.as_bytes());
                let _ = s.flush();
            }
        });

        let e = HttpEmbedder::new(
            format!("http://127.0.0.1:{port}"),
            "m",
            DEFAULT_EMBEDDING_DIM,
        );
        let out = e.embed(&["a", "b"]).expect("件数不一致でも fallback で Ok");
        assert_eq!(out.len(), 2, "入力数 == 出力数 の契約を維持");
        assert_eq!(out[0].len(), DEFAULT_EMBEDDING_DIM);
        let _ = handle.join();
    }
}
