//! Framework-agnostic terminal key encoding.
//!
//! Converts key events into byte sequences for writing to a PTY. This module
//! contains no GUI dependencies — adapters in `amux-app` (egui) and
//! `amux-term` (winit) translate their framework types into these types.

/// Modifier state for key encoding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// Named keys that produce escape sequences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedKey {
    Enter,
    Tab,
    Escape,
    Backspace,
    Space,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    Insert,
    Delete,
    PageUp,
    PageDown,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

/// Encode a named key with modifiers into PTY bytes.
///
/// `application_cursor_keys` controls whether arrow keys emit SS3 (`\eOA`)
/// or CSI (`\e[A`) sequences (DECCKM mode).
pub fn encode_named(
    key: NamedKey,
    mods: Modifiers,
    application_cursor_keys: bool,
) -> Option<Vec<u8>> {
    let mod_param = modifier_param(mods);

    match key {
        NamedKey::Enter => Some(vec![0x0d]),
        NamedKey::Tab => {
            if mods.shift {
                Some(b"\x1b[Z".to_vec()) // Backtab
            } else {
                Some(vec![0x09])
            }
        }
        NamedKey::Escape => Some(vec![0x1b]),
        NamedKey::Backspace => {
            if mods.alt {
                Some(vec![0x1b, 0x7f])
            } else {
                Some(vec![0x7f])
            }
        }
        NamedKey::Space => {
            if mods.ctrl {
                Some(vec![0x00]) // Ctrl+Space = NUL
            } else {
                Some(vec![0x20])
            }
        }

        // Arrow keys
        NamedKey::ArrowUp => Some(encode_arrow(b'A', mod_param, application_cursor_keys)),
        NamedKey::ArrowDown => Some(encode_arrow(b'B', mod_param, application_cursor_keys)),
        NamedKey::ArrowRight => Some(encode_arrow(b'C', mod_param, application_cursor_keys)),
        NamedKey::ArrowLeft => Some(encode_arrow(b'D', mod_param, application_cursor_keys)),

        // Navigation — Home/End use letter form without modifiers
        NamedKey::Home => Some(encode_csi_letter(b'H', mod_param)),
        NamedKey::End => Some(encode_csi_letter(b'F', mod_param)),
        NamedKey::Insert => Some(encode_csi_tilde(2, mod_param)),
        NamedKey::Delete => Some(encode_csi_tilde(3, mod_param)),
        NamedKey::PageUp => Some(encode_csi_tilde(5, mod_param)),
        NamedKey::PageDown => Some(encode_csi_tilde(6, mod_param)),

        // Function keys — F1-F4 use SS3 without modifiers, CSI ~ with
        NamedKey::F1 => Some(encode_fn_key(b'P', 11, mod_param)),
        NamedKey::F2 => Some(encode_fn_key(b'Q', 12, mod_param)),
        NamedKey::F3 => Some(encode_fn_key(b'R', 13, mod_param)),
        NamedKey::F4 => Some(encode_fn_key(b'S', 14, mod_param)),
        NamedKey::F5 => Some(encode_csi_tilde(15, mod_param)),
        NamedKey::F6 => Some(encode_csi_tilde(17, mod_param)),
        NamedKey::F7 => Some(encode_csi_tilde(18, mod_param)),
        NamedKey::F8 => Some(encode_csi_tilde(19, mod_param)),
        NamedKey::F9 => Some(encode_csi_tilde(20, mod_param)),
        NamedKey::F10 => Some(encode_csi_tilde(21, mod_param)),
        NamedKey::F11 => Some(encode_csi_tilde(23, mod_param)),
        NamedKey::F12 => Some(encode_csi_tilde(24, mod_param)),
    }
}

/// Encode Ctrl+letter (A=0x01 .. Z=0x1a).
pub fn encode_ctrl_letter(letter_index: u8) -> Vec<u8> {
    vec![letter_index + 1]
}

/// Encode Alt+letter (ESC + lowercase letter).
pub fn encode_alt_letter(letter_index: u8) -> Vec<u8> {
    vec![0x1b, letter_index + b'a']
}

/// Encode Ctrl+Alt+letter (ESC + control byte).
pub fn encode_ctrl_alt_letter(letter_index: u8) -> Vec<u8> {
    vec![0x1b, letter_index + 1]
}

// --- Internal helpers ---

/// Compute the xterm modifier parameter (1-based).
/// Returns None if no modifiers are active.
fn modifier_param(mods: Modifiers) -> Option<u8> {
    let mut param: u8 = 1;
    if mods.shift {
        param += 1;
    }
    if mods.alt {
        param += 2;
    }
    if mods.ctrl {
        param += 4;
    }
    if param == 1 {
        None
    } else {
        Some(param)
    }
}

/// Arrow keys: with modifiers use `\e[1;Nx`, without use CSI or SS3 depending on DECCKM.
fn encode_arrow(letter: u8, mod_param: Option<u8>, application_cursor_keys: bool) -> Vec<u8> {
    if let Some(m) = mod_param {
        format!("\x1b[1;{}{}", m, letter as char).into_bytes()
    } else if application_cursor_keys {
        vec![0x1b, b'O', letter]
    } else {
        vec![0x1b, b'[', letter]
    }
}

/// CSI letter form: `\e[x` or `\e[1;Nx` with modifiers.
fn encode_csi_letter(letter: u8, mod_param: Option<u8>) -> Vec<u8> {
    match mod_param {
        Some(m) => format!("\x1b[1;{}{}", m, letter as char).into_bytes(),
        None => vec![0x1b, b'[', letter],
    }
}

/// CSI number ~ form: `\e[N~` or `\e[N;M~` with modifiers.
fn encode_csi_tilde(number: u8, mod_param: Option<u8>) -> Vec<u8> {
    match mod_param {
        Some(m) => format!("\x1b[{};{}~", number, m).into_bytes(),
        None => format!("\x1b[{}~", number).into_bytes(),
    }
}

/// F1-F4: SS3 letter without modifiers, CSI number ~ with modifiers.
fn encode_fn_key(ss3_letter: u8, csi_number: u8, mod_param: Option<u8>) -> Vec<u8> {
    match mod_param {
        Some(m) => format!("\x1b[{};{}~", csi_number, m).into_bytes(),
        None => vec![0x1b, b'O', ss3_letter],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_mods() -> Modifiers {
        Modifiers::default()
    }

    fn shift() -> Modifiers {
        Modifiers {
            shift: true,
            ..Default::default()
        }
    }

    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Default::default()
        }
    }

    fn alt() -> Modifiers {
        Modifiers {
            alt: true,
            ..Default::default()
        }
    }

    // --- Named keys ---

    #[test]
    fn enter() {
        assert_eq!(
            encode_named(NamedKey::Enter, no_mods(), false),
            Some(vec![0x0d])
        );
    }

    #[test]
    fn tab() {
        assert_eq!(
            encode_named(NamedKey::Tab, no_mods(), false),
            Some(vec![0x09])
        );
    }

    #[test]
    fn backtab() {
        assert_eq!(
            encode_named(NamedKey::Tab, shift(), false),
            Some(b"\x1b[Z".to_vec())
        );
    }

    #[test]
    fn escape() {
        assert_eq!(
            encode_named(NamedKey::Escape, no_mods(), false),
            Some(vec![0x1b])
        );
    }

    #[test]
    fn backspace() {
        assert_eq!(
            encode_named(NamedKey::Backspace, no_mods(), false),
            Some(vec![0x7f])
        );
    }

    #[test]
    fn alt_backspace() {
        assert_eq!(
            encode_named(NamedKey::Backspace, alt(), false),
            Some(vec![0x1b, 0x7f])
        );
    }

    #[test]
    fn space() {
        assert_eq!(
            encode_named(NamedKey::Space, no_mods(), false),
            Some(vec![0x20])
        );
    }

    #[test]
    fn ctrl_space() {
        assert_eq!(
            encode_named(NamedKey::Space, ctrl(), false),
            Some(vec![0x00])
        );
    }

    // --- Arrow keys ---

    #[test]
    fn arrow_up_normal() {
        assert_eq!(
            encode_named(NamedKey::ArrowUp, no_mods(), false),
            Some(vec![0x1b, b'[', b'A'])
        );
    }

    #[test]
    fn arrow_up_application() {
        assert_eq!(
            encode_named(NamedKey::ArrowUp, no_mods(), true),
            Some(vec![0x1b, b'O', b'A'])
        );
    }

    #[test]
    fn shift_arrow_up() {
        assert_eq!(
            encode_named(NamedKey::ArrowUp, shift(), false),
            Some(b"\x1b[1;2A".to_vec())
        );
    }

    #[test]
    fn ctrl_arrow_right() {
        assert_eq!(
            encode_named(NamedKey::ArrowRight, ctrl(), false),
            Some(b"\x1b[1;5C".to_vec())
        );
    }

    // --- Navigation keys ---

    #[test]
    fn home() {
        assert_eq!(
            encode_named(NamedKey::Home, no_mods(), false),
            Some(vec![0x1b, b'[', b'H'])
        );
    }

    #[test]
    fn end() {
        assert_eq!(
            encode_named(NamedKey::End, no_mods(), false),
            Some(vec![0x1b, b'[', b'F'])
        );
    }

    #[test]
    fn delete() {
        assert_eq!(
            encode_named(NamedKey::Delete, no_mods(), false),
            Some(b"\x1b[3~".to_vec())
        );
    }

    #[test]
    fn page_up() {
        assert_eq!(
            encode_named(NamedKey::PageUp, no_mods(), false),
            Some(b"\x1b[5~".to_vec())
        );
    }

    #[test]
    fn insert() {
        assert_eq!(
            encode_named(NamedKey::Insert, no_mods(), false),
            Some(b"\x1b[2~".to_vec())
        );
    }

    // --- Function keys ---

    #[test]
    fn f1_no_mods() {
        assert_eq!(
            encode_named(NamedKey::F1, no_mods(), false),
            Some(vec![0x1b, b'O', b'P'])
        );
    }

    #[test]
    fn f5_no_mods() {
        assert_eq!(
            encode_named(NamedKey::F5, no_mods(), false),
            Some(b"\x1b[15~".to_vec())
        );
    }

    #[test]
    fn f1_with_shift() {
        assert_eq!(
            encode_named(NamedKey::F1, shift(), false),
            Some(b"\x1b[11;2~".to_vec())
        );
    }

    #[test]
    fn f12_no_mods() {
        assert_eq!(
            encode_named(NamedKey::F12, no_mods(), false),
            Some(b"\x1b[24~".to_vec())
        );
    }

    // --- Ctrl/Alt letter ---

    #[test]
    fn ctrl_a() {
        assert_eq!(encode_ctrl_letter(0), vec![0x01]);
    }

    #[test]
    fn ctrl_c() {
        assert_eq!(encode_ctrl_letter(2), vec![0x03]);
    }

    #[test]
    fn ctrl_z() {
        assert_eq!(encode_ctrl_letter(25), vec![0x1a]);
    }

    #[test]
    fn alt_a() {
        assert_eq!(encode_alt_letter(0), vec![0x1b, b'a']);
    }

    #[test]
    fn alt_z() {
        assert_eq!(encode_alt_letter(25), vec![0x1b, b'z']);
    }

    #[test]
    fn ctrl_alt_a() {
        assert_eq!(encode_ctrl_alt_letter(0), vec![0x1b, 0x01]);
    }

    // --- Modifier param ---

    #[test]
    fn modifier_param_none() {
        assert_eq!(modifier_param(no_mods()), None);
    }

    #[test]
    fn modifier_param_shift() {
        assert_eq!(modifier_param(shift()), Some(2));
    }

    #[test]
    fn modifier_param_ctrl() {
        assert_eq!(modifier_param(ctrl()), Some(5));
    }

    #[test]
    fn modifier_param_alt() {
        assert_eq!(modifier_param(alt()), Some(3));
    }
}
