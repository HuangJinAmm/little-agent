# Multi-Provider Agent Design

## 背景

当前项目的 Agent 运行时与 Anthropic SDK 耦合较深，耦合点主要集中在：

- `src/lib.rs` 中的请求构造、响应解析、tool use 循环与 continuation 处理
- `anthropic_ai_sdk::types::message::*` 类型直接出现在主运行时
- `get_llm_client()` 与 `AgentRuntime.client` 直接绑定 `AnthropicClient`

在上一轮配置改造后，项目已经具备集中配置入口，但 LLM provider 仍是单实现。现在需要新增对 OpenAI 兼容规范的 agent 支持，同时保留现有 Anthropic 能力，并为后续接入更多 provider 打开扩展路径，尤其是本地 OpenAI-compatible 服务，例如 LM Studio。

## 目标

- 保留现有 Anthropic 路径，不破坏当前行为
- 新增 OpenAI-compatible provider，实现双后端支持
- 将 Agent 主循环从具体 SDK 类型中抽离，面向统一 provider 接口
- 第一版尽量覆盖当前现有能力：system prompt、多轮消息、tool calling、continuation、compact 摘要调用
- 配置层支持按 provider 切换，并为未来扩展更多 provider 留出结构空间
- 后续可在不重写主循环的前提下接入 LM Studio 这类本地兼容服务

## 非目标

- 本次不实现流式输出
- 本次不扩展 embedding、images、audio 等非当前 agent 必需能力
- 本次不尝试兼容所有 OpenAI Responses API 和 Chat Completions API 的边缘差异
- 本次不对工具系统做协议无关重写，保留当前本地工具注册方式

## 设计总览

采用“Provider 抽象层 + 双实现”的方案：

- 新增统一 `llm` 模块，承载 provider trait、内部通用请求/响应结构和 provider 工厂
- 保留 Anthropic 作为一个 provider 实现
- 新增 `OpenAiCompatibleProvider` 作为第二个 provider 实现
- `Agent` 主循环不再依赖 Anthropic SDK 的响应结构，而是只依赖项目内部定义的 provider 返回值

整体方向是：

1. 将“协议和 SDK 差异”下沉到 provider 实现
2. 将“agent 行为编排”保留在主循环
3. 将“消息与工具调用的最小公共语义”上收为内部类型

这是一种偏保守的演进式重构，避免一次性彻底重写消息系统，但也不继续在 `lib.rs` 中硬编码不同 provider 分支。

## 配置设计

在现有 `src/config.rs` 的基础上扩展 provider 相关配置。推荐结构如下：

```toml
provider = "anthropic"

[anthropic]
model = "claude-sonnet-4-5"
api_key = "your-api-key"
base_url = "https://api.anthropic.com"

[openai_compatible]
model = "gpt-4.1-mini"
api_key = "your-api-key"
base_url = "https://api.openai.com/v1"

[runtime]
context_limit = 50000
max_tokens = 8000
```

也支持将 `provider = "openai_compatible"`，并通过 `[openai_compatible]` 读取对应配置。

这样设计的原因是：

- provider 选择为单值，避免并发启用多个模型源造成运行时歧义
- provider 专属配置天然按 section 隔离
- LM Studio 后续可直接沿用 `openai_compatible` 协议结构，只需给出本地 `base_url`

后续若需要专门支持 `lm_studio`，可以：

- 继续复用 `openai_compatible`
- 或新增独立 section 并实现新的 provider，不影响主循环

## 核心抽象

### Provider Trait

新增类似如下的统一接口：

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &'static str;

    async fn send(
        &self,
        request: ProviderRequest,
    ) -> anyhow::Result<ProviderResponse>;
}
```

这个 trait 只负责一次请求到一次响应的转换，不负责 Agent 的工具执行循环，也不持有对话状态。

这样设计的好处是：

- provider 逻辑单一，只做协议适配
- 主循环仍掌控上下文、compact、continuation、权限和工具执行
- 新增 provider 时，影响范围受限在 `llm/` 模块

### 内部通用请求结构

主循环给 provider 的请求应是内部统一结构，而不是 Anthropic 或 OpenAI 的 SDK 请求类型。例如：

```rust
pub struct ProviderRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<ProviderMessage>,
    pub tools: Vec<ProviderToolSpec>,
    pub max_tokens: u32,
}
```

其中 `ProviderMessage` 需要表达当前 agent 真正依赖的几类语义：

- `system`
- `user`
- `assistant`
- `tool_result`

由于现有主循环依赖“assistant 响应里可能带文本块和 tool use 块”的能力，因此消息内容也需要内部抽象。

### 内部通用响应结构

provider 返回值应至少覆盖当前主循环实际使用到的字段：

```rust
pub struct ProviderResponse {
    pub content: Vec<ProviderContentBlock>,
    pub stop_reason: Option<ProviderStopReason>,
}
```

推荐定义：

- `ProviderContentBlock::Text { text }`
- `ProviderContentBlock::ToolUse { id, name, input }`
- `ProviderContentBlock::ToolResult { tool_use_id, content }`

其中 `ToolResult` 主要用于上下文统一建模，provider 实际返回时未必会生成它，但主循环需要这一类块来回写工具执行结果。

`ProviderStopReason` 推荐覆盖：

- `EndTurn`
- `ToolUse`
- `MaxTokens`

这已经足够覆盖当前行为路径。

## 文件结构设计

建议新增以下模块：

- `src/llm/mod.rs`
  - 暴露公共 trait、内部类型与 provider 工厂
- `src/llm/types.rs`
  - 定义 `ProviderRequest`、`ProviderResponse`、`ProviderContentBlock`、`ProviderStopReason`
- `src/llm/anthropic.rs`
  - 现有 Anthropic 路径的 provider 化封装
- `src/llm/openai_compatible.rs`
  - OpenAI-compatible provider 实现

现有文件修改方向：

- `src/lib.rs`
  - 改为依赖 `Arc<dyn LlmProvider>`
  - 主循环上下文改存项目内部消息类型，而不是 Anthropic `Message`
- `src/main.rs`
  - 根据配置选择 provider 并创建实例
- `src/config.rs`
  - 增加 provider 枚举和 provider-specific 配置

## 主循环改造

### 当前问题

当前 `AgentRuntime.context` 是 `Vec<Message>`，直接绑定 Anthropic 消息类型。这会导致：

- OpenAI-compatible provider 很难直接复用上下文
- 工具结果回写必须使用 Anthropic `ContentBlock::ToolResult`
- continuation 和 compact 调用都被迫通过 Anthropic 请求格式表达

### 改造方向

主循环应改为使用项目内部上下文结构，例如：

```rust
pub struct ProviderMessage {
    pub role: ProviderRole,
    pub content: Vec<ProviderContentBlock>,
}
```

或保守一些，使用枚举表达：

```rust
pub enum ProviderMessage {
    User { content: String },
    Assistant { content: Vec<ProviderContentBlock> },
}
```

推荐采用第一种结构化方案，因为：

- 更适合后续 provider 扩展
- 更容易表达 tool result 与 assistant blocks 的混合内容
- compact / transcript / extract_text 的适配更一致

主循环保留现有逻辑顺序：

1. 拼 system prompt
2. 根据 provider 配置构造 `ProviderRequest`
3. 发送请求
4. 将 provider 响应追加到内部上下文
5. 如遇 `ToolUse`，执行本地工具并回写 `ToolResult`
6. 如遇 `MaxTokens`，追加 continuation 用户消息并重试
7. 如结束则返回

## Anthropic Provider 设计

Anthropic provider 负责：

- 将内部 `ProviderRequest` 映射为 Anthropic `CreateMessageParams`
- 将内部 `ProviderToolSpec` 映射为 Anthropic tool schema
- 将 Anthropic `ContentBlock` 映射回内部 `ProviderContentBlock`
- 将 Anthropic `StopReason` 映射为内部 `ProviderStopReason`

这样可以保证现有 Anthropic 行为基本不变，只是挪动位置。

## OpenAI-Compatible Provider 设计

### 协议选择

第一版建议直接面向 OpenAI-compatible 的 HTTP JSON 协议实现，不强依赖某个特定 SDK。推荐原因：

- 需要支持自定义 `base_url`
- 需要兼容官方 OpenAI 与 LM Studio 这类本地兼容服务
- 通过直接控制请求/响应 JSON，更容易处理不同兼容实现的小差异

因此推荐新增 `reqwest` 作为 HTTP 客户端依赖，并用 `serde` 定义最小请求/响应结构。

### 首版能力范围

OpenAI-compatible provider 首版需要支持：

- `system` + 多轮 `messages`
- `tools`
- `tool_calls`
- `finish_reason`
- `max_tokens`

请求可以基于 Chat Completions 兼容格式：

```json
{
  "model": "...",
  "messages": [...],
  "tools": [...],
  "max_tokens": 8000
}
```

响应适配重点：

- `message.content` 映射为 `ProviderContentBlock::Text`
- `message.tool_calls` 映射为 `ProviderContentBlock::ToolUse`
- `finish_reason = "tool_calls"` 映射为 `ProviderStopReason::ToolUse`
- `finish_reason = "length"` 映射为 `ProviderStopReason::MaxTokens`
- 其他结束原因映射为 `EndTurn`

### LM Studio 兼容策略

LM Studio 常见接入方式是提供本地 OpenAI-compatible base URL，因此本次不需要专门为 LM Studio 做独立协议，只需确保：

- `base_url` 可配置
- `api_key` 可选或允许空值
- 对 OpenAI-compatible 返回中的非关键字段保持宽容解析

这能覆盖后续本地模型运行场景。

## 工具调用与上下文回写

工具系统本身保持不变，仍由：

- `ToolRouter`
- `MCPToolRouter`
- `Hook`
- `PermissionManager`

共同完成。

需要变化的是“工具调用在上下文中的表达形式”：

- provider 返回 tool call 后，主循环统一把它保存为内部 `ProviderContentBlock::ToolUse`
- 工具执行完成后，主循环统一追加 `ProviderContentBlock::ToolResult`
- 下一轮发给不同 provider 时，再由各 provider 转换为它们各自支持的 tool message 格式

这样可以保证 tool loop 仍只有一套，而不是每个 provider 自己再做一遍。

## Compact 与 Continuation

### Continuation

当前 continuation 依赖 `StopReason::MaxTokens`。改造后由 provider 统一映射为 `ProviderStopReason::MaxTokens`，主循环无需关心底层协议差异。

### Compact

当前 compact 也是一次普通模型调用，因此应直接复用当前 provider：

- Anthropic 配置时，compact 走 Anthropic provider
- OpenAI-compatible 配置时，compact 走 OpenAI-compatible provider

这能保持行为一致，也避免 compact 单独绑定 Anthropic。

## 错误处理

需要分三层处理错误：

### 配置层

- provider 未知
- 选中的 provider 缺失对应 section
- provider 必需字段为空

### Provider 调用层

- HTTP 请求失败
- 鉴权失败
- base URL 非法
- 返回体缺失关键字段
- tool call 参数 JSON 非法

### 语义适配层

- provider 返回了主循环无法理解的 finish reason
- provider 返回了不支持的内容类型

错误信息需要明确标识 provider 名称，便于排查。

## 测试策略

建议拆成三层测试：

### 配置测试

- Anthropic 配置可加载
- OpenAI-compatible 配置可加载
- provider 与 section 不匹配时报错

### 纯映射测试

- Anthropic SDK 响应到内部类型的映射测试
- OpenAI-compatible JSON 响应到内部类型的映射测试
- tool call / finish_reason / max token 场景测试

### 运行时回归测试

- `extract_text` 对内部消息结构仍能正确提取最终文本
- `agent_loop` 在 tool use、continuation、normal end 三种路径上行为不变

对 OpenAI-compatible provider 的 HTTP 行为，推荐优先使用最小 mock server 或纯 JSON 反序列化测试，不需要在第一版引入复杂集成测试框架。

## 渐进迁移顺序

推荐按以下顺序实施：

1. 先定义内部消息与 provider 接口
2. 将 Anthropic 路径迁移为 provider 实现，确保行为不回归
3. 再接入 OpenAI-compatible provider
4. 扩展配置与入口工厂
5. 最后补回归测试和 provider 映射测试

这种顺序能保证每一步都可单独验证，不会在同一次提交里同时处理“抽象重构 + 新协议接入”的双重风险。

## 验收标准

- 通过配置可在 Anthropic 与 OpenAI-compatible provider 间切换
- Anthropic 现有行为保持可用
- OpenAI-compatible provider 支持 system prompt、多轮消息、tool calling、continuation、compact
- `Agent` 主循环不再直接依赖 Anthropic SDK 的消息类型
- 后续新增 provider 时，不需要重写 `agent_loop`
- 本地 LM Studio 类服务可通过 OpenAI-compatible 配置路径接入
