use winit::event::{ElementState, MouseButton, MouseScrollDelta};
use winit::keyboard::ModifiersState;

/// Mouse tracking mode as negotiated by the terminal application via DECSET.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseTrackingMode {
    /// No mouse tracking (default).
    #[default]
    None,
    /// X10 mode (DECSET 9): report button press only, no modifiers.
    X10,
    /// Normal mode (DECSET 1000): report button press and release.
    Normal,
    /// Button mode (DECSET 1002): report press, release, and drag with button held.
    Button,
    /// Any-event mode (DECSET 1003): report all motion events.
    AnyEvent,
}

/// Mouse encoding format as negotiated by the terminal application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseEncodingFormat {
    /// X10 legacy format: limited to coordinates ≤ 223.
    #[default]
    X10,
    /// SGR format (DECSET 1006): \e[<Cb;Cx;CyM/m — supports arbitrarily large coordinates.
    Sgr,
}

/// Encodes winit mouse events into byte sequences for the PTY.
pub struct MouseEncoder {
    pub tracking_mode: MouseTrackingMode,
    pub encoding_format: MouseEncodingFormat,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl MouseEncoder {
    pub fn new(cell_width: f32, cell_height: f32) -> Self {
        Self {
            tracking_mode: MouseTrackingMode::None,
            encoding_format: MouseEncodingFormat::Sgr,
            cell_width,
            cell_height,
        }
    }

    /// Encode a mouse button event. `pixel_x` and `pixel_y` are relative to
    /// the top-left of the terminal pane area.
    pub fn encode_button(
        &self,
        button: MouseButton,
        state: ElementState,
        pixel_x: f32,
        pixel_y: f32,
        modifiers: ModifiersState,
    ) -> Option<Vec<u8>> {
        if self.tracking_mode == MouseTrackingMode::None {
            return None;
        }

        let (col, row) = self.pixel_to_cell(pixel_x, pixel_y);
        let button_code = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            _ => return None,
        };

        let cb = button_code | modifier_bits(modifiers);

        match self.encoding_format {
            MouseEncodingFormat::Sgr => {
                let suffix = if state == ElementState::Pressed {
                    'M'
                } else {
                    'm'
                };
                Some(format!("\x1b[<{};{};{}{}", cb, col, row, suffix).into_bytes())
            }
            MouseEncodingFormat::X10 => {
                if state == ElementState::Released && self.tracking_mode == MouseTrackingMode::X10 {
                    return None; // X10 mode doesn't report releases
                }
                let final_cb = if state == ElementState::Released {
                    3 | modifier_bits(modifiers) // release = button 3
                } else {
                    cb
                };
                encode_x10(final_cb, col, row)
            }
        }
    }

    /// Encode a mouse motion event (for Button or AnyEvent tracking modes).
    pub fn encode_motion(
        &self,
        pixel_x: f32,
        pixel_y: f32,
        button_held: Option<MouseButton>,
        modifiers: ModifiersState,
    ) -> Option<Vec<u8>> {
        match self.tracking_mode {
            MouseTrackingMode::AnyEvent => {}
            MouseTrackingMode::Button if button_held.is_some() => {}
            _ => return None,
        }

        let (col, row) = self.pixel_to_cell(pixel_x, pixel_y);
        let button_code = match button_held {
            Some(MouseButton::Left) => 0,
            Some(MouseButton::Middle) => 1,
            Some(MouseButton::Right) => 2,
            _ => 3, // no button
        };

        let cb = button_code | 32 | modifier_bits(modifiers); // 32 = motion flag

        match self.encoding_format {
            MouseEncodingFormat::Sgr => Some(format!("\x1b[<{};{};{}M", cb, col, row).into_bytes()),
            MouseEncodingFormat::X10 => encode_x10(cb, col, row),
        }
    }

    /// Encode a scroll wheel event.
    pub fn encode_scroll(
        &self,
        delta: MouseScrollDelta,
        pixel_x: f32,
        pixel_y: f32,
        modifiers: ModifiersState,
    ) -> Option<Vec<u8>> {
        if self.tracking_mode == MouseTrackingMode::None {
            return None;
        }

        let lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => y,
            MouseScrollDelta::PixelDelta(pos) => (pos.y / self.cell_height as f64) as f32,
        };

        if lines.abs() < 0.1 {
            return None;
        }

        let (col, row) = self.pixel_to_cell(pixel_x, pixel_y);
        // Scroll up = button 64, scroll down = button 65
        let button_code = if lines > 0.0 { 64 } else { 65 };
        let cb = button_code | modifier_bits(modifiers);

        match self.encoding_format {
            MouseEncodingFormat::Sgr => Some(format!("\x1b[<{};{};{}M", cb, col, row).into_bytes()),
            MouseEncodingFormat::X10 => encode_x10(cb, col, row),
        }
    }

    /// Convert pixel coordinates to 1-based cell coordinates.
    fn pixel_to_cell(&self, pixel_x: f32, pixel_y: f32) -> (u32, u32) {
        let col = (pixel_x / self.cell_width).floor() as u32 + 1;
        let row = (pixel_y / self.cell_height).floor() as u32 + 1;
        (col.max(1), row.max(1))
    }
}

/// Compute modifier bits for the mouse protocol.
fn modifier_bits(modifiers: ModifiersState) -> u32 {
    let mut bits = 0u32;
    if modifiers.shift_key() {
        bits |= 4;
    }
    if modifiers.alt_key() {
        bits |= 8;
    }
    if modifiers.control_key() {
        bits |= 16;
    }
    bits
}

/// Encode in X10 format: \e[M Cb Cx Cy (each byte = value + 32).
/// Returns None if coordinates exceed the representable range (> 223).
fn encode_x10(cb: u32, col: u32, row: u32) -> Option<Vec<u8>> {
    if col > 223 || row > 223 {
        return None; // X10 can't represent coordinates > 223
    }
    Some(vec![
        0x1b,
        b'[',
        b'M',
        (cb + 32) as u8,
        (col + 32) as u8,
        (row + 32) as u8,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoder() -> MouseEncoder {
        MouseEncoder::new(8.0, 16.0)
    }

    #[test]
    fn no_tracking_returns_none() {
        let enc = encoder();
        assert!(enc
            .encode_button(
                MouseButton::Left,
                ElementState::Pressed,
                10.0,
                20.0,
                ModifiersState::empty()
            )
            .is_none());
    }

    #[test]
    fn sgr_left_click() {
        let mut enc = encoder();
        enc.tracking_mode = MouseTrackingMode::Normal;
        enc.encoding_format = MouseEncodingFormat::Sgr;
        let result = enc
            .encode_button(
                MouseButton::Left,
                ElementState::Pressed,
                8.0,  // col 2
                16.0, // row 2
                ModifiersState::empty(),
            )
            .unwrap();
        assert_eq!(result, b"\x1b[<0;2;2M");
    }

    #[test]
    fn sgr_left_release() {
        let mut enc = encoder();
        enc.tracking_mode = MouseTrackingMode::Normal;
        enc.encoding_format = MouseEncodingFormat::Sgr;
        let result = enc
            .encode_button(
                MouseButton::Left,
                ElementState::Released,
                0.0, // col 1
                0.0, // row 1
                ModifiersState::empty(),
            )
            .unwrap();
        assert_eq!(result, b"\x1b[<0;1;1m");
    }

    #[test]
    fn sgr_scroll_up() {
        let mut enc = encoder();
        enc.tracking_mode = MouseTrackingMode::Normal;
        enc.encoding_format = MouseEncodingFormat::Sgr;
        let result = enc
            .encode_scroll(
                MouseScrollDelta::LineDelta(0.0, 1.0),
                0.0,
                0.0,
                ModifiersState::empty(),
            )
            .unwrap();
        assert_eq!(result, b"\x1b[<64;1;1M");
    }

    #[test]
    fn sgr_scroll_down() {
        let mut enc = encoder();
        enc.tracking_mode = MouseTrackingMode::Normal;
        enc.encoding_format = MouseEncodingFormat::Sgr;
        let result = enc
            .encode_scroll(
                MouseScrollDelta::LineDelta(0.0, -1.0),
                0.0,
                0.0,
                ModifiersState::empty(),
            )
            .unwrap();
        assert_eq!(result, b"\x1b[<65;1;1M");
    }

    #[test]
    fn x10_left_click() {
        let mut enc = encoder();
        enc.tracking_mode = MouseTrackingMode::Normal;
        enc.encoding_format = MouseEncodingFormat::X10;
        let result = enc
            .encode_button(
                MouseButton::Left,
                ElementState::Pressed,
                0.0, // col 1
                0.0, // row 1
                ModifiersState::empty(),
            )
            .unwrap();
        // cb=0+32=32, col=1+32=33, row=1+32=33
        assert_eq!(result, vec![0x1b, b'[', b'M', 32, 33, 33]);
    }

    #[test]
    fn pixel_to_cell_conversion() {
        let enc = encoder();
        // 0,0 -> cell (1,1)
        assert_eq!(enc.pixel_to_cell(0.0, 0.0), (1, 1));
        // 8,16 -> cell (2,2)
        assert_eq!(enc.pixel_to_cell(8.0, 16.0), (2, 2));
        // 7.9, 15.9 -> still cell (1,1)
        assert_eq!(enc.pixel_to_cell(7.9, 15.9), (1, 1));
    }
}
