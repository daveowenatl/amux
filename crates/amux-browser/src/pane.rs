//! Browser pane — wraps a platform-native webview as a child view.
//!
//! Uses wry to embed WKWebView (macOS) or WebView2 (Windows) as a child
//! of the eframe/egui window. The webview is positioned and sized to fill
//! the pane's content area (below the tab bar).

use raw_window_handle::HasWindowHandle;
use std::sync::{Arc, Mutex};
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{Rect, WebViewBuilder};

/// Shared state updated by webview callbacks (title changes, navigation).
#[derive(Default)]
struct SharedState {
    title: String,
    url: String,
}

/// A browser pane backed by a platform-native webview.
pub struct BrowserPane {
    webview: wry::WebView,
    state: Arc<Mutex<SharedState>>,
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
        let state = Arc::new(Mutex::new(SharedState {
            title: String::new(),
            url: url.to_string(),
        }));

        let title_state = state.clone();
        let nav_state = state.clone();

        let webview = WebViewBuilder::new()
            .with_bounds(bounds.to_wry_rect())
            .with_url(url)
            .with_visible(true)
            .with_clipboard(true)
            .with_hotkeys_zoom(true)
            .with_accept_first_mouse(true)
            .with_document_title_changed_handler(move |title| {
                if let Ok(mut s) = title_state.lock() {
                    s.title = title;
                }
            })
            .with_navigation_handler(move |url| {
                if let Ok(mut s) = nav_state.lock() {
                    s.url = url;
                }
                true // allow all navigations
            })
            .build_as_child(parent)?;

        Ok(Self { webview, state })
    }

    /// Update the webview's position and size within the parent window.
    pub fn set_bounds(&self, bounds: BrowserRect) {
        let _ = self.webview.set_bounds(bounds.to_wry_rect());
    }

    /// Navigate to a URL.
    pub fn navigate(&self, url: &str) {
        if let Ok(mut s) = self.state.lock() {
            s.url = url.to_string();
        }
        let _ = self.webview.load_url(url);
    }

    /// Get the current URL (from callback-tracked state, falls back to webview).
    pub fn url(&self) -> Option<String> {
        if let Ok(s) = self.state.lock() {
            if !s.url.is_empty() {
                return Some(s.url.clone());
            }
        }
        self.webview.url().ok()
    }

    /// Get the page title (updated via document title change callback).
    pub fn title(&self) -> String {
        self.state
            .lock()
            .ok()
            .map(|s| s.title.clone())
            .unwrap_or_default()
    }

    /// Navigate back in history.
    pub fn go_back(&self) {
        let _ = self.webview.evaluate_script("window.history.back()");
    }

    /// Navigate forward in history.
    pub fn go_forward(&self) {
        let _ = self.webview.evaluate_script("window.history.forward()");
    }

    /// Reload the current page.
    pub fn reload(&self) {
        let _ = self.webview.evaluate_script("window.location.reload()");
    }

    /// Stop loading the current page.
    pub fn stop(&self) {
        let _ = self.webview.evaluate_script("window.stop()");
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
