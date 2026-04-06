//! Browser pane — wraps a platform-native webview as a child view.
//!
//! Uses wry to embed WKWebView (macOS) or WebView2 (Windows) as a child
//! of the eframe/egui window. The webview is positioned and sized to fill
//! the pane's content area (below the tab bar).

use raw_window_handle::HasWindowHandle;
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{Rect, WebViewBuilder};

/// A browser pane backed by a platform-native webview.
pub struct BrowserPane {
    webview: wry::WebView,
    current_title: String,
}

/// Logical rectangle for positioning the webview within its parent window.
#[derive(Debug, Clone, Copy)]
pub struct BrowserRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl BrowserRect {
    fn to_wry_rect(self) -> Rect {
        Rect {
            position: LogicalPosition::new(self.x, self.y).into(),
            size: LogicalSize::new(self.width, self.height).into(),
        }
    }
}

impl BrowserPane {
    /// Create a new browser pane as a child view of the given window.
    ///
    /// `bounds` specifies the logical position and size within the parent window.
    /// `url` is the initial URL to load (use "about:blank" for empty).
    pub fn new<W: HasWindowHandle>(
        parent: &W,
        bounds: BrowserRect,
        url: &str,
    ) -> Result<Self, wry::Error> {
        let title = String::new();
        let title_clone = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let _title_ref = title_clone.clone();

        let webview = WebViewBuilder::new()
            .with_bounds(bounds.to_wry_rect())
            .with_url(url)
            .with_visible(true)
            .with_clipboard(true)
            .with_hotkeys_zoom(true)
            .with_accept_first_mouse(true)
            .build_as_child(parent)?;

        Ok(Self {
            webview,
            current_title: title,
        })
    }

    /// Update the webview's position and size within the parent window.
    pub fn set_bounds(&self, bounds: BrowserRect) {
        let _ = self.webview.set_bounds(bounds.to_wry_rect());
    }

    /// Navigate to a URL.
    pub fn navigate(&self, url: &str) {
        let _ = self.webview.load_url(url);
    }

    /// Get the current URL.
    pub fn url(&self) -> Option<String> {
        self.webview.url().ok()
    }

    /// Get the cached page title.
    pub fn title(&self) -> &str {
        &self.current_title
    }

    /// Show or hide the webview.
    pub fn set_visible(&self, visible: bool) {
        let _ = self.webview.set_visible(visible);
    }

    /// Focus the webview for keyboard input.
    pub fn focus(&self) {
        let _ = self.webview.focus();
    }

    /// Return focus to the parent window.
    pub fn focus_parent(&self) {
        let _ = self.webview.focus_parent();
    }

    /// Execute JavaScript in the webview.
    pub fn evaluate_script(&self, js: &str) {
        let _ = self.webview.evaluate_script(js);
    }

    /// Open the platform DevTools inspector.
    pub fn open_devtools(&self) {
        self.webview.open_devtools();
    }

    /// Close the platform DevTools inspector.
    pub fn close_devtools(&self) {
        self.webview.close_devtools();
    }

    /// Set the page zoom level.
    pub fn zoom(&self, scale_factor: f64) {
        let _ = self.webview.zoom(scale_factor);
    }
}
