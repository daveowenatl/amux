use amux_ipc::{read_last_addr, IpcAddr, IpcClient};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "amux", about = "Terminal multiplexer for AI coding agents")]
struct Cli {
    /// Socket path (auto-detected if omitted)
    #[arg(long, global = true)]
    socket: Option<String>,

    /// Output as JSON
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check if the amux server is running
    Ping,
    /// List full hierarchy (workspaces, surfaces, panes)
    Tree,
    /// Send text to a surface
    Send {
        /// Text to send
        text: String,
        /// Target surface ID
        #[arg(long)]
        surface: Option<String>,
    },
    /// Read screen text from a surface
    ReadScreen {
        /// Target surface ID
        #[arg(long)]
        surface: Option<String>,
    },
    /// List server capabilities
    Capabilities,
    /// Identify focused workspace/surface
    Identify,
    /// Split the focused pane
    Split {
        /// Split direction: right or down
        #[arg(long, default_value = "right")]
        direction: String,
    },
    /// Close a pane
    ClosePane {
        /// Pane ID to close (defaults to focused)
        #[arg(long)]
        pane: Option<String>,
    },
    /// Focus a specific pane
    FocusPane {
        /// Pane ID to focus
        pane_id: String,
    },
    /// List all panes in active surface
    ListPanes,
    /// Create a new workspace
    #[command(name = "workspace-create")]
    WorkspaceCreate {
        /// Workspace title
        #[arg(long)]
        title: Option<String>,
    },
    /// List all workspaces
    #[command(name = "workspace-list")]
    WorkspaceList,
    /// Close a workspace
    #[command(name = "workspace-close")]
    WorkspaceClose {
        /// Workspace ID to close
        workspace_id: Option<String>,
    },
    /// Focus a workspace
    #[command(name = "workspace-focus")]
    WorkspaceFocus {
        /// Workspace ID to focus
        workspace_id: String,
    },
    /// Create a new surface (tab) in a workspace
    #[command(name = "surface-create")]
    SurfaceCreate {
        /// Workspace ID (defaults to active)
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Close a surface (tab)
    #[command(name = "surface-close")]
    SurfaceClose {
        /// Surface ID to close (defaults to active)
        surface_id: Option<String>,
    },
    /// Focus a surface (tab)
    #[command(name = "surface-focus")]
    SurfaceFocus {
        /// Surface ID to focus
        surface_id: String,
    },
    /// Set workspace agent status (displayed as a sidebar pill)
    #[command(name = "set-status")]
    SetStatus {
        /// Status state: idle, active, waiting
        state: String,
        /// Optional label text
        label: Option<String>,
        /// Target workspace ID (defaults to AMUX_WORKSPACE_ID)
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Send a notification
    Notify {
        /// Notification body
        body: String,
        /// Notification title
        #[arg(long)]
        title: Option<String>,
        /// Target workspace ID (defaults to AMUX_WORKSPACE_ID)
        #[arg(long)]
        workspace: Option<String>,
        /// Target pane ID (defaults to focused pane)
        #[arg(long)]
        pane: Option<String>,
    },
    /// List notifications
    #[command(name = "list-notifications")]
    ListNotifications,
    /// Clear all notifications
    #[command(name = "clear-notifications")]
    ClearNotifications,
    /// Save the current session
    #[command(name = "session-save")]
    SessionSave,
    /// Clear saved session data
    #[command(name = "session-clear")]
    SessionClear,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let addr = resolve_addr(&cli)?;
    let mut client = IpcClient::connect(&addr).await?;

    match cli.command {
        Command::Ping => {
            let resp = client.call("system.ping", serde_json::json!({})).await?;
            print_response(&resp, cli.json);
        }
        Command::Tree => {
            // Fetch workspace list and surface list to build full hierarchy
            let ws_resp = client.call("workspace.list", serde_json::json!({})).await?;
            let sf_resp = client.call("surface.list", serde_json::json!({})).await?;
            let pane_resp = client.call("pane.list", serde_json::json!({})).await?;
            if cli.json {
                // Combine into one JSON response
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "workspaces": ws_resp.result,
                        "surfaces": sf_resp.result,
                        "panes": pane_resp.result,
                    }))
                    .unwrap()
                );
            } else if let (Some(ws_result), Some(sf_result)) = (&ws_resp.result, &sf_resp.result) {
                print_hierarchy(ws_result, sf_result);
            }
        }
        Command::Send { text, surface } => {
            let surface_id = surface
                .or_else(|| std::env::var("AMUX_SURFACE_ID").ok())
                .unwrap_or_else(|| "default".to_string());
            let resp = client
                .call(
                    "surface.send_text",
                    serde_json::json!({
                        "surface_id": surface_id,
                        "text": text,
                    }),
                )
                .await?;
            print_response(&resp, cli.json);
        }
        Command::ReadScreen { surface } => {
            let surface_id = surface
                .or_else(|| std::env::var("AMUX_SURFACE_ID").ok())
                .unwrap_or_else(|| "default".to_string());
            let resp = client
                .call(
                    "surface.read_text",
                    serde_json::json!({
                        "surface_id": surface_id,
                    }),
                )
                .await?;
            if cli.json {
                print_response(&resp, true);
            } else if let Some(result) = &resp.result {
                if let Some(text) = result.get("text").and_then(|t| t.as_str()) {
                    println!("{}", text);
                }
            }
        }
        Command::Capabilities => {
            let resp = client
                .call("system.capabilities", serde_json::json!({}))
                .await?;
            print_response(&resp, cli.json);
        }
        Command::Identify => {
            let resp = client
                .call("system.identify", serde_json::json!({}))
                .await?;
            print_response(&resp, cli.json);
        }
        Command::Split { direction } => {
            let resp = client
                .call(
                    "pane.split",
                    serde_json::json!({
                        "direction": direction,
                    }),
                )
                .await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                if let Some(result) = &resp.result {
                    if let Some(id) = result.get("pane_id").and_then(|v| v.as_str()) {
                        println!("Split pane created: {}", id);
                    }
                }
            } else {
                print_response(&resp, false);
            }
        }
        Command::ClosePane { pane } => {
            let params = match pane {
                Some(id) => serde_json::json!({"pane_id": id}),
                None => serde_json::json!({}),
            };
            let resp = client.call("pane.close", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                println!("Pane closed");
            } else {
                print_response(&resp, false);
            }
        }
        Command::FocusPane { pane_id } => {
            let resp = client
                .call("pane.focus", serde_json::json!({"pane_id": pane_id}))
                .await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                println!("Focused pane {}", pane_id);
            } else {
                print_response(&resp, false);
            }
        }
        Command::ListPanes => {
            let resp = client.call("pane.list", serde_json::json!({})).await?;
            if cli.json {
                print_response(&resp, true);
            } else if let Some(result) = &resp.result {
                print_pane_list(result);
            }
        }
        // --- Workspace commands ---
        Command::WorkspaceCreate { title } => {
            let mut params = serde_json::json!({});
            if let Some(t) = title {
                params["title"] = serde_json::json!(t);
            }
            let resp = client.call("workspace.create", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                if let Some(result) = &resp.result {
                    if let Some(id) = result.get("workspace_id").and_then(|v| v.as_str()) {
                        println!("Workspace created: {}", id);
                    }
                }
            } else {
                print_response(&resp, false);
            }
        }
        Command::WorkspaceList => {
            let resp = client.call("workspace.list", serde_json::json!({})).await?;
            if cli.json {
                print_response(&resp, true);
            } else if let Some(result) = &resp.result {
                print_workspace_list(result);
            }
        }
        Command::WorkspaceClose { workspace_id } => {
            let params = match workspace_id {
                Some(id) => serde_json::json!({"workspace_id": id}),
                None => {
                    // Get active workspace ID first
                    let id_resp = client
                        .call("system.identify", serde_json::json!({}))
                        .await?;
                    let ws_id = id_resp
                        .result
                        .as_ref()
                        .and_then(|r| r.get("workspace_id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("0")
                        .to_string();
                    serde_json::json!({"workspace_id": ws_id})
                }
            };
            let resp = client.call("workspace.close", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                println!("Workspace closed");
            } else {
                print_response(&resp, false);
            }
        }
        Command::WorkspaceFocus { workspace_id } => {
            let resp = client
                .call(
                    "workspace.focus",
                    serde_json::json!({"workspace_id": workspace_id}),
                )
                .await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                println!("Focused workspace {}", workspace_id);
            } else {
                print_response(&resp, false);
            }
        }
        // --- Surface commands ---
        Command::SurfaceCreate { workspace } => {
            let mut params = serde_json::json!({});
            if let Some(ws) = workspace {
                params["workspace_id"] = serde_json::json!(ws);
            }
            let resp = client.call("surface.create", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                if let Some(result) = &resp.result {
                    if let Some(id) = result.get("surface_id").and_then(|v| v.as_str()) {
                        println!("Surface created: {}", id);
                    }
                }
            } else {
                print_response(&resp, false);
            }
        }
        Command::SurfaceClose { surface_id } => {
            let mut params = serde_json::json!({});
            if let Some(id) = surface_id {
                params["surface_id"] = serde_json::json!(id);
            }
            let resp = client.call("surface.close", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                println!("Surface closed");
            } else {
                print_response(&resp, false);
            }
        }
        Command::SurfaceFocus { surface_id } => {
            let resp = client
                .call(
                    "surface.focus",
                    serde_json::json!({"surface_id": surface_id}),
                )
                .await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                println!("Focused surface {}", surface_id);
            } else {
                print_response(&resp, false);
            }
        }
        // --- Notification / Status commands ---
        Command::SetStatus {
            state,
            label,
            workspace,
        } => {
            let ws_id = workspace
                .or_else(|| std::env::var("AMUX_WORKSPACE_ID").ok())
                .unwrap_or_else(|| "0".to_string());
            let mut params = serde_json::json!({
                "workspace_id": ws_id,
                "state": state,
            });
            if let Some(l) = label {
                params["label"] = serde_json::json!(l);
            }
            let resp = client.call("status.set", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                println!("Status set");
            } else {
                print_response(&resp, false);
            }
        }
        Command::Notify {
            body,
            title,
            workspace,
            pane,
        } => {
            let ws_id = workspace
                .or_else(|| std::env::var("AMUX_WORKSPACE_ID").ok())
                .unwrap_or_else(|| "0".to_string());
            let pane_id = pane.unwrap_or_else(|| "0".to_string());
            let mut params = serde_json::json!({
                "workspace_id": ws_id,
                "pane_id": pane_id,
                "body": body,
            });
            if let Some(t) = title {
                params["title"] = serde_json::json!(t);
            }
            let resp = client.call("notify.send", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                if let Some(result) = &resp.result {
                    if let Some(id) = result.get("notification_id") {
                        println!("Notification sent: {}", id);
                    }
                }
            } else {
                print_response(&resp, false);
            }
        }
        Command::ListNotifications => {
            let resp = client.call("notify.list", serde_json::json!({})).await?;
            print_response(&resp, cli.json);
        }
        Command::ClearNotifications => {
            let resp = client.call("notify.clear", serde_json::json!({})).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                println!("Notifications cleared");
            } else {
                print_response(&resp, false);
            }
        }
        Command::SessionSave => {
            let resp = client.call("session.save", serde_json::json!({})).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                let path = resp
                    .result
                    .as_ref()
                    .and_then(|r| r.get("path"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("(unknown)");
                println!("Session saved to {}", path);
            } else {
                print_response(&resp, false);
            }
        }
        Command::SessionClear => {
            // Direct filesystem operation, no IPC needed
            match amux_session::clear() {
                Ok(()) => {
                    if cli.json {
                        println!("{{\"ok\":true}}");
                    } else {
                        println!("Session cleared");
                    }
                }
                Err(e) => {
                    if cli.json {
                        println!(
                            "{{\"ok\":false,\"error\":\"{}\"}}",
                            e.to_string().replace('"', "\\\"")
                        );
                    } else {
                        eprintln!("Error: {}", e);
                    }
                }
            }
        }
    }
    Ok(())
}

fn resolve_addr(cli: &Cli) -> anyhow::Result<IpcAddr> {
    if let Some(ref socket) = cli.socket {
        return Ok(IpcAddr::from_stored(socket));
    }

    if let Ok(path) = std::env::var("AMUX_SOCKET_PATH") {
        return Ok(IpcAddr::from_stored(&path));
    }

    read_last_addr()
}

fn print_response(resp: &amux_ipc::Response, json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(resp).unwrap());
    } else if resp.ok {
        if let Some(result) = &resp.result {
            println!("{}", serde_json::to_string_pretty(result).unwrap());
        }
    } else if let Some(err) = &resp.error {
        eprintln!("error [{}]: {}", err.code, err.message);
        std::process::exit(1);
    }
}

fn print_hierarchy(ws_result: &serde_json::Value, sf_result: &serde_json::Value) {
    let workspaces = ws_result
        .get("workspaces")
        .and_then(|w| w.as_array())
        .cloned()
        .unwrap_or_default();
    let surfaces = sf_result
        .get("surfaces")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();

    for (ws_i, ws) in workspaces.iter().enumerate() {
        let ws_id = ws.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let ws_title = ws.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let ws_active = ws.get("active").and_then(|v| v.as_bool()).unwrap_or(false);
        let active_marker = if ws_active { " [active]" } else { "" };
        let ws_prefix = if ws_i == workspaces.len() - 1 {
            "└──"
        } else {
            "├──"
        };
        let ws_cont = if ws_i == workspaces.len() - 1 {
            "   "
        } else {
            "│  "
        };

        println!(
            "{} Workspace \"{}\" (id={}){}",
            ws_prefix, ws_title, ws_id, active_marker
        );

        // Find surfaces belonging to this workspace
        let ws_surfaces: Vec<&serde_json::Value> = surfaces
            .iter()
            .filter(|s| s.get("workspace_id").and_then(|v| v.as_str()) == Some(ws_id))
            .collect();

        // Group by surface_id
        let mut surface_ids: Vec<String> = Vec::new();
        for s in &ws_surfaces {
            let sf_id = s
                .get("surface_id")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string();
            if !surface_ids.contains(&sf_id) {
                surface_ids.push(sf_id);
            }
        }

        for (sf_i, sf_id) in surface_ids.iter().enumerate() {
            let sf_prefix = if sf_i == surface_ids.len() - 1 {
                "└──"
            } else {
                "├──"
            };
            let sf_cont = if sf_i == surface_ids.len() - 1 {
                "   "
            } else {
                "│  "
            };

            println!("{}  {} Surface (id={})", ws_cont, sf_prefix, sf_id);

            // Find panes in this surface
            let panes: Vec<&serde_json::Value> = ws_surfaces
                .iter()
                .filter(|s| s.get("surface_id").and_then(|v| v.as_str()) == Some(sf_id))
                .copied()
                .collect();

            for (p_i, pane) in panes.iter().enumerate() {
                let p_prefix = if p_i == panes.len() - 1 {
                    "└──"
                } else {
                    "├──"
                };
                let id = pane.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                let title = pane.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let cols = pane.get("cols").and_then(|v| v.as_u64()).unwrap_or(0);
                let rows = pane.get("rows").and_then(|v| v.as_u64()).unwrap_or(0);
                let alive = pane.get("alive").and_then(|v| v.as_bool()).unwrap_or(false);
                let status = if alive { "running" } else { "exited" };
                println!(
                    "{}  {}  {} Pane {} \"{}\" {}x{} [{}]",
                    ws_cont, sf_cont, p_prefix, id, title, cols, rows, status
                );
            }
        }
    }
}

fn print_workspace_list(result: &serde_json::Value) {
    if let Some(workspaces) = result.get("workspaces").and_then(|w| w.as_array()) {
        for ws in workspaces {
            let id = ws.get("id").and_then(|v| v.as_str()).unwrap_or("?");
            let title = ws.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let count = ws
                .get("surface_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let active = ws.get("active").and_then(|v| v.as_bool()).unwrap_or(false);
            let marker = if active { " *" } else { "" };
            println!(
                "workspace:{}{} \"{}\" ({} surface{})",
                id,
                marker,
                title,
                count,
                if count == 1 { "" } else { "s" }
            );
        }
    }
}

fn print_pane_list(result: &serde_json::Value) {
    if let Some(panes) = result.get("panes").and_then(|p| p.as_array()) {
        for pane in panes {
            let id = pane.get("id").and_then(|v| v.as_str()).unwrap_or("?");
            let focused = pane
                .get("focused")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let cols = pane.get("cols").and_then(|v| v.as_u64()).unwrap_or(0);
            let rows = pane.get("rows").and_then(|v| v.as_u64()).unwrap_or(0);
            let alive = pane.get("alive").and_then(|v| v.as_bool()).unwrap_or(false);
            let focus_marker = if focused { " *" } else { "" };
            let status = if alive { "running" } else { "exited" };
            println!("pane:{}{} {}x{} [{}]", id, focus_marker, cols, rows, status);
        }
    }
}
