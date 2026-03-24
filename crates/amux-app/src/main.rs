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
const DEFAULT_SIDEBAR_WIDTH: f32 = 200.0;
const TAB_BAR_HEIGHT: f32 = 24.0;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let (ipc_rx, ipc_addr) = amux_ipc::start_server()?;
    tracing::info!("IPC server: {}", ipc_addr);

    let config = Arc::new(AmuxTermConfig::default());

    // Spawn initial pane with one surface
    let initial_pane_id: PaneId = 0;
    let surface = spawn_surface(80, 24, &ipc_addr, &config, 0, 0)?;

    let managed = ManagedPane {
        surfaces: vec![surface],
        active_surface_idx: 0,
    };

    let mut panes = HashMap::new();
    panes.insert(initial_pane_id, managed);

    let initial_workspace = Workspace {
        id: 0,
        title: "default".to_string(),
        tree: PaneTree::new(initial_pane_id),
        focused_pane: initial_pane_id,
        zoomed: None,
        dragging_divider: None,
        last_pane_sizes: HashMap::new(),
        status: None,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 600.0])
            .with_title("amux"),
        ..Default::default()
    };

    let ipc_addr_cleanup = ipc_addr.clone();
    let result = eframe::run_native(
        "amux",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(AmuxApp {
                workspaces: vec![initial_workspace],
                active_workspace_idx: 0,
                panes,
                next_pane_id: 1,
                next_workspace_id: 1,
                next_surface_id: 1,
                sidebar: SidebarState {
                    visible: true,
                    width: DEFAULT_SIDEBAR_WIDTH,
                },
                ipc_rx,
                socket_addr: ipc_addr,
                config,
                last_panel_rect: None,
                focus_changed_at: std::time::Instant::now(),
                focus_highlight_pane: initial_pane_id,
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

// --- Data Model ---
// Hierarchy: Workspace > PaneTree (splits) > Pane (each has tab bar) > Surface (terminal tab)

/// A terminal tab within a pane. Each pane can have multiple surfaces.
struct PaneSurface {
    id: u64,
    pane: TerminalPane,
    byte_rx: mpsc::Receiver<Vec<u8>>,
    scroll_offset: usize,
    scroll_accum: f32,
}

/// A leaf in the split tree. Each pane has its own tab bar with surfaces.
struct ManagedPane {
    surfaces: Vec<PaneSurface>,
    active_surface_idx: usize,
}

impl ManagedPane {
    fn active_surface(&self) -> &PaneSurface {
        &self.surfaces[self.active_surface_idx]
    }

    fn active_surface_mut(&mut self) -> &mut PaneSurface {
        &mut self.surfaces[self.active_surface_idx]
    }
}

/// A workspace shown in the sidebar. Owns the split tree.
struct Workspace {
    id: u64,
    title: String,
    tree: PaneTree,
    focused_pane: PaneId,
    zoomed: Option<PaneId>,
    dragging_divider: Option<DragState>,
    last_pane_sizes: HashMap<PaneId, (usize, usize)>,
    #[allow(dead_code)]
    status: Option<String>,
}

struct SidebarState {
    visible: bool,
    width: f32,
}

struct DragState {
    node_path: Vec<usize>,
    direction: SplitDirection,
}

fn spawn_surface(
    cols: u16,
    rows: u16,
    ipc_addr: &amux_ipc::IpcAddr,
    config: &Arc<AmuxTermConfig>,
    workspace_id: u64,
    surface_id: u64,
) -> anyhow::Result<PaneSurface> {
    let shell = default_shell();
    let mut cmd = CommandBuilder::new(&shell);
    cmd.env("AMUX_SOCKET_PATH", ipc_addr.to_string());
    cmd.env("AMUX_WORKSPACE_ID", workspace_id.to_string());
    cmd.env("AMUX_SURFACE_ID", surface_id.to_string());
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

    Ok(PaneSurface {
        id: surface_id,
        pane,
        byte_rx,
        scroll_offset: 0,
        scroll_accum: 0.0,
    })
}

// --- App ---

struct AmuxApp {
    workspaces: Vec<Workspace>,
    active_workspace_idx: usize,
    panes: HashMap<PaneId, ManagedPane>,
    next_pane_id: PaneId,
    next_workspace_id: u64,
    next_surface_id: u64,
    sidebar: SidebarState,
    ipc_rx: std::sync::mpsc::Receiver<IpcCommand>,
    socket_addr: amux_ipc::IpcAddr,
    config: Arc<AmuxTermConfig>,
    last_panel_rect: Option<egui::Rect>,
    /// Instant when focus last changed pane, for fade-out animation.
    focus_changed_at: std::time::Instant,
    /// The pane that received focus (for rendering the fade indicator).
    focus_highlight_pane: PaneId,
}

impl AmuxApp {
    fn active_workspace(&self) -> &Workspace {
        &self.workspaces[self.active_workspace_idx]
    }

    fn active_workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspaces[self.active_workspace_idx]
    }

    fn set_focus(&mut self, pane_id: PaneId) {
        let ws = self.active_workspace_mut();
        if ws.focused_pane != pane_id {
            ws.focused_pane = pane_id;
            self.focus_changed_at = std::time::Instant::now();
            self.focus_highlight_pane = pane_id;
        }
    }

    fn flash_focus(&mut self) {
        self.focus_changed_at = std::time::Instant::now();
        self.focus_highlight_pane = self.focused_pane_id();
    }

    fn focused_pane_id(&self) -> PaneId {
        self.active_workspace().focused_pane
    }
}

impl eframe::App for AmuxApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Drain PTY output from all surfaces in all panes
        let mut got_data = false;
        for managed in self.panes.values_mut() {
            for surface in &mut managed.surfaces {
                while let Ok(bytes) = surface.byte_rx.try_recv() {
                    got_data = true;
                    surface.pane.feed_bytes(&bytes);
                }
            }
        }

        // Process IPC commands
        self.process_ipc_commands();

        // Handle keyboard shortcuts BEFORE terminal input
        let shortcut_consumed = self.handle_shortcuts(ctx);

        // Handle keyboard/paste input -> focused pane's active surface only
        if !shortcut_consumed {
            self.handle_input(ctx);
        }

        // Render sidebar
        if self.sidebar.visible {
            self.render_sidebar(ctx);
        }

        // Render main content
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                let panel_rect = ui.available_rect_before_wrap();
                self.last_panel_rect = Some(panel_rect);

                // Handle divider dragging
                self.handle_divider_drag(ui, panel_rect);

                let zoomed = self.active_workspace().zoomed;
                if let Some(zoomed_id) = zoomed {
                    // Zoomed mode: render single pane fullscreen
                    self.render_single_pane(ui, zoomed_id, panel_rect, true);
                    self.resize_pane_if_needed(zoomed_id, panel_rect, ui);
                } else {
                    // Normal mode: render all panes at computed rects
                    let layout = self.active_workspace().tree.layout(panel_rect);
                    let focused = self.focused_pane_id();

                    // Click-to-focus
                    if ui.input(|i| i.pointer.any_pressed()) {
                        if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                            for &(id, rect) in &layout {
                                if rect.contains(pos) && id != focused {
                                    self.set_focus(id);
                                    break;
                                }
                            }
                        }
                    }

                    // Render dividers
                    let dividers = self.active_workspace().tree.dividers(panel_rect);
                    let painter = ui.painter();
                    for div in &dividers {
                        painter.rect_filled(div.rect, 0.0, egui::Color32::from_gray(60));
                    }

                    // Render each pane (with its own tab bar)
                    let focused = self.focused_pane_id();
                    for &(id, rect) in &layout {
                        let is_focused = id == focused;
                        self.render_single_pane(ui, id, rect, is_focused);
                        self.resize_pane_if_needed(id, rect, ui);
                    }
                }

                ui.allocate_rect(panel_rect, egui::Sense::click_and_drag());
            });

        // Update window title from focused pane's active surface
        let focused_id = self.focused_pane_id();
        if let Some(managed) = self.panes.get(&focused_id) {
            let title = managed.active_surface().pane.title();
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
    /// Render a single pane: tab bar (if >1 surface) + terminal content.
    fn render_single_pane(
        &mut self,
        ui: &mut egui::Ui,
        pane_id: PaneId,
        rect: egui::Rect,
        is_focused: bool,
    ) {
        let managed = match self.panes.get_mut(&pane_id) {
            Some(m) => m,
            None => return,
        };

        // Always show tab bar
        let tab_rect = egui::Rect::from_min_size(
            rect.min,
            egui::vec2(rect.width(), TAB_BAR_HEIGHT),
        );
        let content_rect = egui::Rect::from_min_max(
            egui::pos2(rect.min.x, rect.min.y + TAB_BAR_HEIGHT),
            rect.max,
        );

        {
            let painter = ui.painter();
            painter.rect_filled(tab_rect, 0.0, egui::Color32::from_gray(35));

            let active_idx = managed.active_surface_idx;
            let tab_font = egui::FontId::proportional(11.0);
            let mut x = tab_rect.min.x + 2.0;

            // Track actions to apply after rendering
            let mut switch_to: Option<usize> = None;
            let mut close_tab: Option<usize> = None;

            for (idx, surface) in managed.surfaces.iter().enumerate() {
                let is_active = idx == active_idx;
                let label = surface.pane.title();
                let label = if label.is_empty() {
                    format!("tab {}", idx + 1)
                } else {
                    // Truncate long titles
                    if label.len() > 20 {
                        format!("{}...", &label[..17])
                    } else {
                        label.to_string()
                    }
                };

                let text_galley = painter.layout_no_wrap(
                    label.clone(),
                    tab_font.clone(),
                    egui::Color32::WHITE,
                );
                let text_width = text_galley.size().x;
                let tab_w = text_width + 24.0;

                let this_tab = egui::Rect::from_min_size(
                    egui::pos2(x, tab_rect.min.y),
                    egui::vec2(tab_w, TAB_BAR_HEIGHT),
                );

                // Tab background
                if is_active {
                    painter.rect_filled(this_tab, 0.0, egui::Color32::from_gray(50));
                    // Active underline
                    let underline = egui::Rect::from_min_size(
                        egui::pos2(x, tab_rect.max.y - 2.0),
                        egui::vec2(tab_w, 2.0),
                    );
                    painter.rect_filled(
                        underline,
                        0.0,
                        egui::Color32::from_rgb(80, 140, 220),
                    );
                }

                let text_color = if is_active {
                    egui::Color32::WHITE
                } else {
                    egui::Color32::from_gray(130)
                };
                painter.text(
                    egui::pos2(x + 6.0, tab_rect.min.y + 5.0),
                    egui::Align2::LEFT_TOP,
                    &label,
                    tab_font.clone(),
                    text_color,
                );

                // Close button
                let close_rect = egui::Rect::from_center_size(
                    egui::pos2(x + tab_w - 10.0, tab_rect.center().y),
                    egui::vec2(12.0, 12.0),
                );
                painter.text(
                    close_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "x",
                    egui::FontId::proportional(9.0),
                    egui::Color32::from_gray(90),
                );

                // Hit testing
                if ui.input(|i| i.pointer.any_pressed()) {
                    if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                        if close_rect.contains(pos) && managed.surfaces.len() > 1 {
                            close_tab = Some(idx);
                        } else if this_tab.contains(pos) && !is_active {
                            switch_to = Some(idx);
                        }
                    }
                }

                x += tab_w + 1.0;
            }

            // "+" button to add tab
            let plus_rect = egui::Rect::from_min_size(
                egui::pos2(x + 2.0, tab_rect.min.y),
                egui::vec2(20.0, TAB_BAR_HEIGHT),
            );
            painter.text(
                plus_rect.center(),
                egui::Align2::CENTER_CENTER,
                "+",
                egui::FontId::proportional(14.0),
                egui::Color32::from_gray(100),
            );
            if ui.input(|i| i.pointer.any_pressed()) {
                if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                    if plus_rect.contains(pos) {
                        let ws_id = self.active_workspace().id;
                        let sf_id = self.next_surface_id;
                        self.next_surface_id += 1;
                        if let Ok(surface) =
                            spawn_surface(80, 24, &self.socket_addr, &self.config, ws_id, sf_id)
                        {
                            // Re-borrow managed after spawn_surface
                            if let Some(m) = self.panes.get_mut(&pane_id) {
                                m.surfaces.push(surface);
                                m.active_surface_idx = m.surfaces.len() - 1;
                            }
                        }
                        return; // skip further rendering this frame
                    }
                }
            }

            // Apply tab switch/close (need to re-borrow managed)
            let managed = self.panes.get_mut(&pane_id).unwrap();
            if let Some(idx) = switch_to {
                managed.active_surface_idx = idx;
            }
            if let Some(idx) = close_tab {
                managed.surfaces.remove(idx);
                if managed.active_surface_idx >= managed.surfaces.len() {
                    managed.active_surface_idx = managed.surfaces.len() - 1;
                }
            }
        }

        // Render terminal content for the active surface
        let managed = match self.panes.get_mut(&pane_id) {
            Some(m) => m,
            None => return,
        };
        let surface = managed.active_surface_mut();
        render_pane(
            ui,
            &mut surface.pane,
            content_rect,
            is_focused,
            surface.scroll_offset,
        );

        // Fading focus indicator
        if pane_id == self.focus_highlight_pane {
            let elapsed = self.focus_changed_at.elapsed().as_secs_f32();
            let fade_duration = 1.0; // seconds
            let alpha = ((1.0 - elapsed / fade_duration).clamp(0.0, 1.0) * 255.0) as u8;
            if alpha > 0 {
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(
                        2.0,
                        egui::Color32::from_rgba_unmultiplied(80, 140, 220, alpha),
                    ),
                    egui::StrokeKind::Inside,
                );
                // Keep repainting during the fade
                ui.ctx().request_repaint();
            }
        }
    }

    // --- Sidebar ---

    fn render_sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("sidebar")
            .resizable(true)
            .default_width(self.sidebar.width)
            .min_width(120.0)
            .max_width(400.0)
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_gray(30))
                    .inner_margin(8.0),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;

                ui.label(
                    egui::RichText::new("Workspaces")
                        .strong()
                        .color(egui::Color32::from_gray(180)),
                );
                ui.add_space(4.0);

                let active_idx = self.active_workspace_idx;
                let mut switch_to: Option<usize> = None;

                for (idx, ws) in self.workspaces.iter().enumerate() {
                    let is_active = idx == active_idx;
                    let bg = if is_active {
                        egui::Color32::from_gray(55)
                    } else {
                        egui::Color32::TRANSPARENT
                    };

                    let response = ui.horizontal(|ui| {
                        let (rect, response) = ui.allocate_exact_size(
                            ui.available_size_before_wrap(),
                            egui::Sense::click(),
                        );
                        if ui.is_rect_visible(rect) {
                            ui.painter().rect_filled(rect, 4.0, bg);
                            let text_color = if is_active {
                                egui::Color32::WHITE
                            } else {
                                egui::Color32::from_gray(160)
                            };
                            ui.painter().text(
                                rect.min + egui::vec2(8.0, 4.0),
                                egui::Align2::LEFT_TOP,
                                &ws.title,
                                egui::FontId::proportional(14.0),
                                text_color,
                            );
                            // Pane count badge
                            let count = ws.tree.iter_panes().len();
                            let count_text = format!("{}", count);
                            ui.painter().text(
                                egui::pos2(rect.right() - 8.0, rect.min.y + 4.0),
                                egui::Align2::RIGHT_TOP,
                                &count_text,
                                egui::FontId::proportional(11.0),
                                egui::Color32::from_gray(100),
                            );
                        }
                        response
                    });
                    if response.inner.clicked() && !is_active {
                        switch_to = Some(idx);
                    }
                }

                if let Some(idx) = switch_to {
                    self.active_workspace_idx = idx;
                }

                ui.add_space(8.0);

                if ui
                    .button(
                        egui::RichText::new("+ New Workspace")
                            .color(egui::Color32::from_gray(140)),
                    )
                    .clicked()
                {
                    self.create_workspace(None);
                }
            });
    }

    // --- Resize ---

    fn resize_pane_if_needed(&mut self, id: PaneId, rect: egui::Rect, ui: &egui::Ui) {
        let font_id = egui::FontId::monospace(FONT_SIZE);
        let cell_width = ui.fonts(|f| f.glyph_width(&font_id, 'M'));
        let cell_height = ui.fonts(|f| f.row_height(&font_id));

        // Account for tab bar height (always shown)
        let content_height = rect.height() - TAB_BAR_HEIGHT;

        let cols = (rect.width() / cell_width).floor() as usize;
        let rows = (content_height / cell_height).floor() as usize;

        if cols == 0 || rows == 0 {
            return;
        }

        // Resize the active surface if its dimensions don't match the pane rect.
        // This handles both pane rect changes and tab switches (new surface at 80x24).
        if let Some(managed) = self.panes.get_mut(&id) {
            let surface = managed.active_surface_mut();
            let (cur_cols, cur_rows) = surface.pane.dimensions();
            if cur_cols != cols || cur_rows != rows {
                let _ = surface.pane.resize(cols as u16, rows as u16);
            }
        }
    }

    // --- Pane/Workspace management ---

    fn spawn_pane_with_surface(&mut self) -> PaneId {
        let pane_id = self.next_pane_id;
        self.next_pane_id += 1;
        let sf_id = self.next_surface_id;
        self.next_surface_id += 1;

        let ws_id = self.active_workspace().id;

        match spawn_surface(80, 24, &self.socket_addr, &self.config, ws_id, sf_id) {
            Ok(surface) => {
                self.panes.insert(
                    pane_id,
                    ManagedPane {
                        surfaces: vec![surface],
                        active_surface_idx: 0,
                    },
                );
            }
            Err(e) => {
                tracing::error!("Failed to spawn pane: {}", e);
            }
        }
        pane_id
    }

    fn create_workspace(&mut self, title: Option<String>) -> u64 {
        let ws_id = self.next_workspace_id;
        self.next_workspace_id += 1;

        let title = title.unwrap_or_else(|| format!("workspace-{}", ws_id));

        let pane_id = self.next_pane_id;
        self.next_pane_id += 1;
        let sf_id = self.next_surface_id;
        self.next_surface_id += 1;

        match spawn_surface(80, 24, &self.socket_addr, &self.config, ws_id, sf_id) {
            Ok(surface) => {
                self.panes.insert(
                    pane_id,
                    ManagedPane {
                        surfaces: vec![surface],
                        active_surface_idx: 0,
                    },
                );
            }
            Err(e) => {
                tracing::error!("Failed to spawn pane for workspace: {}", e);
                return ws_id;
            }
        }

        let workspace = Workspace {
            id: ws_id,
            title,
            tree: PaneTree::new(pane_id),
            focused_pane: pane_id,
            zoomed: None,
            dragging_divider: None,
            last_pane_sizes: HashMap::new(),
            status: None,
        };

        self.workspaces.push(workspace);
        self.active_workspace_idx = self.workspaces.len() - 1;
        ws_id
    }

    fn add_surface_to_focused_pane(&mut self) -> Option<u64> {
        let sf_id = self.next_surface_id;
        self.next_surface_id += 1;
        let ws_id = self.active_workspace().id;
        let focused = self.focused_pane_id();

        match spawn_surface(80, 24, &self.socket_addr, &self.config, ws_id, sf_id) {
            Ok(surface) => {
                if let Some(managed) = self.panes.get_mut(&focused) {
                    managed.surfaces.push(surface);
                    managed.active_surface_idx = managed.surfaces.len() - 1;
                    Some(sf_id)
                } else {
                    None
                }
            }
            Err(e) => {
                tracing::error!("Failed to spawn surface: {}", e);
                None
            }
        }
    }

    fn close_workspace_at(&mut self, ws_idx: usize) {
        if self.workspaces.len() <= 1 {
            return;
        }
        let pane_ids: Vec<PaneId> = self.workspaces[ws_idx].tree.iter_panes();
        for id in pane_ids {
            self.panes.remove(&id);
        }
        self.workspaces.remove(ws_idx);
        if self.active_workspace_idx >= self.workspaces.len() {
            self.active_workspace_idx = self.workspaces.len() - 1;
        }
    }

    // --- Shortcuts ---

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
                #[cfg(target_os = "macos")]
                let is_cmd = modifiers.mac_cmd || modifiers.command;
                #[cfg(not(target_os = "macos"))]
                let is_cmd = modifiers.ctrl && modifiers.shift;

                // Toggle sidebar: Cmd+B / Ctrl+B
                #[cfg(target_os = "macos")]
                if is_cmd && !modifiers.shift && *key == egui::Key::B {
                    self.sidebar.visible = !self.sidebar.visible;
                    return true;
                }
                #[cfg(not(target_os = "macos"))]
                if modifiers.ctrl && !modifiers.shift && *key == egui::Key::B {
                    self.sidebar.visible = !self.sidebar.visible;
                    return true;
                }

                // New workspace: Cmd+N / Ctrl+Shift+N
                #[cfg(target_os = "macos")]
                if is_cmd && !modifiers.shift && *key == egui::Key::N {
                    self.create_workspace(None);
                    return true;
                }
                #[cfg(not(target_os = "macos"))]
                if modifiers.ctrl && modifiers.shift && *key == egui::Key::N {
                    self.create_workspace(None);
                    return true;
                }

                // New tab in focused pane: Cmd+T / Ctrl+Shift+T
                #[cfg(target_os = "macos")]
                if is_cmd && !modifiers.shift && *key == egui::Key::T {
                    self.add_surface_to_focused_pane();
                    return true;
                }
                #[cfg(not(target_os = "macos"))]
                if modifiers.ctrl && modifiers.shift && *key == egui::Key::T {
                    self.add_surface_to_focused_pane();
                    return true;
                }

                // Next workspace: Cmd+Shift+]
                #[cfg(target_os = "macos")]
                if is_cmd && modifiers.shift && *key == egui::Key::CloseBracket {
                    if self.workspaces.len() > 1 {
                        self.active_workspace_idx =
                            (self.active_workspace_idx + 1) % self.workspaces.len();
                    }
                    return true;
                }
                #[cfg(not(target_os = "macos"))]
                if modifiers.ctrl && modifiers.shift && *key == egui::Key::CloseBracket {
                    if self.workspaces.len() > 1 {
                        self.active_workspace_idx =
                            (self.active_workspace_idx + 1) % self.workspaces.len();
                    }
                    return true;
                }

                // Prev workspace: Cmd+Shift+[
                #[cfg(target_os = "macos")]
                if is_cmd && modifiers.shift && *key == egui::Key::OpenBracket {
                    if self.workspaces.len() > 1 {
                        self.active_workspace_idx = if self.active_workspace_idx == 0 {
                            self.workspaces.len() - 1
                        } else {
                            self.active_workspace_idx - 1
                        };
                    }
                    return true;
                }
                #[cfg(not(target_os = "macos"))]
                if modifiers.ctrl && modifiers.shift && *key == egui::Key::OpenBracket {
                    if self.workspaces.len() > 1 {
                        self.active_workspace_idx = if self.active_workspace_idx == 0 {
                            self.workspaces.len() - 1
                        } else {
                            self.active_workspace_idx - 1
                        };
                    }
                    return true;
                }

                // Jump to workspace 1-8
                #[cfg(target_os = "macos")]
                let is_jump_mod = is_cmd && !modifiers.shift;
                #[cfg(not(target_os = "macos"))]
                let is_jump_mod = modifiers.ctrl && !modifiers.shift;

                if is_jump_mod {
                    let num = match key {
                        egui::Key::Num1 => Some(0usize),
                        egui::Key::Num2 => Some(1),
                        egui::Key::Num3 => Some(2),
                        egui::Key::Num4 => Some(3),
                        egui::Key::Num5 => Some(4),
                        egui::Key::Num6 => Some(5),
                        egui::Key::Num7 => Some(6),
                        egui::Key::Num8 => Some(7),
                        _ => None,
                    };
                    if let Some(idx) = num {
                        if idx < self.workspaces.len() {
                            self.active_workspace_idx = idx;
                            return true;
                        }
                    }
                }

                // Next tab in focused pane: Ctrl+Tab
                if modifiers.ctrl && !modifiers.shift && *key == egui::Key::Tab {
                    if let Some(managed) = self.panes.get_mut(&self.focused_pane_id()) {
                        if managed.surfaces.len() > 1 {
                            managed.active_surface_idx =
                                (managed.active_surface_idx + 1) % managed.surfaces.len();
                        }
                    }
                    return true;
                }

                // Prev tab in focused pane: Ctrl+Shift+Tab
                if modifiers.ctrl && modifiers.shift && *key == egui::Key::Tab {
                    if let Some(managed) = self.panes.get_mut(&self.focused_pane_id()) {
                        if managed.surfaces.len() > 1 {
                            managed.active_surface_idx = if managed.active_surface_idx == 0 {
                                managed.surfaces.len() - 1
                            } else {
                                managed.active_surface_idx - 1
                            };
                        }
                    }
                    return true;
                }

                // --- Pane shortcuts ---

                // Split right: Cmd+D / Ctrl+Shift+D
                if is_cmd && !modifiers.shift && *key == egui::Key::D {
                    return self.do_split(SplitDirection::Horizontal);
                }
                // Split down: Cmd+Shift+D / Ctrl+Shift+Down
                #[cfg(target_os = "macos")]
                if is_cmd && modifiers.shift && *key == egui::Key::D {
                    return self.do_split(SplitDirection::Vertical);
                }
                #[cfg(not(target_os = "macos"))]
                if modifiers.ctrl && modifiers.shift && *key == egui::Key::ArrowDown {
                    return self.do_split(SplitDirection::Vertical);
                }

                // Close: Cmd+W — cascade: tab -> pane -> workspace
                if is_cmd && *key == egui::Key::W {
                    return self.do_close_cascade();
                }

                // Navigate: Option+Cmd+Arrow / Ctrl+Alt+Arrow
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

                // Zoom toggle: Cmd+Shift+Enter / Ctrl+Shift+Enter
                #[cfg(target_os = "macos")]
                let is_zoom = is_cmd && modifiers.shift && *key == egui::Key::Enter;
                #[cfg(not(target_os = "macos"))]
                let is_zoom = modifiers.ctrl && modifiers.shift && *key == egui::Key::Enter;

                if is_zoom {
                    return self.do_toggle_zoom();
                }

                // Scroll
                if modifiers.shift && *key == egui::Key::PageUp {
                    return self.do_scroll(-1);
                }
                if modifiers.shift && *key == egui::Key::PageDown {
                    return self.do_scroll(1);
                }
            }
        }

        // Mouse wheel scrolling
        let scroll_delta = ctx.input(|i| i.smooth_scroll_delta.y);
        if scroll_delta != 0.0 {
            let focused_id = self.focused_pane_id();
            if let Some(managed) = self.panes.get_mut(&focused_id) {
                let surface = managed.active_surface_mut();
                let font_id = egui::FontId::monospace(FONT_SIZE);
                let cell_height = ctx.fonts(|f| f.row_height(&font_id));

                surface.scroll_accum += -scroll_delta / cell_height;
                let whole_lines = surface.scroll_accum.trunc() as isize;
                if whole_lines != 0 {
                    surface.scroll_accum -= whole_lines as f32;
                    self.do_scroll_lines(whole_lines);
                }
            }
        }

        false
    }

    fn do_split(&mut self, direction: SplitDirection) -> bool {
        let new_id = self.spawn_pane_with_surface();
        let ws = self.active_workspace_mut();
        ws.tree.split(ws.focused_pane, direction, new_id);
        self.set_focus(new_id);
        true
    }

    fn do_close_cascade(&mut self) -> bool {
        let focused_id = self.focused_pane_id();

        // First check: close a tab if >1 tab in focused pane
        if let Some(managed) = self.panes.get_mut(&focused_id) {
            if managed.surfaces.len() > 1 {
                managed.surfaces.remove(managed.active_surface_idx);
                if managed.active_surface_idx >= managed.surfaces.len() {
                    managed.active_surface_idx = managed.surfaces.len() - 1;
                }
                return true;
            }
        }

        // Then close pane if >1 pane
        let pane_count = self.active_workspace().tree.iter_panes().len();
        if pane_count > 1 {
            let ws = self.active_workspace_mut();
            if let Some(new_focus) = ws.tree.close(focused_id) {
                ws.last_pane_sizes.remove(&focused_id);
                if ws.zoomed == Some(focused_id) {
                    ws.zoomed = None;
                }
                self.panes.remove(&focused_id);
                self.set_focus(new_focus);
            }
            return true;
        }

        // Last pane in workspace -> close workspace
        let ws_idx = self.active_workspace_idx;
        self.close_workspace_at(ws_idx);
        true
    }

    fn do_navigate(&mut self, dir: NavDirection) -> bool {
        if let Some(rect) = self.last_panel_rect {
            let ws = self.active_workspace();
            if let Some(neighbor) = ws.tree.neighbor(ws.focused_pane, dir, rect) {
                self.set_focus(neighbor);
            } else {
                self.flash_focus();
            }
        }
        true
    }

    fn do_toggle_zoom(&mut self) -> bool {
        let ws = self.active_workspace_mut();
        if ws.zoomed.is_some() {
            ws.zoomed = None;
        } else {
            ws.zoomed = Some(ws.focused_pane);
        }
        true
    }

    fn do_scroll(&mut self, pages: isize) -> bool {
        let focused_id = self.focused_pane_id();
        if let Some(managed) = self.panes.get_mut(&focused_id) {
            let surface = managed.active_surface_mut();
            let (_, rows) = surface.pane.dimensions();
            let page_size = rows.saturating_sub(1).max(1);
            let lines = pages * page_size as isize;
            let total = surface.pane.screen().scrollback_rows();
            let max_offset = total.saturating_sub(rows);
            let new_offset = surface.scroll_offset as isize - lines;
            surface.scroll_offset = (new_offset.max(0) as usize).min(max_offset);
        }
        true
    }

    fn do_scroll_lines(&mut self, lines: isize) {
        let focused_id = self.focused_pane_id();
        if let Some(managed) = self.panes.get_mut(&focused_id) {
            let surface = managed.active_surface_mut();
            let (_, rows) = surface.pane.dimensions();
            let total = surface.pane.screen().scrollback_rows();
            let max_offset = total.saturating_sub(rows);
            let new_offset = surface.scroll_offset as isize - lines;
            surface.scroll_offset = (new_offset.max(0) as usize).min(max_offset);
        }
    }

    // --- Divider Drag ---

    fn handle_divider_drag(&mut self, ui: &egui::Ui, panel_rect: egui::Rect) {
        let zoomed = self.active_workspace().zoomed;
        if zoomed.is_some() {
            return;
        }

        let dividers = self.active_workspace().tree.dividers(panel_rect);
        let pointer_pos = ui.input(|i| i.pointer.hover_pos());
        let primary_down = ui.input(|i| i.pointer.primary_down());
        let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
        let primary_released = ui.input(|i| i.pointer.primary_released());

        let is_dragging = self.active_workspace().dragging_divider.is_some();

        if let Some(pos) = pointer_pos {
            if !is_dragging {
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

        if primary_pressed && !is_dragging {
            if let Some(pos) = pointer_pos {
                if let Some(div) = dividers.iter().find(|d| d.rect.expand(4.0).contains(pos)) {
                    self.active_workspace_mut().dragging_divider = Some(DragState {
                        node_path: div.node_path.clone(),
                        direction: div.direction,
                    });
                }
            }
        }

        if primary_down {
            let ws = self.active_workspace_mut();
            if let Some(ref drag) = ws.dragging_divider {
                let delta = ui.input(|i| i.pointer.delta());
                let px_delta = match drag.direction {
                    SplitDirection::Horizontal => delta.x,
                    SplitDirection::Vertical => delta.y,
                };
                if px_delta != 0.0 {
                    let path = drag.node_path.clone();
                    let dir = drag.direction;
                    ws.tree.resize_divider(&path, px_delta, panel_rect);
                    match dir {
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

        if primary_released {
            self.active_workspace_mut().dragging_divider = None;
        }
    }

    // --- Input ---

    fn handle_input(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        let focused_id = self.focused_pane_id();

        let managed = match self.panes.get_mut(&focused_id) {
            Some(m) => m,
            None => return,
        };
        let surface = managed.active_surface_mut();

        for event in &events {
            match event {
                egui::Event::Text(text) => {
                    surface.scroll_offset = 0;
                    surface.scroll_accum = 0.0;
                    let _ = surface.pane.write_bytes(text.as_bytes());
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if let Some(bytes) = encode_egui_key(key, modifiers) {
                        surface.scroll_offset = 0;
                        surface.scroll_accum = 0.0;
                        let _ = surface.pane.write_bytes(&bytes);
                    }
                }
                egui::Event::Paste(text) => {
                    surface.scroll_offset = 0;
                    surface.scroll_accum = 0.0;
                    let _ = surface.pane.write_bytes(b"\x1b[200~");
                    let _ = surface.pane.write_bytes(text.as_bytes());
                    let _ = surface.pane.write_bytes(b"\x1b[201~");
                }
                _ => {}
            }
        }
    }

    // --- IPC ---

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
            "system.identify" => {
                let ws = self.active_workspace();
                let focused = ws.focused_pane;
                let sf_id = self
                    .panes
                    .get(&focused)
                    .map(|m| m.active_surface().id)
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
                        if let Some(managed) = self.panes.get_mut(&pane_id) {
                            for (sf_idx, sf) in managed.surfaces.iter_mut().enumerate() {
                                let (cols, rows) = sf.pane.dimensions();
                                surfaces.push(serde_json::json!({
                                    "id": sf.id.to_string(),
                                    "pane_id": pane_id.to_string(),
                                    "workspace_id": ws.id.to_string(),
                                    "title": sf.pane.title(),
                                    "cols": cols,
                                    "rows": rows,
                                    "alive": sf.pane.is_alive(),
                                    "active": sf_idx == managed.active_surface_idx,
                                }));
                            }
                        }
                    }
                }
                Response::ok(req.id.clone(), serde_json::json!({"surfaces": surfaces}))
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
                                let text = sf.pane.read_screen_text();
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
                        let new_id = self.spawn_pane_with_surface();
                        let ws = self.active_workspace_mut();
                        ws.tree.split(ws.focused_pane, dir, new_id);
                        self.set_focus(new_id);
                        Response::ok(
                            req.id.clone(),
                            serde_json::json!({"pane_id": new_id.to_string()}),
                        )
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
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
                    if let Some(managed) = self.panes.get_mut(id) {
                        let sf = managed.active_surface_mut();
                        let (cols, rows) = sf.pane.dimensions();
                        let alive = sf.pane.is_alive();
                        let tab_count = managed.surfaces.len();
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
                        let ws_id = self.create_workspace(params.title);
                        Response::ok(
                            req.id.clone(),
                            serde_json::json!({"workspace_id": ws_id.to_string()}),
                        )
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

                        let sf_id = self.next_surface_id;
                        self.next_surface_id += 1;
                        let ws_id = self.active_workspace().id;

                        match spawn_surface(
                            80,
                            24,
                            &self.socket_addr,
                            &self.config,
                            ws_id,
                            sf_id,
                        ) {
                            Ok(surface) => {
                                if let Some(managed) = self.panes.get_mut(&target_pane) {
                                    managed.surfaces.push(surface);
                                    managed.active_surface_idx = managed.surfaces.len() - 1;
                                    Response::ok(
                                        req.id.clone(),
                                        serde_json::json!({"surface_id": sf_id.to_string()}),
                                    )
                                } else {
                                    Response::err(
                                        req.id.clone(),
                                        "not_found",
                                        "pane not found",
                                    )
                                }
                            }
                            Err(e) => Response::err(
                                req.id.clone(),
                                "spawn_error",
                                &e.to_string(),
                            ),
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
                                for managed in self.panes.values_mut() {
                                    if let Some(idx) =
                                        managed.surfaces.iter().position(|s| s.id == sf_id)
                                    {
                                        if managed.surfaces.len() <= 1 {
                                            return Response::err(
                                                req.id.clone(),
                                                "last_surface",
                                                "cannot close the last surface in a pane",
                                            );
                                        }
                                        managed.surfaces.remove(idx);
                                        if managed.active_surface_idx >= managed.surfaces.len() {
                                            managed.active_surface_idx =
                                                managed.surfaces.len() - 1;
                                        }
                                        return Response::ok(
                                            req.id.clone(),
                                            serde_json::json!({}),
                                        );
                                    }
                                }
                                Response::err(req.id.clone(), "not_found", "surface not found")
                            } else {
                                Response::err(
                                    req.id.clone(),
                                    "invalid_params",
                                    "invalid surface_id",
                                )
                            }
                        } else {
                            // Close active surface in focused pane
                            if let Some(managed) = self.panes.get_mut(&focused) {
                                if managed.surfaces.len() <= 1 {
                                    return Response::err(
                                        req.id.clone(),
                                        "last_surface",
                                        "cannot close the last surface in a pane",
                                    );
                                }
                                managed.surfaces.remove(managed.active_surface_idx);
                                if managed.active_surface_idx >= managed.surfaces.len() {
                                    managed.active_surface_idx = managed.surfaces.len() - 1;
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
                            for (pane_id, managed) in &mut self.panes {
                                if let Some(idx) =
                                    managed.surfaces.iter().position(|s| s.id == sf_id)
                                {
                                    managed.active_surface_idx = idx;
                                    // Also focus the pane containing this surface
                                    let pid = *pane_id;
                                    self.set_focus(pid);
                                    return Response::ok(req.id.clone(), serde_json::json!({}));
                                }
                            }
                            Response::err(req.id.clone(), "not_found", "surface not found")
                        } else {
                            Response::err(
                                req.id.clone(),
                                "invalid_params",
                                "invalid surface_id",
                            )
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
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
            return self
                .panes
                .get_mut(&focused)
                .map(|m| m.active_surface_mut());
        }

        // Try as surface ID first: find which pane contains it
        if let Ok(sf_id) = surface_id.parse::<u64>() {
            let target_pane = self
                .panes
                .iter()
                .find(|(_, m)| m.surfaces.iter().any(|s| s.id == sf_id))
                .map(|(pid, _)| *pid);

            if let Some(pid) = target_pane {
                return self.panes.get_mut(&pid).and_then(|m| {
                    m.surfaces.iter_mut().find(|s| s.id == sf_id)
                });
            }

            // Fall back to treating it as a pane ID
            if let Ok(pane_id) = surface_id.parse::<PaneId>() {
                return self
                    .panes
                    .get_mut(&pane_id)
                    .map(|m| m.active_surface_mut());
            }
        }

        None
    }

    fn resolve_surface(&self, surface_id: &str) -> Option<&PaneSurface> {
        if surface_id == "default" || surface_id.is_empty() {
            let focused = self.focused_pane_id();
            self.panes.get(&focused).map(|m| m.active_surface())
        } else if let Ok(sf_id) = surface_id.parse::<u64>() {
            for managed in self.panes.values() {
                if let Some(sf) = managed.surfaces.iter().find(|s| s.id == sf_id) {
                    return Some(sf);
                }
            }
            if let Ok(pane_id) = surface_id.parse::<PaneId>() {
                self.panes.get(&pane_id).map(|m| m.active_surface())
            } else {
                None
            }
        } else {
            None
        }
    }
}

// --- Rendering ---

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

    let terminal_rect = egui::Rect::from_min_size(
        origin,
        egui::vec2(
            actual_cols as f32 * cell_width,
            actual_rows as f32 * cell_height,
        ),
    );
    let clipped_rect = terminal_rect.intersect(rect);
    painter.rect_filled(clipped_rect, 0.0, bg_default);

    let total = screen.scrollback_rows();
    let end = total.saturating_sub(scroll_offset);
    let start = end.saturating_sub(actual_rows);
    let lines = screen.lines_in_phys_range(start..end);

    for (row_idx, line) in lines.iter().enumerate() {
        let y = origin.y + row_idx as f32 * cell_height;
        if y + cell_height < rect.min.y || y > rect.max.y {
            continue;
        }

        for cell_ref in line.visible_cells() {
            let col_idx = cell_ref.cell_index();
            if col_idx >= actual_cols {
                break;
            }

            let x = origin.x + col_idx as f32 * cell_width;
            if x + cell_width < rect.min.x || x > rect.max.x {
                continue;
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

    // Draw cursor
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
                let bar_rect =
                    egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(2.0, cell_height));
                painter.rect_filled(bar_rect, 0.0, cursor_color);
            }
            CursorShape::BlinkingUnderline | CursorShape::SteadyUnderline => {
                let underline_rect = egui::Rect::from_min_size(
                    egui::pos2(cx, cy + cell_height - 2.0),
                    egui::vec2(cell_width, 2.0),
                );
                painter.rect_filled(underline_rect, 0.0, cursor_color);
            }
            CursorShape::Default | CursorShape::BlinkingBlock | CursorShape::SteadyBlock => {
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

    // Scroll indicator
    if scroll_offset > 0 {
        let indicator = format!("[+{}]", scroll_offset);
        let indicator_font = egui::FontId::monospace(FONT_SIZE * 0.8);
        let text_color = egui::Color32::from_rgba_unmultiplied(255, 200, 50, 200);
        let bg_color = egui::Color32::from_rgba_unmultiplied(40, 40, 40, 180);

        let galley = painter.layout_no_wrap(indicator, indicator_font, text_color);
        let text_size = galley.size();
        let padding = 4.0;
        let indicator_rect = egui::Rect::from_min_size(
            egui::pos2(
                rect.right() - text_size.x - padding * 2.0,
                rect.top() + padding,
            ),
            egui::vec2(text_size.x + padding * 2.0, text_size.y + padding),
        );
        painter.rect_filled(indicator_rect, 3.0, bg_color);
        painter.galley(
            egui::pos2(
                indicator_rect.left() + padding,
                indicator_rect.top() + padding * 0.5,
            ),
            galley,
            text_color,
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

// --- Key encoding (egui events -> terminal bytes) ---

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
