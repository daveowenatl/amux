//! Single-pane rendering: tab bar + terminal content area.
//!
//! Draws the per-pane tab bar (surface tabs) and delegates the terminal
//! content rendering to either the GPU or software renderer. Handles
//! tab interactions: click to activate, middle-click to close, right-click
//! context menu, drag to reorder, and the trailing "+" to add a surface.

use crate::*;

/// Truncate a tab title to fit within `max_chars`. When `is_path`
/// is true (title sourced from CWD metadata), uses middle ellipsis
/// to keep both the root and leaf visible: `~/src/…/my-branch`.
/// Otherwise truncates from the end with trailing `…`.
fn truncate_tab_title(title: &str, max_chars: usize, is_path: bool) -> String {
    let len = title.chars().count();
    if len <= max_chars {
        return title.to_string();
    }

    if is_path {
        // Middle ellipsis: keep the start (root) and end (leaf).
        // Split roughly 40% start / 60% end so the leaf (most
        // distinguishing part) gets more space.
        let budget = max_chars.saturating_sub(1); // 1 char for …
        let start_len = budget * 2 / 5;
        let end_len = budget - start_len;
        let start: String = title.chars().take(start_len).collect();
        let end: String = title.chars().skip(len - end_len).collect();
        format!("{start}\u{2026}{end}")
    } else {
        let prefix: String = title.chars().take(max_chars - 1).collect();
        format!("{prefix}\u{2026}")
    }
}

impl AmuxApp {
    /// Render a single pane: tab bar (if >1 surface) + terminal content.
    pub(crate) fn render_single_pane(
        &mut self,
        ui: &mut egui::Ui,
        pane_id: PaneId,
        rect: egui::Rect,
        is_focused: bool,
    ) {
        // Collect info we need before borrowing panes mutably
        let (_active_is_browser_initial, active_browser_id, browser_pane_ids) =
            match self.panes.get(&pane_id) {
                Some(PaneEntry::Terminal(m)) => {
                    let active_bid = match m.active_tab() {
                        managed_pane::ActiveTab::Browser(bid) => Some(bid),
                        _ => None,
                    };
                    (m.active_is_browser(), active_bid, m.browser_pane_ids())
                }
                _ => return,
            };

        // Manage browser webview visibility: show active, hide others.
        // Native webviews sit above egui content, so hide them when an
        // overlay (notification panel, rename modal, find bar) is open.
        // TODO: replace with screenshot+hide for smoother UX.
        let overlay_open = self.show_notification_panel
            || self.rename_modal.is_some()
            || self.settings_modal.is_some()
            || self.find_state.is_some();
        for &bid in &browser_pane_ids {
            if let Some(PaneEntry::Browser(b)) = self.panes.get(&bid) {
                b.set_visible(active_browser_id == Some(bid) && !overlay_open);
            }
        }

        // Pre-collect browser tab info and favicon textures before mutable borrow.
        // Build a map from browser PaneId to (title, icon_tex) for tab rendering.
        let browser_tab_info: HashMap<PaneId, (String, Option<egui::TextureId>)> = browser_pane_ids
            .iter()
            .map(|&bid| {
                let (title, favicon_url) = self
                    .panes
                    .get(&bid)
                    .and_then(|e| e.as_browser())
                    .map(|b| {
                        let t = b.title();
                        let title = if t.is_empty() {
                            b.url().unwrap_or_else(|| "Browser".to_string())
                        } else {
                            t
                        };
                        (title, b.favicon_url())
                    })
                    .unwrap_or_else(|| ("Browser".to_string(), None));
                let icon_tex = favicon_url.and_then(|url| self.get_favicon(ui.ctx(), &url, bid));
                (bid, (title, icon_tex))
            })
            .collect();

        let managed = match self.panes.get_mut(&pane_id) {
            Some(PaneEntry::Terminal(m)) => m,
            _ => return,
        };

        // Always show tab bar
        let tab_rect =
            egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), TAB_BAR_HEIGHT));
        let content_rect = egui::Rect::from_min_max(
            egui::pos2(rect.min.x, rect.min.y + TAB_CONTENT_TOP_INSET),
            egui::pos2(rect.max.x, rect.max.y - TERMINAL_BOTTOM_PAD),
        );
        // Paint bottom padding strip with terminal background color.
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.min.x, rect.max.y - TERMINAL_BOTTOM_PAD),
                rect.max,
            ),
            0.0,
            self.theme.terminal_bg(),
        );

        // The tab context menu is themed per-call-site below using
        // `popup_theme::with_menu_palette` — we no longer mutate
        // this `ui`'s style here, because that would leak into
        // every widget rendered later in `render_single_pane`
        // (terminal overlays, modals, etc.).

        {
            let painter = ui.painter();
            painter.rect_filled(tab_rect, 0.0, self.theme.tab_bar_bg());
            let bar_stroke = egui::Stroke::new(1.0, self.theme.chrome.tab_bar_border);
            painter.hline(tab_rect.x_range(), tab_rect.min.y, bar_stroke);
            painter.hline(tab_rect.x_range(), tab_rect.max.y, bar_stroke);

            let active_idx = managed.active_tab_idx;
            let tab_font = egui::FontId::proportional(11.5);
            let mut x = tab_rect.min.x + 2.0;

            // Get pointer state for hover detection and drag
            let hover_pos = ui.input(|i| i.pointer.hover_pos());
            let primary_pressed = ui.input(|i| i.pointer.primary_pressed());

            let any_released = ui.input(|i| i.pointer.any_released());
            let primary_down = ui.input(|i| i.pointer.primary_down());
            let interact_pos = ui.input(|i| i.pointer.interact_pos());

            // Track actions to apply after rendering
            let mut switch_to: Option<usize> = None;

            let mut close_tab: Option<usize> = None;
            let mut tab_rects: Vec<egui::Rect> = Vec::new();
            let mut drag_start: Option<usize> = None;
            let mut start_rename_tab: Option<usize> = None;

            // Build tab info for rendering from the unified tab list
            enum TabIcon {
                Terminal,
                Favicon(egui::TextureId),
                None,
            }
            struct TabInfo {
                icon: TabIcon,
                title: String,
                is_path: bool,
                is_dead: bool,
                is_browser: bool,
            }
            let tabs: Vec<TabInfo> = managed
                .tabs
                .iter()
                .map(|tab| match tab {
                    managed_pane::TabEntry::Terminal(surface) => {
                        // Run the pane's own title (the OSC-set /
                        // ConPTY-inherited value) through
                        // `sanitize_pane_title` to collapse ugly
                        // shell exe paths. `user_title` is
                        // passed through unchanged because that's
                        // explicitly user-set.
                        let sanitized_pane_title =
                            crate::title_sanitize::sanitize_pane_title(surface.pane.title());
                        let raw_title: &str = surface
                            .user_title
                            .as_deref()
                            .unwrap_or(sanitized_pane_title.as_ref());
                        let (raw, is_path) = if raw_title.is_empty() || raw_title == "?" {
                            let cwd_title = surface
                                .metadata
                                .cwd
                                .as_deref()
                                .map(|p| {
                                    if let Some(home) = dirs::home_dir() {
                                        let path = std::path::Path::new(p);
                                        if let Ok(rest) = path.strip_prefix(&home) {
                                            if rest.as_os_str().is_empty() {
                                                return "~".to_string();
                                            }
                                            return format!("~/{}", rest.to_string_lossy());
                                        }
                                    }
                                    p.to_string()
                                })
                                .unwrap_or_else(|| "Tab".to_string());
                            (cwd_title, true)
                        } else {
                            (raw_title.to_string(), false)
                        };
                        TabInfo {
                            icon: TabIcon::Terminal,
                            is_path,
                            title: raw,
                            is_dead: surface.exited.is_some(),
                            is_browser: false,
                        }
                    }
                    managed_pane::TabEntry::Browser(bid) => {
                        let (title, icon_tex) = browser_tab_info
                            .get(bid)
                            .cloned()
                            .unwrap_or_else(|| ("Browser".to_string(), None));
                        TabInfo {
                            icon: match icon_tex {
                                Some(tex) => TabIcon::Favicon(tex),
                                None => TabIcon::None,
                            },
                            title,
                            is_path: false,
                            is_dead: false,
                            is_browser: true,
                        }
                    }
                })
                .collect();

            for (idx, tab) in tabs.iter().enumerate() {
                let is_active = idx == active_idx;
                let title = truncate_tab_title(&tab.title, 30, tab.is_path);
                let is_dead = tab.is_dead;
                let has_icon = !matches!(tab.icon, TabIcon::None);
                let icon_space = if has_icon {
                    tab_icons::ICON_SIZE + 4.0
                } else {
                    0.0
                };

                let text_galley =
                    painter.layout_no_wrap(title.clone(), tab_font.clone(), egui::Color32::WHITE);
                let text_width = text_galley.size().x;
                let tab_w = (icon_space + text_width + 24.0).clamp(TAB_MIN_WIDTH, TAB_MAX_WIDTH);

                let this_tab = egui::Rect::from_min_size(
                    egui::pos2(x, tab_rect.min.y),
                    egui::vec2(tab_w, TAB_BAR_HEIGHT),
                );
                tab_rects.push(this_tab);

                let tab_hovered = hover_pos.is_some_and(|p| this_tab.contains(p));
                let is_leftmost = idx == 0;

                // Tab background + border
                let border_color = self.theme.chrome.tab_border;
                let side_stroke = egui::Stroke::new(1.0, border_color);
                if is_active {
                    painter.rect_filled(this_tab, 0.0, self.theme.chrome.tab_active_bg);
                    // Active highlight at the top
                    let topline = egui::Rect::from_min_size(
                        egui::pos2(x, tab_rect.min.y),
                        egui::vec2(tab_w, 2.0),
                    );
                    let accent = if is_dead {
                        egui::Color32::from_gray(60)
                    } else {
                        self.theme.chrome.accent
                    };
                    painter.rect_filled(topline, 0.0, accent);
                    // Side borders only (no bottom border — tab merges with terminal)
                    if !is_leftmost {
                        painter.vline(this_tab.min.x, this_tab.y_range(), side_stroke);
                    }
                    painter.vline(this_tab.max.x, this_tab.y_range(), side_stroke);
                    // Paint over the tab bar bottom border so active tab merges cleanly
                    painter.rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(this_tab.min.x + 1.0, tab_rect.max.y - 1.0),
                            egui::pos2(this_tab.max.x, tab_rect.max.y + 1.0),
                        ),
                        0.0,
                        self.theme.chrome.tab_active_bg,
                    );
                } else {
                    // Inactive tabs: top, right, bottom borders (skip left for leftmost)
                    painter.hline(this_tab.x_range(), this_tab.min.y, side_stroke);
                    painter.hline(this_tab.x_range(), this_tab.max.y, side_stroke);
                    painter.vline(this_tab.max.x, this_tab.y_range(), side_stroke);
                    if !is_leftmost {
                        painter.vline(this_tab.min.x, this_tab.y_range(), side_stroke);
                    }
                }
                let text_color = if is_dead {
                    egui::Color32::from_gray(80)
                } else if is_active {
                    egui::Color32::from_gray(220)
                } else {
                    egui::Color32::from_gray(160)
                };

                // Draw icon + title text
                let mut text_x = x + 6.0;
                let icon_color = if is_dead {
                    egui::Color32::from_gray(80)
                } else if is_active {
                    egui::Color32::from_gray(200)
                } else {
                    egui::Color32::from_gray(140)
                };
                match &tab.icon {
                    TabIcon::Terminal => {
                        let icon_y = tab_rect.min.y + (TAB_BAR_HEIGHT - tab_icons::ICON_SIZE) / 2.0;
                        tab_icons::paint_terminal_icon(
                            painter,
                            egui::pos2(text_x, icon_y),
                            tab_icons::ICON_SIZE,
                            icon_color,
                        );
                        text_x += tab_icons::ICON_SIZE + 4.0;
                    }
                    TabIcon::Favicon(tex_id) => {
                        let icon_y = tab_rect.min.y + (TAB_BAR_HEIGHT - tab_icons::ICON_SIZE) / 2.0;
                        let icon_rect = egui::Rect::from_min_size(
                            egui::pos2(text_x, icon_y),
                            egui::vec2(tab_icons::ICON_SIZE, tab_icons::ICON_SIZE),
                        );
                        painter.image(
                            *tex_id,
                            icon_rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            if is_dead {
                                egui::Color32::from_gray(80)
                            } else {
                                egui::Color32::WHITE
                            },
                        );
                        text_x += tab_icons::ICON_SIZE + 4.0;
                    }
                    TabIcon::None => {}
                }
                painter.text(
                    egui::pos2(text_x, tab_rect.min.y + 6.0),
                    egui::Align2::LEFT_TOP,
                    &title,
                    tab_font.clone(),
                    text_color,
                );

                // Close button — only visible on hover
                let close_center = egui::pos2(x + tab_w - 12.0, tab_rect.center().y);
                let close_rect = egui::Rect::from_center_size(close_center, egui::vec2(16.0, 16.0));
                if tab_hovered {
                    let close_hovered = hover_pos.is_some_and(|p| close_rect.contains(p));
                    let close_color = if close_hovered {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::from_gray(90)
                    };
                    sidebar::paint_close_x(painter, close_center, 6.0, close_color);
                }

                // Right-click context menu. `Response::context_menu`
                // builds its popup `Frame` from `ui.ctx().style()`,
                // so we scope the ctx-style mutation via
                // `with_menu_palette` — the palette is active only
                // during the synchronous `.context_menu(...)` call,
                // and the original ctx style is restored before any
                // other widgets paint.
                let tab_id = ui.id().with("tab_ctx").with(pane_id).with(idx);
                let tab_response = ui.interact(this_tab, tab_id, egui::Sense::click());
                let palette = crate::popup_theme::MenuPalette::from_theme(&self.theme);
                crate::popup_theme::with_menu_palette(ui.ctx(), palette, || {
                    tab_response.context_menu(|ui| {
                        if !tab.is_browser && ui.button("Rename Tab").clicked() {
                            start_rename_tab = Some(idx);
                            ui.close_menu();
                        }
                        if ui.button("Close Tab").clicked() {
                            close_tab = Some(idx);
                            ui.close_menu();
                        }
                    });
                });

                // Hit testing (primary button only)
                if primary_pressed {
                    if let Some(pos) = interact_pos {
                        if tab_hovered && close_rect.contains(pos) {
                            close_tab = Some(idx);
                        } else if this_tab.contains(pos) && !is_active {
                            switch_to = Some(idx);
                        }
                        // Start tab drag
                        if this_tab.contains(pos)
                            && !close_rect.contains(pos)
                            && self.tab_drag.is_none()
                        {
                            drag_start = Some(idx);
                        }
                    }
                }

                x += tab_w + 1.0;
            }

            // Tab drag reorder logic
            if let Some(src) = drag_start {
                self.tab_drag = Some(TabDragState {
                    pane_id,
                    source_idx: src,
                    drop_target_idx: src,
                });
            }

            if let Some(drag) = &mut self.tab_drag {
                if drag.pane_id == pane_id {
                    if any_released || !primary_down {
                        let from = drag.source_idx;
                        let to = drag.drop_target_idx;
                        self.tab_drag = None;
                        if from != to {
                            if let Some(PaneEntry::Terminal(m)) = self.panes.get_mut(&pane_id) {
                                if from >= m.tabs.len() || to >= m.tabs.len() {
                                    return;
                                }
                                let tab = m.tabs.remove(from);
                                let insert_idx = if from < to {
                                    (to - 1).min(m.tabs.len())
                                } else {
                                    to.min(m.tabs.len())
                                };
                                m.tabs.insert(insert_idx, tab);
                                if m.active_tab_idx == from {
                                    m.active_tab_idx = insert_idx;
                                } else if from < m.active_tab_idx && insert_idx >= m.active_tab_idx
                                {
                                    m.active_tab_idx -= 1;
                                } else if from > m.active_tab_idx && insert_idx <= m.active_tab_idx
                                {
                                    m.active_tab_idx += 1;
                                }
                            }
                            return;
                        }
                    } else if let Some(pos) = hover_pos {
                        // Compute drop target from X position vs tab midpoints
                        let tab_midpoints: Vec<f32> =
                            tab_rects.iter().map(|r| r.center().x).collect();
                        let mut target = tab_midpoints.len();
                        for (i, &mid) in tab_midpoints.iter().enumerate() {
                            if pos.x < mid {
                                target = i;
                                break;
                            }
                        }
                        drag.drop_target_idx = target;

                        // Paint drop indicator
                        let drop_x = if target == 0 {
                            tab_rects.first().map(|r| r.min.x).unwrap_or(x)
                        } else if target < tab_rects.len() {
                            let left = tab_rects[target - 1].max.x;
                            let right = tab_rects[target].min.x;
                            (left + right) / 2.0
                        } else {
                            tab_rects.last().map(|r| r.max.x).unwrap_or(x)
                        };
                        let indicator_rect = egui::Rect::from_min_size(
                            egui::pos2(drop_x - 1.0, tab_rect.min.y + 2.0),
                            egui::vec2(2.0, TAB_BAR_HEIGHT - 4.0),
                        );
                        painter.rect_filled(indicator_rect, 1.0, self.theme.chrome.accent);
                    }
                }
            }

            // Pane toolbar: new terminal, new browser, split vertical, split horizontal
            let icon_size = tab_icons::ICON_SIZE;
            let icon_pad = 6.0;
            let button_count = 4.0;
            let toolbar_width = button_count * icon_size + (button_count - 1.0) * icon_pad;
            let toolbar_x = tab_rect.max.x - toolbar_width - 6.0;
            let icon_y = tab_rect.min.y + (TAB_BAR_HEIGHT - icon_size) / 2.0;

            struct ToolbarButton {
                rect: egui::Rect,
                action: ToolbarAction,
            }
            #[derive(Clone, Copy)]
            enum ToolbarAction {
                NewTerminal,
                NewBrowser,
                SplitVertical,
                SplitHorizontal,
            }

            let buttons = [
                ToolbarAction::NewTerminal,
                ToolbarAction::NewBrowser,
                ToolbarAction::SplitVertical,
                ToolbarAction::SplitHorizontal,
            ];
            let toolbar_buttons: Vec<ToolbarButton> = buttons
                .iter()
                .enumerate()
                .map(|(i, &action)| {
                    let bx = toolbar_x + i as f32 * (icon_size + icon_pad);
                    ToolbarButton {
                        rect: egui::Rect::from_min_size(
                            egui::pos2(bx, icon_y),
                            egui::vec2(icon_size, icon_size),
                        ),
                        action,
                    }
                })
                .collect();

            let mut toolbar_action: Option<ToolbarAction> = None;

            for btn in &toolbar_buttons {
                let hovered = hover_pos.is_some_and(|p| btn.rect.contains(p));
                let color = if hovered {
                    egui::Color32::from_gray(240)
                } else {
                    egui::Color32::from_gray(180)
                };
                match btn.action {
                    ToolbarAction::NewTerminal => {
                        tab_icons::paint_terminal_icon(painter, btn.rect.min, icon_size, color);
                    }
                    ToolbarAction::NewBrowser => {
                        tab_icons::paint_globe_icon(painter, btn.rect.min, icon_size, color);
                    }
                    ToolbarAction::SplitVertical => {
                        tab_icons::paint_split_vertical_icon(
                            painter,
                            btn.rect.min,
                            icon_size,
                            color,
                        );
                    }
                    ToolbarAction::SplitHorizontal => {
                        tab_icons::paint_split_horizontal_icon(
                            painter,
                            btn.rect.min,
                            icon_size,
                            color,
                        );
                    }
                }
                if primary_pressed {
                    if let Some(pos) = interact_pos {
                        if btn.rect.contains(pos) {
                            toolbar_action = Some(btn.action);
                        }
                    }
                }
            }

            // Handle toolbar actions
            if let Some(action) = toolbar_action {
                match action {
                    ToolbarAction::NewTerminal => {
                        let ws_id = self.active_workspace().id;
                        let sf_id = self.next_surface_id;
                        self.next_surface_id += 1;
                        let cwd = self
                            .panes
                            .get(&pane_id)
                            .and_then(|e| e.as_terminal())
                            .and_then(|m| m.active_surface())
                            .and_then(|sf| sf.metadata.cwd.clone());
                        if let Ok(surface) = startup::spawn_surface(
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
                            if let Some(PaneEntry::Terminal(m)) = self.panes.get_mut(&pane_id) {
                                let insert_at = (m.active_tab_idx + 1).min(m.tabs.len());
                                m.tabs.insert(
                                    insert_at,
                                    managed_pane::TabEntry::Terminal(Box::new(surface)),
                                );
                                m.active_tab_idx = insert_at;
                            }
                        }
                        return;
                    }
                    ToolbarAction::NewBrowser => {
                        self.queue_browser_pane(pane_id, DEFAULT_BROWSER_URL.to_string());
                        return;
                    }
                    ToolbarAction::SplitVertical => {
                        self.do_split(SplitDirection::Horizontal);
                        return;
                    }
                    ToolbarAction::SplitHorizontal => {
                        self.do_split(SplitDirection::Vertical);
                        return;
                    }
                }
            }

            // Apply tab switch/close (need to re-borrow managed)
            if let Some(idx) = close_tab {
                let managed = self
                    .panes
                    .get(&pane_id)
                    .and_then(|e| e.as_terminal())
                    .unwrap();
                if managed.tabs.len() <= 1 {
                    self.close_pane(pane_id);
                    return;
                }
                // Don't remove the last terminal tab — close the whole pane instead
                // to preserve the "at least one terminal surface" invariant.
                let is_browser = managed.tabs[idx].is_browser();
                if !is_browser && managed.tabs.iter().filter(|t| !t.is_browser()).count() <= 1 {
                    self.close_pane(pane_id);
                    return;
                }
                // Get the browser pane ID before removing (if it's a browser tab)
                let browser_pane_id = managed.tabs[idx].browser_pane_id();
                let managed = self
                    .panes
                    .get_mut(&pane_id)
                    .unwrap()
                    .as_terminal_mut()
                    .unwrap();
                managed.tabs.remove(idx);
                if let Some(bid) = browser_pane_id {
                    self.panes.remove(&bid);
                    self.omnibar_state.remove(&bid);
                }
                let managed = self
                    .panes
                    .get_mut(&pane_id)
                    .unwrap()
                    .as_terminal_mut()
                    .unwrap();
                if idx < managed.active_tab_idx {
                    managed.active_tab_idx -= 1;
                } else if managed.active_tab_idx >= managed.tabs.len() {
                    managed.active_tab_idx = managed.tabs.len() - 1;
                }
            } else if let Some(idx) = switch_to {
                // Determine old and new browser tab IDs for visibility management
                let old_browser = self
                    .panes
                    .get(&pane_id)
                    .and_then(|e| e.as_terminal())
                    .and_then(|m| match m.active_tab() {
                        managed_pane::ActiveTab::Browser(bid) => Some(bid),
                        _ => None,
                    });
                let managed = self
                    .panes
                    .get_mut(&pane_id)
                    .unwrap()
                    .as_terminal_mut()
                    .unwrap();
                managed.active_tab_idx = idx;
                let new_browser = match managed.active_tab() {
                    managed_pane::ActiveTab::Browser(bid) => Some(bid),
                    _ => None,
                };
                // Hide old browser webview, show new one; manage keyboard focus
                if old_browser != new_browser {
                    if let Some(bid) = old_browser {
                        if let Some(PaneEntry::Browser(b)) = self.panes.get(&bid) {
                            b.set_visible(false);
                            b.focus_parent();
                        }
                    }
                    if let Some(bid) = new_browser {
                        if let Some(PaneEntry::Browser(b)) = self.panes.get(&bid) {
                            b.set_visible(true);
                            b.focus();
                        }
                    }
                }
            }

            // Open rename modal for tab (terminal tabs only).
            // Sanitize the pane title here too so the rename modal
            // pre-fills with `pwsh` instead of the ugly exe path.
            if let Some(idx) = start_rename_tab {
                if let Some(PaneEntry::Terminal(managed)) = self.panes.get(&pane_id) {
                    if let Some(surface) = managed.tabs[idx].as_surface() {
                        let current_title = surface.user_title.clone().unwrap_or_else(|| {
                            crate::title_sanitize::sanitize_pane_title(surface.pane.title())
                                .into_owned()
                        });
                        self.rename_modal = Some(RenameModal {
                            target: RenameTarget::Tab {
                                pane_id,
                                surface_id: surface.id,
                            },
                            buf: current_title,
                            just_opened: true,
                        });
                    }
                }
            }
        }

        // Recompute after tab mutation (switch_to or close_tab may have changed the active tab)
        let active_is_browser = self
            .panes
            .get(&pane_id)
            .and_then(|e| e.as_terminal())
            .is_some_and(|m| m.active_is_browser());

        // If active tab is a browser, render browser content and return
        if active_is_browser {
            if let Some(PaneEntry::Terminal(managed)) = self.panes.get(&pane_id) {
                if let managed_pane::ActiveTab::Browser(browser_pane_id) = managed.active_tab() {
                    self.render_browser_pane(ui, browser_pane_id, content_rect, is_focused);
                }
            }
            return;
        }

        // Collect find highlights for this pane
        let (find_highlights, current_highlight) = self
            .find_state
            .as_ref()
            .filter(|f| f.pane_id == pane_id)
            .map(|f| (f.matches.clone(), Some(f.current_match)))
            .unwrap_or_default();

        // Build selection: use copy mode visual selection if active, else normal selection
        let copy_mode_sel = self
            .copy_mode
            .as_ref()
            .filter(|cm| cm.pane_id == pane_id && cm.visual_anchor.is_some())
            .map(|cm| {
                let anchor = cm.visual_anchor.unwrap();
                let cursor = cm.cursor;
                let (start, end) =
                    if anchor.1 < cursor.1 || (anchor.1 == cursor.1 && anchor.0 <= cursor.0) {
                        (anchor, cursor)
                    } else {
                        (cursor, anchor)
                    };
                SelectionState {
                    anchor: start,
                    end,
                    mode: if cm.line_visual {
                        SelectionMode::Line
                    } else {
                        SelectionMode::Cell
                    },
                    active: false,
                }
            });

        // Render terminal content for the active surface
        let managed = match self.panes.get_mut(&pane_id) {
            Some(PaneEntry::Terminal(m)) => m,
            _ => return,
        };
        let selection = copy_mode_sel
            .as_ref()
            .or(managed.selection.as_ref())
            .cloned();
        let surface = match managed.active_surface_mut() {
            Some(s) => s,
            None => return,
        };
        // Cursor blink: 500ms on, 500ms off cycle, reset on input.
        let blink_elapsed_ms = self.cursor_blink_since.elapsed().as_millis();
        let cursor_blink_on = (blink_elapsed_ms % 1000) < 500;
        render::render_pane(
            ui,
            &mut surface.pane,
            content_rect,
            is_focused,
            surface.scroll_offset,
            selection.as_ref(),
            self.font_size,
            &find_highlights,
            current_highlight,
            cursor_blink_on,
            self.theme.chrome.pane_dim_alpha,
            #[cfg(feature = "gpu-renderer")]
            self.gpu_renderer.as_ref(),
            #[cfg(feature = "gpu-renderer")]
            pane_id,
        );

        // Scrollbar overlay — thin, auto-hiding, anchor-based dragging.
        // Interaction model follows wezterm / OS scrollbar conventions:
        //   mousedown on thumb → start drag with anchor offset
        //   drag → thumb follows mouse exactly (no jump)
        //   click above thumb → page up
        //   click below thumb → page down
        let mut scrollbar_jump: Option<usize> = None;
        {
            let total_rows = surface.pane.scrollback_rows();
            let (_, viewport_rows) = surface.pane.dimensions();
            if total_rows > viewport_rows {
                let max_offset = total_rows - viewport_rows;
                let bar_width_default = 4.0_f32;
                let bar_width_hover = 8.0_f32;
                let hit_width = 16.0_f32;
                let bar_margin = 4.0_f32;
                let track_top = content_rect.min.y + 2.0;
                let track_bottom = content_rect.max.y - 2.0;
                let track_height = (track_bottom - track_top).max(0.0);
                if track_height < 20.0 {
                    // Pane too small for a meaningful scrollbar
                } else {
                    let viewport_frac = viewport_rows as f32 / total_rows as f32;
                    let thumb_height = (track_height * viewport_frac).max(12.0);
                    let available = track_height - thumb_height;

                    let scroll_frac = surface.scroll_offset as f32 / max_offset as f32;
                    let thumb_top = track_top + available * (1.0 - scroll_frac);

                    // Check hover/drag state first to pick the thumb width
                    let hit_left = content_rect.max.x - hit_width;
                    let pointer_pos = ui.input(|i| i.pointer.hover_pos());
                    let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
                    let primary_down = ui.input(|i| i.pointer.primary_down());
                    let primary_released = ui.input(|i| i.pointer.primary_released());

                    let in_hit_zone = pointer_pos.is_some_and(|p| {
                        p.x >= hit_left && p.y >= track_top && p.y <= track_bottom
                    });

                    // Expand thumb width on hover for easier grabbing
                    let is_dragging_this = self
                        .scrollbar_drag
                        .as_ref()
                        .is_some_and(|d| d.pane_id == pane_id);
                    let bar_width = if in_hit_zone || is_dragging_this {
                        bar_width_hover
                    } else {
                        bar_width_default
                    };

                    let thumb_rect = egui::Rect::from_min_size(
                        egui::pos2(content_rect.max.x - bar_width - bar_margin, thumb_top),
                        egui::vec2(bar_width, thumb_height),
                    );
                    let thumb_hit = egui::Rect::from_min_max(
                        egui::pos2(content_rect.max.x - hit_width, thumb_top),
                        egui::pos2(content_rect.max.x, thumb_top + thumb_height),
                    );
                    let on_thumb = pointer_pos.is_some_and(|p| thumb_hit.contains(p));

                    // Drag state machine
                    let is_dragging = self
                        .scrollbar_drag
                        .as_ref()
                        .is_some_and(|d| d.pane_id == pane_id);

                    if primary_released {
                        if is_dragging {
                            self.scrollbar_drag = None;
                        }
                    } else if primary_pressed && on_thumb {
                        // Start anchor-based drag
                        if let Some(pos) = pointer_pos {
                            self.scrollbar_drag = Some(ScrollbarDrag {
                                pane_id,
                                anchor_offset: pos.y - thumb_top,
                            });
                        }
                    } else if primary_pressed && in_hit_zone && !on_thumb {
                        // Click above/below thumb → page up/down
                        if let Some(pos) = pointer_pos {
                            if pos.y < thumb_top {
                                // Page up (increase scroll_offset)
                                let new_off =
                                    (surface.scroll_offset + viewport_rows).min(max_offset);
                                scrollbar_jump = Some(new_off);
                            } else {
                                // Page down (decrease scroll_offset)
                                let new_off = surface.scroll_offset.saturating_sub(viewport_rows);
                                scrollbar_jump = Some(new_off);
                            }
                        }
                    }

                    // During drag: compute scroll from anchor
                    if is_dragging && primary_down {
                        if let Some(pos) = pointer_pos {
                            let anchor = self.scrollbar_drag.as_ref().unwrap().anchor_offset;
                            let effective_top = (pos.y - anchor - track_top).clamp(0.0, available);
                            let frac = effective_top / available;
                            let new_offset = ((1.0 - frac) * max_offset as f32).round() as usize;
                            scrollbar_jump = Some(new_offset);
                        }
                    }

                    let hovering = in_hit_zone || is_dragging;

                    // Fade: visible while hovering/dragging, otherwise hold
                    // for 1.5s after last scroll then fade over 0.5s.
                    let since_scroll = surface.last_scroll_at.elapsed();
                    let alpha = if hovering || is_dragging {
                        0.6_f32
                    } else {
                        let hold_ms: u32 = 1500;
                        let fade_ms: u32 = 500;
                        let ms = since_scroll.as_millis() as u32;
                        if ms < hold_ms {
                            0.5
                        } else if ms < hold_ms + fade_ms {
                            0.5 * (1.0 - (ms - hold_ms) as f32 / fade_ms as f32)
                        } else {
                            0.0
                        }
                    };

                    if alpha > 0.01 {
                        let a = (alpha * 255.0) as u8;
                        let color = egui::Color32::from_rgba_unmultiplied(200, 200, 200, a);
                        ui.painter().rect_filled(thumb_rect, bar_width / 2.0, color);
                        if alpha < 0.5 {
                            ui.ctx().request_repaint();
                        }
                    }
                } // track_height >= 20
            }
        }

        // Capture values from the immutable surface borrow before we
        // need mutable access for the scrollbar drag.
        let exit_message = surface.exited.as_ref().map(|e| e.message.clone());

        // Apply scrollbar drag (needs mutable access).
        if let Some(new_offset) = scrollbar_jump {
            if let Some(PaneEntry::Terminal(managed)) = self.panes.get_mut(&pane_id) {
                if let Some(surface) = managed.active_surface_mut() {
                    let (_, rows) = surface.pane.dimensions();
                    let total = surface.pane.scrollback_rows();
                    let max_offset = total.saturating_sub(rows);
                    let clamped = new_offset.min(max_offset);
                    if surface.pane.manages_own_scroll() {
                        // Absolute positioning: reset to bottom then scroll
                        // up by the exact desired offset. Incremental deltas
                        // drift because we can't query ghostty's actual
                        // viewport position.
                        surface.pane.scroll_to_bottom();
                        if clamped > 0 {
                            surface.pane.scroll_viewport(-(clamped as isize));
                        }
                    }
                    surface.scroll_offset = clamped;
                    surface.last_scroll_at = Instant::now();
                }
            }
        }

        // Render exit overlay when process has exited
        if let Some(msg) = &exit_message {
            render::render_exit_overlay(ui, content_rect, msg, self.font_size);
        }

        // Render copy mode cursor overlay
        if let Some(cm) = self.copy_mode.as_ref().filter(|cm| cm.pane_id == pane_id) {
            let (cell_w, cell_h) = self.cell_dimensions(ui);
            if let Some(PaneEntry::Terminal(managed)) = self.panes.get(&pane_id) {
                let surface = match managed.active_surface() {
                    Some(s) => s,
                    None => return,
                };
                let (_, rows) = surface.pane.dimensions();
                let total = surface.pane.scrollback_rows();
                let end = total.saturating_sub(surface.scroll_offset);
                let start = end.saturating_sub(rows);
                if cm.cursor.1 >= start && cm.cursor.1 < end {
                    let row_in_view = cm.cursor.1 - start;
                    let x = content_rect.min.x + cm.cursor.0 as f32 * cell_w;
                    let y = content_rect.min.y + row_in_view as f32 * cell_h;
                    let cursor_rect =
                        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(cell_w, cell_h));
                    ui.painter().rect_stroke(
                        cursor_rect,
                        0.0,
                        egui::Stroke::new(2.0, egui::Color32::from_rgb(0, 255, 0)),
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }

        // Notification ring + flash animation (matching cmux)
        let pane_u64 = pane_id;
        let ring_rect = rect.shrink(2.0);

        // Fetch pane state once; used for both flash detection and animation.
        let pane_state = self.notifications.pane_state(pane_u64);
        // Is a flash animation currently running? (used to suppress the
        // persistent ring so the two don't double-stroke the same path)
        let flash_active = pane_state
            .and_then(|s| s.flash_started_at)
            .is_some_and(|started| started.elapsed().as_secs_f32() < FLASH_DURATION);

        // 1. Persistent unread ring (NOT on focused pane, NOT during flash)
        if !is_focused && !flash_active && self.notifications.pane_unread(pane_u64) > 0 {
            // Steady notification ring using the configured theme color
            let rc = self.theme.chrome.notification_ring;
            let ring_color = egui::Color32::from_rgba_unmultiplied(rc.r(), rc.g(), rc.b(), 89);
            ui.painter().rect_stroke(
                ring_rect,
                6.0,
                egui::Stroke::new(2.5, ring_color),
                egui::StrokeKind::Inside,
            );
            ui.ctx().request_repaint();
        }

        // 2. Flash animation (on ANY pane including focused)
        if let Some(state) = pane_state {
            if let Some(started) = state.flash_started_at {
                let elapsed = started.elapsed().as_secs_f32();
                if elapsed < FLASH_DURATION {
                    let alpha = flash_alpha(elapsed);
                    let rc = self.theme.chrome.notification_ring;
                    let base_color = [rc.r(), rc.g(), rc.b()];
                    let glow_alpha = (alpha * 0.6 * 255.0) as u8;
                    let ring_alpha = (alpha * 255.0) as u8;
                    // Glow (wider, more transparent). Anchor on ring_rect with
                    // Outside kind so it butts up directly against the inner ring
                    // at the ring_rect boundary — any gap or expansion leaves a
                    // visible 1px dark line where the terminal bg shows through.
                    ui.painter().rect_stroke(
                        ring_rect,
                        6.0,
                        egui::Stroke::new(
                            4.0,
                            egui::Color32::from_rgba_unmultiplied(
                                base_color[0],
                                base_color[1],
                                base_color[2],
                                glow_alpha,
                            ),
                        ),
                        egui::StrokeKind::Outside,
                    );
                    // Ring
                    ui.painter().rect_stroke(
                        ring_rect,
                        6.0,
                        egui::Stroke::new(
                            2.5,
                            egui::Color32::from_rgba_unmultiplied(
                                base_color[0],
                                base_color[1],
                                base_color[2],
                                ring_alpha,
                            ),
                        ),
                        egui::StrokeKind::Inside,
                    );
                    ui.ctx().request_repaint();
                }
            }
        }
    }

    /// Render browser content: omnibar (back/forward/reload + URL input) + webview bounds update.
    /// `pane_id` is the browser pane ID (PaneEntry::Browser in the panes map).
    /// `rect` is the content area below the tab bar.
    fn render_browser_pane(
        &mut self,
        ui: &mut egui::Ui,
        pane_id: PaneId,
        rect: egui::Rect,
        _is_focused: bool,
    ) {
        const OMNIBAR_HEIGHT: f32 = TAB_BAR_HEIGHT;
        const BUTTON_SIZE: f32 = 20.0;
        const BUTTON_PAD: f32 = 4.0;

        let omnibar_rect =
            egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), OMNIBAR_HEIGHT));
        let content_rect = egui::Rect::from_min_max(
            egui::pos2(rect.min.x, rect.min.y + OMNIBAR_HEIGHT + 1.0),
            rect.max,
        );

        // Update webview bounds
        if let Some(PaneEntry::Browser(browser)) = self.panes.get(&pane_id) {
            browser.set_bounds(amux_browser::BrowserRect {
                x: content_rect.min.x as f64,
                y: content_rect.min.y as f64,
                width: content_rect.width() as f64,
                height: content_rect.height() as f64,
            });
        }

        // Draw omnibar background
        let painter = ui.painter();
        painter.rect_filled(omnibar_rect, 0.0, self.theme.tab_bar_bg());
        painter.hline(
            omnibar_rect.x_range(),
            omnibar_rect.max.y,
            egui::Stroke::new(1.0, self.theme.chrome.tab_bar_border),
        );

        let hover_pos = ui.input(|i| i.pointer.hover_pos());
        let primary_clicked = ui.input(|i| i.pointer.primary_clicked());

        // Navigation buttons: back, forward, reload
        let button_y = omnibar_rect.min.y + (OMNIBAR_HEIGHT - BUTTON_SIZE) / 2.0;
        let mut x = omnibar_rect.min.x + BUTTON_PAD;

        // Back button
        let back_rect = egui::Rect::from_min_size(
            egui::pos2(x, button_y),
            egui::vec2(BUTTON_SIZE, BUTTON_SIZE),
        );
        let back_hovered = hover_pos.is_some_and(|p| back_rect.contains(p));
        let back_color = if back_hovered {
            egui::Color32::WHITE
        } else {
            egui::Color32::from_gray(120)
        };
        painter.text(
            back_rect.center(),
            egui::Align2::CENTER_CENTER,
            "\u{25C0}",
            egui::FontId::proportional(11.0),
            back_color,
        );
        if back_hovered && primary_clicked {
            if let Some(PaneEntry::Browser(browser)) = self.panes.get(&pane_id) {
                browser.go_back();
            }
        }
        x += BUTTON_SIZE + 2.0;

        // Forward button
        let fwd_rect = egui::Rect::from_min_size(
            egui::pos2(x, button_y),
            egui::vec2(BUTTON_SIZE, BUTTON_SIZE),
        );
        let fwd_hovered = hover_pos.is_some_and(|p| fwd_rect.contains(p));
        let fwd_color = if fwd_hovered {
            egui::Color32::WHITE
        } else {
            egui::Color32::from_gray(120)
        };
        painter.text(
            fwd_rect.center(),
            egui::Align2::CENTER_CENTER,
            "\u{25B6}",
            egui::FontId::proportional(11.0),
            fwd_color,
        );
        if fwd_hovered && primary_clicked {
            if let Some(PaneEntry::Browser(browser)) = self.panes.get(&pane_id) {
                browser.go_forward();
            }
        }
        x += BUTTON_SIZE + 2.0;

        // Reload/Stop button (shows stop ✕ when loading, reload ↻ when idle)
        let is_loading = self
            .panes
            .get(&pane_id)
            .and_then(|e| e.as_browser())
            .is_some_and(|b| b.is_loading());
        let reload_rect = egui::Rect::from_min_size(
            egui::pos2(x, button_y),
            egui::vec2(BUTTON_SIZE, BUTTON_SIZE),
        );
        let reload_hovered = hover_pos.is_some_and(|p| reload_rect.contains(p));
        let reload_color = if reload_hovered {
            egui::Color32::WHITE
        } else {
            egui::Color32::from_gray(120)
        };
        let (reload_icon, reload_font_size) = if is_loading {
            ("\u{2715}", 11.0) // ✕ stop
        } else {
            ("\u{21BB}", 13.0) // ↻ reload
        };
        painter.text(
            reload_rect.center(),
            egui::Align2::CENTER_CENTER,
            reload_icon,
            egui::FontId::proportional(reload_font_size),
            reload_color,
        );
        if reload_hovered && primary_clicked {
            if let Some(PaneEntry::Browser(browser)) = self.panes.get(&pane_id) {
                if is_loading {
                    browser.stop();
                } else {
                    browser.reload();
                }
            }
        }
        x += BUTTON_SIZE + BUTTON_PAD;

        // URL input area
        let url_rect = egui::Rect::from_min_max(
            egui::pos2(x, omnibar_rect.min.y + 3.0),
            egui::pos2(omnibar_rect.max.x - BUTTON_PAD, omnibar_rect.max.y - 3.0),
        );

        // Ensure omnibar state exists and sync URL when not editing
        let current_url = self
            .panes
            .get(&pane_id)
            .and_then(|e| e.as_browser())
            .and_then(|b| b.url())
            .unwrap_or_default();

        let state = self
            .omnibar_state
            .entry(pane_id)
            .or_insert_with(|| crate::OmnibarState {
                text: current_url.clone(),
                focused: false,
                last_recorded_url: String::new(),
            });

        // When not focused, keep text synced with actual URL
        if !state.focused {
            state.text = current_url;
        }

        // Draw URL input background
        let url_bg = if state.focused {
            egui::Color32::from_gray(50)
        } else {
            egui::Color32::from_gray(35)
        };
        ui.painter().rect_filled(url_rect, 3.0, url_bg);
        ui.painter().rect_stroke(
            url_rect,
            3.0,
            egui::Stroke::new(
                1.0,
                if state.focused {
                    self.theme.chrome.accent
                } else {
                    egui::Color32::from_gray(60)
                },
            ),
            egui::StrokeKind::Inside,
        );

        // Render the text input using egui's TextEdit
        let text_id = ui.id().with("omnibar").with(pane_id);
        let mut text = state.text.clone();
        let response = ui.put(
            url_rect.shrink2(egui::vec2(6.0, 0.0)),
            egui::TextEdit::singleline(&mut text)
                .id(text_id)
                .font(egui::FontId::proportional(12.0))
                .text_color(egui::Color32::from_gray(220))
                .frame(false)
                .desired_width(url_rect.width() - 12.0),
        );

        // Track focus state — detect clicks on the webview (native subview)
        // via IPC and surrender omnibar focus so paste goes to the web page.
        let was_focused = state.focused;
        let webview_got_focus = self
            .panes
            .get(&pane_id)
            .and_then(|e| e.as_browser())
            .is_some_and(|b| b.take_got_focus());
        let clicked = response.clicked();
        if webview_got_focus && response.has_focus() {
            response.surrender_focus();
        }
        // On Windows, clicking the omnibar TextEdit fires `clicked()` but
        // does not grant egui focus — WebView2's child HWND keeps Win32
        // keyboard focus after the click so egui skips its auto-focus step.
        // Force egui focus and then SetFocus() the parent HWND so the
        // keyboard events reach egui's TextEdit.
        if clicked && !response.has_focus() {
            response.request_focus();
            #[cfg(target_os = "windows")]
            if let Some(hwnd) = self.cached_hwnd {
                crate::windows_chrome::set_focus(hwnd);
            }
        }
        state.focused = response.has_focus();
        state.text = text;

        // Select all on first focus
        if state.focused && !was_focused {
            response.request_focus();
            if let Some(mut text_state) = egui::TextEdit::load_state(ui.ctx(), text_id) {
                text_state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(state.text.chars().count()),
                    )));
                text_state.store(ui.ctx(), text_id);
            }
        }

        // Apply pending select-all from menu bar (Cmd+A consumed by muda before egui).
        if state.focused && self.pending_text_field_select_all {
            self.pending_text_field_select_all = false;
            if let Some(mut text_state) = egui::TextEdit::load_state(ui.ctx(), text_id) {
                let char_count = state.text.chars().count();
                text_state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(char_count),
                    )));
                text_state.store(ui.ctx(), text_id);
            }
        }

        // Apply pending paste from menu bar (Cmd+V consumed by muda before egui).
        if state.focused {
            if let Some(paste_text) = self.pending_text_field_paste.take() {
                if let Some(mut text_state) = egui::TextEdit::load_state(ui.ctx(), text_id) {
                    if let Some(range) = text_state.cursor.char_range() {
                        let start = range.primary.index.min(range.secondary.index);
                        let end = range.primary.index.max(range.secondary.index);
                        let byte_start = state
                            .text
                            .char_indices()
                            .nth(start)
                            .map(|(i, _)| i)
                            .unwrap_or(state.text.len());
                        let byte_end = state
                            .text
                            .char_indices()
                            .nth(end)
                            .map(|(i, _)| i)
                            .unwrap_or(state.text.len());
                        state.text.replace_range(byte_start..byte_end, &paste_text);
                        let new_cursor = start + paste_text.chars().count();
                        text_state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::one(
                                egui::text::CCursor::new(new_cursor),
                            )));
                    } else {
                        state.text.push_str(&paste_text);
                    }
                    text_state.store(ui.ctx(), text_id);
                } else {
                    state.text.push_str(&paste_text);
                }
            }
        }

        // Handle Enter: navigate or search
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let input = state.text.trim().to_string();
            if !input.is_empty() {
                let url = if config::is_url_like(&input) {
                    if input.starts_with("http://")
                        || input.starts_with("https://")
                        || input.starts_with("file://")
                    {
                        input
                    } else {
                        format!("https://{input}")
                    }
                } else {
                    config::search_url(&input, &self.app_config.browser.search_engine)
                };
                if let Some(PaneEntry::Browser(browser)) = self.panes.get(&pane_id) {
                    browser.navigate(&url);
                    let title = browser.title();
                    self.browser_history.record_visit(&url, &title);
                }
            }
        }

        // Handle Escape: unfocus, revert to current URL
        if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            response.surrender_focus();
            let state = self.omnibar_state.get_mut(&pane_id).unwrap();
            state.focused = false;
        }

        // Autocomplete suggestions when omnibar is focused and has text
        let state = self.omnibar_state.get(&pane_id).unwrap();
        if state.focused && !state.text.is_empty() && state.text.len() >= 2 {
            let suggestions = self.browser_history.search(&state.text, 6);
            if !suggestions.is_empty() {
                let popup_id = ui.id().with("omnibar_popup").with(pane_id);
                let popup_rect = egui::Rect::from_min_size(
                    egui::pos2(url_rect.min.x, url_rect.max.y + 2.0),
                    egui::vec2(url_rect.width(), suggestions.len() as f32 * 22.0 + 4.0),
                );

                // Draw popup background
                ui.painter()
                    .rect_filled(popup_rect, 4.0, egui::Color32::from_gray(40));
                ui.painter().rect_stroke(
                    popup_rect,
                    4.0,
                    egui::Stroke::new(1.0, egui::Color32::from_gray(60)),
                    egui::StrokeKind::Inside,
                );

                let mut selected_url: Option<String> = None;
                let mut y = popup_rect.min.y + 2.0;
                for (i, entry) in suggestions.iter().enumerate() {
                    let item_rect = egui::Rect::from_min_size(
                        egui::pos2(popup_rect.min.x, y),
                        egui::vec2(popup_rect.width(), 22.0),
                    );
                    let item_hovered = hover_pos.is_some_and(|p| item_rect.contains(p));
                    if item_hovered {
                        ui.painter()
                            .rect_filled(item_rect, 0.0, egui::Color32::from_gray(55));
                    }

                    // Show title (truncated) + URL domain
                    let display = if entry.title.is_empty() {
                        entry.url.clone()
                    } else {
                        let title: String = entry.title.chars().take(30).collect();
                        let domain = entry
                            .url
                            .split("//")
                            .nth(1)
                            .and_then(|s| s.split('/').next())
                            .unwrap_or("");
                        format!("{title} — {domain}")
                    };

                    ui.painter().text(
                        egui::pos2(item_rect.min.x + 8.0, item_rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        &display,
                        egui::FontId::proportional(11.0),
                        egui::Color32::from_gray(200),
                    );

                    let item_id = popup_id.with(i);
                    let item_response = ui.interact(item_rect, item_id, egui::Sense::click());
                    if item_response.clicked() {
                        selected_url = Some(entry.url.clone());
                    }
                    y += 22.0;
                }

                if let Some(url) = selected_url {
                    if let Some(PaneEntry::Browser(browser)) = self.panes.get(&pane_id) {
                        browser.navigate(&url);
                    }
                    if let Some(state) = self.omnibar_state.get_mut(&pane_id) {
                        state.text = url;
                        state.focused = false;
                    }
                    response.surrender_focus();
                }
            }
        }

        // Cmd+L / Ctrl+L to focus omnibar
        let cmd_l = ui.input(|i| {
            i.events.iter().any(|e| {
                matches!(e, egui::Event::Key {
                    key: egui::Key::L,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.command)
            })
        });
        if cmd_l {
            ui.memory_mut(|m| m.request_focus(text_id));
        }
    }
}
