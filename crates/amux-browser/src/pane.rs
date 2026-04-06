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
    /// Results from JS evaluations sent back via IPC handler.
    /// Key: request ID, Value: JSON string result.
    eval_results: std::collections::HashMap<String, String>,
    /// Console messages captured from the page.
    console_messages: Vec<String>,
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
            ..Default::default()
        }));

        let title_state = state.clone();
        let nav_state = state.clone();
        let ipc_state = state.clone();

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
            .with_ipc_handler(move |msg| {
                // Messages from JS: JSON with {type, id, data}
                let body = msg.body();
                if let Ok(mut s) = ipc_state.lock() {
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(body) {
                        match parsed.get("type").and_then(|t| t.as_str()) {
                            Some("eval_result") => {
                                if let (Some(id), Some(data)) = (
                                    parsed.get("id").and_then(|v| v.as_str()),
                                    parsed.get("data"),
                                ) {
                                    s.eval_results.insert(id.to_string(), data.to_string());
                                }
                            }
                            Some("console") => {
                                if let Some(msg) = parsed.get("message").and_then(|v| v.as_str()) {
                                    s.console_messages.push(msg.to_string());
                                    // Cap at 1000 messages
                                    if s.console_messages.len() > 1000 {
                                        s.console_messages.remove(0);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            })
            .build_as_child(parent)?;

        // Inject console capture script
        let _ = webview.evaluate_script(
            r#"(function(){
                const orig = {log: console.log, warn: console.warn, error: console.error, info: console.info};
                function wrap(level) {
                    return function() {
                        orig[level].apply(console, arguments);
                        try {
                            const msg = Array.from(arguments).map(a => typeof a === 'string' ? a : JSON.stringify(a)).join(' ');
                            window.ipc.postMessage(JSON.stringify({type:'console', level:level, message:'['+level+'] '+msg}));
                        } catch(e) {}
                    };
                }
                console.log = wrap('log');
                console.warn = wrap('warn');
                console.error = wrap('error');
                console.info = wrap('info');
            })()"#,
        );

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

    /// Execute JavaScript and capture the result via IPC bridge.
    /// The result will be available via `take_eval_result(id)`.
    pub fn evaluate_with_result(&self, id: &str, js: &str) {
        let escaped_id = id.replace('\\', "\\\\").replace('\'', "\\'");
        let script = format!(
            r#"(async function() {{
                try {{
                    const __result = await (async function() {{ {js} }})();
                    window.ipc.postMessage(JSON.stringify({{type:'eval_result', id:'{escaped_id}', data:__result}}));
                }} catch(e) {{
                    window.ipc.postMessage(JSON.stringify({{type:'eval_result', id:'{escaped_id}', data:{{"error": e.message}}}}));
                }}
            }})()"#
        );
        let _ = self.webview.evaluate_script(&script);
    }

    /// Take a pending evaluation result by ID (returns None if not yet available).
    pub fn take_eval_result(&self, id: &str) -> Option<String> {
        self.state.lock().ok()?.eval_results.remove(id)
    }

    /// Get visible page text (document.body.innerText).
    pub fn get_text(&self, id: &str) {
        self.evaluate_with_result(id, "return document.body ? document.body.innerText : ''");
    }

    /// Get DOM snapshot (document.documentElement.outerHTML).
    pub fn get_snapshot(&self, id: &str) {
        self.evaluate_with_result(
            id,
            "return document.documentElement ? document.documentElement.outerHTML : ''",
        );
    }

    /// Click at page coordinates.
    pub fn click_at(&self, x: f64, y: f64) {
        let _ = self
            .webview
            .evaluate_script(&format!("document.elementFromPoint({x},{y})?.click()"));
    }

    /// Type text into the currently focused element.
    pub fn type_text(&self, text: &str) {
        let escaped = text
            .replace('\\', "\\\\")
            .replace('\'', "\\'")
            .replace('\n', "\\n");
        let _ = self.webview.evaluate_script(&format!(
            r#"(function(){{
                const el = document.activeElement;
                if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable)) {{
                    const text = '{escaped}';
                    if (el.isContentEditable) {{
                        document.execCommand('insertText', false, text);
                    }} else {{
                        const start = el.selectionStart || 0;
                        const end = el.selectionEnd || 0;
                        el.value = el.value.substring(0, start) + text + el.value.substring(end);
                        el.selectionStart = el.selectionEnd = start + text.length;
                        el.dispatchEvent(new Event('input', {{bubbles: true}}));
                    }}
                }}
            }})()"#
        ));
    }

    /// Scroll the page by (dx, dy) pixels.
    pub fn scroll_by(&self, dx: f64, dy: f64) {
        let _ = self
            .webview
            .evaluate_script(&format!("window.scrollBy({dx},{dy})"));
    }

    /// Drain captured console messages.
    pub fn drain_console(&self) -> Vec<String> {
        self.state
            .lock()
            .ok()
            .map(|mut s| std::mem::take(&mut s.console_messages))
            .unwrap_or_default()
    }
}
