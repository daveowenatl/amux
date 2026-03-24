use std::io::Read;
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use amux_ipc::IpcCommand;
use amux_term::color::resolve_color;
use amux_term::config::AmuxTermConfig;
use amux_term::pane::TerminalPane;
use portable_pty::CommandBuilder;
use wezterm_term::color::SrgbaTuple;

const FONT_SIZE: f32 = 14.0;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    // Start IPC server first to get the socket path for env injection
    let (ipc_rx, ipc_addr) = amux_ipc::start_server()?;
    tracing::info!("IPC server: {}", ipc_addr);

    // Spawn terminal with user's shell + injected env vars
    let shell = default_shell();
    let mut cmd = CommandBuilder::new(&shell);
    cmd.env("AMUX_SOCKET_PATH", ipc_addr.to_string());
    cmd.env("AMUX_WORKSPACE_ID", "default");
    cmd.env("AMUX_SURFACE_ID", "default");
    cmd.env("TERM", "xterm-256color");

    let config = Arc::new(AmuxTermConfig::default());
    let mut pane = TerminalPane::spawn(80, 24, cmd, config)?;

    // Take reader for background thread
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
                pane,
                byte_rx,
                ipc_rx,
                last_size: (0, 0),
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{}", e));

    // Clean up socket file
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

struct AmuxApp {
    pane: TerminalPane,
    byte_rx: mpsc::Receiver<Vec<u8>>,
    ipc_rx: std::sync::mpsc::Receiver<IpcCommand>,
    last_size: (usize, usize),
}

impl eframe::App for AmuxApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Drain PTY output from background reader
        let mut got_data = false;
        while let Ok(bytes) = self.byte_rx.try_recv() {
            got_data = true;
            self.pane.feed_bytes(&bytes);
        }

        // Process IPC commands
        self.process_ipc_commands();

        // Handle keyboard/paste input
        self.handle_input(ctx);

        // Render terminal
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                self.render_terminal(ui);
            });

        // Update window title from terminal
        let title = self.pane.title();
        if !title.is_empty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("amux — {}", title)));
        }

        // Smart repaint: immediate when data flowing, slow poll when idle
        if got_data {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
}

impl AmuxApp {
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
                    "surface_id": "default",
                }),
            ),
            "surface.list" => {
                let (cols, rows) = self.pane.dimensions();
                Response::ok(
                    req.id.clone(),
                    serde_json::json!({
                        "surfaces": [{
                            "id": "default",
                            "title": self.pane.title(),
                            "cols": cols,
                            "rows": rows,
                            "alive": self.pane.is_alive(),
                        }],
                    }),
                )
            }
            "surface.send_text" => {
                match serde_json::from_value::<amux_ipc::methods::SendTextParams>(
                    req.params.clone(),
                ) {
                    Ok(params) => match self.pane.write_bytes(params.text.as_bytes()) {
                        Ok(_) => Response::ok(req.id.clone(), serde_json::json!({})),
                        Err(e) => Response::err(req.id.clone(), "write_error", &e.to_string()),
                    },
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "surface.read_text" => {
                let text = self.pane.read_screen_text();
                Response::ok(req.id.clone(), serde_json::json!({"text": text}))
            }
            _ => Response::err(
                req.id.clone(),
                "method_not_found",
                &format!("unknown method: {}", req.method),
            ),
        }
    }

    fn render_terminal(&mut self, ui: &mut egui::Ui) {
        let font_id = egui::FontId::monospace(FONT_SIZE);
        let cell_width = ui.fonts(|f| f.glyph_width(&font_id, 'M'));
        let cell_height = ui.fonts(|f| f.row_height(&font_id));

        let available = ui.available_size();
        let cols = (available.x / cell_width).floor() as usize;
        let rows = (available.y / cell_height).floor() as usize;

        if cols == 0 || rows == 0 {
            return;
        }

        // Resize terminal if dimensions changed
        if (cols, rows) != self.last_size {
            self.last_size = (cols, rows);
            let _ = self.pane.resize(cols as u16, rows as u16);
        }

        let (actual_cols, actual_rows) = self.pane.dimensions();
        let palette = self.pane.palette();
        let cursor = self.pane.cursor();
        let screen = self.pane.screen();

        let painter = ui.painter();
        let origin = ui.min_rect().min;
        let bg_default = srgba_to_egui(palette.background);

        // Fill terminal background
        let terminal_rect = egui::Rect::from_min_size(
            origin,
            egui::vec2(
                actual_cols as f32 * cell_width,
                actual_rows as f32 * cell_height,
            ),
        );
        painter.rect_filled(terminal_rect, 0.0, bg_default);

        // Draw cells — scrollback_rows() returns total line count (lines.len())
        let total = screen.scrollback_rows();
        let start = total.saturating_sub(actual_rows);
        let lines = screen.lines_in_phys_range(start..total);

        for (row_idx, line) in lines.iter().enumerate() {
            let y = origin.y + row_idx as f32 * cell_height;

            for cell_ref in line.visible_cells() {
                let col_idx = cell_ref.cell_index();
                if col_idx >= actual_cols {
                    break;
                }

                let x = origin.x + col_idx as f32 * cell_width;
                let attrs = cell_ref.attrs();
                let reverse = attrs.reverse();

                let bg =
                    srgba_to_egui(resolve_color(&attrs.background(), &palette, false, reverse));
                let fg = srgba_to_egui(resolve_color(&attrs.foreground(), &palette, true, reverse));

                // Cell background (only if different from default)
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

                // Cell text
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
        if cursor.y >= 0 && (cursor.y as usize) < actual_rows && cursor.x < actual_cols {
            let cx = origin.x + cursor.x as f32 * cell_width;
            let cy = origin.y + cursor.y as f32 * cell_height;
            let cursor_color = srgba_to_egui(palette.cursor_bg);
            painter.rect_filled(
                egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(cell_width, cell_height)),
                0.0,
                cursor_color,
            );
        }

        // Tell egui we used this space
        ui.allocate_rect(terminal_rect, egui::Sense::click_and_drag());
    }

    fn handle_input(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());

        for event in &events {
            match event {
                egui::Event::Text(text) => {
                    let _ = self.pane.write_bytes(text.as_bytes());
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if let Some(bytes) = encode_egui_key(key, modifiers) {
                        let _ = self.pane.write_bytes(&bytes);
                    }
                }
                egui::Event::Paste(text) => {
                    let _ = self.pane.write_bytes(b"\x1b[200~");
                    let _ = self.pane.write_bytes(text.as_bytes());
                    let _ = self.pane.write_bytes(b"\x1b[201~");
                }
                _ => {}
            }
        }
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
    // Ctrl+letter → control character
    if modifiers.ctrl && !modifiers.alt {
        if let Some(byte) = ctrl_byte_for_key(key) {
            return Some(vec![byte]);
        }
    }

    // Alt+letter → ESC prefix + letter
    if modifiers.alt && !modifiers.ctrl {
        if let Some(ch) = key_to_char(key) {
            return Some(vec![0x1b, ch as u8]);
        }
    }

    // Ctrl+Alt → ESC prefix + control character
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

        // Arrow keys
        egui::Key::ArrowUp => Some(encode_arrow(b'A', modifier_param)),
        egui::Key::ArrowDown => Some(encode_arrow(b'B', modifier_param)),
        egui::Key::ArrowRight => Some(encode_arrow(b'C', modifier_param)),
        egui::Key::ArrowLeft => Some(encode_arrow(b'D', modifier_param)),

        // Navigation
        egui::Key::Home => Some(encode_csi_letter(b'H', modifier_param)),
        egui::Key::End => Some(encode_csi_letter(b'F', modifier_param)),
        egui::Key::Insert => Some(encode_csi_tilde(2, modifier_param)),
        egui::Key::Delete => Some(encode_csi_tilde(3, modifier_param)),
        egui::Key::PageUp => Some(encode_csi_tilde(5, modifier_param)),
        egui::Key::PageDown => Some(encode_csi_tilde(6, modifier_param)),

        // Function keys
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
