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
    let command_lower = command.to_lowercase();

    let dangerous = ["del /f /s /q", "format", "shutdown", "rmdir /s /q"];
    if dangerous.iter().any(|item| command_lower.contains(item)) {
        return Err(anyhow::anyhow!("Error: Dangerous command blocked"));
    }

    let child = match Command::new("cmd.exe")
        .arg("/C")
        .arg(command)
        .current_dir(ctx.work_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Err(anyhow::anyhow!("Error: {}", e)),
    };

    let output_future = child.wait_with_output();
    match timeout(Duration::from_secs(120), output_future).await {
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

#[cfg(all(test, windows))]
mod tests {
    use super::CmdTool;
    use crate::tool::{Tool, tests::test_context};

    #[tokio::test]
    async fn cmd_tool_runs_command_and_returns_output() {
        let context = test_context("cmd_tool_runs_command_and_returns_output");

        let output = CmdTool
            .call(context, serde_json::json!({ "command": "echo hello" }))
            .await
            .unwrap();

        assert_eq!(output, "hello");
    }

    #[tokio::test]
    async fn cmd_tool_returns_no_output_marker_for_silent_command() {
        let context = test_context("cmd_tool_returns_no_output_marker_for_silent_command");

        let output = CmdTool
            .call(context, serde_json::json!({ "command": "ver > nul" }))
            .await
            .unwrap();

        assert_eq!(output, "(no output)");
    }
}
