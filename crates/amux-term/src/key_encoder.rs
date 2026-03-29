use amux_core::keys;
use winit::event::ElementState;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

/// Encodes winit key events into byte sequences suitable for writing to a PTY.
///
/// This is a thin adapter over `amux_core::keys` that translates winit types
/// into the framework-agnostic core types.
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
        if state != ElementState::Pressed {
            return None;
        }

        match key {
            Key::Character(text) => self.encode_character(text, modifiers, physical_key),
            Key::Named(named) => {
                let core_key = winit_named_to_core(*named)?;
                let mods = winit_mods_to_core(modifiers);
                keys::encode_named(core_key, mods, self.application_cursor_keys)
            }
            _ => None,
        }
    }

    fn encode_character(
        &self,
        text: &str,
        modifiers: ModifiersState,
        physical_key: PhysicalKey,
    ) -> Option<Vec<u8>> {
        // Ctrl+key → control byte
        if modifiers.control_key() && !modifiers.alt_key() {
            if let PhysicalKey::Code(code) = physical_key {
                if let Some(byte) = ctrl_byte_for_keycode(code) {
                    return Some(keys::encode_ctrl(byte));
                }
            }
        }

        // Alt+key → ESC prefix
        if modifiers.alt_key() && !modifiers.control_key() {
            let mut bytes = vec![0x1b];
            bytes.extend_from_slice(text.as_bytes());
            return Some(bytes);
        }

        // Ctrl+Alt → ESC + control byte
        if modifiers.control_key() && modifiers.alt_key() {
            if let PhysicalKey::Code(code) = physical_key {
                if let Some(byte) = ctrl_byte_for_keycode(code) {
                    return Some(keys::encode_ctrl_alt(byte));
                }
            }
        }

        // Plain text
        Some(text.as_bytes().to_vec())
    }
}

/// Map winit NamedKey to core NamedKey.
fn winit_named_to_core(key: NamedKey) -> Option<keys::NamedKey> {
    Some(match key {
        NamedKey::Enter => keys::NamedKey::Enter,
        NamedKey::Tab => keys::NamedKey::Tab,
        NamedKey::Escape => keys::NamedKey::Escape,
        NamedKey::Backspace => keys::NamedKey::Backspace,
        NamedKey::Space => keys::NamedKey::Space,
        NamedKey::ArrowUp => keys::NamedKey::ArrowUp,
        NamedKey::ArrowDown => keys::NamedKey::ArrowDown,
        NamedKey::ArrowLeft => keys::NamedKey::ArrowLeft,
        NamedKey::ArrowRight => keys::NamedKey::ArrowRight,
        NamedKey::Home => keys::NamedKey::Home,
        NamedKey::End => keys::NamedKey::End,
        NamedKey::Insert => keys::NamedKey::Insert,
        NamedKey::Delete => keys::NamedKey::Delete,
        NamedKey::PageUp => keys::NamedKey::PageUp,
        NamedKey::PageDown => keys::NamedKey::PageDown,
        NamedKey::F1 => keys::NamedKey::F1,
        NamedKey::F2 => keys::NamedKey::F2,
        NamedKey::F3 => keys::NamedKey::F3,
        NamedKey::F4 => keys::NamedKey::F4,
        NamedKey::F5 => keys::NamedKey::F5,
        NamedKey::F6 => keys::NamedKey::F6,
        NamedKey::F7 => keys::NamedKey::F7,
        NamedKey::F8 => keys::NamedKey::F8,
        NamedKey::F9 => keys::NamedKey::F9,
        NamedKey::F10 => keys::NamedKey::F10,
        NamedKey::F11 => keys::NamedKey::F11,
        NamedKey::F12 => keys::NamedKey::F12,
        _ => return None,
    })
}

/// Convert winit ModifiersState to core Modifiers.
fn winit_mods_to_core(mods: ModifiersState) -> keys::Modifiers {
    keys::Modifiers {
        shift: mods.shift_key(),
        ctrl: mods.control_key(),
        alt: mods.alt_key(),
    }
}

/// Map a physical key code to its Ctrl control byte.
/// A=0x01 .. Z=0x1a, plus Ctrl+[ = ESC (0x1b), Ctrl+\ = FS (0x1c), Ctrl+] = GS (0x1d).
fn ctrl_byte_for_keycode(code: KeyCode) -> Option<u8> {
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
        KeyCode::BracketRight => Some(0x1d), // Ctrl+] = GS (telnet escape)
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
