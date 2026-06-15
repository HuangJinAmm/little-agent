use std::sync::Arc;

use async_openai::{
    Client as OpenAiClient,
    config::{Config as AsyncOpenAiConfig, OPENAI_BETA_HEADER},
    types::{
        ChatCompletionMessageToolCall, ChatCompletionRequestAssistantMessage,
        ChatCompletionRequestAssistantMessageContent, ChatCompletionRequestMessage,
        ChatCompletionRequestMessageContentPartImage, ChatCompletionRequestMessageContentPartText,
        ChatCompletionRequestSystemMessage, ChatCompletionRequestSystemMessageContent,
        ChatCompletionRequestToolMessage, ChatCompletionRequestToolMessageContent,
        ChatCompletionRequestUserMessage, ChatCompletionRequestUserMessageContent,
        ChatCompletionRequestUserMessageContentPart, ChatCompletionStreamResponseDelta,
        ChatCompletionTool, ChatCompletionToolType, CreateChatCompletionRequest,
        CreateChatCompletionResponse, CreateChatCompletionStreamResponse, FinishReason,
        FunctionCall, ImageUrl,
    },
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::header::{AUTHORIZATION, HeaderMap};
use secrecy::{ExposeSecret, SecretString};

use crate::config::AgentConfig;

use super::{
    LlmProvider, ProviderContentBlock, ProviderMessage, ProviderRequest, ProviderResponse,
    ProviderRole, ProviderStopReason, ProviderToolSpec, extract_text_from_blocks,
};

pub struct OpenAiProvider {
    client: OpenAiClient<OpenAiCompatibleConfig>,
}

#[derive(Debug, Clone)]
struct OpenAiCompatibleConfig {
    api_base: String,
    api_key: SecretString,
}

impl AsyncOpenAiConfig for OpenAiCompatibleConfig {
    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if !self.api_key.expose_secret().is_empty() {
            headers.insert(
                AUTHORIZATION,
                format!("Bearer {}", self.api_key.expose_secret())
                    .parse()
                    .expect("valid OpenAI authorization header"),
            );
        }

        // Keep the SDK default beta header behavior for compatibility.
        headers.insert(OPENAI_BETA_HEADER, "assistants=v2".parse().unwrap());
        headers
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.api_base, path)
    }

    fn query(&self) -> Vec<(&str, &str)> {
        vec![]
    }

    fn api_base(&self) -> &str {
        &self.api_base
    }

    fn api_key(&self) -> &SecretString {
        &self.api_key
    }
}

impl OpenAiProvider {
    pub fn from_config(config: Arc<AgentConfig>) -> Result<Self> {
        let openai = config
            .openai
            .as_ref()
            .context("openai config is required to create OpenAI provider")?;
        let base_url = openai.base_url.trim_end_matches('/').to_string();
        let client = OpenAiClient::with_config(OpenAiCompatibleConfig {
            api_base: base_url.clone(),
            api_key: SecretString::from(openai.api_key.clone()),
        });

        Ok(Self {
            client,
        })
    }

    #[cfg(test)]
    fn base_url(&self) -> &str {
        self.client.config().api_base()
    }

    #[cfg(test)]
    fn authorization_header(&self) -> Option<String> {
        self.client
            .config()
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &'static str {
        "openai"
    }

    async fn send(&self, request: ProviderRequest) -> Result<ProviderResponse> {
        let sdk_request = build_sdk_chat_request(request)?;
        let sdk_response = self
            .client
            .chat()
            .create(sdk_request)
            .await
            .context("OpenAI chat completion request failed")?;

        map_openai_response(sdk_response)
    }

    async fn send_streaming(
        &self,
        request: ProviderRequest,
        on_text_delta: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<ProviderResponse> {
        let sdk_request = build_sdk_chat_request(request)?;
        let mut stream = self
            .client
            .chat()
            .create_stream(sdk_request)
            .await
            .context("OpenAI chat completion stream request failed")?;

        let mut text = String::new();
        let mut tool_calls = Vec::new();
        let mut finish_reason = None;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("OpenAI chat completion stream chunk failed")?;
            merge_stream_chunk(
                &chunk,
                &mut text,
                &mut tool_calls,
                &mut finish_reason,
                on_text_delta,
            )?;
        }

        build_streamed_response(text, tool_calls, finish_reason)
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
struct StreamedToolCall {
    id: Option<String>,
    kind: Option<ChatCompletionToolType>,
    name: Option<String>,
    arguments: String,
}

#[allow(deprecated)]
fn build_sdk_chat_request(request: ProviderRequest) -> Result<CreateChatCompletionRequest> {
    let mut messages = Vec::new();
    if let Some(system) = request.system {
        messages.push(
            ChatCompletionRequestSystemMessage {
                content: ChatCompletionRequestSystemMessageContent::Text(system),
                name: None,
            }
            .into(),
        );
    }

    for message in request.messages {
        messages.extend(provider_message_to_sdk(message)?);
    }

    let tools = (!request.tools.is_empty())
        .then(|| request.tools.into_iter().map(provider_tool_to_sdk).collect());

    Ok(CreateChatCompletionRequest {
        model: request.model,
        messages,
        tools,
        max_tokens: Some(request.max_tokens),
        ..Default::default()
    })
}

fn provider_message_to_sdk(message: ProviderMessage) -> Result<Vec<ChatCompletionRequestMessage>> {
    match message.role {
        ProviderRole::System => Ok(vec![ChatCompletionRequestSystemMessage {
            content: ChatCompletionRequestSystemMessageContent::Text(text_only_content(
                message.content,
                "system",
            )?),
            name: None,
        }
        .into()]),
        ProviderRole::User => map_user_message(message.content),
        ProviderRole::Assistant => map_assistant_message(message.content),
        ProviderRole::Tool => map_tool_results(message.content),
    }
}

fn map_user_message(content: Vec<ProviderContentBlock>) -> Result<Vec<ChatCompletionRequestMessage>> {
    if content
        .iter()
        .all(|block| matches!(block, ProviderContentBlock::Text { .. }))
    {
        return Ok(vec![ChatCompletionRequestUserMessage {
            content: ChatCompletionRequestUserMessageContent::Text(text_only_content(
                content, "user",
            )?),
            name: None,
        }
        .into()]);
    }

    if content.iter().any(|block| {
        matches!(
            block,
            ProviderContentBlock::ToolResult { .. } | ProviderContentBlock::ToolUse { .. }
        )
    }) {
        return map_tool_results(content);
    }

    Ok(vec![ChatCompletionRequestUserMessage {
        content: ChatCompletionRequestUserMessageContent::Array(map_user_content_parts(content)?),
        name: None,
    }
    .into()])
}

fn map_user_content_parts(
    content: Vec<ProviderContentBlock>,
) -> Result<Vec<ChatCompletionRequestUserMessageContentPart>> {
    content
        .into_iter()
        .map(|block| match block {
            ProviderContentBlock::Text { text } => Ok(ChatCompletionRequestMessageContentPartText {
                text,
            }
            .into()),
            ProviderContentBlock::File { filename, content } => {
                Ok(ChatCompletionRequestMessageContentPartText {
                    text: format!(
                        "[Attached file: {filename}]\n<file-content>\n{content}\n</file-content>"
                    ),
                }
                .into())
            }
            ProviderContentBlock::Image {
                media_type,
                data_base64,
                ..
            } => Ok(ChatCompletionRequestMessageContentPartImage {
                image_url: ImageUrl {
                    url: format!("data:{media_type};base64,{data_base64}"),
                    detail: None,
                },
            }
            .into()),
            ProviderContentBlock::ToolUse { .. } => {
                bail!("user message content parts do not support tool use blocks")
            }
            ProviderContentBlock::ToolResult { .. } => {
                bail!("user message content parts do not support tool result blocks")
            }
        })
        .collect()
}

fn map_assistant_message(
    content: Vec<ProviderContentBlock>,
) -> Result<Vec<ChatCompletionRequestMessage>> {
    let mut text_blocks = Vec::new();
    let mut tool_calls = Vec::new();

    for block in content {
        match block {
            ProviderContentBlock::Text { text } => text_blocks.push(text),
            ProviderContentBlock::File { .. } | ProviderContentBlock::Image { .. } => {
                bail!("assistant messages do not support attachment blocks for OpenAI APIs")
            }
            ProviderContentBlock::ToolUse { id, name, input } => {
                tool_calls.push(ChatCompletionMessageToolCall {
                    id,
                    r#type: ChatCompletionToolType::Function,
                    function: FunctionCall {
                        name,
                        arguments: serde_json::to_string(&input)
                            .context("failed to serialize tool call arguments")?,
                    },
                });
            }
            ProviderContentBlock::ToolResult { .. } => {
                bail!("assistant messages cannot contain tool results for OpenAI APIs")
            }
        }
    }

    Ok(vec![ChatCompletionRequestAssistantMessage {
        content: (!text_blocks.is_empty())
            .then(|| ChatCompletionRequestAssistantMessageContent::Text(text_blocks.join("\n"))),
        tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
        ..Default::default()
    }
    .into()])
}

fn map_tool_results(content: Vec<ProviderContentBlock>) -> Result<Vec<ChatCompletionRequestMessage>> {
    content
        .into_iter()
        .map(|block| match block {
            ProviderContentBlock::ToolResult {
                tool_use_id,
                content,
            } => Ok(ChatCompletionRequestToolMessage {
                content: ChatCompletionRequestToolMessageContent::Text(content),
                tool_call_id: tool_use_id,
            }
            .into()),
            ProviderContentBlock::Text { .. } => {
                bail!("tool result mapping only supports tool result blocks")
            }
            ProviderContentBlock::File { .. } | ProviderContentBlock::Image { .. } => {
                bail!("tool result mapping does not accept attachment blocks")
            }
            ProviderContentBlock::ToolUse { .. } => {
                bail!("tool result mapping does not accept tool use blocks")
            }
        })
        .collect()
}

fn text_only_content(content: Vec<ProviderContentBlock>, role: &str) -> Result<String> {
    if content
        .iter()
        .any(|block| !matches!(block, ProviderContentBlock::Text { .. }))
    {
        bail!("{role} messages can only contain text blocks for OpenAI APIs");
    }

    Ok(extract_text_from_blocks(&content))
}

fn provider_tool_to_sdk(tool: ProviderToolSpec) -> ChatCompletionTool {
    ChatCompletionTool {
        r#type: ChatCompletionToolType::Function,
        function: async_openai::types::FunctionObject {
            name: tool.name,
            description: tool.description,
            parameters: Some(normalize_openai_tool_schema(tool.input_schema)),
            strict: None,
        },
    }
}

fn normalize_openai_tool_schema(schema: serde_json::Value) -> serde_json::Value {
    let mut schema = match schema {
        serde_json::Value::Object(map) => map,
        other => return other,
    };

    let is_object_schema = schema
        .get("type")
        .and_then(|value| value.as_str())
        .map(|value| value == "object")
        .unwrap_or(false);

    if is_object_schema {
        schema
            .entry("properties".to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        schema
            .entry("required".to_string())
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    }

    serde_json::Value::Object(schema)
}

fn map_openai_response(response: CreateChatCompletionResponse) -> Result<ProviderResponse> {
    let choice = response
        .choices
        .into_iter()
        .next()
        .context("OpenAI response contained no choices")?;

    let mut content = Vec::new();
    if let Some(text) = choice.message.content.filter(|value| !value.is_empty()) {
        content.push(ProviderContentBlock::Text { text });
    }
    if let Some(tool_calls) = choice.message.tool_calls {
        for tool_call in tool_calls {
            content.push(openai_tool_call_to_provider(tool_call)?);
        }
    }

    Ok(ProviderResponse {
        content,
        stop_reason: map_finish_reason(choice.finish_reason),
    })
}

fn merge_stream_chunk(
    chunk: &CreateChatCompletionStreamResponse,
    text: &mut String,
    tool_calls: &mut Vec<StreamedToolCall>,
    finish_reason: &mut Option<FinishReason>,
    on_text_delta: &mut (dyn for<'a> FnMut(&'a str) + Send),
) -> Result<()> {
    let Some(choice) = chunk
        .choices
        .iter()
        .find(|choice| choice.index == 0)
        .or_else(|| chunk.choices.first())
    else {
        return Ok(());
    };

    merge_stream_delta(&choice.delta, text, tool_calls, on_text_delta);

    if let Some(reason) = choice.finish_reason {
        *finish_reason = Some(reason);
    }

    Ok(())
}

fn merge_stream_delta(
    delta: &ChatCompletionStreamResponseDelta,
    text: &mut String,
    tool_calls: &mut Vec<StreamedToolCall>,
    on_text_delta: &mut (dyn for<'a> FnMut(&'a str) + Send),
) {
    if let Some(content) = delta.content.as_deref().filter(|content| !content.is_empty()) {
        text.push_str(content);
        on_text_delta(content);
    }

    if let Some(delta_tool_calls) = &delta.tool_calls {
        merge_stream_tool_calls(delta_tool_calls, tool_calls);
    }
}

fn merge_stream_tool_calls(
    delta_tool_calls: &[async_openai::types::ChatCompletionMessageToolCallChunk],
    tool_calls: &mut Vec<StreamedToolCall>,
) {
    for delta_tool_call in delta_tool_calls {
        let index = delta_tool_call.index as usize;
        if tool_calls.len() <= index {
            tool_calls.resize_with(index + 1, StreamedToolCall::default);
        }

        let tool_call = &mut tool_calls[index];
        if let Some(id) = &delta_tool_call.id {
            tool_call.id = Some(id.clone());
        }
        if let Some(kind) = &delta_tool_call.r#type {
            tool_call.kind = Some(kind.clone());
        }
        if let Some(function) = &delta_tool_call.function {
            if let Some(name) = &function.name {
                tool_call.name = Some(name.clone());
            }
            if let Some(arguments) = &function.arguments {
                tool_call.arguments.push_str(arguments);
            }
        }
    }
}

fn build_streamed_response(
    text: String,
    tool_calls: Vec<StreamedToolCall>,
    finish_reason: Option<FinishReason>,
) -> Result<ProviderResponse> {
    let mut content = Vec::new();
    if !text.is_empty() {
        content.push(ProviderContentBlock::Text { text });
    }
    for tool_call in tool_calls {
        content.push(openai_tool_call_to_provider(complete_streamed_tool_call(tool_call)?)?);
    }

    Ok(ProviderResponse {
        content,
        stop_reason: map_finish_reason(finish_reason),
    })
}

fn complete_streamed_tool_call(tool_call: StreamedToolCall) -> Result<ChatCompletionMessageToolCall> {
    Ok(ChatCompletionMessageToolCall {
        id: tool_call
            .id
            .context("streamed OpenAI tool call missing id")?,
        r#type: tool_call
            .kind
            .context("streamed OpenAI tool call missing type")?,
        function: FunctionCall {
            name: tool_call
                .name
                .context("streamed OpenAI tool call missing function name")?,
            arguments: tool_call.arguments,
        },
    })
}

fn openai_tool_call_to_provider(tool_call: ChatCompletionMessageToolCall) -> Result<ProviderContentBlock> {
    if tool_call.r#type != ChatCompletionToolType::Function {
        bail!("unsupported OpenAI tool call type: {:?}", tool_call.r#type);
    }

    Ok(ProviderContentBlock::ToolUse {
        id: tool_call.id,
        name: tool_call.function.name,
        input: serde_json::from_str(&tool_call.function.arguments)
            .context("failed to parse OpenAI tool call arguments")?,
    })
}

fn map_finish_reason(value: Option<FinishReason>) -> Option<ProviderStopReason> {
    value.map(|reason| match reason {
        FinishReason::ToolCalls | FinishReason::FunctionCall => ProviderStopReason::ToolUse,
        FinishReason::Length => ProviderStopReason::MaxTokens,
        FinishReason::Stop | FinishReason::ContentFilter => ProviderStopReason::EndTurn,
    })
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use std::sync::Arc;

    use async_openai::types::{
        ChatChoice, ChatChoiceStream, ChatCompletionResponseMessage,
        ChatCompletionStreamResponseDelta, ChatCompletionToolType, CompletionUsage,
        CreateChatCompletionResponse, CreateChatCompletionStreamResponse, FinishReason,
        FunctionCallStream, Role,
    };
    use serde_json::json;

    use super::{
        OpenAiProvider, StreamedToolCall, build_sdk_chat_request, build_streamed_response,
        map_openai_response, merge_stream_chunk,
    };
    use crate::config::{AgentConfig, OpenAiConfig, ProviderKind, RuntimeConfig};
    use crate::llm::{
        ProviderContentBlock, ProviderMessage, ProviderRequest, ProviderRole, ProviderStopReason,
        ProviderToolSpec,
    };

    fn openai_provider_config(api_key: &str, base_url: &str) -> Arc<AgentConfig> {
        Arc::new(AgentConfig {
            provider: ProviderKind::OpenAi,
            anthropic: None,
            openai: Some(OpenAiConfig {
                model: "local-model".to_string(),
                api_key: api_key.to_string(),
                base_url: base_url.to_string(),
            }),
            runtime: RuntimeConfig {
                context_limit: 50_000,
                max_tokens: 8_000,
            },
        })
    }

    fn sample_chat_response(
        content: Option<&str>,
        tool_calls: Option<Vec<async_openai::types::ChatCompletionMessageToolCall>>,
        finish_reason: Option<FinishReason>,
    ) -> CreateChatCompletionResponse {
        CreateChatCompletionResponse {
            id: "chatcmpl-test".to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatCompletionResponseMessage {
                    content: content.map(str::to_string),
                    refusal: None,
                    tool_calls,
                    role: Role::Assistant,
                    function_call: None,
                    audio: None,
                },
                finish_reason,
                logprobs: None,
            }],
            created: 0,
            model: "local-model".to_string(),
            service_tier: None,
            system_fingerprint: None,
            object: "chat.completion".to_string(),
            usage: Some(CompletionUsage::default()),
        }
    }

    fn sample_stream_chunk(
        content: Option<&str>,
        tool_calls: Option<Vec<async_openai::types::ChatCompletionMessageToolCallChunk>>,
        finish_reason: Option<FinishReason>,
    ) -> CreateChatCompletionStreamResponse {
        CreateChatCompletionStreamResponse {
            id: "chatcmpl-test".to_string(),
            choices: vec![ChatChoiceStream {
                index: 0,
                delta: ChatCompletionStreamResponseDelta {
                    content: content.map(str::to_string),
                    function_call: None,
                    tool_calls,
                    role: Some(Role::Assistant),
                    refusal: None,
                },
                finish_reason,
                logprobs: None,
            }],
            created: 0,
            model: "local-model".to_string(),
            service_tier: None,
            system_fingerprint: None,
            object: "chat.completion.chunk".to_string(),
            usage: None,
        }
    }

    #[test]
    fn builds_openai_provider_with_custom_base_url() {
        let provider = OpenAiProvider::from_config(openai_provider_config(
            "test-key",
            "http://127.0.0.1:1234/v1/",
        ))
        .unwrap();

        assert_eq!(provider.base_url(), "http://127.0.0.1:1234/v1");
    }

    #[test]
    fn allows_empty_api_key_for_local_base_url() {
        let provider =
            OpenAiProvider::from_config(openai_provider_config("", "http://127.0.0.1:1234/v1"))
                .unwrap();

        assert_eq!(provider.base_url(), "http://127.0.0.1:1234/v1");
        assert_eq!(provider.authorization_header(), None);
    }

    #[test]
    fn maps_provider_messages_and_tools_to_sdk_request() {
        let request = ProviderRequest {
            model: "local-model".to_string(),
            system: Some("system prompt".to_string()),
            messages: vec![
                ProviderMessage::new_text(ProviderRole::User, "hello"),
                ProviderMessage::new_blocks(
                    ProviderRole::Assistant,
                    vec![
                        ProviderContentBlock::Text {
                            text: "I'll use a tool".to_string(),
                        },
                        ProviderContentBlock::ToolUse {
                            id: "call_1".to_string(),
                            name: "read_file".to_string(),
                            input: json!({"path": "Cargo.toml"}),
                        },
                    ],
                ),
                ProviderMessage::new_blocks(
                    ProviderRole::User,
                    vec![ProviderContentBlock::ToolResult {
                        tool_use_id: "call_1".to_string(),
                        content: "file contents".to_string(),
                    }],
                ),
            ],
            tools: vec![ProviderToolSpec {
                name: "read_file".to_string(),
                description: Some("Read a file".to_string()),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"}
                    },
                    "required": ["path"]
                }),
            }],
            max_tokens: 2048,
        };

        let mapped = build_sdk_chat_request(request).unwrap();
        let mapped_json = serde_json::to_value(&mapped).unwrap();

        assert_eq!(mapped_json["model"], json!("local-model"));
        assert_eq!(mapped_json["max_tokens"], json!(2048));
        assert_eq!(mapped_json["messages"].as_array().unwrap().len(), 4);
        assert_eq!(mapped_json["messages"][0]["role"], json!("system"));
        assert_eq!(mapped_json["messages"][0]["content"], json!("system prompt"));
        assert_eq!(mapped_json["messages"][1]["role"], json!("user"));
        assert_eq!(mapped_json["messages"][1]["content"], json!("hello"));
        assert_eq!(mapped_json["messages"][2]["role"], json!("assistant"));
        assert_eq!(mapped_json["messages"][2]["content"], json!("I'll use a tool"));
        assert_eq!(
            mapped_json["messages"][2]["tool_calls"][0]["function"]["arguments"],
            json!("{\"path\":\"Cargo.toml\"}")
        );
        assert_eq!(mapped_json["messages"][3]["role"], json!("tool"));
        assert_eq!(mapped_json["messages"][3]["tool_call_id"], json!("call_1"));
        assert_eq!(mapped_json["messages"][3]["content"], json!("file contents"));
        assert_eq!(mapped_json["tools"].as_array().unwrap().len(), 1);
        assert_eq!(mapped_json["tools"][0]["type"], json!("function"));
        assert_eq!(mapped_json["tools"][0]["function"]["name"], json!("read_file"));
    }

    #[test]
    fn normalizes_empty_object_tool_schema_for_openai_compatible_servers() {
        let request = ProviderRequest {
            model: "local-model".to_string(),
            system: None,
            messages: vec![ProviderMessage::new_text(ProviderRole::User, "hello")],
            tools: vec![ProviderToolSpec {
                name: "ping".to_string(),
                description: Some("Ping without arguments".to_string()),
                input_schema: json!({
                    "type": "object",
                    "title": "PingInput"
                }),
            }],
            max_tokens: 256,
        };

        let mapped = build_sdk_chat_request(request).unwrap();
        let mapped_json = serde_json::to_value(&mapped).unwrap();

        assert_eq!(
            mapped_json["tools"][0]["function"]["parameters"]["type"],
            json!("object")
        );
        assert_eq!(
            mapped_json["tools"][0]["function"]["parameters"]["properties"],
            json!({})
        );
        assert_eq!(
            mapped_json["tools"][0]["function"]["parameters"]["required"],
            json!([])
        );
    }

    #[test]
    fn maps_user_file_attachment_to_text_content_part() {
        let request = ProviderRequest {
            model: "local-model".to_string(),
            system: None,
            messages: vec![ProviderMessage::new_blocks(
                ProviderRole::User,
                vec![
                    ProviderContentBlock::Text {
                        text: "summarize this file".to_string(),
                    },
                    ProviderContentBlock::File {
                        filename: "agent.toml".to_string(),
                        content: "provider = \"openai\"".to_string(),
                    },
                ],
            )],
            tools: Vec::new(),
            max_tokens: 2048,
        };

        let mapped = build_sdk_chat_request(request).unwrap();
        let mapped_json = serde_json::to_value(&mapped).unwrap();

        assert_eq!(mapped_json["messages"].as_array().unwrap().len(), 1);
        assert_eq!(mapped_json["messages"][0]["role"], json!("user"));
        assert_eq!(mapped_json["messages"][0]["content"][0]["type"], json!("text"));
        assert_eq!(
            mapped_json["messages"][0]["content"][0]["text"],
            json!("summarize this file")
        );
        assert_eq!(mapped_json["messages"][0]["content"][1]["type"], json!("text"));
        assert_eq!(
            mapped_json["messages"][0]["content"][1]["text"],
            json!("[Attached file: agent.toml]\n<file-content>\nprovider = \"openai\"\n</file-content>")
        );
    }

    #[test]
    fn maps_user_image_attachment_to_image_url_content_part() {
        let request = ProviderRequest {
            model: "local-model".to_string(),
            system: None,
            messages: vec![ProviderMessage::new_blocks(
                ProviderRole::User,
                vec![
                    ProviderContentBlock::Text {
                        text: "describe this diagram".to_string(),
                    },
                    ProviderContentBlock::Image {
                        source_name: "diagram.png".to_string(),
                        media_type: "image/png".to_string(),
                        data_base64: "ZmFrZQ==".to_string(),
                    },
                ],
            )],
            tools: Vec::new(),
            max_tokens: 2048,
        };

        let mapped = build_sdk_chat_request(request).unwrap();
        let mapped_json = serde_json::to_value(&mapped).unwrap();

        assert_eq!(mapped_json["messages"].as_array().unwrap().len(), 1);
        assert_eq!(mapped_json["messages"][0]["role"], json!("user"));
        assert_eq!(mapped_json["messages"][0]["content"][0]["type"], json!("text"));
        assert_eq!(
            mapped_json["messages"][0]["content"][0]["text"],
            json!("describe this diagram")
        );
        assert_eq!(
            mapped_json["messages"][0]["content"][1]["type"],
            json!("image_url")
        );
        assert_eq!(
            mapped_json["messages"][0]["content"][1]["image_url"]["url"],
            json!("data:image/png;base64,ZmFrZQ==")
        );
    }

    #[test]
    fn maps_tool_calls_finish_reason_to_provider_tool_use() {
        let response = sample_chat_response(
            None,
            Some(vec![async_openai::types::ChatCompletionMessageToolCall {
                id: "call_1".to_string(),
                r#type: async_openai::types::ChatCompletionToolType::Function,
                function: async_openai::types::FunctionCall {
                    name: "read_file".to_string(),
                    arguments: "{\"path\":\"Cargo.toml\"}".to_string(),
                },
            }]),
            Some(FinishReason::ToolCalls),
        );

        let mapped = map_openai_response(response).unwrap();

        assert_eq!(mapped.stop_reason, Some(ProviderStopReason::ToolUse));
        assert_eq!(
            mapped.content,
            vec![ProviderContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "read_file".to_string(),
                input: json!({"path": "Cargo.toml"}),
            }]
        );
    }

    #[test]
    fn maps_length_finish_reason_to_max_tokens() {
        let response =
            sample_chat_response(Some("partial answer"), None, Some(FinishReason::Length));

        let mapped = map_openai_response(response).unwrap();

        assert_eq!(mapped.stop_reason, Some(ProviderStopReason::MaxTokens));
        assert_eq!(
            mapped.content,
            vec![ProviderContentBlock::Text {
                text: "partial answer".to_string(),
            }]
        );
    }

    #[test]
    fn merges_stream_chunks_into_complete_response() {
        let first = sample_stream_chunk(
            Some("Hel"),
            Some(vec![async_openai::types::ChatCompletionMessageToolCallChunk {
                index: 0,
                id: Some("call_1".to_string()),
                r#type: Some(ChatCompletionToolType::Function),
                function: Some(FunctionCallStream {
                    name: Some("read_file".to_string()),
                    arguments: Some("{\"path\":\"C".to_string()),
                }),
            }]),
            None,
        );
        let second = sample_stream_chunk(
            Some("lo"),
            Some(vec![async_openai::types::ChatCompletionMessageToolCallChunk {
                index: 0,
                id: None,
                r#type: None,
                function: Some(FunctionCallStream {
                    name: None,
                    arguments: Some("argo.toml\"}".to_string()),
                }),
            }]),
            Some(FinishReason::ToolCalls),
        );

        let mut text = String::new();
        let mut tool_calls = Vec::<StreamedToolCall>::new();
        let mut finish_reason = None;
        let mut deltas = Vec::new();

        merge_stream_chunk(
            &first,
            &mut text,
            &mut tool_calls,
            &mut finish_reason,
            &mut |delta| deltas.push(delta.to_string()),
        )
        .unwrap();
        merge_stream_chunk(
            &second,
            &mut text,
            &mut tool_calls,
            &mut finish_reason,
            &mut |delta| deltas.push(delta.to_string()),
        )
        .unwrap();

        let response = build_streamed_response(text, tool_calls, finish_reason).unwrap();

        assert_eq!(deltas, vec!["Hel".to_string(), "lo".to_string()]);
        assert_eq!(response.stop_reason, Some(ProviderStopReason::ToolUse));
        assert_eq!(
            response.content,
            vec![
                ProviderContentBlock::Text {
                    text: "Hello".to_string(),
                },
                ProviderContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    input: json!({"path": "Cargo.toml"}),
                },
            ]
        );
    }
}
