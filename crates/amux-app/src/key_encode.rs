//! Map egui keyboard events to terminal byte sequences.

/// Encode an egui key event into terminal escape bytes.
pub(crate) fn encode_egui_key(key: &egui::Key, modifiers: &egui::Modifiers) -> Option<Vec<u8>> {
    use amux_core::keys;

    // Ctrl+key / Alt+key / Ctrl+Alt+key
    if let Some(byte) = egui_ctrl_byte(key) {
        if modifiers.ctrl && !modifiers.alt {
            return Some(keys::encode_ctrl(byte));
        }
        if modifiers.alt && !modifiers.ctrl {
            return Some(keys::encode_alt_char(byte - 1 + b'a'));
        }
        if modifiers.ctrl && modifiers.alt {
            return Some(keys::encode_ctrl_alt(byte));
        }
    }

    // Named keys — delegate to core encoder
    let core_key = egui_key_to_core(key)?;
    let mods = keys::Modifiers {
        shift: modifiers.shift,
        ctrl: modifiers.ctrl,
        alt: modifiers.alt,
    };
    keys::encode_named(core_key, mods, false)
}

/// Map egui letter keys to their Ctrl control byte (A=0x01 .. Z=0x1a).
fn egui_ctrl_byte(key: &egui::Key) -> Option<u8> {
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

/// Map egui Key to core NamedKey.
fn egui_key_to_core(key: &egui::Key) -> Option<amux_core::keys::NamedKey> {
    use amux_core::keys::NamedKey;
    Some(match key {
        egui::Key::Enter => NamedKey::Enter,
        egui::Key::Tab => NamedKey::Tab,
        egui::Key::Escape => NamedKey::Escape,
        egui::Key::Backspace => NamedKey::Backspace,
        egui::Key::Space => NamedKey::Space,
        egui::Key::ArrowUp => NamedKey::ArrowUp,
        egui::Key::ArrowDown => NamedKey::ArrowDown,
        egui::Key::ArrowLeft => NamedKey::ArrowLeft,
        egui::Key::ArrowRight => NamedKey::ArrowRight,
        egui::Key::Home => NamedKey::Home,
        egui::Key::End => NamedKey::End,
        egui::Key::Insert => NamedKey::Insert,
        egui::Key::Delete => NamedKey::Delete,
        egui::Key::PageUp => NamedKey::PageUp,
        egui::Key::PageDown => NamedKey::PageDown,
        egui::Key::F1 => NamedKey::F1,
        egui::Key::F2 => NamedKey::F2,
        egui::Key::F3 => NamedKey::F3,
        egui::Key::F4 => NamedKey::F4,
        egui::Key::F5 => NamedKey::F5,
        egui::Key::F6 => NamedKey::F6,
        egui::Key::F7 => NamedKey::F7,
        egui::Key::F8 => NamedKey::F8,
        egui::Key::F9 => NamedKey::F9,
        egui::Key::F10 => NamedKey::F10,
        egui::Key::F11 => NamedKey::F11,
        egui::Key::F12 => NamedKey::F12,
        _ => return None,
    })
}
