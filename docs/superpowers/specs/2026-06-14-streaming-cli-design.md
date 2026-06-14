# Streaming CLI Output Design

## 背景

当前交互式 CLI 的工作方式是：

1. `main.rs` 读取一轮用户输入
2. 调用 `agent.agent_loop().await?`
3. 整个 provider 请求、工具循环、恢复逻辑都完成后
4. 再从最后一条 assistant 消息中提取文本并一次性打印

这意味着：

- 首字时间取决于整个请求完成时间
- 用户在模型生成过程中看不到渐进输出
- 即使底层 OpenAI/Anthropic 支持流式，也没有被上层利用

本次目标是把“交互式窗口”的最终文本改成流式输出，同时保持现有 tool loop、compact、history 写入和多 provider 架构不被破坏。

## 目标

- 交互式主窗口支持最终文本的真流式输出
- 流式输出过程中，终端按增量文本实时打印
- 流结束后仍组装出完整 `ProviderResponse`
- 完整响应仍写入上下文，保证 tool loop、compact、transcript 和历史一致
- 优先支持 OpenAI provider 的真流式
- Anthropic provider 允许第一版内部回退到非流式，但接口要统一

## 非目标

- 本次不要求工具执行结果也变成流式
- 本次不要求 tool call 前的所有中间阶段都流式显示
- 本次不重写 recovery、compact 的整体架构
- 本次不改变附件输入与工具系统
- 本次不引入新的 UI 框架或 TUI 组件

## 推荐方案

采用“provider trait 增加流式接口 + CLI 侧打印增量文本 + 最终仍产出完整响应”的方案：

- 保留 `send()`
- 新增统一流式接口
- 在 `agent_loop()` 内对交互式主窗口调用流式接口
- 对不支持真流式的 provider，允许回退到非流式实现

这个方案的关键优点是：

1. 流式是 provider 能力，而不是 CLI 小技巧
2. 主循环仍然基于统一 `ProviderResponse` 工作
3. 不会破坏当前的 tool loop 和 compact 行为
4. OpenAI 与 Anthropic 可以逐步补齐，而不用一次重写所有逻辑

## 方案对比

### 方案 A：只在 CLI 做伪流式

- provider 继续一次性返回完整结果
- CLI 再按字符慢慢打印

优点：

- 实现最简单

缺点：

- 不改善首字时间
- 不是真流式
- 只改善视觉表现，不改善交互体验

### 方案 B：provider trait 增加流式接口

- `LlmProvider` 提供统一的 streaming 方法
- CLI 只负责消费事件并打印
- 最终仍得到完整 `ProviderResponse`

优点：

- 结构最清晰
- 和现有多 provider 架构一致
- 后续扩展 tool 前文本流式也更自然

缺点：

- 要改 trait 和 `agent_loop()`

### 方案 C：先只为 OpenAI 做专用流式分支

- `main.rs` 或 `lib.rs` 判断 provider 类型
- 只有 OpenAI 走 streaming，其他 provider 走旧路径

优点：

- 见效快

缺点：

- 抽象层被破坏
- provider 行为不一致

## 最终选择

采用方案 B。

Anthropic 第一版可以在实现上回退到非流式，但接口层统一按流式能力设计，避免以后再拆接口。

## 架构设计

### LlmProvider 扩展

在 `src/llm/mod.rs` 中扩展 `LlmProvider`，新增流式接口。推荐不要把底层 SDK 的 event 类型暴露出去，而是定义项目内部事件，例如：

```rust
pub enum ProviderStreamEvent {
    TextDelta(String),
    Done(ProviderResponse),
}
```

或者更稳妥一点，不暴露 `Done` 事件，而是让流式接口返回：

- 一个异步事件流，只产出文本增量
- 一个最终完整响应

但从 Rust 落地复杂度考虑，第一版更推荐 provider 内部直接完成聚合，并通过 callback 推送文本增量。

推荐接口形状：

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

这样：

- trait 有默认实现
- 不支持真流式的 provider 无需立刻重写
- 支持真流式的 provider 可以覆盖实现

## 运行时改造

### Agent 主循环

`src/lib.rs` 中的 `agent_loop()` 当前直接调用 `provider.send(request)`。

本次建议新增一个专用于主交互路径的流式分支，例如：

- `agent_loop()` 默认启用流式打印
- 但在 compact 这类内部请求中继续使用 `send()`

为了尽量减少改动，推荐：

1. 提取“发送一轮请求”的逻辑为一个私有辅助函数
2. 增加参数控制是否流式打印

例如：

- `request_once(system, stream_text: bool) -> Result<ProviderResponse>`

当 `stream_text = true`：

- 调用 `send_streaming()`
- 在 callback 中把文本增量打印到终端

当 `stream_text = false`：

- 保持原来的 `send()`

### 只在交互式主窗口启用

第一版流式仅用于主窗口的正常问答轮次：

- `main.rs -> agent_loop()` 触发的用户交互请求：启用流式
- `compact_history()`：继续非流式
- 其它内部请求：继续非流式

这样能最大化降低回归风险。

## CLI 行为

### 输出策略

在流式过程中：

- 文本增量立刻 `print!`
- 及时 `stdout().flush()`
- 流完成后补一个换行，避免后续日志粘在同一行

### 无文本场景

如果本轮响应只有 tool call、没有任何文本：

- 不打印空白流式输出
- 后续工具调用逻辑照常执行

### 部分文本后失败

如果流式过程中已经输出了部分文本，但请求最终失败：

- 先打印换行
- 再走现有错误逻辑

这可以避免终端提示和半截文本粘连。

## Provider 实现策略

### OpenAI

`src/llm/openai.rs` 当前基于 `async-openai` 的 chat completions 非流式接口。

第一版推荐：

- 使用 `chat().create_stream(...)`
- 逐步读取 delta
- 将文本 delta 通过 callback 交给 CLI
- 同时在 provider 内部聚合完整 assistant 文本
- 如果响应包含 tool calls，也要在流结束后还原成完整 `ProviderResponse`

必须保持：

- 现有 tool calling 语义不变
- `finish_reason -> ProviderStopReason` 映射不变

### Anthropic

如果当前 SDK 支持稳定的 streaming：

- 接入真流式

如果不支持或改动成本过高：

- 使用 trait 默认实现回退

这样第一版就能先满足主窗口流式体验，同时不阻塞多 provider 结构。

## 数据一致性

流式打印出的文本，必须与最终进入：

- `self.runtime.context`
- compact transcript
- transcript 写盘

的文本保持一致。

因此禁止在 CLI 里单独维护一套显示文案；增量输出只能来自 provider 的真实响应聚合过程。

## 错误处理

### 请求前错误

请求构造失败、schema 失败等：

- 保持现有错误路径

### 流式中错误

如果在流式过程中出现传输错误：

- 尚未输出文本：按现有恢复/重试逻辑处理
- 已输出部分文本：先换行，再继续恢复/重试或报错

### provider 回退

若 provider 未覆写 `send_streaming()`：

- 自动走默认实现
- 行为退化为“整段文本一次性输出”

这不会比当前更差。

## 测试策略

### 抽象层测试

为 `LlmProvider` 流式默认实现增加测试：

- 默认实现会把响应中的 text block 回调出去
- 不会回调 `ToolUse` 或 `ToolResult`

### OpenAI 测试

- 流式 delta 能聚合成完整文本
- `finish_reason` 映射保持不变
- 包含 tool call 的响应在流式路径下仍能得到正确的 `ProviderResponse`

### Agent/CLI 测试

- 流式打印后上下文仍写入完整 assistant 文本
- 没有文本的 tool-only 响应不会打印空输出

### 全量验证

- `cargo test`
- `cargo check`

## 实施顺序

1. 为 `LlmProvider` 增加默认流式接口
2. 在 `lib.rs` 提取单轮请求逻辑并接入流式打印
3. 为 OpenAI provider 增加真流式实现
4. 让 Anthropic 暂时复用默认回退或补真流式
5. 补测试并跑全量验证

## 验收标准

- 交互式主窗口文本响应改为流式输出
- 流式输出后上下文中仍保留完整 assistant 响应
- OpenAI provider 支持真流式
- Anthropic provider 至少兼容统一流式接口
- tool calling、compact、recovery 行为不回归
- `cargo test` 与 `cargo check` 通过
