use serde::{Deserialize, Serialize};

/// メッセージの役割
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// マルチモーダル添付ファイル（Gemma 4対応）
#[derive(Debug, Clone)]
pub enum Attachment {
    Image(Vec<u8>),
}

/// 会話メッセージ
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    #[serde(skip)]
    pub attachments: Vec<Attachment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            attachments: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            attachments: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            attachments: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn tool(content: impl Into<String>, tool_call_id: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            attachments: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    pub fn has_image(&self) -> bool {
        self.attachments.iter().any(|a| matches!(a, Attachment::Image(_)))
    }
}

/// パースされたツール呼び出し
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

/// LLM出力のパース結果
#[derive(Debug, Clone)]
pub struct ParsedOutput {
    /// `<think>` タグ内の自由形式推論
    pub thinking: Option<String>,
    /// `<tool_call>` タグ内のツール呼び出し
    pub tool_calls: Vec<ToolCall>,
    /// 最終回答テキスト
    pub text: Option<String>,
}

/// セッション（会話履歴の単位）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub messages: Vec<Message>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub summary: Option<String>,
}

impl Session {
    pub fn new() -> Self {
        let now = chrono::Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            messages: Vec::new(),
            created_at: now,
            updated_at: now,
            summary: None,
        }
    }

    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
        self.updated_at = chrono::Utc::now();
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_system() {
        let msg = Message::system("あなたはAIアシスタントです");
        assert_eq!(msg.role, Role::System);
        assert_eq!(msg.content, "あなたはAIアシスタントです");
        assert!(msg.tool_call_id.is_none());
    }

    #[test]
    fn test_message_user() {
        let msg = Message::user("こんにちは");
        assert_eq!(msg.role, Role::User);
        assert!(!msg.has_image());
    }

    #[test]
    fn test_message_with_image() {
        let mut msg = Message::user("この画像は何？");
        msg.attachments.push(Attachment::Image(vec![0xFF, 0xD8]));
        assert!(msg.has_image());
    }

    #[test]
    fn test_message_tool() {
        let msg = Message::tool("結果", "call_123");
        assert_eq!(msg.role, Role::Tool);
        assert_eq!(msg.tool_call_id, Some("call_123".to_string()));
    }

    #[test]
    fn test_tool_call_serialization() {
        let call = ToolCall {
            name: "shell".to_string(),
            arguments: serde_json::json!({"command": "ls"}),
        };
        let json = serde_json::to_string(&call).unwrap();
        let deserialized: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "shell");
    }

    #[test]
    fn test_session_new() {
        let session = Session::new();
        assert!(session.messages.is_empty());
        assert!(session.summary.is_none());
        assert!(!session.id.is_empty());
    }

    #[test]
    fn test_session_add_message() {
        let mut session = Session::new();
        let before = session.updated_at;
        session.add_message(Message::user("テスト"));
        assert_eq!(session.messages.len(), 1);
        assert!(session.updated_at >= before);
    }

    #[test]
    fn test_role_serialization() {
        let json = serde_json::to_string(&Role::Assistant).unwrap();
        assert_eq!(json, "\"assistant\"");
        let role: Role = serde_json::from_str("\"tool\"").unwrap();
        assert_eq!(role, Role::Tool);
    }
}
