# Platform Shell Tool Design

## 背景

当前项目只提供 `bash` 工具：

- `src/tool/bash.rs` 定义 `bash` 工具
- `src/tool/mod.rs` 在 `toolset()` 中注册 `BashTool`
- `src/tool/mod.rs` 在 `subagent_toolset()` 中也注册 `BashTool`

这会带来两个问题：

1. 在 Windows 上，工具名和执行环境不匹配，`bash` 语义不准确
2. 主代理和子代理都固定暴露 `bash`，无法根据运行平台切换为更自然的 shell 工具

本次目标是在程序启动时识别当前运行系统：

- Windows 系统加载 `cmd` 工具
- 其他系统加载 `bash` 工具

并且这个行为同时作用于主代理和子代理工具集。

## 目标

- 新增 `cmd` 工具，接口风格与现有 `bash` 工具保持一致
- Windows 平台只注册 `cmd`
- 非 Windows 平台只注册 `bash`
- `toolset()` 与 `subagent_toolset()` 使用同一套平台选择逻辑
- 不改变其他工具的注册顺序和整体结构

## 非目标

- 本次不新增 PowerShell 工具
- 本次不重构现有 `bash` 工具的整体执行模型
- 本次不改变工具调用协议或工具 schema 生成机制
- 本次不同时暴露 `bash` 和 `cmd`

## 方案对比

### 方案 A：统一平台 shell 路由

- 新增 `src/tool/cmd.rs`
- 在 `src/tool/mod.rs` 提取统一的 shell 注册函数
- `toolset()` 与 `subagent_toolset()` 都复用该函数
- 根据平台选择注册 `CmdTool` 或 `BashTool`

优点：

- 逻辑集中
- 主代理与子代理行为一致
- 后续扩展新的 shell 工具时改动点少

缺点：

- 需要调整 `mod.rs` 的工具注册代码结构

### 方案 B：分别在两个 toolset 中做平台判断

- `toolset()` 单独判断一次
- `subagent_toolset()` 再判断一次

优点：

- 实现直接

缺点：

- 存在重复逻辑
- 以后修改时容易漏掉其中一处

### 方案 C：保留 `bash` 工具名，内部按平台切换实现

- 工具名字仍叫 `bash`
- Windows 内部走 `cmd.exe /C`
- 其他系统走 `sh -c`

优点：

- 改动表面较小

缺点：

- 不满足“Windows 加载 `cmd` 工具，其他系统用 `bash` 工具”的显式需求
- 工具名与真实执行环境不一致

## 最终选择

采用方案 A：统一平台 shell 路由。

原因：

- 最符合用户对工具暴露层的要求
- 不会让主代理和子代理出现不一致行为
- 便于未来继续扩展 shell 工具

## 设计

### 新增 `cmd` 工具

新增文件：

- `src/tool/cmd.rs`

该工具与 `bash` 工具保持相同的输入形状：

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CmdInput {
    #[schemars(description = "Command to run in the current workspace.")]
    pub command: String,
}
```

工具定义方向：

- 工具名：`cmd`
- 描述：运行 Windows `cmd.exe` 命令
- 执行方式：`cmd.exe /C <command>`
- 工作目录：`ctx.work_dir`
- 输出策略：
  - 合并 stdout/stderr
  - 空输出返回 `(no output)`
  - 截断到当前 `bash` 工具一致的上限
- 超时策略：
  - 延续 `bash` 当前的 `120s`
- 安全策略：
  - 复用与 `bash` 工具同级别的危险命令拦截思路

### 平台选择逻辑

修改文件：

- `src/tool/mod.rs`

新增：

- `mod cmd;`
- `use cmd::CmdTool;`

并提取统一的 shell 注册函数，例如：

```rust
fn route_platform_shell(router: ToolRouter) -> ToolRouter {
    if cfg!(target_os = "windows") {
        router.route(CmdTool)
    } else {
        router.route(BashTool)
    }
}
```

然后让两个入口都复用：

- `toolset()`
- `subagent_toolset()`

### 工具暴露规则

平台规则明确如下：

- Windows：
  - 注册 `cmd`
  - 不注册 `bash`
- 非 Windows：
  - 注册 `bash`
  - 不注册 `cmd`

这样可以避免模型在同一平台看到两个相似 shell 工具后做出不稳定选择。

## 数据流与运行时行为

程序启动时不需要额外保存平台状态。

工具集构建时直接通过编译目标平台判断：

- `cfg!(target_os = "windows")`

该判断会在运行到 `toolset()` / `subagent_toolset()` 构建阶段生效，最终决定注册哪个 shell 工具。

对上层 Agent 来说，变化只有工具规格列表不同：

- Windows 下工具列表包含 `cmd`
- 非 Windows 下工具列表包含 `bash`

工具调用路径本身不需要调整。

## 错误处理

### Windows 下 `cmd.exe` 不存在

理论上 Windows 环境应当存在 `cmd.exe`。如果启动失败：

- 返回与 `bash` 工具同风格的错误字符串
- 不做额外回退到 PowerShell 或 Bash

### 非法或危险命令

`cmd` 工具保持与 `bash` 同级别的危险命令拦截。

注意本次不追求完美的命令安全模型，只保持与现有系统一致的防护级别。

### 超时

`cmd` 工具与 `bash` 工具一致，超时报：

- `Error: Timeout (120s)`

## 测试策略

### 工具注册测试

在 `src/tool/mod.rs` 中新增最小测试，验证当前平台下注册到的 shell 工具名称：

- Windows 断言存在 `cmd` 且不存在 `bash`
- 非 Windows 断言存在 `bash` 且不存在 `cmd`

同时覆盖：

- `toolset()`
- `subagent_toolset()`

### `cmd` 工具测试

在 `src/tool/cmd.rs` 中新增最小测试：

- 命令成功执行并返回输出
- 空输出返回 `(no output)`

这些测试仅在 Windows 下运行，避免跨平台误报。

### 回归验证

全量至少执行：

- `cargo test`
- `cargo check`

并检查以下文件 diagnostics：

- `src/tool/cmd.rs`
- `src/tool/mod.rs`
- 如果有改动则包含 `src/tool/bash.rs`

## 实施顺序

1. 新增 `src/tool/cmd.rs`
2. 在 `src/tool/mod.rs` 提取统一 shell 注册函数
3. 让 `toolset()` 与 `subagent_toolset()` 共同复用平台路由
4. 增加平台注册测试和 Windows 条件测试
5. 跑测试、编译检查和 diagnostics

## 验收标准

- Windows 平台仅暴露 `cmd`
- 非 Windows 平台仅暴露 `bash`
- 主代理和子代理工具集行为一致
- `cmd` 工具执行逻辑与 `bash` 保持相近风格
- `cargo test` 与 `cargo check` 通过
