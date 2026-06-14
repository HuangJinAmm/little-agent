# Streaming CLI Output Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将交互式 CLI 的最终文本输出改成流式显示，同时保持完整响应仍写入上下文并兼容现有 tool loop。

**Architecture:** 在 `LlmProvider` 上增加带默认实现的流式接口，先让 `lib.rs` 和 CLI 支持消费文本增量；默认实现回退到当前非流式 `send()`，然后只为 OpenAI provider 接入真流式。Anthropic 第一版继续复用默认实现，避免扩大改动范围。

**Tech Stack:** Rust、Tokio、`async-trait`、`async-openai`、现有 provider 抽象、标准输出 flush

---

## 文件结构

- Modify: `d:\3-ai-project\little-agent\src\llm\mod.rs`
  - 为 `LlmProvider` 增加默认流式接口和基础测试支撑
- Modify: `d:\3-ai-project\little-agent\src\lib.rs`
  - 提取单轮请求逻辑并接入流式文本打印
- Modify: `d:\3-ai-project\little-agent\src\llm\openai.rs`
  - 用 `async-openai` chat streaming 实现真流式文本增量
- Optional Modify: `d:\3-ai-project\little-agent\src\llm\anthropic.rs`
  - 如果需要编译适配或显式注释回退行为时调整
- Modify: `d:\3-ai-project\little-agent\src\main.rs`
  - 删除“事后统一打印 final response”的旧逻辑，交由流式路径输出

### Task 1: 为 provider 抽象增加默认流式接口

**Files:**
- Modify: `d:\3-ai-project\little-agent\src\llm\mod.rs`

- [ ] **Step 1: 写失败测试，锁定默认流式接口只回调文本**

```rust
struct FakeProvider;

#[async_trait]
impl LlmProvider for FakeProvider {
    fn name(&self) -> &'static str {
        "fake"
    }

    async fn send(&self, _request: ProviderRequest) -> Result<ProviderResponse> {
        Ok(ProviderResponse {
            content: vec![
                ProviderContentBlock::Text {
                    text: "hello".to_string(),
                },
                ProviderContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "Cargo.toml"}),
                },
            ],
            stop_reason: Some(ProviderStopReason::ToolUse),
        })
    }
}

#[tokio::test]
async fn default_streaming_interface_emits_only_text_blocks() {
    let provider = FakeProvider;
    let mut deltas = Vec::new();

    let response = provider
        .send_streaming(
            ProviderRequest {
                model: "fake".to_string(),
                system: None,
                messages: vec![],
                tools: vec![],
                max_tokens: 16,
            },
            &mut |delta| deltas.push(delta.to_string()),
        )
        .await
        .unwrap();

    assert_eq!(deltas, vec!["hello"]);
    assert_eq!(response.content.len(), 2);
}
```

- [ ] **Step 2: 运行测试确认流式接口尚不存在**

Run: `cargo test llm::tests::default_streaming_interface_emits_only_text_blocks -- --nocapture`
Expected: FAIL，提示 `send_streaming` 未定义

- [ ] **Step 3: 在 `LlmProvider` 上增加默认流式接口**

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &'static str;

    async fn send(&self, request: ProviderRequest) -> Result<ProviderResponse>;

    async fn send_streaming(
        &self,
        request: ProviderRequest,
        on_text_delta: &mut (dyn FnMut(&str) + Send),
    ) -> Result<ProviderResponse> {
        let response = self.send(request).await?;
        for block in &response.content {
            if let ProviderContentBlock::Text { text } = block {
                on_text_delta(text);
            }
        }
        Ok(response)
    }
}
```

- [ ] **Step 4: 增加抽象层测试模块**

```rust
#[cfg(test)]
mod tests {
    // place FakeProvider test here
}
```

- [ ] **Step 5: 运行 `llm::tests`**

Run: `cargo test llm::tests -- --nocapture`
Expected: PASS

- [ ] **Step 6: 提交这一阶段**

```bash
git add src/llm/mod.rs
git commit -m "feat: add default streaming provider interface"
```

### Task 2: 让 agent 主循环支持流式文本打印

**Files:**
- Modify: `d:\3-ai-project\little-agent\src\lib.rs`
- Modify: `d:\3-ai-project\little-agent\src\main.rs`

- [ ] **Step 1: 写失败测试，锁定“请求完成后仍写入完整 assistant 响应”**

```rust
#[tokio::test]
async fn streamed_request_still_appends_full_assistant_message() {
    // build a fake agent/provider and assert runtime.context gets full text
}
```

- [ ] **Step 2: 运行测试确认主循环尚未暴露流式发送入口**

Run: `cargo test streamed_request_still_appends_full_assistant_message -- --nocapture`
Expected: FAIL，提示缺少相关辅助函数或流式入口

- [ ] **Step 3: 在 `lib.rs` 提取单轮请求函数**

```rust
async fn request_once(&mut self, system: &str, stream_text: bool) -> Result<ProviderResponse> {
    let request = ProviderRequest {
        model: get_model(self.runtime.config.as_ref()),
        system: Some(system.to_string()),
        messages: self.runtime.context.clone(),
        tools: self.all_tool_specs(),
        max_tokens: self.runtime.config.runtime.max_tokens,
    };

    if stream_text {
        let mut printed_any = false;
        let response = self
            .runtime
            .provider
            .send_streaming(request, &mut |delta| {
                printed_any = true;
                print!("{delta}");
                let _ = std::io::Write::flush(&mut std::io::stdout());
            })
            .await?;
        if printed_any {
            println!();
        }
        Ok(response)
    } else {
        self.runtime.provider.send(request).await
    }
}
```

- [ ] **Step 4: 在 `agent_loop()` 中主路径调用流式版本**

```rust
let response = match self.request_once(&system, true).await {
    // preserve existing recovery logic
};
```

- [ ] **Step 5: 保持 `compact_history()` 使用非流式**

```rust
let response = self.runtime.provider.send(request).await?;
```

- [ ] **Step 6: 移除 `main.rs` 里旧的“事后统一打印 final response”**

```rust
// remove:
let Some(final_content) = agent.runtime.context.last() else {
    continue;
};
println!("--- Final response:\n{}", extract_text(&final_content.content));
```

- [ ] **Step 7: 运行相关测试与编译检查**

Run: `cargo check`
Expected: PASS

- [ ] **Step 8: 提交这一阶段**

```bash
git add src/lib.rs src/main.rs
git commit -m "feat: stream cli output from agent loop"
```

### Task 3: 为 OpenAI provider 接入真流式

**Files:**
- Modify: `d:\3-ai-project\little-agent\src\llm\openai.rs`

- [ ] **Step 1: 写失败测试，锁定流式 delta 聚合为完整文本**

```rust
#[tokio::test]
async fn openai_streaming_aggregates_text_deltas_into_final_response() {
    // use a helper that maps synthetic streaming chunks into a final response
}
```

- [ ] **Step 2: 运行测试确认 OpenAI provider 尚未覆写 `send_streaming()`**

Run: `cargo test llm::openai::tests::openai_streaming_aggregates_text_deltas_into_final_response -- --nocapture`
Expected: FAIL，提示相关流式辅助函数未定义

- [ ] **Step 3: 增加 OpenAI streaming 请求与聚合辅助函数**

```rust
async fn send_streaming(
    &self,
    request: ProviderRequest,
    on_text_delta: &mut (dyn FnMut(&str) + Send),
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
        merge_stream_chunk(&chunk, &mut text, &mut tool_calls, &mut finish_reason, on_text_delta)?;
    }

    build_streamed_response(text, tool_calls, finish_reason)
}
```

- [ ] **Step 4: 用内部辅助函数处理 delta 合并**

```rust
fn merge_stream_chunk(
    chunk: &CreateChatCompletionStreamResponse,
    text: &mut String,
    tool_calls: &mut Vec<ChatCompletionMessageToolCall>,
    finish_reason: &mut Option<FinishReason>,
    on_text_delta: &mut (dyn FnMut(&str) + Send),
) -> Result<()> {
    // append text deltas, emit callback, merge partial tool call fields
}
```

- [ ] **Step 5: 复用现有 finish reason / tool call 映射**

```rust
fn build_streamed_response(
    text: String,
    tool_calls: Vec<ChatCompletionMessageToolCall>,
    finish_reason: Option<FinishReason>,
) -> Result<ProviderResponse> {
    // build ProviderContentBlock::Text and ToolUse as needed
}
```

- [ ] **Step 6: 运行 OpenAI 模块测试**

Run: `cargo test llm::openai::tests -- --nocapture`
Expected: PASS

- [ ] **Step 7: 提交这一阶段**

```bash
git add src/llm/openai.rs
git commit -m "feat: stream openai chat completions"
```

### Task 4: 校验 Anthropic 回退与终端行为

**Files:**
- Modify: `d:\3-ai-project\little-agent\src\llm\anthropic.rs`
- Modify: `d:\3-ai-project\little-agent\src\lib.rs`

- [ ] **Step 1: 增加一个最小测试，确认非真流式 provider 仍可走默认实现**

```rust
#[tokio::test]
async fn anthropic_provider_can_use_default_streaming_fallback() {
    // or use a fake provider implementing only send()
}
```

- [ ] **Step 2: 确保流式输出失败时会补换行**

```rust
// add a small helper in lib.rs so newline behavior is testable
```

- [ ] **Step 3: 如果 Anthropic 不需要额外代码，则仅补注释或保持不改**

```rust
// no-op if default trait implementation is sufficient
```

- [ ] **Step 4: 运行相关测试**

Run: `cargo test llm::tests -- --nocapture`
Expected: PASS

- [ ] **Step 5: 提交这一阶段**

```bash
git add src/lib.rs src/llm/anthropic.rs src/llm/mod.rs
git commit -m "test: validate streaming fallback behavior"
```

### Task 5: 全量回归与诊断收尾

**Files:**
- Modify: `d:\3-ai-project\little-agent\src\llm\mod.rs`
- Modify: `d:\3-ai-project\little-agent\src\lib.rs`
- Modify: `d:\3-ai-project\little-agent\src\llm\openai.rs`
- Modify: `d:\3-ai-project\little-agent\src\main.rs`

- [ ] **Step 1: 运行流式相关测试子集**

Run: `cargo test llm::tests llm::openai::tests -- --nocapture`
Expected: PASS

- [ ] **Step 2: 运行全量测试**

Run: `cargo test`
Expected: PASS

- [ ] **Step 3: 运行编译检查**

Run: `cargo check`
Expected: PASS

- [ ] **Step 4: 检查最近改动文件 diagnostics**

Run: 使用 diagnostics 检查：
- `file:///d:/3-ai-project/little-agent/src/llm/mod.rs`
- `file:///d:/3-ai-project/little-agent/src/lib.rs`
- `file:///d:/3-ai-project/little-agent/src/llm/openai.rs`
- `file:///d:/3-ai-project/little-agent/src/main.rs`

Expected: 无新增错误

- [ ] **Step 5: 最终提交**

```bash
git add src/llm/mod.rs src/lib.rs src/llm/openai.rs src/main.rs
git commit -m "feat: stream final text in interactive cli"
```
