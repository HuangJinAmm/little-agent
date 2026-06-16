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
use std::{path::PathBuf, sync::Arc};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{mpsc, Mutex};

struct AgentActor {
    tx: mpsc::Sender<String>,
}

#[derive(Clone, serde::Serialize)]
#[serde(tag = "event", content = "data")]
enum ChatEvent {
    Start,
    Token(String),
    Status(String),
    End,
    Error(String),
}

async fn initialize_agent(app: AppHandle) -> anyhow::Result<mpsc::Sender<String>> {
    let (tx, mut rx) = mpsc::channel::<String>(32);

    tokio::spawn(async move {
        let work_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let config_path = work_dir.join(".claude/agent.toml");
        
        let config = Arc::new(AgentConfig::load(&config_path).expect("Failed to load agent config"));
        let provider = build_provider(config.clone()).expect("Failed to build provider");

        let permission_manager = PermissionManager::try_new(PermissionMode::Auto).unwrap();

        let skills_dir = work_dir.join("skills");
        let skill_registry = Arc::new(get_skill_registry(skills_dir).unwrap());
        let store_root = StoreRoot::new(work_dir.join(".claude")).unwrap();
        let task_manager = SharedTaskManager::new(TaskManager::new(&store_root).unwrap());
        let background_manager = SharedBackgroundManager::new(&store_root).unwrap();
        let cron_scheduler = SharedCronScheduler::new(CronScheduler::new(&store_root).unwrap());
        let teammate_manager = SharedTeammateManager::new(TeammateManager::new(&store_root).unwrap());
        let worktree_manager =
            SharedWorktreeManager::new(WorktreeManager::new(&store_root, work_dir.clone()).unwrap());
        let memory_manager = Arc::new(std::sync::Mutex::new(get_memory_manager(
            work_dir.join(".claude/memory"),
        ).unwrap()));
        let mcp_router = load_mcp_router().await.unwrap_or_default();

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

        let app_clone = app.clone();
        agent.set_stream_handler(move |delta| {
            let _ = app_clone.emit("chat-event", ChatEvent::Token(delta));
        });

        let app_clone2 = app.clone();
        agent.set_status_handler(move |status| {
            let _ = app_clone2.emit("chat-event", ChatEvent::Status(status));
        });

        while let Some(query) = rx.recv().await {
            let _ = app.emit("chat-event", ChatEvent::Start);
            
            agent.runtime.context.push(build_user_message(query, vec![]));
            
            if let Err(e) = agent.agent_loop_streaming().await {
                let _ = app.emit("chat-event", ChatEvent::Error(e.to_string()));
            } else {
                let _ = app.emit("chat-event", ChatEvent::End);
            }
        }
    });

    Ok(tx)
}

#[tauri::command]
async fn send_message(query: String, state: State<'_, Arc<Mutex<AgentActor>>>) -> Result<(), String> {
    let actor = state.lock().await;
    actor.tx.send(query).await.map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let app_handle = app.handle().clone();
            tauri::async_runtime::block_on(async move {
                if let Ok(tx) = initialize_agent(app_handle).await {
                    app.manage(Arc::new(Mutex::new(AgentActor { tx })));
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![send_message])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
