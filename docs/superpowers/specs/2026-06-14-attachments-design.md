# Attachment Support Design

## 背景

当前项目的交互入口位于 `src/main.rs`，每轮只接受一行纯文本输入，然后将其包装成：

- `ProviderMessage { role: User, content: [Text] }`

内部消息模型位于 `src/llm/types.rs`，目前只支持三类内容块：

- `Text`
- `ToolUse`
- `ToolResult`

这意味着：

- 用户无法在首轮请求里直接附带本地文件
- 用户无法附带图片作为多模态输入
- 即使后续通过工具读取文件，也不是“用户随消息附带附件”的交互体验

因此本次设计目标是：为 CLI 入口增加本地附件输入能力，并把附件纳入统一的 provider 消息模型中，使 Anthropic 与 OpenAI provider 都能消费这些输入。

## 目标

- 支持在 CLI 中为单轮用户消息附带本地文件路径
- 第一版支持两类附件：
  - 可读取为 UTF-8 文本的文件
  - 常见图片文件
- 附件作为用户消息内容的一部分进入 `ProviderMessage`
- Anthropic 与 OpenAI provider 都支持将附件映射为各自协议可接受的输入
- 不破坏现有 tool calling、compact、主循环与多 provider 架构

## 非目标

- 本次不支持 PDF、Word、Excel、音频、视频
- 本次不支持远程 URL 附件
- 本次不支持超大文件自动切片或分块上传
- 本次不支持跨轮附件缓存与复用
- 本次不新增附件管理工具或 MCP 工具

## 用户交互设计

### CLI 入口

继续保留当前交互式 CLI 模式，但每轮提问流程从：

1. 输入一行文本

扩展为：

1. 输入一行文本
2. 输入可选附件路径列表

推荐交互：

```text
--- How can I help you?
帮我分析这张图和这个配置文件

--- Attachments (optional, separated by ';')
C:\work\diagram.png;C:\work\agent.toml
```

如果附件输入为空，则行为与现在完全一致。

### 路径格式

- 第一版使用本地文件路径
- 多个路径以分号 `;` 分隔，适合 Windows 终端习惯
- 路径在解析时进行 `trim()`

### 错误反馈

在进入 `agent_loop()` 之前完成校验；如果附件存在问题，则本轮输入直接失败并打印错误，不把无效消息送入模型。需要覆盖：

- 文件不存在
- 路径不是普通文件
- 扩展名或 MIME 类型不支持
- 文本文件无法按 UTF-8 读取
- 图片文件过大

## 核心方案

采用“统一附件块模型 + provider 映射”的方案：

- 在 `ProviderContentBlock` 中新增附件类型
- 在 `main.rs` 中把 CLI 输入的附件路径读取并转成内容块
- provider 层分别把这些内容块映射到 Anthropic/OpenAI 的请求结构

这个方案的关键优点是：

1. 用户输入层只负责收集和本地读取
2. 主循环仍只处理统一的 `ProviderMessage`
3. provider 差异被限制在 `src/llm/anthropic.rs` 和 `src/llm/openai.rs`
4. 以后新增 PDF、音频、更多 provider 时可以在同一模型上扩展

## 内部数据模型

### ProviderContentBlock 扩展

在 `src/llm/types.rs` 中新增两种 block：

- `File`
- `Image`

推荐形状：

```rust
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

### 语义约束

- `File` 第一版只表示“已经成功读取为 UTF-8 文本的本地文件”
- `Image` 表示“已经读取为二进制并转成 base64 的图片文件”
- `extract_text_from_blocks()` 继续只提取 `Text`，不自动拼接 `File` 内容，避免破坏当前紧凑化和终端展示逻辑

## 文件类型策略

### 支持的文本文件

第一版只支持“按 UTF-8 读入后作为文本附件发送”的本地文件。可通过扩展名白名单和实际读取双重判断。推荐覆盖：

- `.txt`
- `.md`
- `.rs`
- `.toml`
- `.json`
- `.yaml`
- `.yml`
- `.ts`
- `.tsx`
- `.js`
- `.jsx`
- `.py`
- `.go`
- `.java`
- `.sql`
- `.html`
- `.css`

### 支持的图片文件

推荐第一版支持：

- `.png`
- `.jpg`
- `.jpeg`
- `.webp`

### 大小限制

需要在读取前或读取后做简单大小限制：

- 文本文件：限制为适中的上限，避免把超大文件直接塞进 prompt
- 图片文件：限制为适中的上限，避免 base64 放大后把请求体打爆

设计上不要求复杂分片，只要求超限时明确报错。

## 输入读取与组装

### 新增职责

建议在 `src/main.rs` 附近新增一个小型附件组装单元，例如：

- `src/attachment.rs`

职责：

1. 解析附件路径输入
2. 校验路径
3. 识别文件类型
4. 读取文本或图片内容
5. 生成 `Vec<ProviderContentBlock>`

这样可以避免把 `main.rs` 变成大杂烩。

### 用户消息组装规则

用户消息始终至少包含一条 `Text` block，对应本轮用户提问文本。

如果存在附件，则将其追加到同一条用户消息内容中，例如：

```rust
ProviderMessage::new_blocks(
    ProviderRole::User,
    vec![
        ProviderContentBlock::Text {
            text: query,
        },
        ProviderContentBlock::Image { ... },
        ProviderContentBlock::File { ... },
    ],
)
```

这样 provider 层可以把它们作为“同一轮用户输入”的多个内容块发送。

## Provider 映射设计

### Anthropic

Anthropic provider 原生支持内容块式输入，适合直接扩展：

- `Text` -> text block
- `Image` -> image block
- `File` -> 先降级为 text block

对于 `File`，第一版不依赖 Anthropic 专门的 document/file 能力，而是包装成结构化文本，例如：

```text
[Attached file: agent.toml]
<file-content>
...
</file-content>
```

原因：

- 当前内部 `File` 已经是文本内容
- 降级为 text block 实现成本低，且能保证兼容
- 后续若要支持原生 document block，可以在不改 CLI 入口的前提下升级 provider 映射

### OpenAI `/chat/completions`

OpenAI provider 当前走 chat completions，第一版映射策略：

- `Text` -> user text content
- `Image` -> image_url/data URL 风格内容块
- `File` -> 转为带文件名说明的 text content

同样对 `File` 做文本降级，例如：

```text
[Attached file: agent.toml]
<file-content>
...
</file-content>
```

原因：

- `/chat/completions` 对图片支持明确
- 直接上传“任意文件附件”在当前接口与 SDK 中并不是最稳路径
- 文本文件降级为文字仍符合第一版目标

## OpenAI provider 兼容约束

由于当前 `src/llm/openai.rs` 已经使用 `async-openai`，附件映射必须兼容 SDK 的消息内容结构。

重点要求：

- 不影响现有 `tools`、`tool_calls` 映射
- 用户消息从“纯文本 user message”扩展到“多段内容 user message”
- 图片走 SDK 支持的内容块类型
- 文本附件继续走 text block

如果 SDK 某版本对图片消息类型命名较复杂，实现允许在 `openai.rs` 内封装辅助函数，但不能把 SDK 细节泄漏到主循环。

## 错误处理

### 输入阶段错误

这些错误在进入 provider 前处理：

- 文件不存在：`attachment file does not exist: ...`
- 不是普通文件：`attachment path is not a file: ...`
- 不支持的类型：`unsupported attachment type: ...`
- 文本读取失败：`failed to read text attachment: ...`
- 图片读取失败：`failed to read image attachment: ...`
- 图片过大：`image attachment too large: ...`

### Provider 阶段错误

如果 provider 暂不接受某种 block，应明确失败，而不是静默丢弃。例如：

- `OpenAI provider does not support tool results inside user multimodal messages`
- `Anthropic provider does not support inline image blocks for assistant messages`

不过对当前设计来说，附件只会出现在 `User` 消息中，因此 provider 实现应优先针对这一受控场景。

## 测试策略

### 模型测试

在 `src/llm/types.rs` 中新增测试：

- 附件 block 的序列化/反序列化
- `extract_text_from_blocks()` 不应把 `File` 内容混入普通文本提取

### 附件读取测试

对附件解析模块增加测试：

- 能解析分号分隔路径
- 能把文本文件读成 `File`
- 能把图片读成 `Image`
- 能拒绝不存在的路径
- 能拒绝不支持类型

### Provider 测试

Anthropic：

- 用户消息含 `Image` 时能映射为图片输入
- 用户消息含 `File` 时能降级为 text block

OpenAI：

- 用户消息含 `Image` 时能映射为 chat completions 图片内容
- 用户消息含 `File` 时能降级为 text content
- 不影响现有 tool calling 映射测试

### 全量验证

- `cargo test`
- `cargo check`

## 实施顺序

1. 扩展 `ProviderContentBlock`
2. 新增附件读取与组装模块
3. 修改 `main.rs`，让每轮输入可附带路径列表
4. 扩展 Anthropic provider 的用户消息映射
5. 扩展 OpenAI provider 的用户消息映射
6. 补充单测并跑全量验证

## 验收标准

- CLI 支持输入可选附件路径列表
- 文本文件和图片附件都能加入同一轮用户消息
- Anthropic provider 能消费图片和文本文件附件
- OpenAI provider 能消费图片和文本文件附件
- 不影响现有 tool use、compact、recovery、main loop 行为
- `cargo test` 与 `cargo check` 通过
