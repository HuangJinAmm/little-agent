pub mod anthropic;
pub mod openai;
pub mod types;

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::config::AgentConfig;
use crate::config::ProviderKind;

use self::anthropic::AnthropicProvider;
use self::openai::OpenAiProvider;

pub use self::types::{
    ProviderContentBlock, ProviderMessage, ProviderRequest, ProviderResponse, ProviderRole,
    ProviderStopReason, ProviderToolSpec, extract_text_from_blocks,
};

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &'static str;

    async fn send(&self, request: ProviderRequest) -> Result<ProviderResponse>;

    async fn send_streaming(
        &self,
        request: ProviderRequest,
        on_text_delta: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<ProviderResponse> {
        let response = self.send(request).await?;
        let text_blocks = response
            .content
            .iter()
            .filter_map(|block| match block {
                ProviderContentBlock::Text { text } => Some(text.clone()),
                ProviderContentBlock::File { .. }
                | ProviderContentBlock::Image { .. }
                | ProviderContentBlock::ToolUse { .. }
                | ProviderContentBlock::ToolResult { .. } => None,
            })
            .collect::<Vec<_>>();

        for text in &text_blocks {
            on_text_delta(text.as_str());
        }

        Ok(response)
    }
}

pub fn build_provider(config: Arc<AgentConfig>) -> Result<Arc<dyn LlmProvider>> {
    match &config.provider {
        ProviderKind::Anthropic => Ok(Arc::new(AnthropicProvider::from_config(config)?)),
        ProviderKind::OpenAi => Ok(Arc::new(OpenAiProvider::from_config(config)?)),
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use async_trait::async_trait;
    use serde_json::json;

    use super::{
        LlmProvider, ProviderContentBlock, ProviderRequest, ProviderResponse, ProviderRole,
        ProviderStopReason,
    };

    struct FakeProvider {
        response: ProviderResponse,
    }

    #[async_trait]
    impl LlmProvider for FakeProvider {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn send(&self, _request: ProviderRequest) -> Result<ProviderResponse> {
            Ok(self.response.clone())
        }
    }

    fn test_request() -> ProviderRequest {
        ProviderRequest {
            model: "test-model".to_string(),
            system: None,
            messages: vec![super::ProviderMessage::new_text(ProviderRole::User, "hello")],
            tools: vec![],
            max_tokens: 128,
        }
    }

    #[tokio::test]
    async fn send_streaming_default_impl_calls_back_only_for_text_blocks() {
        let provider = FakeProvider {
            response: ProviderResponse {
                content: vec![
                    ProviderContentBlock::Text {
                        text: "first".to_string(),
                    },
                    ProviderContentBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: "read_file".to_string(),
                        input: json!({"path": "Cargo.toml"}),
                    },
                    ProviderContentBlock::ToolResult {
                        tool_use_id: "call_1".to_string(),
                        content: "contents".to_string(),
                    },
                    ProviderContentBlock::Text {
                        text: "second".to_string(),
                    },
                ],
                stop_reason: Some(ProviderStopReason::ToolUse),
            },
        };
        let mut deltas = Vec::new();

        let response = provider
            .send_streaming(test_request(), &mut |delta| deltas.push(delta.to_string()))
            .await
            .unwrap();

        assert_eq!(deltas, vec!["first".to_string(), "second".to_string()]);
        assert_eq!(response, provider.response);
    }
}
