//! サーバー診断モード (接続・モデル・推論テスト)

use anyhow::Result;
use bonsai_agent::config::{AppConfig, ServerBackend};
use bonsai_agent::runtime::http_agent::{shared_agent, short_agent};

pub fn handle_diagnose_mode(server_url: &str, app_config: &AppConfig) -> Result<()> {
    println!("╔══════════════════════════════════════════╗");
    println!("║       bonsai-agent サーバー診断            ║");
    println!("╚══════════════════════════════════════════╝");

    let backend_name = match app_config.model.backend {
        ServerBackend::LlamaServer => "llama-server",
        ServerBackend::MlxLm => "mlx-lm",
        ServerBackend::BitNet => "bitnet.cpp",
        ServerBackend::Unsloth => "unsloth",
    };
    let mlx_compat = app_config.model.backend == ServerBackend::MlxLm;

    println!("\n📋 設定:");
    println!("  server_url: {}", server_url);
    println!("  backend: {}", backend_name);
    println!("  model_id: {}", app_config.model.model_id);
    println!("  context_length: {}", app_config.model.context_length);
    println!("  mlx_compatible: {}", mlx_compat);

    let inf = &app_config.model.inference;
    println!("\n⚙️  InferenceParams:");
    println!("  temperature: {}", inf.temperature);
    println!("  top_p: {}", inf.top_p);
    println!("  top_k: {}", inf.top_k);
    println!("  min_p: {}", inf.min_p);
    println!("  max_tokens: {}", inf.max_tokens);
    println!("  repeat_penalty: {}", inf.repeat_penalty);

    // 接続テスト
    println!("\n🔌 接続テスト...");
    let health_url = format!("{}/health", server_url);
    let models_url = format!("{}/v1/models", server_url);

    let agent = short_agent();
    let mut models_req = agent.get(&models_url);
    if let Some(ref key) = app_config.model.api_key {
        models_req = models_req.header("Authorization", format!("Bearer {key}"));
    }
    let health_ok = agent.get(&health_url).call().is_ok();
    let models_resp = models_req.call();

    if health_ok {
        println!("  /health: ✓ OK");
    } else {
        println!("  /health: ✗ 応答なし");
    }

    // モデル一覧取得
    match models_resp {
        Ok(resp) => {
            println!("  /v1/models: ✓ OK");
            if let Ok(body) = resp.into_body().read_to_string()
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&body)
                && let Some(data) = json.get("data").and_then(|d| d.as_array())
            {
                println!("\n📦 モデル一覧:");
                for model in data {
                    if let Some(id) = model.get("id").and_then(|v| v.as_str()) {
                        println!("  - {}", id);
                    }
                }
            }
        }
        Err(e) => {
            println!("  /v1/models: ✗ エラー ({})", e);
            println!("\nサーバーに接続できません。llama-serverが起動しているか確認してください。");
            return Ok(());
        }
    }

    println!("\n🧪 テストプロンプト: \"1+1=\"");
    let chat_url = format!("{}/v1/chat/completions", server_url);
    let request_body = serde_json::json!({
        "model": app_config.model.model_id,
        "messages": [{"role": "user", "content": "1+1="}],
        "temperature": inf.temperature,
        "max_tokens": 32_u32,
        "stream": false,
    });

    let start = std::time::Instant::now();
    let mut chat_req = shared_agent()
        .post(&chat_url)
        .header("Content-Type", "application/json");
    if let Some(ref key) = app_config.model.api_key {
        chat_req = chat_req.header("Authorization", format!("Bearer {key}"));
    }
    match chat_req.send_json(&request_body) {
        Ok(resp) => {
            let elapsed = start.elapsed();
            if let Ok(body) = resp.into_body().read_to_string()
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&body)
            {
                let answer = json["choices"][0]["message"]["content"]
                    .as_str()
                    .unwrap_or("(応答なし)");
                let prompt_tokens = json["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
                let completion_tokens = json["usage"]["completion_tokens"].as_u64().unwrap_or(0);
                println!("  応答: {}", answer.trim());
                println!("  応答時間: {:.1}ms", elapsed.as_secs_f64() * 1000.0);
                println!(
                    "  トークン数: prompt={}, completion={}",
                    prompt_tokens, completion_tokens
                );
            } else {
                println!("  応答パースエラー");
            }
        }
        Err(e) => {
            println!("  テストプロンプト失敗: {}", e);
        }
    }

    // 自律知能・認知サブシステムのメトリクス表示
    print_cognitive_dashboard(app_config);

    println!("\n診断完了。");
    Ok(())
}

/// 自律知能・認知サブシステム (DMN / Dreaming / MAGI / A-MEM) の健全性・稼働メトリクスを表示
pub fn print_cognitive_dashboard(_app_config: &AppConfig) {
    println!("\n🧠 自律知能・認知サブシステム診断 (DMN / Dreaming / MAGI / A-MEM):");

    let env_override = std::env::var("BONSAI_DB_PATH").ok();
    let db_path_buf =
        bonsai_agent::config::resolve_db_path(env_override.as_deref(), dirs::data_dir());

    if !db_path_buf.exists() {
        println!(
            "  ローカル記憶ストア: 未初期化 (初回対話後に作成されます: {})",
            db_path_buf.display()
        );
        return;
    }

    println!("  記憶ストア: {}", db_path_buf.display());

    let Ok(store) =
        bonsai_agent::memory::store::MemoryStore::open(db_path_buf.to_str().unwrap_or(""))
    else {
        println!("  記憶ストアのオープンに失敗しました");
        return;
    };

    let metrics = collect_cognitive_metrics(&store);

    let default_vault = dirs::data_dir().map(|d| d.join("bonsai-agent").join("vault"));

    let vault_insights = default_vault
        .map(|vp| {
            let p = vp.join("insights.md");
            if p.exists() {
                std::fs::read_to_string(p)
                    .map(|c| c.lines().filter(|l| l.starts_with("- ")).count())
                    .unwrap_or(0)
            } else {
                0
            }
        })
        .unwrap_or(0);

    println!("  [DMN 自発思考ループ]:");
    println!("    - 内省エピソード蓄積: {} 件", metrics.dmn_reflections);
    println!("    - ナレッジVault還元洞察: {} 件", vault_insights);
    println!("  [Dreaming メタ認知]:");
    println!("    - 長時間アイドル記憶統合: {} 回", metrics.deep_dreams);
    println!("  [MAGI 三重監視]:");
    println!("    - 監査イベント総数: {} 件", metrics.audit_events);
    println!("    - 遮断・是正 (Block): {} 件", metrics.magi_blocked);
    println!("    - 警告・指導 (Warn): {} 件", metrics.magi_warned);
    println!("  [A-MEM 統合記憶]:");
    println!("    - 蓄積記憶レコード: {} 件", metrics.memories);
    println!("    - 連想リンク (Links): {} 件", metrics.memory_links);
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct CognitiveMetrics {
    pub dmn_reflections: i64,
    pub deep_dreams: i64,
    pub audit_events: i64,
    pub magi_blocked: i64,
    pub magi_warned: i64,
    pub memories: i64,
    pub memory_links: i64,
}

pub fn collect_cognitive_metrics(
    store: &bonsai_agent::memory::store::MemoryStore,
) -> CognitiveMetrics {
    let conn = store.conn();
    let dmn_reflections: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM experiences WHERE task_context = 'DMN自発思考ループ'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let deep_dreams: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM experiences WHERE task_context = 'DMN自律Dreaming'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let memories: i64 = conn
        .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
        .unwrap_or(0);

    let memory_links: i64 = conn
        .query_row("SELECT COUNT(*) FROM memory_links", [], |r| r.get(0))
        .unwrap_or(0);

    let audit_events: i64 = conn
        .query_row("SELECT COUNT(*) FROM audit_log", [], |r| r.get(0))
        .unwrap_or(0);

    let magi_blocked: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM audit_log WHERE outcome LIKE '%block%' OR outcome LIKE '%Block%'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let magi_warned: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM audit_log WHERE outcome LIKE '%warn%' OR outcome LIKE '%Warn%'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    CognitiveMetrics {
        dmn_reflections,
        deep_dreams,
        audit_events,
        magi_blocked,
        magi_warned,
        memories,
        memory_links,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bonsai_agent::memory::experience::{ExperienceStore, ExperienceType, RecordParams};
    use bonsai_agent::memory::store::MemoryStore;

    #[test]
    fn test_collect_cognitive_metrics() {
        let store = MemoryStore::in_memory().unwrap();
        let exp_store = ExperienceStore::new(store.conn());

        // DMN 経験を記録
        exp_store
            .record(&RecordParams {
                exp_type: ExperienceType::Insight,
                task_context: "DMN自発思考ループ",
                action: "internal_reflection",
                outcome: "テスト洞察",
                lesson: None,
                tool_name: None,
                error_type: None,
                error_detail: None,
            })
            .unwrap();

        // Dreaming 経験を記録
        exp_store
            .record(&RecordParams {
                exp_type: ExperienceType::Insight,
                task_context: "DMN自律Dreaming",
                action: "deep_dream_consolidation",
                outcome: "テストDream",
                lesson: None,
                tool_name: None,
                error_type: None,
                error_detail: None,
            })
            .unwrap();

        let metrics = collect_cognitive_metrics(&store);
        assert_eq!(metrics.dmn_reflections, 1);
        assert_eq!(metrics.deep_dreams, 1);
        assert_eq!(metrics.memories, 0);
    }
}
