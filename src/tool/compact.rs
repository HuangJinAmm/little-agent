use anyhow::Result;
<<<<<<< HEAD
use schemars::JsonSchema;
use serde::Deserialize;
use tool_macros::tool;
=======
use tool_macros::tool;
use schemars::JsonSchema;
use serde::Deserialize;
>>>>>>> 0c5659a893e311f5ea5433d9c152743b5f648219

use crate::tool::ToolContext;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CompactInput {
    #[schemars(description = "Optional focus to preserve in the compacted summary.")]
    pub focus: Option<String>,
}

#[tool(
    name = "compact",
    description = "Summarize earlier conversation so work can continue in a smaller context."
)]
pub async fn compact(_ctx: ToolContext, input: CompactInput) -> Result<String> {
    let focus = input
        .focus
        .map(|focus| format!(" Focus to preserve: {focus}"))
        .unwrap_or_default();
    Ok(format!("Compacting conversation...{focus}"))
}
