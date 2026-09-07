//! CLI 引数パース定義

use clap::Parser;

#[derive(Parser)]
#[command(name = "bonsai-agent", version, about = "Bonsai-8B自律型エージェント")]
pub struct Cli {
    /// llama-serverのURL（デフォルト: http://localhost:8080）
    #[arg(long, default_value = "http://localhost:8080")]
    pub server_url: String,

    /// サーバーバックエンド (llama-server, mlx-lm, bitnet, unsloth)
    #[arg(long)]
    pub backend: Option<String>,

    /// APIキー (Bearer認証)
    #[arg(long)]
    pub api_key: Option<String>,

    /// モデルID
    #[arg(long)]
    pub model: Option<String>,

    /// 単発実行モード
    #[arg(long)]
    pub exec: Option<String>,

    /// 自律性レベル (readonly, supervised, full)
    #[arg(long)]
    pub autonomy: Option<String>,

    /// モックモード（LLMなしでテスト）
    #[arg(long)]
    pub mock: bool,

    /// セッション一覧を表示
    #[arg(long)]
    pub sessions: bool,

    /// 過去セッションを再開（セッションIDの先頭数文字でOK）
    #[arg(long)]
    pub resume: Option<String>,

    /// 監査ログを表示
    #[arg(long)]
    pub audit: bool,

    /// 未完了タスク一覧
    #[arg(long)]
    pub tasks: bool,

    /// ナレッジVault概要
    #[arg(long)]
    pub vault: bool,

    /// 設定ファイルを初期生成（~/.config/bonsai-agent/config.toml）
    #[arg(long)]
    pub init: bool,

    /// ケイパビリティ一覧
    #[arg(long)]
    pub manifest: bool,

    /// 登録ツール一覧を表示（whitelist 適用後の live registry、BONSAI_ENABLED_TOOLS/LAB_SMOKE 反映）
    #[arg(long)]
    pub list_tools: bool,

    /// arxiv収集+自己改善
    #[arg(long)]
    pub evolve: bool,

    /// REST APIサーバー
    #[arg(long)]
    pub serve: bool,

    /// APIポート
    #[arg(long, default_value = "3030")]
    pub api_port: u16,

    /// MCPサーバー
    #[arg(long)]
    pub mcp_server: bool,

    /// 実験ループ（自律的自己改善）
    #[arg(long)]
    pub lab: bool,

    /// 実験回数上限
    #[arg(long, default_value = "10")]
    pub lab_experiments: usize,

    /// ダッシュボード（advisor/checkpoint/実験統計）
    #[arg(long)]
    pub dashboard: bool,

    /// チェックポイント一覧
    #[arg(long)]
    pub checkpoints: bool,

    /// 指定IDのチェックポイントにロールバック
    #[arg(long)]
    pub rollback: Option<i64>,

    /// サーバー診断（接続・モデル・推論テスト）
    #[arg(long)]
    pub diagnose: bool,

    /// スキルをMarkdownにエクスポート（デフォルト: SKILLS.md）
    #[arg(long)]
    pub skills_export: bool,

    /// ファイル/ディレクトリ(.md/.txt)を memory に取り込む（①知識デーモン Phase 2）
    #[arg(long, value_name = "PATH")]
    pub ingest: Option<std::path::PathBuf>,

    /// --ingest と併用: 取込後、対象 dir から削除されたファイルの孤児 chunk を掃除する
    #[arg(long)]
    pub ingest_prune: bool,

    /// 記憶・知識グラフの力学可視化HTMLを生成
    #[arg(long)]
    pub visualize: bool,

    /// 可視化HTMLの出力パス（デフォルト: memory_graph.html）
    #[arg(long, default_value = "memory_graph.html")]
    pub visualize_output: String,

    /// 可視化HTML生成後にデフォルトブラウザで開く
    #[arg(long)]
    pub visualize_open: bool,

    /// 可視化データからプライバシーサニタイズを無効化（非推奨）
    #[arg(long)]
    pub no_privacy: bool,
}
