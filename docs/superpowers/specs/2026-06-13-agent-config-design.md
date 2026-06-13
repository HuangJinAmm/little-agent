# Agent Config Design

## 背景

当前 `src/lib.rs` 中仍直接通过环境变量读取以下运行配置：

- `ANTHROPIC_MODEL`
- `ANTHROPIC_API_KEY`
- `ANTHROPIC_BASE_URL`

同时，部分运行时参数仍以常量或内联字面量存在：

- `CONTEXT_LIMIT`
- `CreateMessageParams.max_tokens`

这会带来几个问题：

- 配置来源分散在 `lib.rs`，无法统一管理
- 新增配置项时需要继续修改核心逻辑
- 不同模块未来若需要接入配置，缺少统一扩展入口

本次改造目标是将项目中由 `lib.rs` 负责的运行配置迁移到指定的 `toml` 文件中，并提供一个可扩展的 trait，使后续新增配置 section 时能够低成本接入。

## 目标

- 将 `src/lib.rs` 中需要配置化的项目统一改为从指定 `toml` 文件读取
- 使用强类型根配置承载已知配置项
- 提供可复用的泛型 section trait，支持后续新增配置 section
- 让 `lib.rs` 只依赖配置对象，不再直接读取环境变量
- 将配置错误尽量前置为启动时错误，而不是运行时隐式失败

## 非目标

- 本次不改造现有 `.claude` 下的 `json` 存储结构
- 本次不统一改造项目中所有 `std::env::*` 的使用场景
- 本次不引入热加载、配置监听或多配置源合并
- 本次不重构 `main.rs` 的交互式权限模式选择

## 配置文件结构

约定新增一个显式的 `toml` 配置文件，由启动阶段统一加载。配置结构如下：

```toml
[llm]
model = "claude-sonnet-4-5"
api_key = "your-api-key"
base_url = "https://api.anthropic.com"

[runtime]
context_limit = 50000
max_tokens = 8000
```

本次设计只要求配置加载层支持“指定路径的 toml 文件”，路径的最终注入方式可在实现时选择最小改动方案，例如：

- 先提供一个默认常量路径
- 或在 `main.rs` 启动时显式传入

优先级以实现简单、调用清晰为准，但不再回退为 `lib.rs` 直接读环境变量。

## 核心设计

### 强类型根配置

新增 `src/config.rs` 模块，定义根配置对象：

```rust
pub struct AgentConfig {
    pub llm: LlmConfig,
    pub runtime: RuntimeConfig,
}
```

其中：

- `LlmConfig` 负责承载模型、鉴权和网关地址
- `RuntimeConfig` 负责承载上下文限制和请求 token 上限

所有结构体均通过 `serde::Deserialize` 从 `toml` 反序列化得到。

### 泛型 Section Trait

为支持后续模块按 section 自主扩展，新增一个通用 trait，例如：

```rust
pub trait ConfigSection: Sized + serde::de::DeserializeOwned {
    const SECTION: &'static str;

    fn from_root(root: &toml::Value) -> anyhow::Result<Self> {
        let value = root
            .get(Self::SECTION)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing config section: {}", Self::SECTION))?;
        value
            .try_into::<Self>()
            .map_err(|err| anyhow::anyhow!("invalid config section {}: {}", Self::SECTION, err))
    }

    fn validate(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
```

设计原则：

- section 名由类型自己声明，而不是由调用方硬编码
- `from_root` 提供统一默认实现，减少重复样板代码
- `validate` 作为扩展点，允许各 section 在反序列化后补充语义校验

后续若新增 `McpConfig`、`MemoryConfig`、`PermissionConfig` 等模块，只需：

- 定义对应结构体
- 实现 `ConfigSection`
- 在根配置或对应消费方中接入

### 根配置加载器

在 `src/config.rs` 中提供统一入口，例如：

```rust
impl AgentConfig {
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self>;
}
```

加载职责包括：

- 读取文件内容
- 解析为 `toml::Value`
- 通过 section trait 构造各 section
- 调用各 section 的 `validate`
- 最终组装为 `AgentConfig`

这样可以同时满足两个目标：

- 已知配置仍保持强类型聚合
- 底层解析过程具备按 section 扩展的统一能力

## 代码迁移边界

### `src/lib.rs`

以下逻辑将迁移为读取配置对象：

- `get_model()`
- `get_llm_client()`
- `CONTEXT_LIMIT`
- `CreateMessageParams::new(...).max_tokens`

建议改造方向：

- 用配置访问替代 `std::env::var("ANTHROPIC_*")`
- 移除 `dotenvy::dotenv()` 在 `lib.rs` 的配置职责
- 将上下文限制与请求 token 数改为从 `AgentConfig.runtime` 读取

`lib.rs` 改造后不再负责“如何找到配置源”，只消费已经加载好的配置，或通过统一配置访问函数读取。

### `src/main.rs`

启动入口负责尽早加载配置，并在创建 `Agent` 或 LLM client 之前完成校验。

实现上允许两种小范围落地方式：

1. 在启动流程最前面显式调用 `AgentConfig::load(...)`
2. 在 `get_model()` / `get_llm_client()` 背后使用一次性缓存配置

推荐优先考虑“显式加载后注入”，理由如下：

- 依赖关系更清晰
- 更利于测试
- 避免隐藏式全局状态

但如果为了控制本次改动面，采用“受控的懒加载缓存”也可以接受，前提是实现必须保持线程安全且错误信息明确。

## 错误处理

配置相关错误应在启动阶段尽早暴露，并附带明确上下文。典型错误包括：

- 配置文件不存在
- `toml` 语法错误
- 缺失必要 section
- section 字段缺失或类型不匹配
- 语义校验失败，例如 `context_limit == 0`

错误信息应包含：

- 配置文件路径
- section 名称
- 字段语义

避免使用模糊的 “not set” 风格提示，因为配置源已不再是环境变量。

## 兼容性决策

本次改造后，`lib.rs` 的主配置源为 `toml` 文件，不再以环境变量作为主流程。

是否保留环境变量兜底不纳入本次范围，默认不做混合读取，理由如下：

- 避免配置源再次分散
- 避免不同来源优先级引发歧义
- 简化排障路径

如后续确有需要，可在配置加载层单独设计“覆盖策略”，而不是在业务逻辑中混入环境变量读取。

## 测试策略

建议增加针对 `config.rs` 的聚焦测试，覆盖以下场景：

- 能成功加载合法的 `toml`
- 缺失 section 时返回可读错误
- 字段类型错误时返回可读错误
- `validate` 触发语义错误时返回可读错误

对 `lib.rs` 和 `main.rs` 的改造，以编译通过和现有行为不回归为主，不需要为简单字段透传补充低价值测试。

## 实施步骤

1. 新增 `toml` 依赖
2. 新增 `src/config.rs`，实现根配置、section trait 和加载逻辑
3. 在 `lib.rs` 中接入配置读取，替换环境变量和运行时常量
4. 在 `main.rs` 中确定配置加载入口和传递方式
5. 补充针对配置加载的单元测试
6. 运行 `cargo test` 与诊断检查，修正编译或类型问题

## 验收标准

- 项目启动时从指定 `toml` 文件读取 `llm` 与 `runtime` 配置
- `src/lib.rs` 不再直接读取 `ANTHROPIC_MODEL`、`ANTHROPIC_API_KEY`、`ANTHROPIC_BASE_URL`
- `context_limit` 与 `max_tokens` 改为来自配置文件
- 新 section 可以通过实现 trait 接入，不需要复制粘贴读取逻辑
- 配置错误在启动阶段能得到可定位的错误信息
