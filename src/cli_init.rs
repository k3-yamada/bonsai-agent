//! 初期設定生成モード

use anyhow::Result;
use bonsai_agent::config::AppConfig;

pub fn handle_init_mode() -> Result<()> {
    let path = AppConfig::config_path();
    if path.exists() {
        println!("設定ファイルが既に存在します: {}", path.display());
        println!("上書きする場合は手動で削除してください。");
        return Ok(());
    }
    let path = AppConfig::save_default()?;
    println!("設定ファイルを生成しました: {}", path.display());
    println!();
    println!("Advisor API 設定例（config.toml の [advisor] セクション）:");
    println!("  [advisor]");
    println!("  api_endpoint = \"https://api.openai.com/v1/chat/completions\"");
    println!("  api_model = \"gpt-4o-mini\"");
    println!("  # api_key は環境変数 OPENAI_API_KEY から自動検出");
    println!();
    println!("ローカルLLMアドバイザー例:");
    println!("  [advisor]");
    println!("  api_endpoint = \"http://127.0.0.1:8081/v1/chat/completions\"");
    Ok(())
}
