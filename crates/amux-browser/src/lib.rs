//! Browser pane embedding for amux.
//!
//! Wraps platform-native webviews (WKWebView on macOS, WebView2 on Windows)
//! via the `wry` crate, providing a `BrowserPane` that can be embedded as a
//! child view within an eframe/egui window.

mod pane;

pub use pane::BrowserPane;
