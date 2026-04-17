use std::io::BufRead;
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::agent::conversation::Message;
use crate::cancel::CancellationToken;
use crate::config::InferenceParams;
use crate::runtime::inference::{GenerateResult, LlmBackend, TokenUsage};
use crate::tools::ToolSchema;

/// llama-serverプロセスを管理し、OpenAI互換APIで通信するバックエンド
pub struct LlamaServerBackend {
    base_url: String,
    model_id: String,
    inference: InferenceParams,
}

impl LlamaServerBackend {
    /// 既に起動しているllama-serverに接続
    pub fn connect(base_url: &str, model_id: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            model_id: model_id.to_string(),
            inference: InferenceParams::default(),
        }
    }

    /// 推論パラメータ付きで接続
    pub fn connect_with_params(base_url: &str, model_id: &str, inference: InferenceParams) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            model_id: model_id.to_string(),
            inference,
        }
    }

    /// ヘルスチェック（/health → /v1/models フォールバック）
    ///
    /// llama-serverは/health、mlx-lm serverは/v1/modelsで応答する。
    pub fn is_healthy(&self) -> bool {
        let health_url = format!("{}/health", self.base_url);
        if ureq::get(&health_url).call().is_ok() {
            return true;
        }
        // mlx-lm server は /health 未対応 → /v1/models で代替
        let models_url = format!("{}/v1/models", self.base_url);
        ureq::get(&models_url).call().is_ok()
    }

    /// ヘルスチェック+待機リトライ（macOS26/Agent知見: 死活監視パターン）
    ///
    /// サーバーが一時的にダウンしている場合、最大`max_wait`秒待機して復帰を待つ。
    /// 復帰しなければErrを返す。
    pub fn wait_for_health(&self, max_wait: std::time::Duration) -> anyhow::Result<()> {
        if self.is_healthy() {
            return Ok(());
        }
        eprintln!("[llama-server] ヘルスチェック失敗、復帰待機中...");
        let start = std::time::Instant::now();
        while start.elapsed() < max_wait {
            std::thread::sleep(std::time::Duration::from_secs(2));
            if self.is_healthy() {
                eprintln!("[llama-server] 復帰確認 ({}秒)", start.elapsed().as_secs());
                return Ok(());
            }
        }
        anyhow::bail!(
            "llama-server が{}秒以内に復帰しませんでした ({})",
            max_wait.as_secs(),
            self.base_url
        )
    }

    /// メッセージをOpenAI互換のJSON形式に変換
    fn build_request_body(&self, messages: &[Message], tools: &[ToolSchema]) -> serde_json::Value {
        let mut msgs: Vec<serde_json::Value> = Vec::new();

        // ツールスキーマをシステムプロンプトに注入
        if !tools.is_empty() {
            let tool_prompt = format_tool_schemas(tools);
            // 最初のシステムメッセージに追加、またはなければ新規作成
            let has_system = messages
                .iter()
                .any(|m| matches!(m.role, crate::agent::conversation::Role::System));
            if !has_system {
                msgs.push(serde_json::json!({
                    "role": "system",
                    "content": tool_prompt,
                }));
            }
        }

        for msg in messages {
            let role = match msg.role {
                crate::agent::conversation::Role::System => "system",
                crate::agent::conversation::Role::User => "user",
                crate::agent::conversation::Role::Assistant => "assistant",
                crate::agent::conversation::Role::Tool => "user", // llama-serverはtoolロール非対応
            };

            let mut content = msg.content.clone();

            // システムメッセージにツールスキーマを追加
            if matches!(msg.role, crate::agent::conversation::Role::System) && !tools.is_empty() {
                content = format!("{}\n\n{}", content, format_tool_schemas(tools));
            }

            // Toolロールの場合はプレフィックスを付加
            if matches!(msg.role, crate::agent::conversation::Role::Tool)
                && let Some(id) = &msg.tool_call_id
            {
                content = format!(
                    "<tool_response>{}</tool_response>\nツール '{id}' の結果:\n{content}",
                    ""
                );
            }

            msgs.push(serde_json::json!({
                "role": role,
                "content": content,
            }));
        }

        serde_json::json!({
            "messages": msgs,
            "temperature": self.inference.temperature,
            "top_k": self.inference.top_k,
            "top_p": self.inference.top_p,
            "min_p": self.inference.min_p,
            "max_tokens": self.inference.max_tokens,
            "repeat_penalty": self.inference.repeat_penalty,
            "stream": true,
        })
    }

    /// SSEストリームをパースし、トークンごとにon_tokenを呼ぶ
    ///
    /// 各行は `data: {...}` 形式。`data: [DONE]` で終了。
    /// usage情報は最後のチャンクから取得する。
    fn parse_sse_stream(
        &self,
        reader: impl std::io::Read,
        on_token: &mut dyn FnMut(&str),
        cancel: &CancellationToken,
    ) -> Result<(String, usize, usize)> {
        let buf_reader = std::io::BufReader::new(reader);
        let mut full_text = String::new();
        let mut prompt_tokens: usize = 0;
        let mut completion_tokens: usize = 0;

        for line_result in buf_reader.lines() {
            if cancel.is_cancelled() {
                anyhow::bail!("ストリーミング中にキャンセルされました");
            }

            let line = line_result?;

            // SSEでは空行がイベント区切り — スキップ
            if line.is_empty() {
                continue;
            }

            // "data: " プレフィックスを除去
            let Some(data) = line.strip_prefix("data: ") else {
                // コメント行やその他のSSEフィールドはスキップ
                continue;
            };

            // 終了シグナル
            if data == "[DONE]" {
                break;
            }

            // JSONパース
            let chunk: serde_json::Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => continue, // パース失敗は無視（不完全なチャンク等）
            };

            // delta.contentからトークンを抽出
            if let Some(content) = chunk["choices"][0]["delta"]["content"].as_str() {
                if !content.is_empty() {
                    on_token(content);
                    full_text.push_str(content);
                }
            }

            // usage情報（最後のチャンクに含まれる場合）
            if let Some(usage) = chunk.get("usage") {
                prompt_tokens = usage["prompt_tokens"].as_u64().unwrap_or(0) as usize;
                completion_tokens = usage["completion_tokens"].as_u64().unwrap_or(0) as usize;
            }
        }

        Ok((full_text, prompt_tokens, completion_tokens))
    }

    /// 非ストリーミングモードでフォールバック生成
    fn generate_non_streaming(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        on_token: &mut dyn FnMut(&str),
        start: Instant,
    ) -> Result<GenerateResult> {
        let url = format!("{}/v1/chat/completions", self.base_url);
        let mut body = self.build_request_body(messages, tools);
        // ストリーミングを無効化してフォールバック
        body["stream"] = serde_json::json!(false);

        let response: serde_json::Value = ureq::post(&url)
            .header("Content-Type", "application/json")
            .send_json(&body)?
            .body_mut()
            .read_json()?;

        let text = response["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let prompt_tokens = response["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as usize;
        let completion_tokens =
            response["usage"]["completion_tokens"].as_u64().unwrap_or(0) as usize;

        on_token(&text);

        Ok(GenerateResult {
            text,
            usage: TokenUsage {
                prompt_tokens,
                completion_tokens,
                duration: start.elapsed(),
            },
            model_id: self.model_id.clone(),
        })
    }
}

impl LlmBackend for LlamaServerBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn generate(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        on_token: &mut dyn FnMut(&str),
        cancel: &CancellationToken,
    ) -> Result<GenerateResult> {
        if cancel.is_cancelled() {
            anyhow::bail!("キャンセルされました");
        }

        let url = format!("{}/v1/chat/completions", self.base_url);
        let body = self.build_request_body(messages, tools);

        let start = Instant::now();

        // ストリーミングリクエスト送信
        let response = match ureq::post(&url)
            .header("Content-Type", "application/json")
            .send_json(&body)
        {
            Ok(resp) => resp,
            Err(_e) => {
                // ストリーミングリクエスト失敗時は非ストリーミングでフォールバック
                return self.generate_non_streaming(messages, tools, on_token, start);
            }
        };

        // SSEストリームをパース
        let reader = response.into_body().into_reader();
        match self.parse_sse_stream(reader, on_token, cancel) {
            Ok((text, prompt_tokens, completion_tokens)) => Ok(GenerateResult {
                text,
                usage: TokenUsage {
                    prompt_tokens,
                    completion_tokens,
                    duration: start.elapsed(),
                },
                model_id: self.model_id.clone(),
            }),
            Err(e) => {
                // SSEパース失敗時は非ストリーミングでフォールバック
                eprintln!("[llama-server] SSEパース失敗、非ストリーミングにフォールバック: {e}");
                self.generate_non_streaming(messages, tools, on_token, start)
            }
        }
    }
}

/// llama-serverプロセスの管理
pub struct LlamaServerProcess {
    child: Child,
    pub port: u16,
}

impl LlamaServerProcess {
    /// llama-serverを子プロセスとして起動
    pub fn spawn(
        server_binary: &str,
        model_path: &Path,
        context_length: u32,
        kv_cache_type: &str,
    ) -> Result<Self> {
        let port = find_free_port()?;

        let child = Command::new(server_binary)
            .arg("-m")
            .arg(model_path)
            .arg("--port")
            .arg(port.to_string())
            .arg("--host")
            .arg("127.0.0.1")
            .arg("-c")
            .arg(context_length.to_string())
            .arg("-ngl")
            .arg("99")
            .arg("--cache-type-k")
            .arg(kv_cache_type)
            .arg("--cache-type-v")
            .arg(kv_cache_type)
            .arg("--flash-attn")
            .arg("on")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let process = Self { child, port };
        process.wait_until_healthy(Duration::from_secs(60))?;
        Ok(process)
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn wait_until_healthy(&self, timeout: Duration) -> Result<()> {
        let health_url = format!("http://127.0.0.1:{}/health", self.port);
        let models_url = format!("http://127.0.0.1:{}/v1/models", self.port);
        let start = Instant::now();

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!(
                    "サーバーの起動がタイムアウト（{}秒）",
                    timeout.as_secs()
                );
            }
            // /health → /v1/models フォールバック（MLX対応）
            if ureq::get(&health_url).call().is_ok() || ureq::get(&models_url).call().is_ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// プロセスを停止
    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for LlamaServerProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 空きポートを取得
fn find_free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// ツールスキーマをプロンプト用テキストにフォーマット
fn format_tool_schemas(tools: &[ToolSchema]) -> String {
    let mut out = String::from(
        "# 使用可能なツール\n\nツールを呼び出すには以下のXML形式を使用してください:\n<tool_call>{\"name\": \"ツール名\", \"arguments\": {パラメータ}}</tool_call>\n\n",
    );

    for tool in tools {
        out.push_str(&format!(
            "## {}\n{}\nパラメータ: {}\n\n",
            tool.name,
            tool.description,
            serde_json::to_string(&tool.parameters).unwrap_or_default(),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect() {
        let backend = LlamaServerBackend::connect("http://localhost:8080", "bonsai-8b");
        assert_eq!(backend.model_id(), "bonsai-8b");
        assert_eq!(backend.base_url, "http://localhost:8080");
    }

    #[test]
    fn test_connect_trailing_slash() {
        let backend = LlamaServerBackend::connect("http://localhost:8080/", "test");
        assert_eq!(backend.base_url, "http://localhost:8080");
    }

    #[test]
    fn test_build_request_body() {
        let backend = LlamaServerBackend::connect("http://localhost:8080", "test");
        let messages = vec![
            Message::system("あなたはAIです"),
            Message::user("こんにちは"),
        ];
        let body = backend.build_request_body(&messages, &[]);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[1]["role"], "user");
    }

    #[test]
    fn test_build_request_body_with_tools() {
        let backend = LlamaServerBackend::connect("http://localhost:8080", "test");
        let messages = vec![Message::user("ファイル一覧")];
        let tools = vec![ToolSchema {
            name: "shell".to_string(),
            description: "コマンド実行".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }];
        let body = backend.build_request_body(&messages, &tools);
        let msgs = body["messages"].as_array().unwrap();
        // ツールスキーマがシステムメッセージとして追加される
        assert!(msgs.len() >= 2);
    }

    #[test]
    fn test_build_request_body_stream_enabled() {
        let backend = LlamaServerBackend::connect("http://localhost:8080", "test");
        let messages = vec![Message::user("test")];
        let body = backend.build_request_body(&messages, &[]);
        // ストリーミングが有効であること
        assert_eq!(body["stream"], serde_json::json!(true));
    }

    #[test]
    fn test_parse_sse_stream_basic() {
        let backend = LlamaServerBackend::connect("http://localhost:8080", "test");
        let cancel = CancellationToken::new();
        let sse_data = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n\
                        data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\n\
                        data: {\"choices\":[{\"delta\":{\"content\":\"!\"}}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":3}}\n\n\
                        data: [DONE]\n\n";
        let reader = std::io::Cursor::new(sse_data.as_bytes());
        let mut tokens: Vec<String> = Vec::new();

        let (text, prompt, completion) = backend
            .parse_sse_stream(reader, &mut |t| tokens.push(t.to_string()), &cancel)
            .unwrap();

        assert_eq!(text, "Hello world!");
        assert_eq!(tokens, vec!["Hello", " world", "!"]);
        assert_eq!(prompt, 10);
        assert_eq!(completion, 3);
    }

    #[test]
    fn test_parse_sse_stream_cancel() {
        let backend = LlamaServerBackend::connect("http://localhost:8080", "test");
        let cancel = CancellationToken::new();
        cancel.cancel(); // 事前にキャンセル

        let sse_data = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n\
                        data: [DONE]\n\n";
        let reader = std::io::Cursor::new(sse_data.as_bytes());

        let result = backend.parse_sse_stream(reader, &mut |_| {}, &cancel);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_sse_stream_empty_content() {
        let backend = LlamaServerBackend::connect("http://localhost:8080", "test");
        let cancel = CancellationToken::new();
        // role deltaのみ（content無し）のチャンクをスキップ
        let sse_data = "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n\
                        data: {\"choices\":[{\"delta\":{\"content\":\"OK\"}}]}\n\n\
                        data: [DONE]\n\n";
        let reader = std::io::Cursor::new(sse_data.as_bytes());
        let mut tokens: Vec<String> = Vec::new();

        let (text, _, _) = backend
            .parse_sse_stream(reader, &mut |t| tokens.push(t.to_string()), &cancel)
            .unwrap();

        assert_eq!(text, "OK");
        assert_eq!(tokens, vec!["OK"]);
    }

    #[test]
    fn test_parse_sse_stream_malformed_json() {
        let backend = LlamaServerBackend::connect("http://localhost:8080", "test");
        let cancel = CancellationToken::new();
        // 不正JSONは無視される
        let sse_data = "data: {invalid json}\n\n\
                        data: {\"choices\":[{\"delta\":{\"content\":\"OK\"}}]}\n\n\
                        data: [DONE]\n\n";
        let reader = std::io::Cursor::new(sse_data.as_bytes());
        let mut tokens: Vec<String> = Vec::new();

        let (text, _, _) = backend
            .parse_sse_stream(reader, &mut |t| tokens.push(t.to_string()), &cancel)
            .unwrap();

        assert_eq!(text, "OK");
    }

    #[test]
    fn test_find_free_port() {
        let port = find_free_port().unwrap();
        assert!(port > 0);
    }

    #[test]
    fn test_format_tool_schemas() {
        let tools = vec![ToolSchema {
            name: "shell".to_string(),
            description: "コマンド実行".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }];
        let formatted = format_tool_schemas(&tools);
        assert!(formatted.contains("shell"));
        assert!(formatted.contains("tool_call"));
    }

    // llama-serverが起動していない場合のテスト
    #[test]
    fn test_health_check_fails_without_server() {
        let backend = LlamaServerBackend::connect("http://127.0.0.1:19999", "test");
        assert!(!backend.is_healthy());
    }

    // 実サーバーとの統合テスト（#[ignore]で分離）
    #[test]
    #[ignore]
    fn test_generate_with_live_server() {
        let backend = LlamaServerBackend::connect("http://127.0.0.1:8080", "bonsai-8b");
        assert!(backend.is_healthy(), "llama-serverが起動していません");

        let messages = vec![Message::user("1+1は？")];
        let cancel = CancellationToken::new();
        let mut output = String::new();

        let result = backend
            .generate(&messages, &[], &mut |t| output.push_str(t), &cancel)
            .unwrap();

        assert!(!result.text.is_empty());
        assert!(result.usage.completion_tokens > 0);
    }

    #[test]
    fn test_backend_connect() {
        let b = LlamaServerBackend::connect("http://localhost:8080", "test");
        assert_eq!(b.model_id(), "test");
    }

    #[test]
    fn test_backend_base_url() {
        let b = LlamaServerBackend::connect("http://localhost:9090", "m");
        // base_url はトリムされる
        assert!(!b.model_id().is_empty());
    }

    #[test]
    fn test_connect_with_params() {
        let params = InferenceParams {
            temperature: 0.3,
            top_k: 10,
            ..Default::default()
        };
        let b = LlamaServerBackend::connect_with_params("http://localhost:8000", "ternary-bonsai-8b", params);
        assert_eq!(b.model_id(), "ternary-bonsai-8b");
        assert!((b.inference.temperature - 0.3).abs() < f64::EPSILON);
        assert_eq!(b.inference.top_k, 10);
        // 未指定はデフォルト
        assert_eq!(b.inference.max_tokens, 1024);
    }

    #[test]
    fn test_build_request_body_uses_inference_params() {
        let params = InferenceParams {
            temperature: 0.7,
            max_tokens: 2048,
            ..Default::default()
        };
        let b = LlamaServerBackend::connect_with_params("http://localhost:8000", "test", params);
        let messages = vec![Message::user("test")];
        let body = b.build_request_body(&messages, &[]);
        assert!((body["temperature"].as_f64().unwrap() - 0.7).abs() < f64::EPSILON);
        assert_eq!(body["max_tokens"].as_u64().unwrap(), 2048);
    }

    #[test]
    fn test_health_fallback_to_models() {
        // 存在しないサーバーでは /health も /v1/models も失敗 → false
        let b = LlamaServerBackend::connect("http://127.0.0.1:19998", "test");
        assert!(!b.is_healthy());
    }

}
