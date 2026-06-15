# Platform Shell Tool Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为不同操作系统动态加载合适的 shell 工具：Windows 仅加载 `cmd`，其他系统仅加载 `bash`，并让主代理与子代理行为一致。

**Architecture:** 新增 `src/tool/cmd.rs`，以与现有 `bash` 工具相同的输入/输出约定封装 `cmd.exe /C`。在 `src/tool/mod.rs` 提取统一的平台 shell 注册函数，并让 `toolset()` 与 `subagent_toolset()` 共同复用，避免重复平台判断。

**Tech Stack:** Rust、Tokio process/timeout、现有 `tool` proc macro、`schemars`、条件测试

---

## 文件结构

- Create: `d:\3-ai-project\little-agent\src\tool\cmd.rs`
  - 定义 `cmd` 工具、Windows 条件测试
- Modify: `d:\3-ai-project\little-agent\src\tool\mod.rs`
  - 引入 `CmdTool`
  - 提取统一的平台 shell 注册函数
  - 更新 `toolset()` 与 `subagent_toolset()`
  - 增加平台注册测试
- Optional Modify: `d:\3-ai-project\little-agent\src\tool\bash.rs`
  - 仅在需要复用公共常量或保持风格一致时最小调整

### Task 1: 新增 `cmd` 工具

**Files:**
- Create: `d:\3-ai-project\little-agent\src\tool\cmd.rs`

- [ ] **Step 1: 写失败测试，锁定 Windows 下 `cmd` 工具返回命令输出**

```rust
#[cfg(windows)]
#[tokio::test]
async fn cmd_tool_runs_command_and_returns_output() {
    let context = super::test_context("cmd_tool_runs_command_and_returns_output");

    let output = CmdTool
        .call(
            context,
            serde_json::json!({
                "command": "echo hello"
            }),
        )
        .await
        .unwrap();

    assert_eq!(output, "hello");
}
```

- [ ] **Step 2: 写失败测试，锁定空输出返回 `(no output)`**

```rust
#[cfg(windows)]
#[tokio::test]
async fn cmd_tool_returns_no_output_marker_for_silent_command() {
    let context = super::test_context("cmd_tool_returns_no_output_marker_for_silent_command");

    let output = CmdTool
        .call(
            context,
            serde_json::json!({
                "command": "cd ."
            }),
        )
        .await
        .unwrap();

    assert_eq!(output, "(no output)");
}
```

- [ ] **Step 3: 运行 `cmd` 工具测试确认文件尚不存在**

Run: `cargo +1.91.0-x86_64-pc-windows-msvc test cmd_tool_runs_command_and_returns_output -- --nocapture`
Expected: FAIL，提示 `CmdTool` 或 `src/tool/cmd.rs` 不存在

- [ ] **Step 4: 新增 `src/tool/cmd.rs` 最小实现**

```rust
use std::time::Duration;

use crate::tool::ToolContext;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::{process::Command, time::timeout};
use tool_macros::tool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CmdInput {
    #[schemars(description = "Command to run in the current workspace.")]
    pub command: String,
}

#[tool(
    name = "cmd",
    description = "Run a cmd.exe command in the current workspace."
)]
pub async fn cmd(ctx: ToolContext, input: CmdInput) -> Result<String> {
    let command = input.command;

    let dangerous = ["del /f /s /q", "format", "shutdown", "rmdir /s /q"];
    if dangerous
        .iter()
        .any(|item| command.to_lowercase().contains(item))
    {
        return Err(anyhow::anyhow!("Error: Dangerous command blocked"));
    }

    let child = Command::new("cmd.exe")
        .arg("/C")
        .arg(command)
        .current_dir(ctx.work_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| anyhow::anyhow!("Error: {}", e))?;

    match timeout(Duration::from_secs(120), child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let combined = [output.stdout, output.stderr].concat();
            let out_str = String::from_utf8_lossy(&combined);
            let trimmed = out_str.trim();

            if trimmed.is_empty() {
                Ok("(no output)".to_string())
            } else {
                Ok(trimmed.chars().take(50000).collect())
            }
        }
        Ok(Err(e)) => Err(anyhow::anyhow!("Error: {}", e)),
        Err(_) => Err(anyhow::anyhow!("Error: Timeout (120s)")),
    }
}
```

- [ ] **Step 5: 在 `cmd.rs` 中补上 Windows 条件测试模块**

```rust
#[cfg(all(test, windows))]
mod tests {
    use super::CmdTool;
    use crate::tool::{Tool, tests::test_context};

    // place tests here
}
```

- [ ] **Step 6: 运行 `cmd` 工具测试**

Run: `cargo +1.91.0-x86_64-pc-windows-msvc test cmd_tool_ -- --nocapture`
Expected: PASS（Windows）

- [ ] **Step 7: 提交这一阶段**

```bash
git add src/tool/cmd.rs
git commit -m "feat: add cmd shell tool"
```

### Task 2: 提取统一的平台 shell 注册逻辑

**Files:**
- Modify: `d:\3-ai-project\little-agent\src\tool\mod.rs`

- [ ] **Step 1: 写失败测试，锁定当前平台 shell 工具名**

```rust
#[test]
fn toolset_registers_only_platform_shell_tool() {
    let router = toolset();
    let names = router.tool_specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();

    if cfg!(target_os = "windows") {
        assert!(names.iter().any(|name| name == "cmd"));
        assert!(!names.iter().any(|name| name == "bash"));
    } else {
        assert!(names.iter().any(|name| name == "bash"));
        assert!(!names.iter().any(|name| name == "cmd"));
    }
}
```

- [ ] **Step 2: 写失败测试，锁定 `subagent_toolset()` 也遵循同样规则**

```rust
#[test]
fn subagent_toolset_registers_only_platform_shell_tool() {
    let router = subagent_toolset();
    let names = router.tool_specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();

    if cfg!(target_os = "windows") {
        assert!(names.iter().any(|name| name == "cmd"));
        assert!(!names.iter().any(|name| name == "bash"));
    } else {
        assert!(names.iter().any(|name| name == "bash"));
        assert!(!names.iter().any(|name| name == "cmd"));
    }
}
```

- [ ] **Step 3: 运行平台注册测试确认当前仍固定注册 `bash`**

Run: `cargo +1.91.0-x86_64-pc-windows-msvc test platform_shell_tool -- --nocapture`
Expected: FAIL（Windows 下缺少 `cmd`，或测试名尚未存在）

- [ ] **Step 4: 在 `mod.rs` 引入 `cmd` 模块和工具**

```rust
mod cmd;
use cmd::CmdTool;
```

- [ ] **Step 5: 提取统一平台 shell 路由函数**

```rust
fn route_platform_shell(router: ToolRouter) -> ToolRouter {
    if cfg!(target_os = "windows") {
        router.route(CmdTool)
    } else {
        router.route(BashTool)
    }
}
```

- [ ] **Step 6: 修改 `toolset()` 使用统一 shell 路由**

```rust
pub fn toolset() -> ToolRouter {
    route_platform_shell(
        ToolRouter::new()
            .route(AddTool)
            .route(BackgroundRunTool)
            .route(CheckBackgroundTool)
            // keep the rest unchanged
    )
    .route(CronCreateTool)
    // continue existing routes in original order
}
```

- [ ] **Step 7: 修改 `subagent_toolset()` 使用统一 shell 路由**

```rust
pub fn subagent_toolset() -> ToolRouter {
    route_platform_shell(
        ToolRouter::new()
            .route(ReadFileTool)
            .route(WriteFileTool)
            .route(EditFileTool),
    )
}
```

- [ ] **Step 8: 在 `mod.rs` 测试模块中补平台注册测试**

```rust
#[test]
fn platform_shell_tool_is_registered_for_main_toolset() {
    // place exact assertions here
}

#[test]
fn platform_shell_tool_is_registered_for_subagent_toolset() {
    // place exact assertions here
}
```

- [ ] **Step 9: 运行工具模块测试**

Run: `cargo +1.91.0-x86_64-pc-windows-msvc test platform_shell_tool -- --nocapture`
Expected: PASS

- [ ] **Step 10: 提交这一阶段**

```bash
git add src/tool/mod.rs
git commit -m "feat: route shell tools by platform"
```

### Task 3: 全量回归与诊断收尾

**Files:**
- Modify: `d:\3-ai-project\little-agent\src\tool\cmd.rs`
- Modify: `d:\3-ai-project\little-agent\src\tool\mod.rs`
- Optional Modify: `d:\3-ai-project\little-agent\src\tool\bash.rs`

- [ ] **Step 1: 运行工具相关测试子集**

Run: `cargo +1.91.0-x86_64-pc-windows-msvc test src::tool -- --nocapture`
Expected: PASS

- [ ] **Step 2: 运行全量测试**

Run: `cargo +1.91.0-x86_64-pc-windows-msvc test`
Expected: PASS

- [ ] **Step 3: 运行编译检查**

Run: `cargo +1.91.0-x86_64-pc-windows-msvc check`
Expected: PASS

- [ ] **Step 4: 检查 diagnostics**

Run: 使用 diagnostics 检查：
- `file:///d:/3-ai-project/little-agent/src/tool/cmd.rs`
- `file:///d:/3-ai-project/little-agent/src/tool/mod.rs`
- 如果有改动，检查 `file:///d:/3-ai-project/little-agent/src/tool/bash.rs`

Expected: 无新增错误

- [ ] **Step 5: 最终提交**

```bash
git add src/tool/cmd.rs src/tool/mod.rs src/tool/bash.rs
git commit -m "feat: load platform shell tool"
```
