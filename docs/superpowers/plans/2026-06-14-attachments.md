# Attachment Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 CLI agent 增加本地文本文件和图片附件输入能力，并让 Anthropic 与 OpenAI provider 都能消费这些附件。

**Architecture:** 扩展内部 `ProviderContentBlock` 以承载 `File` 和 `Image` 两类附件；新增独立的 `attachment` 模块负责解析路径、读取文件和组装 block；`main.rs` 只负责收集用户文本与附件路径并拼成一条用户消息，provider 层分别完成协议映射。

**Tech Stack:** Rust、Tokio、Serde、Base64、Inquire、Anthropic SDK、`async-openai`

---

## 文件结构

- Create: `d:\3-ai-project\little-agent\src\attachment.rs`
  - 负责解析分号分隔路径、识别文件类型、读取文本/图片、构造附件 blocks
- Modify: `d:\3-ai-project\little-agent\src\lib.rs`
  - 导出 `attachment` 模块
- Modify: `d:\3-ai-project\little-agent\src\llm\types.rs`
  - 新增 `File` 与 `Image` 内容块，补充模型测试
- Modify: `d:\3-ai-project\little-agent\src\main.rs`
  - 在每轮提问时收集附件路径并组装用户消息
- Modify: `d:\3-ai-project\little-agent\src\llm\anthropic.rs`
  - 扩展用户消息映射，支持图片与文本文件附件
- Modify: `d:\3-ai-project\little-agent\src\llm\openai.rs`
  - 扩展 chat completions 用户内容映射，支持图片与文本文件附件

### Task 1: 扩展内部消息模型

**Files:**
- Modify: `d:\3-ai-project\little-agent\src\llm\types.rs`

- [ ] **Step 1: 写失败测试，锁定附件 block 的序列化行为**

```rust
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
fn extract_text_ignores_attachment_blocks() {
    let content = vec![
        ProviderContentBlock::Text {
            text: "question".to_string(),
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
    ];

    assert_eq!(extract_text_from_blocks(&content), "question");
}
```

- [ ] **Step 2: 运行测试确认当前模型尚不支持附件**

Run: `cargo test llm::types::tests::serializes_file_and_image_blocks llm::types::tests::extract_text_ignores_attachment_blocks -- --nocapture`
Expected: FAIL，提示 `ProviderContentBlock::File` 或 `ProviderContentBlock::Image` 未定义

- [ ] **Step 3: 为 `ProviderContentBlock` 添加附件变体**

```rust
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
```

- [ ] **Step 4: 保持文本提取只读取 `Text`**

```rust
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
```

- [ ] **Step 5: 运行 `types.rs` 测试**

Run: `cargo test llm::types::tests -- --nocapture`
Expected: PASS

- [ ] **Step 6: 提交这一阶段**

```bash
git add src/llm/types.rs
git commit -m "feat: add attachment content blocks"
```

### Task 2: 新增附件解析与读取模块

**Files:**
- Create: `d:\3-ai-project\little-agent\src\attachment.rs`
- Modify: `d:\3-ai-project\little-agent\src\lib.rs`

- [ ] **Step 1: 先写失败测试，覆盖路径解析与附件分类**

```rust
#[test]
fn parses_semicolon_separated_attachment_paths() {
    let parsed = parse_attachment_input(r"C:\a.png; C:\b.toml ;").unwrap();
    assert_eq!(parsed, vec!["C:\\a.png", "C:\\b.toml"]);
}

#[test]
fn rejects_unsupported_attachment_extension() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("archive.zip");
    std::fs::write(&path, b"fake").unwrap();

    let error = load_attachment_blocks(&[path]).unwrap_err().to_string();
    assert!(error.contains("unsupported attachment type"));
}
```

- [ ] **Step 2: 运行测试确认模块尚不存在**

Run: `cargo test attachment::tests -- --nocapture`
Expected: FAIL，提示 `attachment` 模块或函数未定义

- [ ] **Step 3: 创建 `src/attachment.rs` 并定义核心接口**

```rust
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::llm::ProviderContentBlock;

pub fn parse_attachment_input(input: &str) -> Result<Vec<String>> {
    Ok(input
        .split(';')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

pub fn load_attachment_blocks(paths: &[PathBuf]) -> Result<Vec<ProviderContentBlock>> {
    let mut blocks = Vec::new();
    for path in paths {
        blocks.push(load_single_attachment(path)?);
    }
    Ok(blocks)
}
```

- [ ] **Step 4: 实现文本文件与图片文件读取**

```rust
fn load_single_attachment(path: &Path) -> Result<ProviderContentBlock> {
    if !path.exists() {
        bail!("attachment file does not exist: {}", path.display());
    }
    if !path.is_file() {
        bail!("attachment path is not a file: {}", path.display());
    }

    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();

    if is_supported_image_extension(&extension) {
        return load_image_attachment(path);
    }
    if is_supported_text_extension(&extension) {
        return load_text_attachment(path);
    }

    bail!("unsupported attachment type: {}", path.display());
}
```

- [ ] **Step 5: 导出模块**

```rust
pub mod attachment;
```

- [ ] **Step 6: 运行附件模块测试**

Run: `cargo test attachment::tests -- --nocapture`
Expected: PASS

- [ ] **Step 7: 提交这一阶段**

```bash
git add src/attachment.rs src/lib.rs
git commit -m "feat: add attachment loading module"
```

### Task 3: 将附件接入 CLI 输入流

**Files:**
- Modify: `d:\3-ai-project\little-agent\src\main.rs`
- Test: `d:\3-ai-project\little-agent\src\attachment.rs`

- [ ] **Step 1: 先写失败测试，锁定用户消息组装规则**

```rust
#[test]
fn builds_user_message_with_text_and_attachments() {
    let message = build_user_message(
        "analyze these",
        vec![
            ProviderContentBlock::File {
                filename: "agent.toml".to_string(),
                content: "provider = \"openai\"".to_string(),
            },
            ProviderContentBlock::Image {
                source_name: "diagram.png".to_string(),
                media_type: "image/png".to_string(),
                data_base64: "ZmFrZQ==".to_string(),
            },
        ],
    );

    assert_eq!(message.role, ProviderRole::User);
    assert_eq!(message.content.len(), 3);
}
```

- [ ] **Step 2: 运行相关测试确认组装辅助函数尚不存在**

Run: `cargo test attachment::tests::builds_user_message_with_text_and_attachments -- --nocapture`
Expected: FAIL，提示 `build_user_message` 未定义

- [ ] **Step 3: 在 `attachment.rs` 增加组装辅助函数**

```rust
pub fn build_user_message(
    query: impl Into<String>,
    attachments: Vec<ProviderContentBlock>,
) -> ProviderMessage {
    let mut content = vec![ProviderContentBlock::Text { text: query.into() }];
    content.extend(attachments);
    ProviderMessage::new_blocks(ProviderRole::User, content)
}
```

- [ ] **Step 4: 在 `main.rs` 中增加附件输入读取**

```rust
let query = Text::new("--- How can I help you?")
    .prompt()
    .context("An error happened or user cancelled the input.")?;

let attachment_input = Text::new("--- Attachments (optional, separated by ';')")
    .with_default("")
    .prompt()
    .context("An error happened or user cancelled the attachment input.")?;

let attachment_paths = parse_attachment_input(&attachment_input)?
    .into_iter()
    .map(std::path::PathBuf::from)
    .collect::<Vec<_>>();
let attachment_blocks = load_attachment_blocks(&attachment_paths)?;

agent.runtime.context.push(build_user_message(query, attachment_blocks));
```

- [ ] **Step 5: 删除原来的纯文本 push**

```rust
// remove:
agent.runtime
    .context
    .push(ProviderMessage::new_text(ProviderRole::User, query));
```

- [ ] **Step 6: 运行附件和主流程相关测试**

Run: `cargo test attachment::tests -- --nocapture`
Expected: PASS

- [ ] **Step 7: 提交这一阶段**

```bash
git add src/main.rs src/attachment.rs
git commit -m "feat: accept attachments from cli prompts"
```

### Task 4: 扩展 Anthropic provider 的附件映射

**Files:**
- Modify: `d:\3-ai-project\little-agent\src\llm\anthropic.rs`

- [ ] **Step 1: 写失败测试，锁定图片与文本附件的用户消息映射**

```rust
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
    assert_eq!(serialized["content"][0]["type"], "image");
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
    assert!(serialized["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("[Attached file: agent.toml]"));
}
```

- [ ] **Step 2: 运行测试确认当前 provider 不支持附件块**

Run: `cargo test llm::anthropic::tests -- --nocapture`
Expected: FAIL，提示不支持 `File` / `Image`

- [ ] **Step 3: 在 Anthropic provider 中扩展用户 block 映射**

```rust
fn provider_block_to_anthropic(block: ProviderContentBlock) -> Result<ContentBlock> {
    match block {
        ProviderContentBlock::Text { text } => Ok(ContentBlock::Text { text }),
        ProviderContentBlock::File { filename, content } => Ok(ContentBlock::Text {
            text: format!(
                "[Attached file: {filename}]\n<file-content>\n{content}\n</file-content>"
            ),
        }),
        ProviderContentBlock::Image {
            media_type,
            data_base64,
            ..
        } => Ok(ContentBlock::Image {
            source: anthropic_ai_sdk::types::message::ImageSource::Base64 {
                media_type,
                data: data_base64,
            },
        }),
        ProviderContentBlock::ToolUse { id, name, input } => Ok(ContentBlock::ToolUse {
            id,
            name,
            input,
        }),
        ProviderContentBlock::ToolResult {
            tool_use_id,
            content,
        } => Ok(ContentBlock::ToolResult {
            tool_use_id,
            content,
        }),
    }
}
```

- [ ] **Step 4: 运行 Anthropic 模块测试**

Run: `cargo test llm::anthropic::tests -- --nocapture`
Expected: PASS

- [ ] **Step 5: 提交这一阶段**

```bash
git add src/llm/anthropic.rs
git commit -m "feat: map attachments for anthropic provider"
```

### Task 5: 扩展 OpenAI provider 的附件映射

**Files:**
- Modify: `d:\3-ai-project\little-agent\src\llm\openai.rs`

- [ ] **Step 1: 写失败测试，锁定 chat completions 附件内容映射**

```rust
#[test]
fn maps_user_image_attachment_to_openai_multimodal_content() {
    let request = ProviderRequest {
        model: "local-model".to_string(),
        system: None,
        messages: vec![ProviderMessage::new_blocks(
            ProviderRole::User,
            vec![ProviderContentBlock::Image {
                source_name: "diagram.png".to_string(),
                media_type: "image/png".to_string(),
                data_base64: "ZmFrZQ==".to_string(),
            }],
        )],
        tools: vec![],
        max_tokens: 256,
    };

    let mapped = build_sdk_chat_request(request).unwrap();
    let mapped_json = serde_json::to_value(&mapped).unwrap();
    assert_eq!(mapped_json["messages"][0]["content"][0]["type"], "image_url");
}

#[test]
fn maps_user_file_attachment_to_openai_text_content() {
    let request = ProviderRequest {
        model: "local-model".to_string(),
        system: None,
        messages: vec![ProviderMessage::new_blocks(
            ProviderRole::User,
            vec![ProviderContentBlock::File {
                filename: "agent.toml".to_string(),
                content: "provider = \"openai\"".to_string(),
            }],
        )],
        tools: vec![],
        max_tokens: 256,
    };

    let mapped = build_sdk_chat_request(request).unwrap();
    let mapped_json = serde_json::to_value(&mapped).unwrap();
    assert!(mapped_json["messages"][0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("[Attached file: agent.toml]"));
}
```

- [ ] **Step 2: 运行测试确认 OpenAI user message 仍只接受纯文本**

Run: `cargo test llm::openai::tests -- --nocapture`
Expected: FAIL，提示 user message 不支持 `File` / `Image`

- [ ] **Step 3: 扩展 `map_user_message()`，支持多段内容 user message**

```rust
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

    Ok(vec![ChatCompletionRequestUserMessage {
        content: ChatCompletionRequestUserMessageContent::Array(
            content
                .into_iter()
                .map(provider_block_to_openai_user_content)
                .collect::<Result<Vec<_>>>()?,
        ),
        name: None,
    }
    .into()])
}
```

- [ ] **Step 4: 为 `File` 和 `Image` 实现 OpenAI 内容块映射**

```rust
fn provider_block_to_openai_user_content(
    block: ProviderContentBlock,
) -> Result<ChatCompletionRequestUserMessageContentPart> {
    match block {
        ProviderContentBlock::Text { text } => Ok(
            ChatCompletionRequestUserMessageContentPart::Text(
                ChatCompletionRequestMessageContentPartText {
                    text,
                },
            ),
        ),
        ProviderContentBlock::File { filename, content } => Ok(
            ChatCompletionRequestUserMessageContentPart::Text(
                ChatCompletionRequestMessageContentPartText {
                    text: format!(
                        "[Attached file: {filename}]\n<file-content>\n{content}\n</file-content>"
                    ),
                },
            ),
        ),
        ProviderContentBlock::Image {
            media_type,
            data_base64,
            ..
        } => Ok(
            ChatCompletionRequestUserMessageContentPart::ImageUrl(
                ChatCompletionRequestMessageContentPartImage {
                    image_url: ImageUrl {
                        url: format!("data:{media_type};base64,{data_base64}"),
                        detail: None,
                    },
                },
            ),
        ),
        ProviderContentBlock::ToolUse { .. } | ProviderContentBlock::ToolResult { .. } => {
            bail!("OpenAI user multimodal content does not accept tool blocks")
        }
    }
}
```

- [ ] **Step 5: 运行 OpenAI 模块测试**

Run: `cargo test llm::openai::tests -- --nocapture`
Expected: PASS

- [ ] **Step 6: 提交这一阶段**

```bash
git add src/llm/openai.rs
git commit -m "feat: map attachments for openai provider"
```

### Task 6: 全量回归与诊断收尾

**Files:**
- Modify: `d:\3-ai-project\little-agent\src\attachment.rs`
- Modify: `d:\3-ai-project\little-agent\src\main.rs`
- Modify: `d:\3-ai-project\little-agent\src\llm\types.rs`
- Modify: `d:\3-ai-project\little-agent\src\llm\anthropic.rs`
- Modify: `d:\3-ai-project\little-agent\src\llm\openai.rs`

- [ ] **Step 1: 运行新增附件测试子集**

Run: `cargo test attachment::tests llm::types::tests llm::anthropic::tests llm::openai::tests -- --nocapture`
Expected: PASS

- [ ] **Step 2: 运行全量测试**

Run: `cargo test`
Expected: PASS

- [ ] **Step 3: 运行编译检查**

Run: `cargo check`
Expected: PASS

- [ ] **Step 4: 检查最近改动文件的诊断**

Run: 使用编辑器 diagnostics 检查：
- `file:///d:/3-ai-project/little-agent/src/attachment.rs`
- `file:///d:/3-ai-project/little-agent/src/main.rs`
- `file:///d:/3-ai-project/little-agent/src/llm/types.rs`
- `file:///d:/3-ai-project/little-agent/src/llm/anthropic.rs`
- `file:///d:/3-ai-project/little-agent/src/llm/openai.rs`

Expected: 无新增错误

- [ ] **Step 5: 最终提交**

```bash
git add src/attachment.rs src/lib.rs src/main.rs src/llm/types.rs src/llm/anthropic.rs src/llm/openai.rs
git commit -m "feat: add attachment support for user prompts"
```
