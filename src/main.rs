use std::{path::PathBuf, sync::Arc};

use anyhow::Context;
use inquire::{Select, Text};

use little_agent::{
    Agent, AgentSystemPrompt,
    attachment::{build_user_message, load_attachment_blocks, parse_attachment_input},
    background::SharedBackgroundManager,
    config::AgentConfig,
    cron::{CronScheduler, SharedCronScheduler},
    llm::build_provider,
    mcp::load_mcp_router,
    memory::get_memory_manager,
    permission::{PermissionManager, PermissionMode},
    skill::get_skill_registry,
    store::StoreRoot,
    task::{SharedTaskManager, TaskManager},
    team::{SharedTeammateManager, TeammateManager},
    tool::{ToolContext, toolset},
    worktree::{SharedWorktreeManager, WorktreeManager},
};

const SKILLS_DIR: &str = "skills";
const AGENT_CONFIG_PATH: &str = ".claude/agent.toml";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let work_dir = std::env::current_dir()?;
    let config_path = work_dir.join(AGENT_CONFIG_PATH);
    let config = Arc::new(AgentConfig::load(&config_path)?);
    let provider = build_provider(config.clone())?;

    let mode = Select::new(
        "Permission mode:",
        vec![
            PermissionMode::Default,
            PermissionMode::Plan,
            PermissionMode::Auto,
        ],
    )
    .prompt()
    .context("An error happened or user cancelled the input.")?;
    let permission_manager = PermissionManager::try_new(mode)?;
    println!("[Permission mode: {}]", permission_manager.mode());

    let skills_dir = work_dir.join(SKILLS_DIR);
    let skill_registry = Arc::new(get_skill_registry(skills_dir)?);
    let store_root = StoreRoot::new(work_dir.join(".claude"))?;
    let task_manager = SharedTaskManager::new(TaskManager::new(&store_root)?);
    let background_manager = SharedBackgroundManager::new(&store_root)?;
    let cron_scheduler = SharedCronScheduler::new(CronScheduler::new(&store_root)?);
    let teammate_manager = SharedTeammateManager::new(TeammateManager::new(&store_root)?);
    let worktree_manager =
        SharedWorktreeManager::new(WorktreeManager::new(&store_root, work_dir.clone())?);
    let memory_manager = Arc::new(std::sync::Mutex::new(get_memory_manager(
        work_dir.join(".claude/memory"),
    )?));
    let mcp_router = load_mcp_router().await?;

    let tools = toolset();
    let tool_context = ToolContext {
        config: config.clone(),
        skill_registry: skill_registry.clone(),
        memory_manager,
        work_dir,
        task_manager,
        background_manager,
        cron_scheduler,
        teammate_manager,
        worktree_manager,
    };

    let mut agent = Agent::new(
        config,
        provider,
        tool_context,
        tools,
        mcp_router,
        permission_manager,
        AgentSystemPrompt::Dynamic,
    );

    loop {
        let query = Text::new("--- How can I help you?")
            .prompt()
            .context("An error happened or user cancelled the input.")?;

        //break out of the loop if the user enters exit()
        if query.trim() == "exit()" {
            break;
        }
        let attachment_input =
            Text::new("--- Optional attachments (semicolon-separated paths, leave blank for none)")
                .prompt()
                .context("An error happened or user cancelled the input.")?;
        let attachment_paths = parse_attachment_input(&attachment_input)?
            .into_iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        let attachments = load_attachment_blocks(&attachment_paths)?;
        agent
            .runtime
            .context
            .push(build_user_message(query, attachments));

        agent.agent_loop_streaming().await?;
    }

    Ok(())
}
