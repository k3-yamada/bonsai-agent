use serde::Deserialize;

/// MLX server `/props` エンドポイントのレスポンス形式
#[derive(Debug, Deserialize, Default)]
struct ServerProps {
    n_ctx: Option<u32>,
}

/// server の `/props` エンドポイントから `n_ctx` を取得する。
///
/// サーバーが到達不能または `n_ctx` フィールドが存在しない場合は `None` を返す。
/// HTTP エラー・JSON パースエラーはすべて `None` として扱い、呼び出し元に伝播しない。
///
/// **実機 finding (2026-06-04)**: `/props` は **llama.cpp 専用**エンドポイント。
/// 現行 MLX backend (mlx-openai-server) は `/props` 404 + `/v1/models` にも
/// `context_length` 非搭載のため、B-3 auto-clamp は **MLX backend では永久 no-op**。
/// llama.cpp backend (fallback chain) 使用時のみ有効。env-gated かつ None で無害なため残置。
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
}
