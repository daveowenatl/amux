mod claude_hook;
mod cli;
mod codex_hook;
mod gemini_hook;
mod hook_action;
mod install;
mod print;

use amux_ipc::{read_last_addr, read_last_token, IpcAddr, IpcClient};
use clap::Parser;

use claude_hook::handle_claude_hook;
use cli::{Cli, Command};
use codex_hook::handle_codex_hook;
use gemini_hook::handle_gemini_hook;
use install::install_shell_integration;
use print::{print_hierarchy, print_pane_list, print_response, print_workspace_list};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // These commands are direct filesystem operations — handle before IPC connection.
    if matches!(cli.command, Command::InstallShellIntegration) {
        install_shell_integration()?;
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
        Command::Browser { url } => {
            let url = url.unwrap_or_else(|| "https://google.com".to_string());
            let resp = client
                .call("pane.create-browser", serde_json::json!({"url": url}))
                .await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                println!("Browser pane opened: {}", url);
            } else {
                print_response(&resp, false);
            }
        }
        Command::BrowserNavigate { url, pane } => {
            let mut params = serde_json::json!({"url": url});
            if let Some(p) = pane {
                params["pane_id"] = serde_json::json!(p);
            }
            let resp = client.call("browser.navigate", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                println!("Navigated to {}", url);
            } else {
                print_response(&resp, false);
            }
        }
        Command::BrowserBack { pane } => {
            let params = match pane {
                Some(p) => serde_json::json!({"pane_id": p}),
                None => serde_json::json!({}),
            };
            let resp = client.call("browser.go-back", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                println!("Navigated back");
            } else {
                print_response(&resp, false);
            }
        }
        Command::BrowserForward { pane } => {
            let params = match pane {
                Some(p) => serde_json::json!({"pane_id": p}),
                None => serde_json::json!({}),
            };
            let resp = client.call("browser.go-forward", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                println!("Navigated forward");
            } else {
                print_response(&resp, false);
            }
        }
        Command::BrowserReload { pane } => {
            let params = match pane {
                Some(p) => serde_json::json!({"pane_id": p}),
                None => serde_json::json!({}),
            };
            let resp = client.call("browser.reload", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                println!("Page reloaded");
            } else {
                print_response(&resp, false);
            }
        }
        Command::BrowserUrl { pane } => {
            let params = match pane {
                Some(p) => serde_json::json!({"pane_id": p}),
                None => serde_json::json!({}),
            };
            let resp = client.call("browser.get-url", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                if let Some(url) = resp
                    .result
                    .as_ref()
                    .and_then(|r| r.get("url"))
                    .and_then(|v| v.as_str())
                {
                    println!("{}", url);
                }
            } else {
                print_response(&resp, false);
            }
        }
        Command::BrowserTitle { pane } => {
            let params = match pane {
                Some(p) => serde_json::json!({"pane_id": p}),
                None => serde_json::json!({}),
            };
            let resp = client.call("browser.get-title", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                if let Some(title) = resp
                    .result
                    .as_ref()
                    .and_then(|r| r.get("title"))
                    .and_then(|v| v.as_str())
                {
                    println!("{}", title);
                }
            } else {
                print_response(&resp, false);
            }
        }
        Command::BrowserExec { script, pane } => {
            let mut params = serde_json::json!({"script": script});
            if let Some(p) = pane {
                params["pane_id"] = serde_json::json!(p);
            }
            let resp = client.call("browser.execute-script", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                println!("Script executed");
            } else {
                print_response(&resp, false);
            }
        }
        Command::BrowserEval { script, pane } => {
            let mut params = serde_json::json!({"script": script});
            if let Some(p) = &pane {
                params["pane_id"] = serde_json::json!(p);
            }
            let resp = client.call("browser.evaluate", params).await?;
            let resp = poll_eval_result(&mut client, resp, pane.as_deref(), 5000).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                if let Some(result) = resp.result.as_ref().and_then(|r| r.get("result")) {
                    // Print strings without JSON quotes
                    if let Some(s) = result.as_str() {
                        println!("{}", s);
                    } else {
                        println!("{}", result);
                    }
                } else {
                    eprintln!("Timed out waiting for eval result");
                    std::process::exit(1);
                }
            } else {
                print_response(&resp, false);
            }
        }
        Command::BrowserText { pane } => {
            let params = match &pane {
                Some(p) => serde_json::json!({"pane_id": p}),
                None => serde_json::json!({}),
            };
            let resp = client.call("browser.get-text", params).await?;
            let resp = poll_eval_result(&mut client, resp, pane.as_deref(), 5000).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                if let Some(text) = resp
                    .result
                    .as_ref()
                    .and_then(|r| r.get("result"))
                    .and_then(|v| v.as_str())
                {
                    println!("{}", text);
                } else {
                    eprintln!("Timed out waiting for result");
                    std::process::exit(1);
                }
            } else {
                print_response(&resp, false);
            }
        }
        Command::BrowserSnapshot { pane } => {
            let params = match &pane {
                Some(p) => serde_json::json!({"pane_id": p}),
                None => serde_json::json!({}),
            };
            let resp = client.call("browser.snapshot", params).await?;
            let resp = poll_eval_result(&mut client, resp, pane.as_deref(), 5000).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                if let Some(text) = resp
                    .result
                    .as_ref()
                    .and_then(|r| r.get("result"))
                    .and_then(|v| v.as_str())
                {
                    println!("{}", text);
                } else {
                    eprintln!("Timed out waiting for result");
                    std::process::exit(1);
                }
            } else {
                print_response(&resp, false);
            }
        }
        Command::BrowserScreenshot { pane, output } => {
            let params = match &pane {
                Some(p) => serde_json::json!({"pane_id": p}),
                None => serde_json::json!({}),
            };
            let resp = client.call("browser.screenshot", params).await?;
            let resp = poll_eval_result(&mut client, resp, pane.as_deref(), 10000).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                if let Some(result) = resp
                    .result
                    .as_ref()
                    .and_then(|r| r.get("result"))
                    .and_then(|v| v.as_str())
                {
                    if let Some(path) = output {
                        // If result is a data URL, decode base64 and write to file
                        if result.starts_with("data:image") {
                            if let Some(comma_pos) = result.find(',') {
                                let b64 = &result[comma_pos + 1..];
                                use base64::Engine;
                                match base64::engine::general_purpose::STANDARD.decode(b64) {
                                    Ok(bytes) => {
                                        std::fs::write(&path, &bytes)?;
                                        println!("Screenshot written to {}", path);
                                    }
                                    Err(e) => {
                                        eprintln!("Failed to decode screenshot: {}", e);
                                        std::process::exit(1);
                                    }
                                }
                            } else {
                                eprintln!("Unexpected data URL format");
                                std::process::exit(1);
                            }
                        } else {
                            eprintln!(
                                "Screenshot not available (got metadata instead of image data)"
                            );
                            eprintln!("{}", result);
                            std::process::exit(1);
                        }
                    } else {
                        println!("{}", result);
                    }
                }
            } else {
                print_response(&resp, false);
            }
        }
        Command::BrowserDevtools { pane, open } => {
            let mut params = serde_json::json!({});
            if let Some(p) = pane {
                params["pane_id"] = serde_json::json!(p);
            }
            if let Some(o) = open {
                params["open"] = serde_json::json!(o);
            }
            let resp = client.call("browser.toggle-devtools", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                println!("DevTools toggled");
            } else {
                print_response(&resp, false);
            }
        }
        Command::BrowserConsole { pane } => {
            let params = match pane {
                Some(p) => serde_json::json!({"pane_id": p}),
                None => serde_json::json!({}),
            };
            let resp = client.call("browser.console", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if let Some(result) = &resp.result {
                if let Some(messages) = result.get("messages").and_then(|v| v.as_array()) {
                    for msg in messages {
                        if let Some(s) = msg.as_str() {
                            println!("{}", s);
                        }
                    }
                }
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
        Command::SetEntry {
            key,
            text,
            priority,
            icon,
            color,
            ttl,
            workspace,
        } => {
            if key.starts_with("agent.") {
                anyhow::bail!(
                    "keys starting with 'agent.' are reserved for set-status (got '{key}')"
                );
            }
            let ws_id = workspace
                .or_else(|| std::env::var("AMUX_WORKSPACE_ID").ok())
                .unwrap_or_else(|| "0".to_string());
            let mut params = serde_json::json!({
                "workspace_id": ws_id,
                "key": key,
                "text": text,
            });
            if let Some(p) = priority {
                params["priority"] = serde_json::json!(p);
            }
            if let Some(i) = icon {
                params["icon"] = serde_json::json!(i);
            }
            if let Some(c) = color {
                let rgba = parse_hex_rgba(&c).ok_or_else(|| {
                    anyhow::anyhow!("invalid color '{c}' (expected #RRGGBB or #RRGGBBAA)")
                })?;
                params["color"] = serde_json::json!(rgba);
            }
            if let Some(secs) = ttl {
                if !secs.is_finite() || secs <= 0.0 {
                    anyhow::bail!("--ttl must be a positive number of seconds (got {secs})");
                }
                // ceil (not round) so that a tiny positive --ttl can never
                // become 0ms and fire an immediate expiry.
                let ttl_ms_f64 = (secs * 1000.0).ceil();
                if ttl_ms_f64 > u64::MAX as f64 {
                    anyhow::bail!(
                        "--ttl is too large to represent in milliseconds (got {secs} seconds)"
                    );
                }
                let ttl_ms = ttl_ms_f64 as u64;
                params["ttl_ms"] = serde_json::json!(ttl_ms);
            }
            let resp = client.call("status.upsert_entry", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                println!("Entry set");
            } else {
                print_response(&resp, false);
            }
        }
        Command::SetProgress {
            value,
            label,
            clear,
            workspace,
        } => {
            let ws_id = workspace
                .or_else(|| std::env::var("AMUX_WORKSPACE_ID").ok())
                .unwrap_or_else(|| "0".to_string());
            let value = if clear { None } else { value };
            if let Some(v) = value {
                if !v.is_finite() {
                    anyhow::bail!("progress value must be finite (got {v})");
                }
                if !(0.0..=1.0).contains(&v) {
                    anyhow::bail!("progress value must be in [0.0, 1.0] (got {v})");
                }
            } else if !clear && label.is_some() {
                // A label without a bar is meaningless — the server
                // drops labels on a `None` value anyway. Surface this
                // to the caller so a script bug doesn't silently no-op.
                anyhow::bail!("--label requires a progress value or --clear");
            }
            let mut params = serde_json::json!({
                "workspace_id": ws_id,
                "value": value,
            });
            if let Some(ref l) = label {
                params["label"] = serde_json::json!(l);
            }
            let resp = client.call("status.progress", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if resp.ok {
                match value {
                    Some(v) => println!("Progress set to {v:.2}"),
                    None => println!("Progress cleared"),
                }
            } else {
                print_response(&resp, false);
            }
        }
        Command::RemoveEntry { key, workspace } => {
            if key.starts_with("agent.") {
                anyhow::bail!(
                    "keys starting with 'agent.' are reserved for set-status (got '{key}')"
                );
            }
            let ws_id = workspace
                .or_else(|| std::env::var("AMUX_WORKSPACE_ID").ok())
                .unwrap_or_else(|| "0".to_string());
            let params = serde_json::json!({
                "workspace_id": ws_id,
                "key": &key,
            });
            let resp = client.call("status.remove_entry", params).await?;
            if cli.json {
                print_response(&resp, true);
            } else if !resp.ok {
                print_response(&resp, false);
            } else {
                // The server reports `removed: true` when the key existed,
                // `false` when it didn't. Print distinct messages and exit
                // non-zero on the no-op so scripts can tell them apart.
                let removed = resp
                    .result
                    .as_ref()
                    .and_then(|v| v.get("removed"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if removed {
                    println!("Entry removed");
                } else {
                    anyhow::bail!("no entry to remove for key '{key}'");
                }
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
            let pane_id = pane
                .or_else(|| std::env::var("AMUX_SURFACE_ID").ok())
                .unwrap_or_else(|| "0".to_string());
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
        Command::GeminiHook { event } => {
            handle_gemini_hook(&mut client, &event).await?;
        }
        Command::CodexHook { event } => {
            handle_codex_hook(&mut client, &event).await?;
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
        Command::SessionClear | Command::InstallShellIntegration => {
            unreachable!("handled before IPC connection");
        }
    }
    Ok(())
}

/// Poll `browser.get-eval-result` until the result is complete or timeout is reached.
///
/// If the initial response is not ok, already complete, or has no `eval_id`, it is
/// returned immediately. Otherwise we poll every 50 ms until the result is ready or the
/// timeout elapses, at which point the last response is returned as-is.
async fn poll_eval_result(
    client: &mut IpcClient,
    initial: amux_ipc::Response,
    pane_id: Option<&str>,
    timeout_ms: u64,
) -> anyhow::Result<amux_ipc::Response> {
    if !initial.ok {
        return Ok(initial);
    }

    // Extract eval_id; if absent the call is already synchronous — return as-is.
    let eval_id = match initial
        .result
        .as_ref()
        .and_then(|r| r.get("eval_id"))
        .and_then(|v| v.as_str())
    {
        Some(id) => id.to_string(),
        None => return Ok(initial),
    };

    // Already complete on the first response (unlikely but handle it).
    if initial
        .result
        .as_ref()
        .and_then(|r| r.get("status"))
        .and_then(|v| v.as_str())
        == Some("complete")
    {
        return Ok(initial);
    }

    let started = std::time::Instant::now();
    let mut last = initial;
    loop {
        if started.elapsed().as_millis() as u64 >= timeout_ms {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut params = serde_json::json!({"eval_id": eval_id});
        if let Some(p) = pane_id {
            params["pane_id"] = serde_json::json!(p);
        }
        let resp = client.call("browser.get-eval-result", params).await?;
        let complete = resp
            .result
            .as_ref()
            .and_then(|r| r.get("status"))
            .and_then(|v| v.as_str())
            == Some("complete");
        last = resp;
        if complete {
            break;
        }
    }
    Ok(last)
}

/// Parse `#RRGGBB` or `#RRGGBBAA` hex into RGBA bytes.
///
/// The leading `#` is optional. RGB form defaults alpha to 255.
fn parse_hex_rgba(s: &str) -> Option<[u8; 4]> {
    let hex = s.strip_prefix('#').unwrap_or(s);
    let (rgb, a) = match hex.len() {
        6 => (hex, 0xFF),
        8 => (&hex[..6], u8::from_str_radix(&hex[6..8], 16).ok()?),
        _ => return None,
    };
    let r = u8::from_str_radix(&rgb[0..2], 16).ok()?;
    let g = u8::from_str_radix(&rgb[2..4], 16).ok()?;
    let b = u8::from_str_radix(&rgb[4..6], 16).ok()?;
    Some([r, g, b, a])
}

#[cfg(test)]
mod parse_hex_rgba_tests {
    use super::parse_hex_rgba;

    #[test]
    fn accepts_rgb_form_with_full_alpha() {
        assert_eq!(parse_hex_rgba("#ff8800"), Some([0xFF, 0x88, 0x00, 0xFF]));
        assert_eq!(parse_hex_rgba("FF8800"), Some([0xFF, 0x88, 0x00, 0xFF]));
    }

    #[test]
    fn accepts_rgba_form() {
        assert_eq!(parse_hex_rgba("#ff880080"), Some([0xFF, 0x88, 0x00, 0x80]));
    }

    #[test]
    fn rejects_bad_lengths_and_nonhex() {
        assert_eq!(parse_hex_rgba("#fff"), None);
        assert_eq!(parse_hex_rgba("#ff88000"), None);
        assert_eq!(parse_hex_rgba("#gghhii"), None);
        assert_eq!(parse_hex_rgba(""), None);
    }
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
