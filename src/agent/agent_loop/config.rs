//! AgentConfig 設定型と TaskType 別推論パラメータ派生
//!
//! 元 `agent_loop.rs` から分離（refactor commit 2/8）。
//! 公開 API: `AgentConfig`, `inference_for_task`（mod.rs から `pub use`）。

use crate::config::InferenceParams;
use crate::runtime::model_router::AdvisorConfig;
use crate::tools::TaskType;

/// エージェント設定
#[derive(Clone)]
pub struct AgentConfig {
    pub max_iterations: usize,
    pub max_retries: usize,
    pub max_tools_selected: usize,
    pub system_prompt: String,
    /// アドバイザー設定（完了前自己検証の呼び出し回数を制御）
    pub advisor: AdvisorConfig,
    /// タスク開始時に自動チェックポイント作成（git stash + DB永続化）
    pub auto_checkpoint: bool,
    /// ツール出力の最大文字数（超過分は切り詰め、コンテキスト節約）
    pub max_tool_output_chars: usize,
    /// コンテキストに含めるツールの最大数
    pub max_tools_in_context: usize,
    /// MCPツールの追加枠（ビルトインとは別枠）
    pub max_mcp_tools_in_context: usize,
    /// ベース推論パラメータ（TaskTypeで動的調整）
    pub base_inference: InferenceParams,
    /// タスク単位のウォールクロックタイムアウト（None=無制限）
    pub task_timeout: Option<std::time::Duration>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_iterations: 10,
            max_retries: 3,
            max_tools_selected: 5,
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            advisor: AdvisorConfig::default(),
            auto_checkpoint: true,
            max_tool_output_chars: 4000,
            max_tools_in_context: 8,
            max_mcp_tools_in_context: 3,
            base_inference: InferenceParams::default(),
            task_timeout: None,
        }
    }
}

/// タスク種別に応じた推論パラメータを導出
pub fn inference_for_task(task_type: TaskType, base: &InferenceParams) -> InferenceParams {
    let mut params = base.clone();
    match task_type {
        TaskType::FileOperation | TaskType::CodeExecution => {
            params.temperature = 0.3; // 精密操作
        }
        TaskType::Research => {
            params.temperature = 0.6; // 探索的
        }
        TaskType::General => {} // ベースのまま
    }
    params
}

/// 1ビットモデル向けに最適化されたシステムプロンプト。
/// arxiv知見: スキーマファースト（ツール定義をプロンプト先頭に配置）、
/// 簡潔な指示、明確なフォーマット例が小型モデルの精度を最大化する。
const DEFAULT_SYSTEM_PROMPT: &str = r#"あなたはbonsai-agent、ローカルで動作する自律型AIアシスタントです。

## ツールの使い方

ツールを呼び出すには、以下のXML形式を使ってください:

<tool_call>{"name": "ツール名", "arguments": {"パラメータ名": "値"}}</tool_call>

### 例

ファイルを読む:
<tool_call>{"name": "file_read", "arguments": {"path": "README.md"}}</tool_call>

コマンドを実行する:
<tool_call>{"name": "shell", "arguments": {"command": "ls -la"}}</tool_call>

ファイルの一部を編集する:
<tool_call>{"name": "file_write", "arguments": {"path": "main.rs", "old_text": "hello", "new_text": "world"}}</tool_call>

Gitの状態を確認する:
<tool_call>{"name": "git", "arguments": {"subcommand": "status"}}</tool_call>

## ルール

1. 回答は簡潔にする。聞かれたことだけ答える
2. 同じ内容を繰り返さない
3. 日本語で回答する
4. 考える必要があれば <think>ここに思考</think> タグを使う
5. ツール呼び出しのJSONは正しい形式にする
6. ツール結果を元に簡潔に回答する
7. わからないことは「わからない」と答える
8. 「検索して」→ web_search。URLが分かっている時だけ web_fetch
9. 複数ステップが必要な場合、まず計画を <think> に書いてから実行する
10. ツールを使う前に必ず <think> で意図と期待結果を書く
11. ツール結果を確認せずに内容を主張しない。「たぶん」「おそらく」は使わない
12. 同じファイルを連続で再読込しない。前回の結果を使う
13. ツール結果が期待と違う場合、別のツールを試す
14. <tool_persistence>ツールが使える場面では必ずツールを使い、推測で回答しない</tool_persistence>
15. 回答を出す前にファイルの内容を確認する。未読のファイルについて断定しない
"#;
