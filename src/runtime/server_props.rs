use serde::Deserialize;

/// MLX server `/props` エンドポイントのレスポンス形式
#[derive(Debug, Deserialize, Default)]
struct ServerProps {
    n_ctx: Option<u32>,
}

/// HuggingFace 形式 `config.json` (model card) の関連フィールド。
///
/// MLX / llama.cpp どちらの model でも標準で持つ `max_position_embeddings` を
/// context 上限の source とする。multimodal 等の `text_config` ネストは対象外。
#[derive(Debug, Deserialize, Default)]
struct ModelCard {
    max_position_embeddings: Option<u32>,
}

/// server の `/props` エンドポイントから `n_ctx` を取得する。
///
/// サーバーが到達不能または `n_ctx` フィールドが存在しない場合は `None` を返す。
/// HTTP エラー・JSON パースエラーはすべて `None` として扱い、呼び出し元に伝播しない。
///
/// **実機 finding (2026-06-04)**: `/props` は **llama.cpp 専用**エンドポイント。
/// 現行 MLX backend (mlx-openai-server) は `/props` 404 + `/v1/models` にも
/// `context_length` 非搭載のため、この関数は MLX backend では常に `None`。
/// MLX 用の context 取得は [`fetch_model_ctx_from_card`] が担い、両者は
/// [`resolve_server_n_ctx`] で 2 段 fallback として統合される。
pub fn fetch_server_n_ctx(server_url: &str) -> Option<u32> {
    let agent = crate::runtime::http_agent::short_agent();
    let url = format!("{}/props", server_url.trim_end_matches('/'));
    let mut resp = agent.get(&url).call().ok()?;
    let props: ServerProps = resp.body_mut().read_json().ok()?;
    props.n_ctx
}

/// `configured` と `server_n_ctx` の小さい方を返す。
///
/// `server_n_ctx` が `None`（サーバー未応答）の場合は `configured` をそのまま返す。
pub fn clamp_context_to_server(configured: u32, server_n_ctx: Option<u32>) -> u32 {
    match server_n_ctx {
        Some(n) => configured.min(n),
        None => configured,
    }
}

/// model card (`{model_path}/config.json`) の `max_position_embeddings` を読み取る。
///
/// MLX backend (mlx-openai-server) は `/props` を持たないため、ローカル model dir の
/// `config.json` を直接読んで context 上限を得る fallback。`model_path` がローカル
/// ディレクトリでない (HF repo id 等) / `config.json` 不在 / パース失敗 / フィールド不在は
/// すべて `None`。呼び出し元に error を伝播しない (B-3 の no-op セマンティクス保持)。
pub fn fetch_model_ctx_from_card(model_path: &str) -> Option<u32> {
    if model_path.is_empty() {
        return None;
    }
    let config_path = std::path::Path::new(model_path).join("config.json");
    let raw = std::fs::read_to_string(&config_path).ok()?;
    let card: ModelCard = serde_json::from_str(&raw).ok()?;
    card.max_position_embeddings
}

/// server context 上限を解決する。`/props` (llama.cpp) を優先し、`None` のとき
/// model card (`config.json`、MLX 含む) に fallback する。
///
/// **B-3 (LocalAI fit_params 思想)**: llama.cpp は `/props`、MLX はローカル model dir の
/// `config.json` から context 上限を得る 2 段 fallback。両方失敗で `None` → clamp は no-op。
pub fn resolve_server_n_ctx(server_url: &str, model_path: &str) -> Option<u32> {
    fetch_server_n_ctx(server_url).or_else(|| fetch_model_ctx_from_card(model_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_clamp_respects_server_limit() {
        assert_eq!(clamp_context_to_server(16384, Some(4096)), 4096);
    }

    #[test]
    fn t_clamp_keeps_configured_when_server_is_larger() {
        assert_eq!(clamp_context_to_server(4096, Some(32768)), 4096);
    }

    #[test]
    fn t_clamp_no_op_when_server_unavailable() {
        assert_eq!(clamp_context_to_server(14000, None), 14000);
    }

    #[test]
    fn t_clamp_equal_values() {
        assert_eq!(clamp_context_to_server(8192, Some(8192)), 8192);
    }

    #[test]
    fn t_is_mlx_auto_clamp_off_by_default() {
        // SAFETY: テスト専用スレッド、他スレッドからの同 env 変数アクセスなし
        unsafe { std::env::remove_var("BONSAI_MLX_AUTO_CLAMP") };
        assert!(!crate::config::is_mlx_auto_clamp());
    }

    // ── B-3 follow-up: model card fallback ──

    fn write_temp_config(dir: &std::path::Path, body: &str) {
        std::fs::write(dir.join("config.json"), body).unwrap();
    }

    #[test]
    fn t_fetch_model_ctx_from_card_reads_max_position_embeddings() {
        let tmp = std::env::temp_dir().join(format!("bonsai_card_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        write_temp_config(&tmp, r#"{"max_position_embeddings": 32768}"#);
        let got = fetch_model_ctx_from_card(tmp.to_str().unwrap());
        std::fs::remove_dir_all(&tmp).ok();
        assert_eq!(got, Some(32768));
    }

    #[test]
    fn t_fetch_model_ctx_from_card_none_when_field_absent() {
        let tmp = std::env::temp_dir().join(format!("bonsai_card_noctx_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        write_temp_config(&tmp, r#"{"hidden_size": 4096}"#);
        let got = fetch_model_ctx_from_card(tmp.to_str().unwrap());
        std::fs::remove_dir_all(&tmp).ok();
        assert_eq!(got, None, "max_position_embeddings 不在で None");
    }

    #[test]
    fn t_fetch_model_ctx_from_card_none_when_missing_dir() {
        assert_eq!(
            fetch_model_ctx_from_card("/nonexistent/path/xyz"),
            None,
            "存在しない dir で None"
        );
    }

    #[test]
    fn t_fetch_model_ctx_from_card_none_when_empty_path() {
        assert_eq!(fetch_model_ctx_from_card(""), None, "空 path で None");
    }

    #[test]
    fn t_resolve_falls_back_to_card_when_server_unreachable() {
        // 到達不可 server → /props None → model card にフォールバック
        let tmp = std::env::temp_dir().join(format!("bonsai_resolve_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        write_temp_config(&tmp, r#"{"max_position_embeddings": 8192}"#);
        let got = resolve_server_n_ctx("http://127.0.0.1:1", tmp.to_str().unwrap());
        std::fs::remove_dir_all(&tmp).ok();
        assert_eq!(got, Some(8192), "server 不応答時 card にフォールバック");
    }

    #[test]
    fn t_resolve_none_when_both_unavailable() {
        assert_eq!(
            resolve_server_n_ctx("http://127.0.0.1:1", "/nonexistent/xyz"),
            None,
            "server 不応答 + card 不在で None (clamp no-op)"
        );
    }
}
