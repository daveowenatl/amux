mod claude_hook;
mod cli;
mod install;
mod print;

use amux_ipc::{read_last_addr, read_last_token, IpcAddr, IpcClient};
use clap::Parser;

use claude_hook::handle_claude_hook;
use cli::{Cli, Command};
use install::{install_claude_hooks, install_shell_integration, uninstall_claude_hooks};
use print::{print_hierarchy, print_pane_list, print_response, print_workspace_list};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // These commands are direct filesystem operations — handle before IPC connection.
    if matches!(cli.command, Command::InstallShellIntegration) {
        install_shell_integration()?;
        return Ok(());
    }
    if let Command::InstallHooks { claude, uninstall } = &cli.command {
        if *claude {
            if *uninstall {
                uninstall_claude_hooks()?;
            } else {
                install_claude_hooks()?;
            }
        } else {
            eprintln!("Specify --claude to install Claude Code hooks");
            std::process::exit(1);
        }
        return Ok(());
    }
    if matches!(cli.command, Command::SessionClear) {
        match amux_session::clear() {
            Ok(()) => {
                if cli.json {
                    println!("{}", serde_json::json!({"ok": true}));
                } else {
                    println!("Session cleared");
                }
            }
            Err(e) => {
                if cli.json {
                    println!(
                        "{}",
                        serde_json::json!({"ok": false, "error": e.to_string()})
                    );
                } else {
                    eprintln!("Error: {}", e);
                }
            }
        }
        return Ok(());
    }

    let addr = resolve_addr(&cli)?;
    let mut client = IpcClient::connect(&addr).await?;

    let token = cli
        .token
        .clone()
        .or_else(|| std::env::var("AMUX_SOCKET_TOKEN").ok())
        .or_else(|| read_last_token().ok())
        .unwrap_or_default();
    if token.is_empty() {
        anyhow::bail!("No auth token found. Set AMUX_SOCKET_TOKEN or use --token.");
    }
    client.authenticate(&token).await?;

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
            } else {
                // Show first error found
                let all = [&ws_resp, &sf_resp, &pane_resp];
                let err_resp = all.iter().find(|r| !r.ok).unwrap_or(&all[0]);
                print_response(err_resp, false);
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
        Command::ReadScreen {
            surface,
            ansi,
            lines,
        } => {
            let surface_id = surface
                .or_else(|| std::env::var("AMUX_SURFACE_ID").ok())
                .unwrap_or_else(|| "default".to_string());
            let resp = client
                .call(
                    "surface.read_text",
                    serde_json::json!({
                        "surface_id": surface_id,
                        "ansi": ansi,
                        "lines": lines,
                    }),
                )
                .await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                if let Some(result) = &resp.result {
                    if let Some(text) = result.get("text").and_then(|t| t.as_str()) {
                        println!("{}", text);
                    }
                }
            } else {
                print_response(&resp, false);
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
            } else if resp.ok {
                if let Some(result) = &resp.result {
                    print_pane_list(result);
                }
            } else {
                print_response(&resp, false);
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
        Command::SurfaceCreate { pane } => {
            let mut params = serde_json::json!({});
            if let Some(p) = pane {
                params["pane_id"] = serde_json::json!(p);
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
        // --- Metadata commands ---
        Command::SetCwd {
            cwd,
            clear,
            surface,
        } => {
            let surface_id = surface
                .or_else(|| std::env::var("AMUX_SURFACE_ID").ok())
                .unwrap_or_else(|| "default".to_string());
            let params = if clear {
                serde_json::json!({ "surface_id": surface_id })
            } else {
                serde_json::json!({ "surface_id": surface_id, "cwd": cwd })
            };
            let resp = client.call("surface.set_cwd", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                println!("CWD set");
            } else {
                print_response(&resp, false);
            }
        }
        Command::SetGit {
            branch,
            dirty,
            clear,
            surface,
        } => {
            let surface_id = surface
                .or_else(|| std::env::var("AMUX_SURFACE_ID").ok())
                .unwrap_or_else(|| "default".to_string());
            let params = if clear {
                serde_json::json!({
                    "surface_id": surface_id,
                })
            } else {
                serde_json::json!({
                    "surface_id": surface_id,
                    "branch": branch,
                    "dirty": dirty,
                })
            };
            let resp = client.call("surface.set_git", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                println!("Git info set");
            } else {
                print_response(&resp, false);
            }
        }
        Command::SetPr {
            number,
            title,
            state,
            clear,
            surface,
        } => {
            let surface_id = surface
                .or_else(|| std::env::var("AMUX_SURFACE_ID").ok())
                .unwrap_or_else(|| "default".to_string());
            let params = if clear {
                serde_json::json!({
                    "surface_id": surface_id,
                })
            } else {
                serde_json::json!({
                    "surface_id": surface_id,
                    "number": number,
                    "title": title,
                    "state": state,
                })
            };
            let resp = client.call("surface.set_pr", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                println!("PR info set");
            } else {
                print_response(&resp, false);
            }
        }
        // --- Notification / Status commands ---
        Command::SetStatus {
            state,
            label,
            task,
            message,
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
            if let Some(t) = task {
                params["task"] = serde_json::json!(t);
            }
            if let Some(m) = message {
                params["message"] = serde_json::json!(m);
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
            subtitle,
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
            if let Some(s) = subtitle {
                params["subtitle"] = serde_json::json!(s);
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
        Command::ClaudeHook { event } => {
            handle_claude_hook(&mut client, &event).await?;
        }
        Command::Subscribe { events } => {
            let resp = client
                .call("subscribe", serde_json::json!({ "events": events }))
                .await?;
            if !resp.ok {
                print_response(&resp, cli.json);
                std::process::exit(1);
            }
            if let Some(result) = &resp.result {
                if let Some(subscribed) = result.get("subscribed").and_then(|s| s.as_array()) {
                    let names: Vec<&str> = subscribed.iter().filter_map(|v| v.as_str()).collect();
                    if !cli.json {
                        eprintln!("Subscribed to: {}", names.join(", "));
                    }
                }
            }
            // Stream events until the connection closes or Ctrl+C
            let stdout = std::io::stdout();
            loop {
                match client.read_line().await {
                    Ok(Some(line)) => {
                        use std::io::Write;
                        let mut lock = stdout.lock();
                        let _ = writeln!(lock, "{}", line);
                        let _ = lock.flush();
                    }
                    Ok(None) => break,
                    Err(e) => {
                        eprintln!("Error reading event: {}", e);
                        break;
                    }
                }
            }
        }
        Command::SessionClear | Command::InstallShellIntegration | Command::InstallHooks { .. } => {
            unreachable!("handled before IPC connection");
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

    Ok(read_last_addr()?)
}
