//! Response formatting and tree-view printers for CLI output.
//!
//! Pure functions that format IPC responses into human-readable
//! terminal output: generic response printer, workspace/pane tree
//! hierarchy, workspace list, and pane list.

pub fn print_response(resp: &amux_ipc::Response, json: bool) {
    if json {
        match serde_json::to_string_pretty(resp) {
            Ok(s) => println!("{}", s),
            Err(e) => {
                eprintln!("error: failed to serialize response: {}", e);
                std::process::exit(1);
            }
        }
    } else if resp.ok {
        if let Some(result) = &resp.result {
            match serde_json::to_string_pretty(result) {
                Ok(s) => println!("{}", s),
                Err(e) => {
                    eprintln!("error: failed to serialize result: {}", e);
                    std::process::exit(1);
                }
            }
        }
    } else if let Some(err) = &resp.error {
        eprintln!("error [{}]: {}", err.code, err.message);
        std::process::exit(1);
    } else {
        eprintln!("error: request failed with no error details");
        std::process::exit(1);
    }
}

pub fn print_hierarchy(ws_result: &serde_json::Value, sf_result: &serde_json::Value) {
    let empty = Vec::new();
    let workspaces = ws_result
        .get("workspaces")
        .and_then(|w| w.as_array())
        .unwrap_or(&empty);
    let surfaces = sf_result
        .get("surfaces")
        .and_then(|s| s.as_array())
        .unwrap_or(&empty);

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

pub fn print_workspace_list(result: &serde_json::Value) {
    if let Some(workspaces) = result.get("workspaces").and_then(|w| w.as_array()) {
        for ws in workspaces {
            let id = ws.get("id").and_then(|v| v.as_str()).unwrap_or("?");
            let title = ws.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let count = ws.get("pane_count").and_then(|v| v.as_u64()).unwrap_or(0);
            let active = ws.get("active").and_then(|v| v.as_bool()).unwrap_or(false);
            let marker = if active { " *" } else { "" };
            println!(
                "workspace:{}{} \"{}\" ({} pane{})",
                id,
                marker,
                title,
                count,
                if count == 1 { "" } else { "s" }
            );
        }
    }
}

pub fn print_pane_list(result: &serde_json::Value) {
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
