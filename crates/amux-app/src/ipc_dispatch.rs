//! IPC command dispatch — handles all JSON-RPC methods from connected clients.

use amux_layout::{PaneId, SplitDirection};
use amux_notify::NotificationSource;
use amux_term::TerminalBackend;

use crate::managed_pane;

use crate::managed_pane::{PaneEntry, PaneSurface};
use crate::{startup, AmuxApp, DEFAULT_BROWSER_URL};

impl AmuxApp {
    pub(crate) fn process_ipc_commands(&mut self) {
        while let Ok(cmd) = self.ipc_rx.try_recv() {
            let response = self.dispatch_ipc(&cmd.request);
            let _ = cmd.reply_tx.send(response);
        }
    }

    fn dispatch_ipc(&mut self, req: &amux_ipc::Request) -> amux_ipc::Response {
        use amux_ipc::Response;
        match req.method.as_str() {
            "system.ping" => Response::ok(req.id.clone(), serde_json::json!({"pong": true})),
            "system.capabilities" => Response::ok(
                req.id.clone(),
                serde_json::json!({"methods": amux_ipc::methods::METHODS}),
            ),
            "system.identify" => {
                let ws = self.active_workspace();
                let focused = ws.focused_pane;
                let sf_id = self
                    .panes
                    .get(&focused)
                    .and_then(|e| e.as_terminal())
                    .and_then(|m| m.active_surface())
                    .map(|sf| sf.id)
                    .unwrap_or(0);
                Response::ok(
                    req.id.clone(),
                    serde_json::json!({
                        "workspace_id": ws.id.to_string(),
                        "surface_id": sf_id.to_string(),
                    }),
                )
            }
            "surface.list" => {
                let mut surfaces = Vec::new();
                for ws in &self.workspaces {
                    for pane_id in ws.tree.iter_panes() {
                        if let Some(PaneEntry::Terminal(managed)) = self.panes.get_mut(&pane_id) {
                            let active_idx = managed.active_tab_idx;
                            for (tab_idx, tab) in managed.tabs.iter_mut().enumerate() {
                                if let managed_pane::TabEntry::Terminal(sf) = tab {
                                    let (cols, rows) = sf.pane.dimensions();
                                    surfaces.push(serde_json::json!({
                                        "id": sf.id.to_string(),
                                        "pane_id": pane_id.to_string(),
                                        "workspace_id": ws.id.to_string(),
                                        "title": sf.pane.title(),
                                        "cols": cols,
                                        "rows": rows,
                                        "alive": sf.pane.is_alive(),
                                        "active": tab_idx == active_idx,
                                    }));
                                }
                            }
                        }
                    }
                }
                Response::ok(req.id.clone(), serde_json::json!({"surfaces": surfaces}))
            }
            "surface.set_cwd" => {
                match serde_json::from_value::<amux_ipc::methods::SetCwdParams>(req.params.clone())
                {
                    Ok(params) => {
                        let surface = self.resolve_surface_mut(&params.surface_id);
                        match surface {
                            Some(sf) => {
                                sf.metadata.cwd = params.cwd.filter(|s| !s.is_empty());
                                Response::ok(req.id.clone(), serde_json::json!({}))
                            }
                            None => Response::err(req.id.clone(), "not_found", "surface not found"),
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "surface.set_git" => {
                match serde_json::from_value::<amux_ipc::methods::SetGitParams>(req.params.clone())
                {
                    Ok(params) => {
                        let surface = self.resolve_surface_mut(&params.surface_id);
                        match surface {
                            Some(sf) => {
                                sf.metadata.git_branch = params.branch;
                                sf.metadata.git_dirty = params.dirty;
                                Response::ok(req.id.clone(), serde_json::json!({}))
                            }
                            None => Response::err(req.id.clone(), "not_found", "surface not found"),
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "surface.set_pr" => {
                match serde_json::from_value::<amux_ipc::methods::SetPrParams>(req.params.clone()) {
                    Ok(params) => {
                        let surface = self.resolve_surface_mut(&params.surface_id);
                        match surface {
                            Some(sf) => {
                                sf.metadata.pr_number = params.number;
                                sf.metadata.pr_title = params.title;
                                sf.metadata.pr_state = params.state;
                                Response::ok(req.id.clone(), serde_json::json!({}))
                            }
                            None => Response::err(req.id.clone(), "not_found", "surface not found"),
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "surface.send_text" => {
                match serde_json::from_value::<amux_ipc::methods::SendTextParams>(
                    req.params.clone(),
                ) {
                    Ok(params) => {
                        let surface = self.resolve_surface_mut(&params.surface_id);
                        match surface {
                            Some(sf) => match sf.pane.write_bytes(params.text.as_bytes()) {
                                Ok(_) => Response::ok(req.id.clone(), serde_json::json!({})),
                                Err(e) => {
                                    Response::err(req.id.clone(), "write_error", &e.to_string())
                                }
                            },
                            None => Response::err(req.id.clone(), "not_found", "surface not found"),
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "surface.read_text" => {
                match serde_json::from_value::<amux_ipc::methods::ReadTextParams>(
                    req.params.clone(),
                ) {
                    Ok(params) => {
                        let surface = self.resolve_surface(&params.surface_id);
                        match surface {
                            Some(sf) => {
                                let text = if let Some(ref line_spec) = params.lines {
                                    sf.pane.read_screen_lines(line_spec, params.ansi)
                                } else if params.ansi {
                                    let (_, rows) = sf.pane.dimensions();
                                    sf.pane.read_scrollback_text(rows)
                                } else {
                                    sf.pane.read_screen_text()
                                };
                                Response::ok(req.id.clone(), serde_json::json!({"text": text}))
                            }
                            None => Response::err(req.id.clone(), "not_found", "surface not found"),
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "pane.split" => {
                #[derive(serde::Deserialize)]
                struct SplitParams {
                    #[serde(default = "default_direction")]
                    direction: String,
                }
                fn default_direction() -> String {
                    "right".to_string()
                }
                match serde_json::from_value::<SplitParams>(req.params.clone()) {
                    Ok(params) => {
                        let dir = match params.direction.as_str() {
                            "down" | "vertical" => SplitDirection::Vertical,
                            _ => SplitDirection::Horizontal,
                        };
                        match self.spawn_pane_with_surface() {
                            Some(new_id) => {
                                let ws = self.active_workspace_mut();
                                if ws.tree.split(ws.focused_pane, dir, new_id) {
                                    self.set_focus(new_id);
                                    Response::ok(
                                        req.id.clone(),
                                        serde_json::json!({"pane_id": new_id.to_string()}),
                                    )
                                } else {
                                    self.panes.remove(&new_id);
                                    Response::err(
                                        req.id.clone(),
                                        "split_failed",
                                        "failed to split pane tree",
                                    )
                                }
                            }
                            None => Response::err(
                                req.id.clone(),
                                "spawn_failed",
                                "failed to spawn pane",
                            ),
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "pane.create-browser" => {
                #[derive(serde::Deserialize)]
                struct BrowserParams {
                    #[serde(default = "default_browser_url")]
                    url: String,
                }
                fn default_browser_url() -> String {
                    DEFAULT_BROWSER_URL.to_string()
                }
                match serde_json::from_value::<BrowserParams>(req.params.clone()) {
                    Ok(params) => {
                        let pane_id = self.focused_pane_id();
                        self.queue_browser_pane(pane_id, params.url);
                        Response::ok(req.id.clone(), serde_json::json!({"status": "queued"}))
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "browser.navigate" => {
                #[derive(serde::Deserialize)]
                struct NavParams {
                    #[serde(default)]
                    pane_id: Option<String>,
                    url: String,
                }
                match serde_json::from_value::<NavParams>(req.params.clone()) {
                    Ok(params) => {
                        let target = self.resolve_browser_pane(params.pane_id.as_deref());
                        match target {
                            Some(browser) => {
                                browser.navigate(&params.url);
                                Response::ok(req.id.clone(), serde_json::json!({}))
                            }
                            None => {
                                Response::err(req.id.clone(), "not_found", "no browser pane found")
                            }
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "browser.go-back" => {
                match self.resolve_browser_pane(Self::pane_id_param(&req.params).as_deref()) {
                    Some(browser) => {
                        browser.go_back();
                        Response::ok(req.id.clone(), serde_json::json!({}))
                    }
                    None => Response::err(req.id.clone(), "not_found", "no browser pane found"),
                }
            }
            "browser.go-forward" => {
                match self.resolve_browser_pane(Self::pane_id_param(&req.params).as_deref()) {
                    Some(browser) => {
                        browser.go_forward();
                        Response::ok(req.id.clone(), serde_json::json!({}))
                    }
                    None => Response::err(req.id.clone(), "not_found", "no browser pane found"),
                }
            }
            "browser.reload" => {
                match self.resolve_browser_pane(Self::pane_id_param(&req.params).as_deref()) {
                    Some(browser) => {
                        browser.reload();
                        Response::ok(req.id.clone(), serde_json::json!({}))
                    }
                    None => Response::err(req.id.clone(), "not_found", "no browser pane found"),
                }
            }
            "browser.stop" => {
                match self.resolve_browser_pane(Self::pane_id_param(&req.params).as_deref()) {
                    Some(browser) => {
                        browser.stop();
                        Response::ok(req.id.clone(), serde_json::json!({}))
                    }
                    None => Response::err(req.id.clone(), "not_found", "no browser pane found"),
                }
            }
            "browser.get-url" => {
                let pane_id_str = Self::pane_id_param(&req.params);
                match self.resolve_browser_pane_ref(pane_id_str.as_deref()) {
                    Some(browser) => {
                        let url = browser.url().unwrap_or_default();
                        Response::ok(req.id.clone(), serde_json::json!({"url": url}))
                    }
                    None => Response::err(req.id.clone(), "not_found", "no browser pane found"),
                }
            }
            "browser.get-title" => {
                let pane_id_str = Self::pane_id_param(&req.params);
                match self.resolve_browser_pane_ref(pane_id_str.as_deref()) {
                    Some(browser) => {
                        let title = browser.title();
                        Response::ok(req.id.clone(), serde_json::json!({"title": title}))
                    }
                    None => Response::err(req.id.clone(), "not_found", "no browser pane found"),
                }
            }
            "browser.execute-script" => {
                #[derive(serde::Deserialize)]
                struct ScriptParams {
                    #[serde(default)]
                    pane_id: Option<String>,
                    script: String,
                }
                match serde_json::from_value::<ScriptParams>(req.params.clone()) {
                    Ok(params) => match self.resolve_browser_pane(params.pane_id.as_deref()) {
                        Some(browser) => {
                            browser.evaluate_script(&params.script);
                            Response::ok(req.id.clone(), serde_json::json!({}))
                        }
                        None => Response::err(req.id.clone(), "not_found", "no browser pane found"),
                    },
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "browser.evaluate" => {
                #[derive(serde::Deserialize)]
                struct EvalParams {
                    #[serde(default)]
                    pane_id: Option<String>,
                    script: String,
                }
                match serde_json::from_value::<EvalParams>(req.params.clone()) {
                    Ok(params) => {
                        let eval_id = format!("eval_{}", req.id);
                        match self.resolve_browser_pane(params.pane_id.as_deref()) {
                            Some(browser) => {
                                browser.evaluate_with_result(&eval_id, &params.script);
                                // Return the eval_id so the caller can poll for results
                                Response::ok(
                                    req.id.clone(),
                                    serde_json::json!({"eval_id": eval_id, "status": "pending"}),
                                )
                            }
                            None => {
                                Response::err(req.id.clone(), "not_found", "no browser pane found")
                            }
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "browser.get-text" => {
                let eval_id = format!("text_{}", req.id);
                let pane_id_str = Self::pane_id_param(&req.params);
                match self.resolve_browser_pane(pane_id_str.as_deref()) {
                    Some(browser) => {
                        browser.get_text(&eval_id);
                        Response::ok(
                            req.id.clone(),
                            serde_json::json!({"eval_id": eval_id, "status": "pending"}),
                        )
                    }
                    None => Response::err(req.id.clone(), "not_found", "no browser pane found"),
                }
            }
            "browser.snapshot" => {
                let eval_id = format!("snap_{}", req.id);
                let pane_id_str = Self::pane_id_param(&req.params);
                match self.resolve_browser_pane(pane_id_str.as_deref()) {
                    Some(browser) => {
                        browser.get_snapshot(&eval_id);
                        Response::ok(
                            req.id.clone(),
                            serde_json::json!({"eval_id": eval_id, "status": "pending"}),
                        )
                    }
                    None => Response::err(req.id.clone(), "not_found", "no browser pane found"),
                }
            }
            "browser.screenshot" => {
                let eval_id = format!("screenshot_{}", req.id);
                let pane_id_str = Self::pane_id_param(&req.params);
                match self.resolve_browser_pane(pane_id_str.as_deref()) {
                    Some(browser) => {
                        browser.screenshot(&eval_id);
                        Response::ok(
                            req.id.clone(),
                            serde_json::json!({"eval_id": eval_id, "status": "pending"}),
                        )
                    }
                    None => Response::err(req.id.clone(), "not_found", "no browser pane found"),
                }
            }
            "browser.get-eval-result" => {
                #[derive(serde::Deserialize)]
                struct GetEvalResultParams {
                    eval_id: String,
                    #[serde(default)]
                    pane_id: Option<String>,
                }
                match serde_json::from_value::<GetEvalResultParams>(req.params.clone()) {
                    Ok(params) => match self.resolve_browser_pane_ref(params.pane_id.as_deref()) {
                        Some(browser) => match browser.take_eval_result(&params.eval_id) {
                            Some(result) => {
                                let value: serde_json::Value = serde_json::from_str(&result)
                                    .unwrap_or(serde_json::Value::String(result));
                                Response::ok(
                                    req.id.clone(),
                                    serde_json::json!({"status": "complete", "result": value}),
                                )
                            }
                            None => Response::ok(
                                req.id.clone(),
                                serde_json::json!({"status": "pending"}),
                            ),
                        },
                        None => Response::err(req.id.clone(), "not_found", "no browser pane found"),
                    },
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "browser.click" => {
                #[derive(serde::Deserialize)]
                struct ClickParams {
                    #[serde(default)]
                    pane_id: Option<String>,
                    x: f64,
                    y: f64,
                }
                match serde_json::from_value::<ClickParams>(req.params.clone()) {
                    Ok(params) => match self.resolve_browser_pane(params.pane_id.as_deref()) {
                        Some(browser) => {
                            browser.click_at(params.x, params.y);
                            Response::ok(req.id.clone(), serde_json::json!({}))
                        }
                        None => Response::err(req.id.clone(), "not_found", "no browser pane found"),
                    },
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "browser.type" => {
                #[derive(serde::Deserialize)]
                struct TypeParams {
                    #[serde(default)]
                    pane_id: Option<String>,
                    text: String,
                }
                match serde_json::from_value::<TypeParams>(req.params.clone()) {
                    Ok(params) => match self.resolve_browser_pane(params.pane_id.as_deref()) {
                        Some(browser) => {
                            browser.type_text(&params.text);
                            Response::ok(req.id.clone(), serde_json::json!({}))
                        }
                        None => Response::err(req.id.clone(), "not_found", "no browser pane found"),
                    },
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "browser.scroll" => {
                #[derive(serde::Deserialize)]
                struct ScrollParams {
                    #[serde(default)]
                    pane_id: Option<String>,
                    #[serde(default)]
                    dx: f64,
                    #[serde(default)]
                    dy: f64,
                }
                match serde_json::from_value::<ScrollParams>(req.params.clone()) {
                    Ok(params) => match self.resolve_browser_pane(params.pane_id.as_deref()) {
                        Some(browser) => {
                            browser.scroll_by(params.dx, params.dy);
                            Response::ok(req.id.clone(), serde_json::json!({}))
                        }
                        None => Response::err(req.id.clone(), "not_found", "no browser pane found"),
                    },
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "browser.console" => {
                let pane_id_str = Self::pane_id_param(&req.params);
                match self.resolve_browser_pane(pane_id_str.as_deref()) {
                    Some(browser) => {
                        let messages = browser.drain_console();
                        Response::ok(req.id.clone(), serde_json::json!({"messages": messages}))
                    }
                    None => Response::err(req.id.clone(), "not_found", "no browser pane found"),
                }
            }
            "browser.toggle-devtools" => {
                #[derive(serde::Deserialize)]
                struct DevToolsParams {
                    #[serde(default)]
                    pane_id: Option<String>,
                    #[serde(default)]
                    open: Option<bool>,
                }
                match serde_json::from_value::<DevToolsParams>(req.params.clone()) {
                    Ok(params) => {
                        match self.resolve_browser_pane(params.pane_id.as_deref()) {
                            Some(browser) => {
                                match params.open {
                                    Some(true) => browser.open_devtools(),
                                    Some(false) => browser.close_devtools(),
                                    None => browser.open_devtools(), // toggle defaults to open
                                }
                                Response::ok(req.id.clone(), serde_json::json!({}))
                            }
                            None => {
                                Response::err(req.id.clone(), "not_found", "no browser pane found")
                            }
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "browser.zoom" => {
                #[derive(serde::Deserialize)]
                struct ZoomParams {
                    #[serde(default)]
                    pane_id: Option<String>,
                    #[serde(default = "default_zoom")]
                    level: f64,
                }
                fn default_zoom() -> f64 {
                    1.0
                }
                match serde_json::from_value::<ZoomParams>(req.params.clone()) {
                    Ok(params) => match self.resolve_browser_pane(params.pane_id.as_deref()) {
                        Some(browser) => {
                            browser.zoom(params.level);
                            Response::ok(req.id.clone(), serde_json::json!({}))
                        }
                        None => Response::err(req.id.clone(), "not_found", "no browser pane found"),
                    },
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "browser.list-profiles" => {
                let profiles = amux_browser::list_profiles();
                Response::ok(req.id.clone(), serde_json::json!({"profiles": profiles}))
            }
            "browser.delete-profile" => {
                #[derive(serde::Deserialize)]
                struct DeleteProfileParams {
                    name: String,
                }
                match serde_json::from_value::<DeleteProfileParams>(req.params.clone()) {
                    Ok(params) => match amux_browser::delete_profile(&params.name) {
                        Ok(()) => Response::ok(req.id.clone(), serde_json::json!({})),
                        Err(e) => Response::err(req.id.clone(), "delete_failed", &e.to_string()),
                    },
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "browser.get-profile" => {
                let pane_id_str = Self::pane_id_param(&req.params);
                match self.resolve_browser_pane_ref(pane_id_str.as_deref()) {
                    Some(browser) => Response::ok(
                        req.id.clone(),
                        serde_json::json!({"profile": browser.profile()}),
                    ),
                    None => Response::err(req.id.clone(), "not_found", "no browser pane found"),
                }
            }
            "pane.close" => {
                #[derive(serde::Deserialize)]
                struct CloseParams {
                    #[serde(default)]
                    pane_id: Option<String>,
                }
                match serde_json::from_value::<CloseParams>(req.params.clone()) {
                    Ok(params) => {
                        let focused = self.focused_pane_id();
                        let target = params
                            .pane_id
                            .and_then(|s| s.parse::<PaneId>().ok())
                            .unwrap_or(focused);
                        let ws = self.active_workspace_mut();
                        if ws.tree.iter_panes().len() <= 1 {
                            return Response::err(
                                req.id.clone(),
                                "last_pane",
                                "cannot close the last pane",
                            );
                        }
                        if let Some(new_focus) = ws.tree.close(target) {
                            let should_refocus = ws.focused_pane == target;
                            ws.last_pane_sizes.remove(&target);
                            if ws.zoomed == Some(target) {
                                ws.zoomed = None;
                            }
                            self.panes.remove(&target);
                            self.notifications.remove_pane(target);
                            if should_refocus {
                                self.set_focus(new_focus);
                            }
                            Response::ok(
                                req.id.clone(),
                                serde_json::json!({"focused": self.focused_pane_id().to_string()}),
                            )
                        } else {
                            Response::err(req.id.clone(), "not_found", "pane not found in tree")
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "pane.focus" => {
                #[derive(serde::Deserialize)]
                struct FocusParams {
                    pane_id: String,
                }
                match serde_json::from_value::<FocusParams>(req.params.clone()) {
                    Ok(params) => match params.pane_id.parse::<PaneId>() {
                        Ok(id) if self.active_workspace().tree.contains(id) => {
                            self.set_focus(id);
                            Response::ok(req.id.clone(), serde_json::json!({}))
                        }
                        _ => Response::err(req.id.clone(), "not_found", "pane not found"),
                    },
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "pane.list" => {
                let ws = self.active_workspace();
                let pane_ids = ws.tree.iter_panes();
                let focused = ws.focused_pane;
                let mut pane_list = Vec::new();
                for id in &pane_ids {
                    if let Some(PaneEntry::Terminal(managed)) = self.panes.get_mut(id) {
                        let tab_count = managed.tab_count();
                        let (cols, rows, alive) = if let Some(sf) = managed.active_surface_mut() {
                            let (c, r) = sf.pane.dimensions();
                            (c, r, sf.pane.is_alive())
                        } else {
                            (80, 24, false)
                        };
                        pane_list.push(serde_json::json!({
                            "id": id.to_string(),
                            "focused": *id == focused,
                            "cols": cols,
                            "rows": rows,
                            "alive": alive,
                            "tab_count": tab_count,
                        }));
                    }
                }
                Response::ok(req.id.clone(), serde_json::json!({"panes": pane_list}))
            }
            "workspace.create" => {
                #[derive(serde::Deserialize)]
                struct CreateParams {
                    #[serde(default)]
                    title: Option<String>,
                }
                match serde_json::from_value::<CreateParams>(req.params.clone()) {
                    Ok(params) => {
                        if let Some(ws_id) = self.create_workspace(params.title) {
                            Response::ok(
                                req.id.clone(),
                                serde_json::json!({"workspace_id": ws_id.to_string()}),
                            )
                        } else {
                            Response::err(
                                req.id.clone(),
                                "spawn_failed",
                                "Failed to spawn pane for workspace",
                            )
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "workspace.list" => {
                let list: Vec<serde_json::Value> = self
                    .workspaces
                    .iter()
                    .enumerate()
                    .map(|(idx, ws)| {
                        serde_json::json!({
                            "id": ws.id.to_string(),
                            "title": ws.title,
                            "pane_count": ws.tree.iter_panes().len(),
                            "active": idx == self.active_workspace_idx,
                        })
                    })
                    .collect();
                Response::ok(req.id.clone(), serde_json::json!({"workspaces": list}))
            }
            "workspace.close" => {
                #[derive(serde::Deserialize)]
                struct CloseParams {
                    workspace_id: String,
                }
                match serde_json::from_value::<CloseParams>(req.params.clone()) {
                    Ok(params) => {
                        if let Some(idx) = self
                            .workspaces
                            .iter()
                            .position(|ws| ws.id.to_string() == params.workspace_id)
                        {
                            if self.workspaces.len() <= 1 {
                                return Response::err(
                                    req.id.clone(),
                                    "last_workspace",
                                    "cannot close the last workspace",
                                );
                            }
                            self.close_workspace_at(idx);
                            Response::ok(req.id.clone(), serde_json::json!({}))
                        } else {
                            Response::err(req.id.clone(), "not_found", "workspace not found")
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "workspace.focus" => {
                #[derive(serde::Deserialize)]
                struct FocusParams {
                    workspace_id: String,
                }
                match serde_json::from_value::<FocusParams>(req.params.clone()) {
                    Ok(params) => {
                        if let Some(idx) = self
                            .workspaces
                            .iter()
                            .position(|ws| ws.id.to_string() == params.workspace_id)
                        {
                            self.active_workspace_idx = idx;
                            Response::ok(req.id.clone(), serde_json::json!({}))
                        } else {
                            Response::err(req.id.clone(), "not_found", "workspace not found")
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "surface.create" => {
                #[derive(serde::Deserialize)]
                struct CreateParams {
                    #[serde(default)]
                    pane_id: Option<String>,
                }
                match serde_json::from_value::<CreateParams>(req.params.clone()) {
                    Ok(params) => {
                        let target_pane = params
                            .pane_id
                            .and_then(|s| s.parse::<PaneId>().ok())
                            .unwrap_or(self.focused_pane_id());

                        // Validate pane exists and is a terminal before spawning a surface
                        if !matches!(self.panes.get(&target_pane), Some(PaneEntry::Terminal(_))) {
                            Response::err(req.id.clone(), "not_found", "terminal pane not found")
                        } else {
                            // Find the workspace that owns this pane
                            let ws_id = self
                                .workspaces
                                .iter()
                                .find(|ws| ws.tree.iter_panes().contains(&target_pane))
                                .map(|ws| ws.id)
                                .unwrap_or_else(|| self.active_workspace().id);

                            let sf_id = self.next_surface_id;
                            self.next_surface_id += 1;
                            let cwd = self
                                .panes
                                .get(&target_pane)
                                .and_then(|e| e.as_terminal())
                                .and_then(|m| m.active_surface())
                                .and_then(|sf| sf.metadata.cwd.clone());

                            match startup::spawn_surface(
                                80,
                                24,
                                &self.socket_addr,
                                &self.socket_token,
                                &self.config,
                                ws_id,
                                sf_id,
                                cwd.as_deref(),
                                None,
                                self.app_config.shell.as_deref(),
                            ) {
                                Ok(surface) => {
                                    let managed = self
                                        .panes
                                        .get_mut(&target_pane)
                                        .unwrap()
                                        .as_terminal_mut()
                                        .unwrap();
                                    managed
                                        .tabs
                                        .push(managed_pane::TabEntry::Terminal(Box::new(surface)));
                                    managed.active_tab_idx = managed.tabs.len() - 1;
                                    Response::ok(
                                        req.id.clone(),
                                        serde_json::json!({"surface_id": sf_id.to_string()}),
                                    )
                                }
                                Err(e) => {
                                    Response::err(req.id.clone(), "spawn_error", &e.to_string())
                                }
                            }
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "surface.close" => {
                #[derive(serde::Deserialize)]
                struct CloseParams {
                    #[serde(default)]
                    surface_id: Option<String>,
                }
                match serde_json::from_value::<CloseParams>(req.params.clone()) {
                    Ok(params) => {
                        let focused = self.focused_pane_id();
                        if let Some(sf_id_str) = params.surface_id {
                            // Find and close the specific surface
                            if let Ok(sf_id) = sf_id_str.parse::<u64>() {
                                // Find which pane owns this surface
                                let target = self.panes.iter().find_map(|(&pid, entry)| {
                                    let m = entry.as_terminal()?;
                                    m.tabs
                                        .iter()
                                        .position(|t| t.as_surface().is_some_and(|s| s.id == sf_id))
                                        .map(|idx| (pid, idx))
                                });
                                if let Some((pane_id, idx)) = target {
                                    let managed = self
                                        .panes
                                        .get_mut(&pane_id)
                                        .unwrap()
                                        .as_terminal_mut()
                                        .unwrap();
                                    if managed.tabs.len() <= 1 {
                                        self.close_pane(pane_id);
                                    } else {
                                        managed.tabs.remove(idx);
                                        if idx < managed.active_tab_idx {
                                            managed.active_tab_idx -= 1;
                                        } else if managed.active_tab_idx >= managed.tabs.len() {
                                            managed.active_tab_idx = managed.tabs.len() - 1;
                                        }
                                    }
                                    Response::ok(req.id.clone(), serde_json::json!({}))
                                } else {
                                    Response::err(req.id.clone(), "not_found", "surface not found")
                                }
                            } else {
                                Response::err(
                                    req.id.clone(),
                                    "invalid_params",
                                    "invalid surface_id",
                                )
                            }
                        } else {
                            // Close active surface in focused pane
                            if let Some(PaneEntry::Terminal(managed)) = self.panes.get_mut(&focused)
                            {
                                if managed.tabs.len() <= 1 {
                                    self.close_pane(focused);
                                } else {
                                    managed.tabs.remove(managed.active_tab_idx);
                                    if managed.active_tab_idx >= managed.tabs.len() {
                                        managed.active_tab_idx = managed.tabs.len() - 1;
                                    }
                                }
                                Response::ok(req.id.clone(), serde_json::json!({}))
                            } else {
                                Response::err(req.id.clone(), "not_found", "pane not found")
                            }
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "surface.focus" => {
                #[derive(serde::Deserialize)]
                struct FocusParams {
                    surface_id: String,
                }
                match serde_json::from_value::<FocusParams>(req.params.clone()) {
                    Ok(params) => {
                        if let Ok(sf_id) = params.surface_id.parse::<u64>() {
                            // Find the pane that owns this surface
                            let found = self.panes.iter_mut().find_map(|(pid, entry)| {
                                let managed = entry.as_terminal_mut()?;
                                managed
                                    .tabs
                                    .iter()
                                    .position(|t| t.as_surface().is_some_and(|s| s.id == sf_id))
                                    .map(|idx| (*pid, idx))
                            });
                            if let Some((pid, idx)) = found {
                                let m =
                                    self.panes.get_mut(&pid).unwrap().as_terminal_mut().unwrap();
                                m.active_tab_idx = idx;
                                // Switch to the owning workspace before setting focus
                                if let Some(ws_idx) = self
                                    .workspaces
                                    .iter()
                                    .position(|ws| ws.tree.iter_panes().contains(&pid))
                                {
                                    self.active_workspace_idx = ws_idx;
                                }
                                self.set_focus(pid);
                                return Response::ok(req.id.clone(), serde_json::json!({}));
                            }
                            Response::err(req.id.clone(), "not_found", "surface not found")
                        } else {
                            Response::err(req.id.clone(), "invalid_params", "invalid surface_id")
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "status.set" => {
                match serde_json::from_value::<amux_ipc::methods::StatusSetParams>(
                    req.params.clone(),
                ) {
                    Ok(params) => {
                        let ws_id = params.workspace_id.parse::<u64>().unwrap_or(0);
                        let state = match params.state.as_str() {
                            "active" => amux_notify::AgentState::Active,
                            "waiting" => amux_notify::AgentState::Waiting,
                            _ => amux_notify::AgentState::Idle,
                        };
                        self.notifications.set_status(
                            ws_id,
                            state,
                            params.label,
                            params.task,
                            params.message,
                        );
                        Response::ok(req.id.clone(), serde_json::json!({}))
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "notify.send" => {
                match serde_json::from_value::<amux_ipc::methods::NotifySendParams>(
                    req.params.clone(),
                ) {
                    Ok(params) => {
                        let ws_id = params.workspace_id.parse::<u64>().unwrap_or(0);
                        // Resolve pane_id: CLI may send a surface ID, a
                        // pane ID, or nothing. Search managed panes for
                        // a match by pane ID or surface ID, then fall
                        // back to the focused pane.
                        let raw_id = params
                            .pane_id
                            .as_ref()
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(0);
                        let pane_id = if raw_id == 0 {
                            self.focused_pane_id()
                        } else {
                            self.panes
                                .iter()
                                .find_map(|(&pid, entry)| {
                                    if pid == raw_id {
                                        return Some(pid);
                                    }
                                    if let PaneEntry::Terminal(managed) = entry {
                                        if managed.surfaces().any(|s| s.id == raw_id) {
                                            return Some(pid);
                                        }
                                    }
                                    None
                                })
                                .unwrap_or_else(|| self.focused_pane_id())
                        };
                        let surface_id = params
                            .surface_id
                            .as_ref()
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(0);
                        let title = params.title.unwrap_or_default();
                        let subtitle = params.subtitle.unwrap_or_default();
                        let body = params.body;
                        // Explicit notify.send always creates an unread
                        // notification — no Tier 3 suppression. If
                        // someone (agent, user, hook) explicitly sends
                        // a notification, it should always be visible.
                        let nid = self.notifications.push(
                            ws_id,
                            pane_id,
                            surface_id,
                            title.clone(),
                            subtitle.clone(),
                            body.clone(),
                            NotificationSource::Cli,
                        );
                        if self.app_config.notifications.auto_reorder_workspaces {
                            self.bubble_workspace(ws_id);
                        }
                        // System toast + sound + custom command
                        if self.app_config.notifications.system_notifications {
                            self.system_notifier.send(&title, &body, ws_id, pane_id);
                        }
                        if let Some(player) = &self.sound_player {
                            player.play();
                        }
                        if let Some(cmd) = &self.app_config.notifications.custom_command {
                            self.system_notifier
                                .run_custom_command(cmd, &title, &body, "cli");
                        }
                        // Broadcast to IPC subscribers
                        self.event_broadcaster.send(amux_ipc::ServerEvent {
                            event: "notification".to_string(),
                            data: serde_json::json!({
                                "notification_id": nid,
                                "workspace_id": ws_id,
                                "pane_id": pane_id,
                                "title": title,
                                "subtitle": subtitle,
                                "body": body,
                            }),
                        });
                        Response::ok(req.id.clone(), serde_json::json!({"notification_id": nid}))
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "notify.list" => {
                let entries: Vec<serde_json::Value> = self
                    .notifications
                    .all_notifications()
                    .iter()
                    .map(|n| {
                        let source_str = match n.source {
                            NotificationSource::Toast => "toast",
                            NotificationSource::Bell => "bell",
                            NotificationSource::Cli => "cli",
                        };
                        serde_json::json!({
                            "id": n.id,
                            "workspace_id": n.workspace_id.to_string(),
                            "pane_id": n.pane_id.to_string(),
                            "title": n.title,
                            "subtitle": n.subtitle,
                            "body": n.body,
                            "source": source_str,
                            "read": n.read,
                        })
                    })
                    .collect();
                Response::ok(
                    req.id.clone(),
                    serde_json::json!({"notifications": entries}),
                )
            }
            "notify.clear" => {
                self.notifications.clear_all();
                Response::ok(req.id.clone(), serde_json::json!({}))
            }
            "session.save" => {
                self.flush_pending_io();
                let data = self.build_session_data();
                match amux_session::save(&data) {
                    Ok(()) => Response::ok(
                        req.id.clone(),
                        serde_json::json!({"path": amux_session::session_path().to_string_lossy()}),
                    ),
                    Err(e) => Response::err(req.id.clone(), "save_failed", &e.to_string()),
                }
            }
            _ => Response::err(
                req.id.clone(),
                "method_not_found",
                &format!("unknown method: {}", req.method),
            ),
        }
    }

    fn resolve_surface_mut(&mut self, surface_id: &str) -> Option<&mut PaneSurface> {
        if surface_id == "default" || surface_id.is_empty() {
            let focused = self.focused_pane_id();
            let m = self.panes.get_mut(&focused)?.as_terminal_mut()?;
            return m.active_surface_mut();
        }

        // Try as surface ID first: find which pane contains it
        if let Ok(sf_id) = surface_id.parse::<u64>() {
            let target_pane = self
                .panes
                .iter()
                .find(|(_, entry)| {
                    entry.as_terminal().is_some_and(|m| {
                        m.tabs
                            .iter()
                            .any(|t| t.as_surface().is_some_and(|s| s.id == sf_id))
                    })
                })
                .map(|(pid, _)| *pid);

            if let Some(pid) = target_pane {
                let m = self.panes.get_mut(&pid)?.as_terminal_mut()?;
                return m
                    .tabs
                    .iter_mut()
                    .filter_map(|t| t.as_surface_mut())
                    .find(|s| s.id == sf_id);
            }

            // Fall back to treating it as a pane ID
            if let Ok(pane_id) = surface_id.parse::<PaneId>() {
                let m = self.panes.get_mut(&pane_id)?.as_terminal_mut()?;
                return m.active_surface_mut();
            }
        }

        None
    }

    /// Extract optional pane_id from JSON params.
    fn pane_id_param(params: &serde_json::Value) -> Option<String> {
        params
            .get("pane_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// Resolve a browser pane (mutable) by optional pane_id, falling back to
    /// the active browser tab in the focused pane.
    fn resolve_browser_pane(
        &mut self,
        pane_id: Option<&str>,
    ) -> Option<&mut amux_browser::BrowserPane> {
        let target = if let Some(id_str) = pane_id {
            id_str.parse::<PaneId>().ok()?
        } else {
            // Find active browser tab in focused pane
            let focused = self.focused_pane_id();
            let managed = self.panes.get(&focused)?.as_terminal()?;
            match managed.active_tab() {
                managed_pane::ActiveTab::Browser(bid) => bid,
                _ => return None,
            }
        };
        self.panes.get_mut(&target)?.as_browser_mut()
    }

    /// Resolve a browser pane (immutable) by optional pane_id, falling back to
    /// the active browser tab in the focused pane.
    fn resolve_browser_pane_ref(
        &self,
        pane_id: Option<&str>,
    ) -> Option<&amux_browser::BrowserPane> {
        let target = if let Some(id_str) = pane_id {
            id_str.parse::<PaneId>().ok()?
        } else {
            let focused = self.focused_pane_id();
            let managed = self.panes.get(&focused)?.as_terminal()?;
            match managed.active_tab() {
                managed_pane::ActiveTab::Browser(bid) => bid,
                _ => return None,
            }
        };
        self.panes.get(&target)?.as_browser()
    }

    fn resolve_surface(&self, surface_id: &str) -> Option<&PaneSurface> {
        if surface_id == "default" || surface_id.is_empty() {
            let focused = self.focused_pane_id();
            let m = self.panes.get(&focused)?.as_terminal()?;
            m.active_surface()
        } else if let Ok(sf_id) = surface_id.parse::<u64>() {
            for entry in self.panes.values() {
                if let PaneEntry::Terminal(managed) = entry {
                    if let Some(sf) = managed.surfaces().find(|s| s.id == sf_id) {
                        return Some(sf);
                    }
                }
            }
            if let Ok(pane_id) = surface_id.parse::<PaneId>() {
                let m = self.panes.get(&pane_id)?.as_terminal()?;
                m.active_surface()
            } else {
                None
            }
        } else {
            None
        }
    }
}
