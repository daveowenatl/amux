pub mod any_backend;
pub mod backend;
pub mod color;
pub mod config;
pub mod font;
#[cfg(feature = "libghostty")]
pub mod ghostty_pane;
pub mod key_encoder;
pub mod mouse_encoder;
pub mod osc;
pub mod pane;

pub use any_backend::AnyBackend;
pub use backend::{
    Color, CursorPos, CursorShape, Palette, ProcessExit, ScreenCell, ScreenRow, StableRow,
    TerminalBackend,
};
pub use pane::TermError;
