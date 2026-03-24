use std::collections::HashMap;
use std::io::Read;
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use amux_ipc::IpcCommand;
use amux_layout::{NavDirection, PaneId, PaneTree, SplitDirection};
use amux_term::color::resolve_color;
use amux_term::config::AmuxTermConfig;
use amux_term::pane::TerminalPane;
use portable_pty::CommandBuilder;
use wezterm_surface::{CursorShape, CursorVisibility};
use wezterm_term::color::SrgbaTuple;

const FONT_SIZE: f32 = 14.0;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    // Start IPC server first to get the socket path for env injection
    let (ipc_rx, ipc_addr) = amux_ipc::start_server()?;
    tracing::info!("IPC server: {}", ipc_addr);

    let config = Arc::new(AmuxTermConfig::default());

    // Spawn initial pane
    let initial_id: PaneId = 0;
    let managed = spawn_managed_pane(80, 24, &ipc_addr, &config)?;

    let mut panes = HashMap::new();
    panes.insert(initial_id, managed);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0])
            .with_title("amux"),
        ..Default::default()
    };

    let ipc_addr_cleanup = ipc_addr.clone();
    let result = eframe::run_native(
        "amux",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(AmuxApp {
                panes,
                tree: PaneTree::new(initial_id),
                focused: initial_id,
                zoomed: None,
                next_pane_id: 1,
                ipc_rx,
                socket_addr: ipc_addr,
                config,
                last_panel_rect: None,
                dragging_divider: None,
                last_pane_sizes: HashMap::new(),
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{}", e));

    cleanup_addr(&ipc_addr_cleanup);
    result
}

fn default_shell() -> String {
    #[cfg(unix)]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
    }
    #[cfg(windows)]
    {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
    }
}

fn cleanup_addr(addr: &amux_ipc::IpcAddr) {
    match addr {
        #[cfg(unix)]
        amux_ipc::IpcAddr::Unix(path) => {
            let _ = std::fs::remove_file(path);
        }
        #[cfg(windows)]
        amux_ipc::IpcAddr::NamedPipe(_) => {}
    }
}

struct ManagedPane {
    pane: TerminalPane,
    byte_rx: mpsc::Receiver<Vec<u8>>,
    /// Scrollback offset: 0 = bottom (live), >0 = scrolled up by N lines.
    scroll_offset: usize,
}

fn spawn_managed_pane(
    cols: u16,
    rows: u16,
    ipc_addr: &amux_ipc::IpcAddr,
    config: &Arc<AmuxTermConfig>,
) -> anyhow::Result<ManagedPane> {
    let shell = default_shell();
    let mut cmd = CommandBuilder::new(&shell);
    cmd.env("AMUX_SOCKET_PATH", ipc_addr.to_string());
    cmd.env("AMUX_WORKSPACE_ID", "default");
    cmd.env("AMUX_SURFACE_ID", "default");
    cmd.env("TERM", "xterm-256color");

    let mut pane = TerminalPane::spawn(cols, rows, cmd, config.clone())?;

    let mut reader = pane.take_reader().expect("reader already taken");
    let (byte_tx, byte_rx) = mpsc::channel::<Vec<u8>>();

    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if byte_tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    Ok(ManagedPane {
        pane,
        byte_rx,
        scroll_offset: 0,
    })
}

struct AmuxApp {
    panes: HashMap<PaneId, ManagedPane>,
    tree: PaneTree,
    focused: PaneId,
    zoomed: Option<PaneId>,
    next_pane_id: PaneId,
    ipc_rx: std::sync::mpsc::Receiver<IpcCommand>,
    socket_addr: amux_ipc::IpcAddr,
    config: Arc<AmuxTermConfig>,
    last_panel_rect: Option<egui::Rect>,
    dragging_divider: Option<DragState>,
    last_pane_sizes: HashMap<PaneId, (usize, usize)>,
}

struct DragState {
    node_path: Vec<usize>,
    direction: SplitDirection,
}

impl eframe::App for AmuxApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Drain PTY output from all panes
        let mut got_data = false;
        for managed in self.panes.values_mut() {
            let mut pane_got_data = false;
            while let Ok(bytes) = managed.byte_rx.try_recv() {
                pane_got_data = true;
                managed.pane.feed_bytes(&bytes);
            }
            if pane_got_data {
                got_data = true;
                // Auto-snap to bottom when new output arrives
                if managed.scroll_offset > 0 {
                    managed.scroll_offset = 0;
                }
            }
        }

        // Process IPC commands
        self.process_ipc_commands();

        // Handle keyboard shortcuts BEFORE terminal input
        let shortcut_consumed = self.handle_shortcuts(ctx);

        // Handle keyboard/paste input → focused pane only
        if !shortcut_consumed {
            self.handle_input(ctx);
        }

        // Render
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                let panel_rect = ui.available_rect_before_wrap();
                self.last_panel_rect = Some(panel_rect);

                // Handle divider dragging
                self.handle_divider_drag(ui, panel_rect);

                if let Some(zoomed_id) = self.zoomed {
                    // Zoomed mode: render single pane fullscreen
                    if let Some(managed) = self.panes.get_mut(&zoomed_id) {
                        render_pane(
                            ui,
                            &mut managed.pane,
                            panel_rect,
                            true,
                            managed.scroll_offset,
                        );
                        self.resize_pane_if_needed(zoomed_id, panel_rect, ui);
                    }
                } else {
                    // Normal mode: render all panes at computed rects
                    let layout = self.tree.layout(panel_rect);

                    // Click-to-focus: switch focus when clicking inside a pane
                    if ui.input(|i| i.pointer.any_pressed()) {
                        if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                            for &(id, rect) in &layout {
                                if rect.contains(pos) && id != self.focused {
                                    self.focused = id;
                                    break;
                                }
                            }
                        }
                    }

                    // Render dividers
                    let dividers = self.tree.dividers(panel_rect);
                    let painter = ui.painter();
                    for div in &dividers {
                        painter.rect_filled(div.rect, 0.0, egui::Color32::from_gray(60));
                    }

                    // Render each pane
                    for &(id, rect) in &layout {
                        let is_focused = id == self.focused;
                        if let Some(managed) = self.panes.get_mut(&id) {
                            render_pane(
                                ui,
                                &mut managed.pane,
                                rect,
                                is_focused,
                                managed.scroll_offset,
                            );
                        }
                        self.resize_pane_if_needed(id, rect, ui);
                    }
                }

                // Allocate the full panel area for interaction
                ui.allocate_rect(panel_rect, egui::Sense::click_and_drag());
            });

        // Update window title from focused pane
        if let Some(managed) = self.panes.get(&self.focused) {
            let title = managed.pane.title();
            if !title.is_empty() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("amux — {}", title)));
            }
        }

        // Smart repaint
        if got_data {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
}

impl AmuxApp {
    fn resize_pane_if_needed(&mut self, id: PaneId, rect: egui::Rect, ui: &egui::Ui) {
        let font_id = egui::FontId::monospace(FONT_SIZE);
        let cell_width = ui.fonts(|f| f.glyph_width(&font_id, 'M'));
        let cell_height = ui.fonts(|f| f.row_height(&font_id));

        let cols = (rect.width() / cell_width).floor() as usize;
        let rows = (rect.height() / cell_height).floor() as usize;

        if cols == 0 || rows == 0 {
            return;
        }

        let last = self.last_pane_sizes.get(&id).copied().unwrap_or((0, 0));
        if (cols, rows) != last {
            self.last_pane_sizes.insert(id, (cols, rows));
            if let Some(managed) = self.panes.get_mut(&id) {
                let _ = managed.pane.resize(cols as u16, rows as u16);
            }
        }
    }

    fn spawn_pane(&mut self) -> PaneId {
        let id = self.next_pane_id;
        self.next_pane_id += 1;

        match spawn_managed_pane(80, 24, &self.socket_addr, &self.config) {
            Ok(managed) => {
                self.panes.insert(id, managed);
            }
            Err(e) => {
                tracing::error!("Failed to spawn pane: {}", e);
            }
        }
        id
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) -> bool {
        let events = ctx.input(|i| i.events.clone());

        for event in &events {
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            {
                // Platform-aware shortcuts
                #[cfg(target_os = "macos")]
                let is_cmd = modifiers.mac_cmd || modifiers.command;
                #[cfg(not(target_os = "macos"))]
                let is_cmd = modifiers.ctrl && modifiers.shift;

                // Split right: Cmd+D (macOS) / Ctrl+Shift+D
                if is_cmd && !modifiers.shift && *key == egui::Key::D {
                    return self.do_split(SplitDirection::Horizontal);
                }
                // Split down: Cmd+Shift+D (macOS) / Ctrl+Shift+Down
                #[cfg(target_os = "macos")]
                if is_cmd && modifiers.shift && *key == egui::Key::D {
                    return self.do_split(SplitDirection::Vertical);
                }
                #[cfg(not(target_os = "macos"))]
                if modifiers.ctrl && modifiers.shift && *key == egui::Key::ArrowDown {
                    return self.do_split(SplitDirection::Vertical);
                }

                // Close pane: Cmd+W (macOS) / Ctrl+Shift+W
                if is_cmd && *key == egui::Key::W {
                    return self.do_close_pane();
                }

                // Navigate: Option+Cmd+Arrow (macOS) / Ctrl+Alt+Arrow
                #[cfg(target_os = "macos")]
                let is_nav = is_cmd && modifiers.alt;
                #[cfg(not(target_os = "macos"))]
                let is_nav = modifiers.ctrl && modifiers.alt;

                if is_nav {
                    let dir = match key {
                        egui::Key::ArrowLeft => Some(NavDirection::Left),
                        egui::Key::ArrowRight => Some(NavDirection::Right),
                        egui::Key::ArrowUp => Some(NavDirection::Up),
                        egui::Key::ArrowDown => Some(NavDirection::Down),
                        _ => None,
                    };
                    if let Some(dir) = dir {
                        return self.do_navigate(dir);
                    }
                }

                // Zoom toggle: Cmd+Shift+Enter (macOS) / Ctrl+Shift+Enter
                #[cfg(target_os = "macos")]
                let is_zoom = is_cmd && modifiers.shift && *key == egui::Key::Enter;
                #[cfg(not(target_os = "macos"))]
                let is_zoom = modifiers.ctrl && modifiers.shift && *key == egui::Key::Enter;

                if is_zoom {
                    return self.do_toggle_zoom();
                }

                // Scroll: Shift+PageUp / Shift+PageDown
                if modifiers.shift && *key == egui::Key::PageUp {
                    return self.do_scroll(-1);
                }
                if modifiers.shift && *key == egui::Key::PageDown {
                    return self.do_scroll(1);
                }
            }
        }

        // Mouse wheel scrolling on focused pane
        let scroll_delta = ctx.input(|i| i.smooth_scroll_delta.y);
        if scroll_delta != 0.0 {
            // Convert pixel delta to lines (3 lines per scroll notch)
            let lines = (-scroll_delta / 20.0).round() as isize;
            if lines != 0 {
                self.do_scroll_lines(lines);
            }
        }

        false
    }

    fn do_split(&mut self, direction: SplitDirection) -> bool {
        let new_id = self.spawn_pane();
        self.tree.split(self.focused, direction, new_id);
        self.focused = new_id;
        true
    }

    fn do_close_pane(&mut self) -> bool {
        // Don't close the last pane
        if self.tree.iter_panes().len() <= 1 {
            return true;
        }
        let closing = self.focused;
        if let Some(new_focus) = self.tree.close(closing) {
            self.focused = new_focus;
            self.panes.remove(&closing);
            self.last_pane_sizes.remove(&closing);
            if self.zoomed == Some(closing) {
                self.zoomed = None;
            }
        }
        true
    }

    fn do_navigate(&mut self, dir: NavDirection) -> bool {
        if let Some(rect) = self.last_panel_rect {
            if let Some(neighbor) = self.tree.neighbor(self.focused, dir, rect) {
                self.focused = neighbor;
            }
        }
        true
    }

    fn do_toggle_zoom(&mut self) -> bool {
        if self.zoomed.is_some() {
            self.zoomed = None;
        } else {
            self.zoomed = Some(self.focused);
        }
        true
    }

    /// Scroll focused pane by pages (-1 = page up, 1 = page down).
    fn do_scroll(&mut self, pages: isize) -> bool {
        if let Some(managed) = self.panes.get_mut(&self.focused) {
            let (_, rows) = managed.pane.dimensions();
            let page_size = rows.saturating_sub(1).max(1);
            let lines = pages * page_size as isize;
            self.do_scroll_lines(lines);
        }
        true
    }

    /// Scroll focused pane by N lines (positive = scroll down/toward bottom,
    /// negative = scroll up/toward history).
    fn do_scroll_lines(&mut self, lines: isize) {
        if let Some(managed) = self.panes.get_mut(&self.focused) {
            let (_, rows) = managed.pane.dimensions();
            let total = managed.pane.screen().scrollback_rows();
            let max_offset = total.saturating_sub(rows);

            let new_offset = managed.scroll_offset as isize - lines;
            managed.scroll_offset = (new_offset.max(0) as usize).min(max_offset);
        }
    }

    fn handle_divider_drag(&mut self, ui: &egui::Ui, panel_rect: egui::Rect) {
        if self.zoomed.is_some() {
            return;
        }

        let dividers = self.tree.dividers(panel_rect);
        let pointer_pos = ui.input(|i| i.pointer.hover_pos());
        let primary_down = ui.input(|i| i.pointer.primary_down());
        let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
        let primary_released = ui.input(|i| i.pointer.primary_released());

        // Hover cursor feedback
        if let Some(pos) = pointer_pos {
            if self.dragging_divider.is_none() {
                if let Some(div) = dividers.iter().find(|d| d.rect.contains(pos)) {
                    match div.direction {
                        SplitDirection::Horizontal => {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                        }
                        SplitDirection::Vertical => {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                        }
                    }
                }
            }
        }

        // Start drag on click over a divider
        if primary_pressed && self.dragging_divider.is_none() {
            if let Some(pos) = pointer_pos {
                // Use a slightly expanded hit area for easier grabbing
                if let Some(div) = dividers.iter().find(|d| d.rect.expand(4.0).contains(pos)) {
                    self.dragging_divider = Some(DragState {
                        node_path: div.node_path.clone(),
                        direction: div.direction,
                    });
                }
            }
        }

        // Continue drag using pointer delta
        if primary_down {
            if let Some(ref drag) = self.dragging_divider {
                let delta = ui.input(|i| i.pointer.delta());
                let px_delta = match drag.direction {
                    SplitDirection::Horizontal => delta.x,
                    SplitDirection::Vertical => delta.y,
                };
                if px_delta != 0.0 {
                    self.tree
                        .resize_divider(&drag.node_path, px_delta, panel_rect);
                }
                // Show resize cursor while dragging
                match drag.direction {
                    SplitDirection::Horizontal => {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    }
                    SplitDirection::Vertical => {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                    }
                }
            }
        }

        if primary_released {
            self.dragging_divider = None;
        }
    }

    fn handle_input(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        let focused = self.focused;

        let managed = match self.panes.get_mut(&focused) {
            Some(m) => m,
            None => return,
        };

        for event in &events {
            match event {
                egui::Event::Text(text) => {
                    let _ = managed.pane.write_bytes(text.as_bytes());
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if let Some(bytes) = encode_egui_key(key, modifiers) {
                        let _ = managed.pane.write_bytes(&bytes);
                    }
                }
                egui::Event::Paste(text) => {
                    let _ = managed.pane.write_bytes(b"\x1b[200~");
                    let _ = managed.pane.write_bytes(text.as_bytes());
                    let _ = managed.pane.write_bytes(b"\x1b[201~");
                }
                _ => {}
            }
        }
    }

    fn process_ipc_commands(&mut self) {
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
            "system.identify" => Response::ok(
                req.id.clone(),
                serde_json::json!({
                    "workspace_id": "default",
                    "surface_id": self.focused.to_string(),
                }),
            ),
            "surface.list" => {
                let surfaces: Vec<serde_json::Value> = self
                    .tree
                    .iter_panes()
                    .iter()
                    .filter_map(|id| {
                        self.panes.get_mut(id).map(|m| {
                            let (cols, rows) = m.pane.dimensions();
                            serde_json::json!({
                                "id": id.to_string(),
                                "title": m.pane.title(),
                                "cols": cols,
                                "rows": rows,
                                "alive": m.pane.is_alive(),
                            })
                        })
                    })
                    .collect();
                Response::ok(req.id.clone(), serde_json::json!({"surfaces": surfaces}))
            }
            "surface.send_text" => {
                match serde_json::from_value::<amux_ipc::methods::SendTextParams>(
                    req.params.clone(),
                ) {
                    Ok(params) => {
                        let pane_id = self.resolve_surface_id(&params.surface_id);
                        match self.panes.get_mut(&pane_id) {
                            Some(m) => match m.pane.write_bytes(params.text.as_bytes()) {
                                Ok(_) => Response::ok(req.id.clone(), serde_json::json!({})),
                                Err(e) => {
                                    Response::err(req.id.clone(), "write_error", &e.to_string())
                                }
                            },
                            None => Response::err(req.id.clone(), "not_found", "pane not found"),
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
                        let pane_id = self.resolve_surface_id(&params.surface_id);
                        match self.panes.get(&pane_id) {
                            Some(m) => {
                                let text = m.pane.read_screen_text();
                                Response::ok(req.id.clone(), serde_json::json!({"text": text}))
                            }
                            None => Response::err(req.id.clone(), "not_found", "pane not found"),
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "pane.split" => {
                #[derive(::serde::Deserialize)]
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
                        let new_id = self.spawn_pane();
                        self.tree.split(self.focused, dir, new_id);
                        self.focused = new_id;
                        Response::ok(
                            req.id.clone(),
                            serde_json::json!({"pane_id": new_id.to_string()}),
                        )
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "pane.close" => {
                #[derive(::serde::Deserialize)]
                struct CloseParams {
                    #[serde(default)]
                    pane_id: Option<String>,
                }
                match serde_json::from_value::<CloseParams>(req.params.clone()) {
                    Ok(params) => {
                        let target = params
                            .pane_id
                            .and_then(|s| s.parse::<PaneId>().ok())
                            .unwrap_or(self.focused);
                        if self.tree.iter_panes().len() <= 1 {
                            return Response::err(
                                req.id.clone(),
                                "last_pane",
                                "cannot close the last pane",
                            );
                        }
                        if let Some(new_focus) = self.tree.close(target) {
                            if self.focused == target {
                                self.focused = new_focus;
                            }
                            self.panes.remove(&target);
                            self.last_pane_sizes.remove(&target);
                            if self.zoomed == Some(target) {
                                self.zoomed = None;
                            }
                            Response::ok(
                                req.id.clone(),
                                serde_json::json!({"focused": self.focused.to_string()}),
                            )
                        } else {
                            Response::err(req.id.clone(), "not_found", "pane not found in tree")
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "pane.focus" => {
                #[derive(::serde::Deserialize)]
                struct FocusParams {
                    pane_id: String,
                }
                match serde_json::from_value::<FocusParams>(req.params.clone()) {
                    Ok(params) => match params.pane_id.parse::<PaneId>() {
                        Ok(id) if self.tree.contains(id) => {
                            self.focused = id;
                            Response::ok(req.id.clone(), serde_json::json!({}))
                        }
                        _ => Response::err(req.id.clone(), "not_found", "pane not found"),
                    },
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "pane.list" => {
                let pane_ids = self.tree.iter_panes();
                let focused = self.focused;
                let mut pane_list = Vec::new();
                for id in &pane_ids {
                    if let Some(m) = self.panes.get_mut(id) {
                        let (cols, rows) = m.pane.dimensions();
                        pane_list.push(serde_json::json!({
                            "id": id.to_string(),
                            "focused": *id == focused,
                            "cols": cols,
                            "rows": rows,
                            "alive": m.pane.is_alive(),
                        }));
                    }
                }
                Response::ok(req.id.clone(), serde_json::json!({"panes": pane_list}))
            }
            _ => Response::err(
                req.id.clone(),
                "method_not_found",
                &format!("unknown method: {}", req.method),
            ),
        }
    }

    fn resolve_surface_id(&self, surface_id: &str) -> PaneId {
        if surface_id == "default" || surface_id.is_empty() {
            self.focused
        } else {
            surface_id.parse::<PaneId>().unwrap_or(self.focused)
        }
    }
}

fn render_pane(
    ui: &mut egui::Ui,
    pane: &mut TerminalPane,
    rect: egui::Rect,
    is_focused: bool,
    scroll_offset: usize,
) {
    let font_id = egui::FontId::monospace(FONT_SIZE);
    let cell_width = ui.fonts(|f| f.glyph_width(&font_id, 'M'));
    let cell_height = ui.fonts(|f| f.row_height(&font_id));

    let (actual_cols, actual_rows) = pane.dimensions();
    if actual_cols == 0 || actual_rows == 0 {
        return;
    }

    let palette = pane.palette();
    let cursor = pane.cursor();
    let screen = pane.screen();

    let painter = ui.painter();
    let origin = rect.min;
    let bg_default = srgba_to_egui(palette.background);

    // Fill terminal background
    let terminal_rect = egui::Rect::from_min_size(
        origin,
        egui::vec2(
            actual_cols as f32 * cell_width,
            actual_rows as f32 * cell_height,
        ),
    );
    // Clip to pane rect
    let clipped_rect = terminal_rect.intersect(rect);
    painter.rect_filled(clipped_rect, 0.0, bg_default);

    // Draw cells — apply scroll offset (0 = bottom/live view)
    let total = screen.scrollback_rows();
    let end = total.saturating_sub(scroll_offset);
    let start = end.saturating_sub(actual_rows);
    let lines = screen.lines_in_phys_range(start..end);

    for (row_idx, line) in lines.iter().enumerate() {
        let y = origin.y + row_idx as f32 * cell_height;
        if y + cell_height < rect.min.y || y > rect.max.y {
            continue; // clip
        }

        for cell_ref in line.visible_cells() {
            let col_idx = cell_ref.cell_index();
            if col_idx >= actual_cols {
                break;
            }

            let x = origin.x + col_idx as f32 * cell_width;
            if x + cell_width < rect.min.x || x > rect.max.x {
                continue; // clip
            }

            let attrs = cell_ref.attrs();
            let reverse = attrs.reverse();
            let bg = srgba_to_egui(resolve_color(&attrs.background(), &palette, false, reverse));
            let fg = srgba_to_egui(resolve_color(&attrs.foreground(), &palette, true, reverse));

            if bg != bg_default {
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(x, y),
                        egui::vec2(cell_width, cell_height),
                    ),
                    0.0,
                    bg,
                );
            }

            let text = cell_ref.str();
            if !text.is_empty() && text != " " {
                painter.text(
                    egui::pos2(x, y),
                    egui::Align2::LEFT_TOP,
                    text,
                    font_id.clone(),
                    fg,
                );
            }
        }
    }

    // Draw cursor: respect visibility (TUI apps hide it) and shape
    if is_focused
        && scroll_offset == 0
        && cursor.visibility == CursorVisibility::Visible
        && cursor.y >= 0
        && (cursor.y as usize) < actual_rows
        && cursor.x < actual_cols
    {
        let cx = origin.x + cursor.x as f32 * cell_width;
        let cy = origin.y + cursor.y as f32 * cell_height;
        let cursor_color = srgba_to_egui(palette.cursor_bg);

        match cursor.shape {
            CursorShape::BlinkingBar | CursorShape::SteadyBar => {
                // Thin vertical bar (2px wide)
                let bar_rect =
                    egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(2.0, cell_height));
                painter.rect_filled(bar_rect, 0.0, cursor_color);
            }
            CursorShape::BlinkingUnderline | CursorShape::SteadyUnderline => {
                // Underline (2px tall at bottom of cell)
                let underline_rect = egui::Rect::from_min_size(
                    egui::pos2(cx, cy + cell_height - 2.0),
                    egui::vec2(cell_width, 2.0),
                );
                painter.rect_filled(underline_rect, 0.0, cursor_color);
            }
            CursorShape::Default | CursorShape::BlinkingBlock | CursorShape::SteadyBlock => {
                // Block cursor: fill background, re-draw character in cursor_fg
                let cursor_rect = egui::Rect::from_min_size(
                    egui::pos2(cx, cy),
                    egui::vec2(cell_width, cell_height),
                );
                let cursor_fg = srgba_to_egui(palette.cursor_fg);
                painter.rect_filled(cursor_rect, 0.0, cursor_color);

                let cursor_line_idx = cursor.y as usize;
                if cursor_line_idx < lines.len() {
                    let line = &lines[cursor_line_idx];
                    for cell_ref in line.visible_cells() {
                        if cell_ref.cell_index() == cursor.x {
                            let text = cell_ref.str();
                            if !text.is_empty() && text != " " {
                                painter.text(
                                    egui::pos2(cx, cy),
                                    egui::Align2::LEFT_TOP,
                                    text,
                                    font_id.clone(),
                                    cursor_fg,
                                );
                            }
                            break;
                        }
                    }
                }
            }
        }
    }

    // Focus indicator: subtle border on focused pane
    if is_focused {
        painter.rect_stroke(
            rect,
            0.0,
            egui::Stroke::new(2.0, egui::Color32::from_rgb(80, 140, 220)),
            egui::StrokeKind::Inside,
        );
    }
}

fn srgba_to_egui(color: SrgbaTuple) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        (color.0 * 255.0).round() as u8,
        (color.1 * 255.0).round() as u8,
        (color.2 * 255.0).round() as u8,
        (color.3 * 255.0).round() as u8,
    )
}

// --- Key encoding (egui events → terminal bytes) ---

fn encode_egui_key(key: &egui::Key, modifiers: &egui::Modifiers) -> Option<Vec<u8>> {
    if modifiers.ctrl && !modifiers.alt {
        if let Some(byte) = ctrl_byte_for_key(key) {
            return Some(vec![byte]);
        }
    }

    if modifiers.alt && !modifiers.ctrl {
        if let Some(ch) = key_to_char(key) {
            return Some(vec![0x1b, ch as u8]);
        }
    }

    if modifiers.ctrl && modifiers.alt {
        if let Some(byte) = ctrl_byte_for_key(key) {
            return Some(vec![0x1b, byte]);
        }
    }

    let modifier_param = egui_modifier_param(modifiers);

    match key {
        egui::Key::Enter => Some(vec![0x0d]),
        egui::Key::Tab => {
            if modifiers.shift {
                Some(b"\x1b[Z".to_vec())
            } else {
                Some(vec![0x09])
            }
        }
        egui::Key::Escape => Some(vec![0x1b]),
        egui::Key::Backspace => {
            if modifiers.alt {
                Some(vec![0x1b, 0x7f])
            } else {
                Some(vec![0x7f])
            }
        }
        egui::Key::Space if modifiers.ctrl => Some(vec![0x00]),

        egui::Key::ArrowUp => Some(encode_arrow(b'A', modifier_param)),
        egui::Key::ArrowDown => Some(encode_arrow(b'B', modifier_param)),
        egui::Key::ArrowRight => Some(encode_arrow(b'C', modifier_param)),
        egui::Key::ArrowLeft => Some(encode_arrow(b'D', modifier_param)),

        egui::Key::Home => Some(encode_csi_letter(b'H', modifier_param)),
        egui::Key::End => Some(encode_csi_letter(b'F', modifier_param)),
        egui::Key::Insert => Some(encode_csi_tilde(2, modifier_param)),
        egui::Key::Delete => Some(encode_csi_tilde(3, modifier_param)),
        egui::Key::PageUp => Some(encode_csi_tilde(5, modifier_param)),
        egui::Key::PageDown => Some(encode_csi_tilde(6, modifier_param)),

        egui::Key::F1 => Some(encode_fn_key(b'P', 11, modifier_param)),
        egui::Key::F2 => Some(encode_fn_key(b'Q', 12, modifier_param)),
        egui::Key::F3 => Some(encode_fn_key(b'R', 13, modifier_param)),
        egui::Key::F4 => Some(encode_fn_key(b'S', 14, modifier_param)),
        egui::Key::F5 => Some(encode_csi_tilde(15, modifier_param)),
        egui::Key::F6 => Some(encode_csi_tilde(17, modifier_param)),
        egui::Key::F7 => Some(encode_csi_tilde(18, modifier_param)),
        egui::Key::F8 => Some(encode_csi_tilde(19, modifier_param)),
        egui::Key::F9 => Some(encode_csi_tilde(20, modifier_param)),
        egui::Key::F10 => Some(encode_csi_tilde(21, modifier_param)),
        egui::Key::F11 => Some(encode_csi_tilde(23, modifier_param)),
        egui::Key::F12 => Some(encode_csi_tilde(24, modifier_param)),

        _ => None,
    }
}

fn encode_arrow(letter: u8, modifier_param: Option<u8>) -> Vec<u8> {
    match modifier_param {
        Some(m) => format!("\x1b[1;{}{}", m, letter as char).into_bytes(),
        None => vec![0x1b, b'[', letter],
    }
}

fn encode_csi_letter(letter: u8, modifier_param: Option<u8>) -> Vec<u8> {
    match modifier_param {
        Some(m) => format!("\x1b[1;{}{}", m, letter as char).into_bytes(),
        None => vec![0x1b, b'[', letter],
    }
}

fn encode_csi_tilde(number: u8, modifier_param: Option<u8>) -> Vec<u8> {
    match modifier_param {
        Some(m) => format!("\x1b[{};{}~", number, m).into_bytes(),
        None => format!("\x1b[{}~", number).into_bytes(),
    }
}

fn encode_fn_key(ss3_letter: u8, csi_number: u8, modifier_param: Option<u8>) -> Vec<u8> {
    match modifier_param {
        Some(m) => format!("\x1b[{};{}~", csi_number, m).into_bytes(),
        None => vec![0x1b, b'O', ss3_letter],
    }
}

fn egui_modifier_param(modifiers: &egui::Modifiers) -> Option<u8> {
    let mut param: u8 = 1;
    if modifiers.shift {
        param += 1;
    }
    if modifiers.alt {
        param += 2;
    }
    if modifiers.ctrl {
        param += 4;
    }
    if param == 1 {
        None
    } else {
        Some(param)
    }
}

fn ctrl_byte_for_key(key: &egui::Key) -> Option<u8> {
    match key {
        egui::Key::A => Some(0x01),
        egui::Key::B => Some(0x02),
        egui::Key::C => Some(0x03),
        egui::Key::D => Some(0x04),
        egui::Key::E => Some(0x05),
        egui::Key::F => Some(0x06),
        egui::Key::G => Some(0x07),
        egui::Key::H => Some(0x08),
        egui::Key::I => Some(0x09),
        egui::Key::J => Some(0x0a),
        egui::Key::K => Some(0x0b),
        egui::Key::L => Some(0x0c),
        egui::Key::M => Some(0x0d),
        egui::Key::N => Some(0x0e),
        egui::Key::O => Some(0x0f),
        egui::Key::P => Some(0x10),
        egui::Key::Q => Some(0x11),
        egui::Key::R => Some(0x12),
        egui::Key::S => Some(0x13),
        egui::Key::T => Some(0x14),
        egui::Key::U => Some(0x15),
        egui::Key::V => Some(0x16),
        egui::Key::W => Some(0x17),
        egui::Key::X => Some(0x18),
        egui::Key::Y => Some(0x19),
        egui::Key::Z => Some(0x1a),
        _ => None,
    }
}

fn key_to_char(key: &egui::Key) -> Option<char> {
    match key {
        egui::Key::A => Some('a'),
        egui::Key::B => Some('b'),
        egui::Key::C => Some('c'),
        egui::Key::D => Some('d'),
        egui::Key::E => Some('e'),
        egui::Key::F => Some('f'),
        egui::Key::G => Some('g'),
        egui::Key::H => Some('h'),
        egui::Key::I => Some('i'),
        egui::Key::J => Some('j'),
        egui::Key::K => Some('k'),
        egui::Key::L => Some('l'),
        egui::Key::M => Some('m'),
        egui::Key::N => Some('n'),
        egui::Key::O => Some('o'),
        egui::Key::P => Some('p'),
        egui::Key::Q => Some('q'),
        egui::Key::R => Some('r'),
        egui::Key::S => Some('s'),
        egui::Key::T => Some('t'),
        egui::Key::U => Some('u'),
        egui::Key::V => Some('v'),
        egui::Key::W => Some('w'),
        egui::Key::X => Some('x'),
        egui::Key::Y => Some('y'),
        egui::Key::Z => Some('z'),
        _ => None,
    }
}
