use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ToolSpec;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderContentBlock {
    Text {
        text: String,
    },
    File {
        filename: String,
        content: String,
    },
    Image {
        source_name: String,
        media_type: String,
        data_base64: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderMessage {
    pub role: ProviderRole,
    pub content: Vec<ProviderContentBlock>,
}

impl ProviderMessage {
    pub fn new_text(role: ProviderRole, text: impl Into<String>) -> Self {
        Self {
            role,
            content: vec![ProviderContentBlock::Text { text: text.into() }],
        }
    }

    pub fn new_blocks(role: ProviderRole, content: Vec<ProviderContentBlock>) -> Self {
        Self { role, content }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<ProviderMessage>,
    pub tools: Vec<ProviderToolSpec>,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderResponse {
    pub content: Vec<ProviderContentBlock>,
    pub stop_reason: Option<ProviderStopReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderToolSpec {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
}

impl From<ToolSpec> for ProviderToolSpec {
    fn from(value: ToolSpec) -> Self {
        Self {
            name: value.name,
            description: value.description,
            input_schema: value.input_schema,
        }
    }
}

pub fn extract_text_from_blocks(content: &[ProviderContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ProviderContentBlock::Text { text } => Some(text.as_str()),
            ProviderContentBlock::File { .. }
            | ProviderContentBlock::Image { .. }
            | ProviderContentBlock::ToolUse { .. }
            | ProviderContentBlock::ToolResult { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ProviderContentBlock, ProviderMessage, ProviderRole, extract_text_from_blocks};

    #[test]
    fn serializes_file_and_image_blocks() {
        let file_block = ProviderContentBlock::File {
            filename: "agent.toml".to_string(),
            content: "provider = \"openai\"".to_string(),
        };
        let image_block = ProviderContentBlock::Image {
            source_name: "diagram.png".to_string(),
            media_type: "image/png".to_string(),
            data_base64: "ZmFrZQ==".to_string(),
        };

        let file_json = serde_json::to_value(&file_block).unwrap();
        let image_json = serde_json::to_value(&image_block).unwrap();

        assert_eq!(file_json["type"], "file");
        assert_eq!(file_json["filename"], "agent.toml");
        assert_eq!(image_json["type"], "image");
        assert_eq!(image_json["media_type"], "image/png");
    }

    #[test]
    fn extracts_text_from_internal_blocks() {
        let message = ProviderMessage {
            role: ProviderRole::Assistant,
            content: vec![
                ProviderContentBlock::Text {
                    text: "first".to_string(),
                },
                ProviderContentBlock::Text {
                    text: "second".to_string(),
                },
            ],
        };

        let text = extract_text_from_blocks(&message.content);

        assert_eq!(text, "first\nsecond");
    }

    #[test]
    fn ignores_non_text_blocks_when_extracting_text() {
        let content = vec![
            ProviderContentBlock::Text {
                text: "before".to_string(),
            },
            ProviderContentBlock::File {
                filename: "notes.md".to_string(),
                content: "secret".to_string(),
            },
            ProviderContentBlock::Image {
                source_name: "diagram.png".to_string(),
                media_type: "image/png".to_string(),
                data_base64: "ZmFrZQ==".to_string(),
            },
            ProviderContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "read_file".to_string(),
                input: json!({"path": "Cargo.toml"}),
            },
            ProviderContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "file contents".to_string(),
            },
            ProviderContentBlock::Text {
                text: "after".to_string(),
            },
        ];

        let text = extract_text_from_blocks(&content);

        assert_eq!(text, "before\nafter");
    }
}
