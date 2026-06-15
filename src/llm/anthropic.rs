use std::sync::Arc;

use anthropic_ai_sdk::{
    client::{AnthropicClient, AnthropicClientBuilder},
    types::message::{
        ContentBlock, CreateMessageParams, ImageSource, Message, MessageClient, MessageError,
        RequiredMessageParams, Role, StopReason,
    },
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;

use crate::config::AgentConfig;

use super::{
    LlmProvider, ProviderContentBlock, ProviderMessage, ProviderRequest, ProviderResponse,
    ProviderRole, ProviderStopReason,
};

pub struct AnthropicProvider {
    client: AnthropicClient,
}

impl AnthropicProvider {
    pub fn from_config(config: Arc<AgentConfig>) -> Result<Self> {
        let anthropic = config
            .anthropic
            .as_ref()
            .context("anthropic config is required to create Anthropic provider")?;
        let client = AnthropicClientBuilder::new(anthropic.api_key.clone(), "")
            .with_api_base_url(anthropic.base_url.clone())
            .build::<MessageError>()
            .context("can't create Anthropic client")?;
        Ok(Self { client })
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    async fn send(&self, request: ProviderRequest) -> Result<ProviderResponse> {
        let params = CreateMessageParams::new(RequiredMessageParams {
            model: request.model,
            messages: request
                .messages
                .into_iter()
                .map(provider_message_to_anthropic)
                .collect::<Result<Vec<_>>>()?,
            max_tokens: request.max_tokens,
        });
        let params = match request.system {
            Some(system) => params.with_system(&system),
            None => params,
        };
        let params = params.with_tools(
            request
                .tools
                .into_iter()
                .map(provider_tool_to_anthropic)
                .collect(),
        );

        let response = self.client.create_message(Some(&params)).await?;
        Ok(ProviderResponse {
            content: response
                .content
                .into_iter()
                .map(anthropic_block_to_provider)
                .collect::<Result<Vec<_>>>()?,
            stop_reason: response.stop_reason.map(stop_reason_from_anthropic),
        })
    }
}

fn provider_message_to_anthropic(message: ProviderMessage) -> Result<Message> {
    let ProviderMessage {
        role: provider_role,
        content,
    } = message;
    let role = provider_role_to_anthropic(provider_role.clone())?;
    let should_use_text_message = matches!(content.as_slice(), [ProviderContentBlock::Text { .. }]);
    let blocks = content
        .into_iter()
        .map(|block| provider_block_to_anthropic(&provider_role, block))
        .collect::<Result<Vec<_>>>()?;

    if should_use_text_message {
        let [ContentBlock::Text { text }] = blocks.as_slice() else {
            bail!("expected a single Anthropic text block for text-only message");
        };
        Ok(Message::new_text(role, text.clone()))
    } else {
        Ok(Message::new_blocks(role, blocks))
    }
}

fn provider_role_to_anthropic(role: ProviderRole) -> Result<Role> {
    match role {
        ProviderRole::User | ProviderRole::Tool => Ok(Role::User),
        ProviderRole::Assistant => Ok(Role::Assistant),
        ProviderRole::System => bail!("Anthropic provider does not accept system messages inline"),
    }
}

fn provider_block_to_anthropic(
    role: &ProviderRole,
    block: ProviderContentBlock,
) -> Result<ContentBlock> {
    match block {
        ProviderContentBlock::Text { text } => Ok(ContentBlock::Text { text }),
        ProviderContentBlock::File { filename, content } => {
            if *role != ProviderRole::User {
                bail!("only user messages can contain file attachments for Anthropic APIs");
            }

            Ok(ContentBlock::Text {
                text: format!(
                    "[Attached file: {filename}]\n<file-content>\n{content}\n</file-content>"
                ),
            })
        }
        ProviderContentBlock::Image {
            media_type,
            data_base64,
            ..
        } => {
            if *role != ProviderRole::User {
                bail!("only user messages can contain image attachments for Anthropic APIs");
            }

            Ok(ContentBlock::Image {
                source: ImageSource {
                    type_: "base64".to_string(),
                    media_type,
                    data: data_base64,
                },
            })
        }
        ProviderContentBlock::ToolUse { id, name, input } => {
            Ok(ContentBlock::ToolUse { id, name, input })
        }
        ProviderContentBlock::ToolResult {
            tool_use_id,
            content,
        } => Ok(ContentBlock::ToolResult {
            tool_use_id,
            content,
        }),
    }
}

fn provider_tool_to_anthropic(tool: super::types::ProviderToolSpec) -> crate::ToolSpec {
    crate::ToolSpec {
        name: tool.name,
        description: tool.description,
        input_schema: tool.input_schema,
    }
}

fn anthropic_block_to_provider(block: ContentBlock) -> Result<ProviderContentBlock> {
    match block {
        ContentBlock::Text { text } => Ok(ProviderContentBlock::Text { text }),
        ContentBlock::ToolUse { id, name, input } => {
            Ok(ProviderContentBlock::ToolUse { id, name, input })
        }
        ContentBlock::ToolResult {
            tool_use_id,
            content,
        } => Ok(ProviderContentBlock::ToolResult {
            tool_use_id,
            content,
        }),
        other => bail!("unsupported Anthropic content block: {other:?}"),
    }
}

fn stop_reason_from_anthropic(stop_reason: StopReason) -> ProviderStopReason {
    match stop_reason {
        StopReason::EndTurn => ProviderStopReason::EndTurn,
        StopReason::ToolUse => ProviderStopReason::ToolUse,
        StopReason::MaxTokens => ProviderStopReason::MaxTokens,
        _ => ProviderStopReason::EndTurn,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::provider_message_to_anthropic;
    use crate::llm::{ProviderContentBlock, ProviderMessage, ProviderRole};

    #[test]
    fn maps_user_image_attachment_to_anthropic_image_block() {
        let message = ProviderMessage::new_blocks(
            ProviderRole::User,
            vec![ProviderContentBlock::Image {
                source_name: "diagram.png".to_string(),
                media_type: "image/png".to_string(),
                data_base64: "ZmFrZQ==".to_string(),
            }],
        );

        let mapped = provider_message_to_anthropic(message).unwrap();
        let serialized = serde_json::to_value(&mapped).unwrap();

        assert_eq!(serialized["role"], "user");
        assert_eq!(serialized["content"][0]["type"], "image");
        assert_eq!(serialized["content"][0]["source"]["type"], "base64");
        assert_eq!(serialized["content"][0]["source"]["media_type"], "image/png");
        assert_eq!(serialized["content"][0]["source"]["data"], "ZmFrZQ==");
    }

    #[test]
    fn maps_user_file_attachment_to_anthropic_text_block() {
        let message = ProviderMessage::new_blocks(
            ProviderRole::User,
            vec![ProviderContentBlock::File {
                filename: "agent.toml".to_string(),
                content: "provider = \"openai\"".to_string(),
            }],
        );

        let mapped = provider_message_to_anthropic(message).unwrap();
        let serialized = serde_json::to_value(&mapped).unwrap();

        assert_eq!(serialized["role"], "user");
        assert_eq!(serialized["content"][0]["type"], "text");
        assert_eq!(
            serialized["content"][0]["text"],
            json!("[Attached file: agent.toml]\n<file-content>\nprovider = \"openai\"\n</file-content>")
        );
    }
}
