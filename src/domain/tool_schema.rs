//! ツールスキーマ DTO — tools/runtime/prompt/cache 間で共有される中立的な能力記述。
//! Tool trait (振る舞い) は tools 層に残す。ここは「何を」だけを表す純粋 DTO。

use serde::{Deserialize, Serialize};

/// ツールのスキーマ情報（LLMのシステムプロンプトに注入する）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}
