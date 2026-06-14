# OpenAI SDK Provider Design

## 背景

项目当前已经具备多 Provider 架构，并且 `openai` provider 已经按 `/chat/completions` 规范工作。但现阶段 `src/llm/openai.rs` 仍然是手写 `reqwest + serde` 的 HTTP/JSON 适配实现。

新的目标是：

- `openai` provider 改为使用 OpenAI 官方 SDK
- 继续沿用当前 `[openai]` 配置结构
- 继续支持自定义 `base_url`
- 不破坏后续接入 LM Studio 等本地 OpenAI 风格服务的能力

这里的关键约束不是“能否访问 OpenAI 官方云端”，而是“在官方 SDK 路径下，仍然保留自定义 `base_url` 和 tool calling 映射能力”。

## 目标

- 用 OpenAI 官方 SDK 替换 `src/llm/openai.rs` 中的手写 HTTP 请求发送逻辑
- 保留当前 `provider = "openai"` 与 `[openai]` 的配置接口
- 保留 `/chat/completions` 语义，而不是切换到不兼容的自定义内部协议
- 保留现有内部 `ProviderRequest` / `ProviderResponse` / `ProviderContentBlock` 抽象
- 保持 LM Studio 这类基于 OpenAI 风格接口的服务可通过 `base_url` 接入

## 非目标

- 本次不改动 Anthropic provider
- 本次不改动 `Agent` 主循环与 compact 逻辑
- 本次不引入流式输出
- 本次不引入 OpenAI Responses API 新抽象；仍以 `/chat/completions` 为目标协议
- 本次不为 LM Studio 单独新增 provider

## 推荐方案

采用“官方 SDK 优先 + provider 内部适配层”的方案：

- `OpenAiProvider` 继续作为项目内部 provider
- 但 `send()` 不再手写 `reqwest.post(.../chat/completions)`，而是调用官方 SDK
- `OpenAiProvider` 继续负责把项目内部 `Provider*` 类型映射到 SDK 请求/响应模型
- `base_url`、`api_key`、`model` 仍由 `[openai]` 配置提供

这个方案的核心原则是：

1. 主循环不感知 SDK
2. SDK 细节不泄漏到 `src/lib.rs`
3. 对 OpenAI 和 LM Studio 的差异，优先封装在 `src/llm/openai.rs`

## 方案对比

### 方案 A：官方 SDK 完全替换手写 HTTP

- 在 `src/llm/openai.rs` 中完全移除 `reqwest` 请求构造
- 所有请求和响应都走官方 SDK 类型

优点：

- 最符合“采用官方 SDK”的目标
- 少掉一套自定义 HTTP 序列化/反序列化代码

风险：

- 如果 SDK 对 `base_url` 或 tool calling 的可定制性有限，实现会受阻
- 需要验证 SDK 是否允许空 `api_key` 或弱鉴权场景

### 方案 B：SDK 优先 + provider 内部窄兼容逻辑

- provider 主体使用官方 SDK
- 若 SDK 对个别字段配置存在硬限制，则在 provider 内部补一层最小兼容处理

优点：

- 最稳妥，兼顾目标与兼容性
- 不需要破坏当前 provider 抽象

缺点：

- 语义上是“SDK 为主”，而不是“100% 不含自定义兼容逻辑”

### 方案 C：OpenAI 官方 SDK 与本地兼容服务拆成两个 provider

- `openai` provider 只服务官方云
- `lm_studio` 或 `openai_local` 单独实现

优点：

- 边界清晰

缺点：

- 不符合当前“必须保留自定义 `base_url`”的目标

## 最终选择

采用方案 B：

- `openai` provider 以官方 SDK 为主实现
- 保留 provider 内部适配层
- 继续支持自定义 `base_url`
- 继续支持空 `api_key` 的本地兼容场景

这是当前目标和现有架构之间成本最低、风险最低的做法。

## 架构设计

### 现有稳定边界

以下边界本次不动：

- `src/lib.rs` 中的 `LlmProvider` 调用方式
- `src/llm/types.rs` 中的内部消息模型
- `src/config.rs` 中的 `ProviderKind::OpenAi` 和 `[openai]` 配置结构

因此这次调整应聚焦在：

- `Cargo.toml`
- `src/llm/openai.rs`

### OpenAI Provider 责任

`OpenAiProvider` 继续负责三件事：

1. 从配置创建 client
2. 把 `ProviderRequest` 映射到 OpenAI chat completions 请求
3. 把 OpenAI chat completions 响应映射回 `ProviderResponse`

换句话说，本次只替换“请求发送与响应类型来源”，不替换 provider 的职责。

### 配置注入

继续沿用：

```toml
provider = "openai"

[openai]
model = "gpt-4.1-mini"
api_key = ""
base_url = "http://127.0.0.1:1234/v1"
```

其中：

- `model` 继续作为每次请求的模型名
- `api_key` 允许为空
- `base_url` 继续支持官方地址与本地地址

## SDK 集成设计

### Client 初始化

`from_config()` 应使用官方 SDK 提供的 client/builder 初始化方式，并注入：

- `api_key`
- `base_url`

要求：

- 当 `api_key` 非空时，按 SDK 规范注入鉴权
- 当 `api_key` 为空时，仍允许构造 client，以支持 LM Studio 等本地服务

如果 SDK 的 builder 默认强制要求 API key，本次实现允许传入占位字符串，但 provider 内必须把这一点写清楚并封装在初始化路径中，不能让调用方承担。

### 请求映射

`ProviderRequest` 到 SDK 请求需要覆盖：

- `model`
- `system`
- `messages`
- `tools`
- `max_tokens`

内部映射规则保持现状：

- `ProviderRole::System` -> `system`
- `ProviderRole::User` -> `user`
- `ProviderRole::Assistant + Text` -> `assistant.content`
- `ProviderRole::Assistant + ToolUse` -> `assistant.tool_calls`
- `ProviderContentBlock::ToolResult` -> `tool` message

即使 SDK 的类型命名不同，这些语义也必须完整保留。

### 响应映射

SDK 响应需要映射为：

- 文本内容 -> `ProviderContentBlock::Text`
- `tool_calls` -> `ProviderContentBlock::ToolUse`
- `finish_reason = tool_calls` -> `ProviderStopReason::ToolUse`
- `finish_reason = length` -> `ProviderStopReason::MaxTokens`
- 其他结束原因 -> `ProviderStopReason::EndTurn`

这部分语义不能退化，否则主循环的 tool loop 与 continuation 会回归。

## 自定义 base_url 与 LM Studio 兼容

这是本次设计的关键要求。

### 兼容目标

以下场景都要可用：

- 官方 OpenAI：`https://api.openai.com/v1`
- 本地 LM Studio：`http://127.0.0.1:1234/v1`
- 其他兼容 `/chat/completions` 的 OpenAI 风格服务

### 兼容策略

- `OpenAiProvider` 不应把 `base_url` 写死
- 仍从 `[openai].base_url` 读取并传给 SDK
- provider 内保持对空 `api_key` 的兼容策略

如果官方 SDK 在某些兼容服务上解析更严格，本次不要求“兼容一切私有扩展字段”，只要求兼容当前 provider 已使用的 `/chat/completions` 必需字段。

## 错误处理

### 配置错误

- `[openai]` section 缺失
- `openai.model` 为空
- `openai.base_url` 为空

### Client 初始化错误

- SDK client 初始化失败
- `base_url` 不合法

### 请求/响应映射错误

- assistant 消息混入非法 block
- tool call 参数无法序列化/反序列化
- SDK 响应缺少 choices
- 返回了不支持的 tool call 类型

错误信息继续保持 provider 语义清晰，优先在文案中出现 `OpenAI` 而不是底层 SDK 内部术语。

## 依赖策略

### 新增

- 增加 OpenAI 官方 Rust SDK 依赖

### 保留或移除

- 如果 `reqwest` 仍被项目其他模块使用，则继续保留
- 如果 `reqwest` 只在 `src/llm/openai.rs` 使用，且迁移后不再需要，可在后续单独清理

本次不强制做依赖瘦身，以降低改动风险。

## 测试策略

### 保留现有映射测试

`src/llm/openai.rs` 现有这类测试应继续存在，只是底层请求/响应结构改成 SDK 版本：

- `maps_provider_messages_and_tools_to_openai_request`
- `maps_tool_calls_finish_reason_to_provider_tool_use`
- `maps_length_finish_reason_to_max_tokens`

### 新增或调整

- 增加 client 初始化测试，确认自定义 `base_url` 可注入
- 增加空 `api_key` 场景测试，确认 provider 初始化不会直接失败

### 全量验证

- `cargo test`
- `cargo check`

## 实施顺序

1. 在 `Cargo.toml` 引入 OpenAI 官方 SDK
2. 重写 `src/llm/openai.rs` 的 client 初始化逻辑
3. 将 chat completions 请求改为使用 SDK 请求类型
4. 将响应解析改为使用 SDK 响应类型
5. 调整并补充单测
6. 运行全量测试和编译检查

## 验收标准

- `openai` provider 使用 OpenAI 官方 SDK 实现请求发送
- 当前 `[openai]` 配置结构保持不变
- `base_url` 仍可配置
- 空 `api_key` 场景仍可用于本地兼容服务
- tool calling、finish reason、continuation 语义保持不变
- `cargo test` 与 `cargo check` 通过
