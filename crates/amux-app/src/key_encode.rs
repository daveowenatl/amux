//! Map egui keyboard events to libghostty-vt key events for encoding.
//!
//! This module translates egui's key types into libghostty-vt's key types,
//! then delegates to the terminal backend's encoder which handles all keyboard
//! protocols (legacy VT, CSI-u, Kitty, modifyOtherKeys).

use amux_term::key_types::{Action, Key, Mods};
use amux_term::TerminalBackend;

/// Encode an egui key event by translating it to libghostty-vt types and
/// delegating to the terminal's built-in key encoder.
///
/// Returns `None` for keys the encoder doesn't handle (e.g. plain character
/// keys without Ctrl/Alt — those are handled via `Event::Text` instead).
pub(crate) fn encode_egui_key(
    pane: &mut impl TerminalBackend,
    key: &egui::Key,
    modifiers: &egui::Modifiers,
    repeat: bool,
) -> Option<Vec<u8>> {
    let gkey = egui_key_to_ghostty(key)?;
    let mods = egui_mods_to_ghostty(modifiers);
    let action = if repeat {
        Action::Repeat
    } else {
        Action::Press
    };

    // For character keys without Ctrl/Alt, return None so Event::Text handles them.
    // This avoids double-sending: egui fires both Event::Key and Event::Text for
    // plain character input.
    if is_character_key(key) && !modifiers.ctrl && !modifiers.alt && !modifiers.command {
        return None;
    }

    // For character keys with modifiers, provide the unshifted codepoint and
    // UTF-8 text so the encoder can produce correct Kitty/CSI-u sequences.
    let (text, unshifted_cp) = if let Some(ch) = key_to_char(key) {
        let text_str = if modifiers.shift {
            ch.to_uppercase().to_string()
        } else {
            ch.to_string()
        };
        (Some(text_str), Some(ch))
    } else {
        (None, None)
    };

    pane.encode_key(gkey, mods, action, text.as_deref(), unshifted_cp)
}

/// Map an egui Key to the corresponding libghostty Key.
/// Returns None for keys we don't handle (e.g. unknown/unrecognized).
fn egui_key_to_ghostty(key: &egui::Key) -> Option<Key> {
    Some(match key {
        // Named keys
        egui::Key::Enter => Key::Enter,
        egui::Key::Tab => Key::Tab,
        egui::Key::Escape => Key::Escape,
        egui::Key::Backspace => Key::Backspace,
        egui::Key::Space => Key::Space,
        egui::Key::ArrowUp => Key::ArrowUp,
        egui::Key::ArrowDown => Key::ArrowDown,
        egui::Key::ArrowLeft => Key::ArrowLeft,
        egui::Key::ArrowRight => Key::ArrowRight,
        egui::Key::Home => Key::Home,
        egui::Key::End => Key::End,
        egui::Key::Insert => Key::Insert,
        egui::Key::Delete => Key::Delete,
        egui::Key::PageUp => Key::PageUp,
        egui::Key::PageDown => Key::PageDown,

        // Function keys
        egui::Key::F1 => Key::F1,
        egui::Key::F2 => Key::F2,
        egui::Key::F3 => Key::F3,
        egui::Key::F4 => Key::F4,
        egui::Key::F5 => Key::F5,
        egui::Key::F6 => Key::F6,
        egui::Key::F7 => Key::F7,
        egui::Key::F8 => Key::F8,
        egui::Key::F9 => Key::F9,
        egui::Key::F10 => Key::F10,
        egui::Key::F11 => Key::F11,
        egui::Key::F12 => Key::F12,

        // Letter keys
        egui::Key::A => Key::A,
        egui::Key::B => Key::B,
        egui::Key::C => Key::C,
        egui::Key::D => Key::D,
        egui::Key::E => Key::E,
        egui::Key::F => Key::F,
        egui::Key::G => Key::G,
        egui::Key::H => Key::H,
        egui::Key::I => Key::I,
        egui::Key::J => Key::J,
        egui::Key::K => Key::K,
        egui::Key::L => Key::L,
        egui::Key::M => Key::M,
        egui::Key::N => Key::N,
        egui::Key::O => Key::O,
        egui::Key::P => Key::P,
        egui::Key::Q => Key::Q,
        egui::Key::R => Key::R,
        egui::Key::S => Key::S,
        egui::Key::T => Key::T,
        egui::Key::U => Key::U,
        egui::Key::V => Key::V,
        egui::Key::W => Key::W,
        egui::Key::X => Key::X,
        egui::Key::Y => Key::Y,
        egui::Key::Z => Key::Z,

        // Digit keys
        egui::Key::Num0 => Key::Digit0,
        egui::Key::Num1 => Key::Digit1,
        egui::Key::Num2 => Key::Digit2,
        egui::Key::Num3 => Key::Digit3,
        egui::Key::Num4 => Key::Digit4,
        egui::Key::Num5 => Key::Digit5,
        egui::Key::Num6 => Key::Digit6,
        egui::Key::Num7 => Key::Digit7,
        egui::Key::Num8 => Key::Digit8,
        egui::Key::Num9 => Key::Digit9,

        // Punctuation / symbol keys
        egui::Key::Minus => Key::Minus,
        egui::Key::Equals => Key::Equal,
        egui::Key::OpenBracket => Key::BracketLeft,
        egui::Key::CloseBracket => Key::BracketRight,
        egui::Key::Backslash => Key::Backslash,
        egui::Key::Semicolon => Key::Semicolon,
        egui::Key::Quote => Key::Quote,
        egui::Key::Comma => Key::Comma,
        egui::Key::Period => Key::Period,
        egui::Key::Slash => Key::Slash,
        egui::Key::Backtick => Key::Backquote,

        _ => return None,
    })
}

/// Map egui Modifiers to libghostty Mods bitmask.
fn egui_mods_to_ghostty(m: &egui::Modifiers) -> Mods {
    let mut mods = Mods::empty();
    if m.shift {
        mods |= Mods::SHIFT;
    }
    if m.ctrl {
        mods |= Mods::CTRL;
    }
    if m.alt {
        mods |= Mods::ALT;
    }
    if m.command {
        mods |= Mods::SUPER;
    }
    mods
}

/// Whether an egui Key represents a printable character (letter, digit, punctuation).
/// Named keys like Enter/Tab/F-keys are NOT character keys.
fn is_character_key(key: &egui::Key) -> bool {
    key_to_char(key).is_some()
}

/// Map a character-producing egui Key to its base (unshifted, lowercase) character.
fn key_to_char(key: &egui::Key) -> Option<char> {
    Some(match key {
        egui::Key::A => 'a',
        egui::Key::B => 'b',
        egui::Key::C => 'c',
        egui::Key::D => 'd',
        egui::Key::E => 'e',
        egui::Key::F => 'f',
        egui::Key::G => 'g',
        egui::Key::H => 'h',
        egui::Key::I => 'i',
        egui::Key::J => 'j',
        egui::Key::K => 'k',
        egui::Key::L => 'l',
        egui::Key::M => 'm',
        egui::Key::N => 'n',
        egui::Key::O => 'o',
        egui::Key::P => 'p',
        egui::Key::Q => 'q',
        egui::Key::R => 'r',
        egui::Key::S => 's',
        egui::Key::T => 't',
        egui::Key::U => 'u',
        egui::Key::V => 'v',
        egui::Key::W => 'w',
        egui::Key::X => 'x',
        egui::Key::Y => 'y',
        egui::Key::Z => 'z',
        egui::Key::Num0 => '0',
        egui::Key::Num1 => '1',
        egui::Key::Num2 => '2',
        egui::Key::Num3 => '3',
        egui::Key::Num4 => '4',
        egui::Key::Num5 => '5',
        egui::Key::Num6 => '6',
        egui::Key::Num7 => '7',
        egui::Key::Num8 => '8',
        egui::Key::Num9 => '9',
        egui::Key::Minus => '-',
        egui::Key::Equals => '=',
        egui::Key::OpenBracket => '[',
        egui::Key::CloseBracket => ']',
        egui::Key::Backslash => '\\',
        egui::Key::Semicolon => ';',
        egui::Key::Quote => '\'',
        egui::Key::Comma => ',',
        egui::Key::Period => '.',
        egui::Key::Slash => '/',
        egui::Key::Backtick => '`',
        egui::Key::Space => ' ',
        _ => return None,
    })
}
