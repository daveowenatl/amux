use winit::event::ElementState;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

/// Encodes winit key events into byte sequences suitable for writing to a PTY.
///
/// Handles:
/// - ASCII control characters (Ctrl+A through Ctrl+Z)
/// - Function keys (F1–F12)
/// - Arrow keys with application cursor mode (DECCKM)
/// - Home/End/PgUp/PgDn/Insert/Delete
/// - Modifier encoding (CSI 1;N suffixes)
///
/// The encoder is deliberately bypass-able: amux decides whether a key is a
/// shortcut or should be forwarded to the PTY. The encoder only runs on
/// forwarded keys.
#[derive(Default)]
pub struct KeyEncoder {
    /// Application cursor key mode (DECCKM). When true, arrow keys emit
    /// SS3 sequences instead of CSI sequences.
    pub application_cursor_keys: bool,
}

impl KeyEncoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Encode a key event into bytes to send to the PTY.
    ///
    /// Returns `None` if the key event should not produce any output
    /// (e.g. key release, or unknown key).
    pub fn encode(
        &self,
        key: &Key,
        physical_key: PhysicalKey,
        modifiers: ModifiersState,
        state: ElementState,
    ) -> Option<Vec<u8>> {
        // Only encode key press events
        if state != ElementState::Pressed {
            return None;
        }

        match key {
            Key::Character(text) => self.encode_character(text, modifiers, physical_key),
            Key::Named(named) => self.encode_named(*named, modifiers),
            _ => None,
        }
    }

    fn encode_character(
        &self,
        text: &str,
        modifiers: ModifiersState,
        physical_key: PhysicalKey,
    ) -> Option<Vec<u8>> {
        // Ctrl+letter → control character
        if modifiers.control_key() && !modifiers.alt_key() {
            if let PhysicalKey::Code(code) = physical_key {
                if let Some(ctrl_byte) = ctrl_code_for_key(code) {
                    return Some(vec![ctrl_byte]);
                }
            }
        }

        // Alt+key → ESC prefix
        if modifiers.alt_key() && !modifiers.control_key() {
            let mut bytes = vec![0x1b];
            bytes.extend_from_slice(text.as_bytes());
            return Some(bytes);
        }

        // Ctrl+Alt combinations
        if modifiers.control_key() && modifiers.alt_key() {
            if let PhysicalKey::Code(code) = physical_key {
                if let Some(ctrl_byte) = ctrl_code_for_key(code) {
                    return Some(vec![0x1b, ctrl_byte]);
                }
            }
        }

        // Plain text (including Shift variants handled by the OS)
        Some(text.as_bytes().to_vec())
    }

    fn encode_named(&self, named: NamedKey, modifiers: ModifiersState) -> Option<Vec<u8>> {
        let modifier_param = modifier_param(modifiers);

        match named {
            NamedKey::Enter => Some(vec![0x0d]),
            NamedKey::Tab => {
                if modifiers.shift_key() {
                    Some(b"\x1b[Z".to_vec()) // Backtab
                } else {
                    Some(vec![0x09])
                }
            }
            NamedKey::Backspace => {
                if modifiers.alt_key() {
                    Some(vec![0x1b, 0x7f])
                } else {
                    Some(vec![0x7f])
                }
            }
            NamedKey::Escape => Some(vec![0x1b]),
            NamedKey::Space => {
                if modifiers.control_key() {
                    Some(vec![0x00]) // Ctrl+Space = NUL
                } else {
                    Some(vec![0x20])
                }
            }

            // Arrow keys
            NamedKey::ArrowUp => Some(self.encode_arrow(b'A', modifier_param)),
            NamedKey::ArrowDown => Some(self.encode_arrow(b'B', modifier_param)),
            NamedKey::ArrowRight => Some(self.encode_arrow(b'C', modifier_param)),
            NamedKey::ArrowLeft => Some(self.encode_arrow(b'D', modifier_param)),

            // Navigation keys
            NamedKey::Home => Some(encode_csi_tilde_or_letter(1, b'H', modifier_param)),
            NamedKey::End => Some(encode_csi_tilde_or_letter(4, b'F', modifier_param)),
            NamedKey::Insert => Some(encode_csi_tilde(2, modifier_param)),
            NamedKey::Delete => Some(encode_csi_tilde(3, modifier_param)),
            NamedKey::PageUp => Some(encode_csi_tilde(5, modifier_param)),
            NamedKey::PageDown => Some(encode_csi_tilde(6, modifier_param)),

            // Function keys
            NamedKey::F1 => Some(encode_ss3_or_csi(b'P', 11, modifier_param)),
            NamedKey::F2 => Some(encode_ss3_or_csi(b'Q', 12, modifier_param)),
            NamedKey::F3 => Some(encode_ss3_or_csi(b'R', 13, modifier_param)),
            NamedKey::F4 => Some(encode_ss3_or_csi(b'S', 14, modifier_param)),
            NamedKey::F5 => Some(encode_csi_tilde(15, modifier_param)),
            NamedKey::F6 => Some(encode_csi_tilde(17, modifier_param)),
            NamedKey::F7 => Some(encode_csi_tilde(18, modifier_param)),
            NamedKey::F8 => Some(encode_csi_tilde(19, modifier_param)),
            NamedKey::F9 => Some(encode_csi_tilde(20, modifier_param)),
            NamedKey::F10 => Some(encode_csi_tilde(21, modifier_param)),
            NamedKey::F11 => Some(encode_csi_tilde(23, modifier_param)),
            NamedKey::F12 => Some(encode_csi_tilde(24, modifier_param)),

            _ => None,
        }
    }

    fn encode_arrow(&self, letter: u8, modifier_param: Option<u8>) -> Vec<u8> {
        if let Some(m) = modifier_param {
            // With modifiers: \e[1;Nletter
            format!("\x1b[1;{}{}", m, letter as char).into_bytes()
        } else if self.application_cursor_keys {
            // Application mode: SS3 letter
            vec![0x1b, b'O', letter]
        } else {
            // Normal mode: CSI letter
            vec![0x1b, b'[', letter]
        }
    }
}

/// Compute the xterm modifier parameter (1-based).
/// Returns None if no modifiers are active.
fn modifier_param(modifiers: ModifiersState) -> Option<u8> {
    let mut param: u8 = 1; // base value
    if modifiers.shift_key() {
        param += 1;
    }
    if modifiers.alt_key() {
        param += 2;
    }
    if modifiers.control_key() {
        param += 4;
    }
    if param == 1 {
        None
    } else {
        Some(param)
    }
}

/// Encode CSI number ~ sequences (e.g. Delete = \e[3~, with modifiers \e[3;N~)
fn encode_csi_tilde(number: u8, modifier_param: Option<u8>) -> Vec<u8> {
    match modifier_param {
        Some(m) => format!("\x1b[{};{}~", number, m).into_bytes(),
        None => format!("\x1b[{}~", number).into_bytes(),
    }
}

/// Encode Home/End: without modifiers use \e[H/\e[F, with modifiers use \e[1;NH/\e[1;NF
fn encode_csi_tilde_or_letter(_number: u8, letter: u8, modifier_param: Option<u8>) -> Vec<u8> {
    match modifier_param {
        Some(m) => format!("\x1b[1;{}{}", m, letter as char).into_bytes(),
        None => vec![0x1b, b'[', letter],
    }
}

/// Encode F1–F4: without modifiers use SS3 (e.g. \eOP), with modifiers use CSI number ~
fn encode_ss3_or_csi(ss3_letter: u8, csi_number: u8, modifier_param: Option<u8>) -> Vec<u8> {
    match modifier_param {
        Some(m) => format!("\x1b[{};{}~", csi_number, m).into_bytes(),
        None => vec![0x1b, b'O', ss3_letter],
    }
}

/// Map a physical key code to its Ctrl+key control character (0x01–0x1a).
fn ctrl_code_for_key(code: KeyCode) -> Option<u8> {
    match code {
        KeyCode::KeyA => Some(0x01),
        KeyCode::KeyB => Some(0x02),
        KeyCode::KeyC => Some(0x03),
        KeyCode::KeyD => Some(0x04),
        KeyCode::KeyE => Some(0x05),
        KeyCode::KeyF => Some(0x06),
        KeyCode::KeyG => Some(0x07),
        KeyCode::KeyH => Some(0x08),
        KeyCode::KeyI => Some(0x09),
        KeyCode::KeyJ => Some(0x0a),
        KeyCode::KeyK => Some(0x0b),
        KeyCode::KeyL => Some(0x0c),
        KeyCode::KeyM => Some(0x0d),
        KeyCode::KeyN => Some(0x0e),
        KeyCode::KeyO => Some(0x0f),
        KeyCode::KeyP => Some(0x10),
        KeyCode::KeyQ => Some(0x11),
        KeyCode::KeyR => Some(0x12),
        KeyCode::KeyS => Some(0x13),
        KeyCode::KeyT => Some(0x14),
        KeyCode::KeyU => Some(0x15),
        KeyCode::KeyV => Some(0x16),
        KeyCode::KeyW => Some(0x17),
        KeyCode::KeyX => Some(0x18),
        KeyCode::KeyY => Some(0x19),
        KeyCode::KeyZ => Some(0x1a),
        KeyCode::BracketLeft => Some(0x1b),  // Ctrl+[ = ESC
        KeyCode::Backslash => Some(0x1c),    // Ctrl+\ = FS
        KeyCode::BracketRight => Some(0x1d), // Ctrl+] = GS
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: Key, physical: KeyCode, mods: ModifiersState) -> Option<Vec<u8>> {
        let encoder = KeyEncoder::new();
        encoder.encode(
            &key,
            PhysicalKey::Code(physical),
            mods,
            ElementState::Pressed,
        )
    }

    fn no_mods() -> ModifiersState {
        ModifiersState::empty()
    }

    #[test]
    fn enter_key() {
        assert_eq!(
            press(Key::Named(NamedKey::Enter), KeyCode::Enter, no_mods()),
            Some(vec![0x0d])
        );
    }

    #[test]
    fn escape_key() {
        assert_eq!(
            press(Key::Named(NamedKey::Escape), KeyCode::Escape, no_mods()),
            Some(vec![0x1b])
        );
    }

    #[test]
    fn plain_character() {
        assert_eq!(
            press(Key::Character("a".into()), KeyCode::KeyA, no_mods()),
            Some(b"a".to_vec())
        );
    }

    #[test]
    fn ctrl_c() {
        assert_eq!(
            press(
                Key::Character("c".into()),
                KeyCode::KeyC,
                ModifiersState::CONTROL
            ),
            Some(vec![0x03])
        );
    }

    #[test]
    fn alt_a() {
        assert_eq!(
            press(
                Key::Character("a".into()),
                KeyCode::KeyA,
                ModifiersState::ALT
            ),
            Some(vec![0x1b, b'a'])
        );
    }

    #[test]
    fn arrow_up_normal_mode() {
        assert_eq!(
            press(Key::Named(NamedKey::ArrowUp), KeyCode::ArrowUp, no_mods()),
            Some(vec![0x1b, b'[', b'A'])
        );
    }

    #[test]
    fn arrow_up_application_mode() {
        let mut encoder = KeyEncoder::new();
        encoder.application_cursor_keys = true;
        assert_eq!(
            encoder.encode(
                &Key::Named(NamedKey::ArrowUp),
                PhysicalKey::Code(KeyCode::ArrowUp),
                no_mods(),
                ElementState::Pressed
            ),
            Some(vec![0x1b, b'O', b'A'])
        );
    }

    #[test]
    fn shift_arrow_up() {
        assert_eq!(
            press(
                Key::Named(NamedKey::ArrowUp),
                KeyCode::ArrowUp,
                ModifiersState::SHIFT
            ),
            Some(b"\x1b[1;2A".to_vec())
        );
    }

    #[test]
    fn f1_no_modifiers() {
        assert_eq!(
            press(Key::Named(NamedKey::F1), KeyCode::F1, no_mods()),
            Some(vec![0x1b, b'O', b'P'])
        );
    }

    #[test]
    fn f5_no_modifiers() {
        assert_eq!(
            press(Key::Named(NamedKey::F5), KeyCode::F5, no_mods()),
            Some(b"\x1b[15~".to_vec())
        );
    }

    #[test]
    fn delete_key() {
        assert_eq!(
            press(Key::Named(NamedKey::Delete), KeyCode::Delete, no_mods()),
            Some(b"\x1b[3~".to_vec())
        );
    }

    #[test]
    fn home_key() {
        assert_eq!(
            press(Key::Named(NamedKey::Home), KeyCode::Home, no_mods()),
            Some(vec![0x1b, b'[', b'H'])
        );
    }

    #[test]
    fn backtab() {
        assert_eq!(
            press(
                Key::Named(NamedKey::Tab),
                KeyCode::Tab,
                ModifiersState::SHIFT
            ),
            Some(b"\x1b[Z".to_vec())
        );
    }

    #[test]
    fn key_release_ignored() {
        let encoder = KeyEncoder::new();
        assert_eq!(
            encoder.encode(
                &Key::Named(NamedKey::Enter),
                PhysicalKey::Code(KeyCode::Enter),
                no_mods(),
                ElementState::Released
            ),
            None
        );
    }
}
