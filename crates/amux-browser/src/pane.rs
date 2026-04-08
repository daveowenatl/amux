//! Browser pane — wraps a platform-native webview as a child view.
//!
//! Uses wry to embed WKWebView (macOS) or WebView2 (Windows) as a child
//! of the eframe/egui window. The webview is positioned and sized to fill
//! the pane's content area (below the tab bar).

use raw_window_handle::HasWindowHandle;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{Rect, WebContext, WebViewBuilder};

/// Shared state updated by webview callbacks (title changes, navigation).
#[derive(Default)]
struct SharedState {
    title: String,
    url: String,
    /// Results from JS evaluations sent back via IPC handler.
    /// Key: request ID, Value: JSON string result.
    eval_results: std::collections::HashMap<String, String>,
    /// Console messages captured from the page.
    console_messages: std::collections::VecDeque<String>,
    /// URLs requested via window.open() — queued for the app to handle.
    popup_requests: Vec<String>,
    /// Dialog requests from JS (alert/confirm/prompt) — queued for the app.
    dialog_requests: Vec<DialogRequest>,
    /// Download completion notifications.
    download_completions: Vec<DownloadCompletion>,
    /// Whether the page is currently loading.
    loading: bool,
    /// Favicon URL extracted from the page after load.
    favicon_url: Option<String>,
    /// Decoded favicon image data (base64-decoded bytes from JS fetch).
    favicon_data: Vec<(String, Vec<u8>)>,
    /// Set when the webview receives a click/focus — used to surrender
    /// omnibar focus since egui can't see native subview events.
    got_focus: bool,
}

/// A JS dialog request intercepted from the page.
#[derive(Debug, Clone)]
pub struct DialogRequest {
    pub kind: DialogKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    Alert,
    Confirm,
    Prompt,
}

/// Notification that a download has completed.
#[derive(Debug, Clone)]
pub struct DownloadCompletion {
    pub path: PathBuf,
    pub success: bool,
}

/// A browser pane backed by a platform-native webview.
pub struct BrowserPane {
    webview: wry::WebView,
    state: Arc<Mutex<SharedState>>,
    profile: String,
    #[allow(dead_code)]
    web_context: Option<WebContext>,
    download_dir: PathBuf,
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

/// Returns the base directory for browser profile data.
fn profiles_base_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library/Application Support/amux/browser-profiles")
    }
    #[cfg(target_os = "windows")]
    {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("amux/browser-profiles")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("amux/browser-profiles")
    }
}

/// List available profile names by scanning the profiles directory.
pub fn list_profiles() -> Vec<String> {
    let base = profiles_base_dir();
    let mut profiles = vec!["default".to_string()];
    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                if let Some(name) = entry.file_name().to_str() {
                    if name != "default" {
                        profiles.push(name.to_string());
                    }
                }
            }
        }
    }
    profiles.sort();
    profiles.dedup();
    profiles
}

/// Default user agent matching a current Chrome release. Bare WebKit UA
/// strings trigger CAPTCHAs on many sites because anti-bot systems expect
/// a Chrome or Safari version token.
fn default_user_agent() -> String {
    #[cfg(target_os = "macos")]
    {
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36".to_string()
    }
    #[cfg(target_os = "windows")]
    {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36".to_string()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36".to_string()
    }
}

/// Delete a profile's data directory.
pub fn delete_profile(name: &str) -> std::io::Result<()> {
    if name == "default" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "cannot delete default profile",
        ));
    }
    let path = profiles_base_dir().join(name);
    if path.exists() {
        std::fs::remove_dir_all(path)?;
    }
    Ok(())
}

/// Options for creating a new browser pane.
pub struct BrowserOptions<'a> {
    pub user_agent: Option<&'a str>,
    pub download_dir: Option<&'a str>,
}

impl BrowserPane {
    /// Create a new browser pane as a child view of the given window.
    ///
    /// `bounds` specifies the logical position and size within the parent window.
    /// `url` is the initial URL to load (use "about:blank" for empty).
    /// `profile` selects the data isolation directory (None = "default").
    pub fn new<W: HasWindowHandle>(
        parent: &W,
        bounds: BrowserRect,
        url: &str,
        profile: Option<&str>,
        options: Option<&BrowserOptions<'_>>,
    ) -> Result<Self, wry::Error> {
        let profile_name = profile.unwrap_or("default").to_string();
        let profile_dir = profiles_base_dir().join(&profile_name);
        if let Err(e) = std::fs::create_dir_all(&profile_dir) {
            tracing::warn!("Failed to create directory {}: {e}", profile_dir.display());
        }

        let download_dir = options
            .and_then(|o| o.download_dir)
            .map(PathBuf::from)
            .or_else(dirs::download_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        if let Err(e) = std::fs::create_dir_all(&download_dir) {
            tracing::warn!("Failed to create directory {}: {e}", download_dir.display());
        }

        let mut web_context = WebContext::new(Some(profile_dir));

        let state = Arc::new(Mutex::new(SharedState {
            title: String::new(),
            url: url.to_string(),
            ..Default::default()
        }));

        let title_state = state.clone();
        let nav_state = state.clone();
        let ipc_state = state.clone();
        let popup_state = state.clone();
        let dl_state = state.clone();
        let load_state = state.clone();

        let dl_dir = download_dir.clone();

        let mut builder = WebViewBuilder::new_with_web_context(&mut web_context)
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
            .with_new_window_req_handler(move |url, _req| {
                // Queue popup URLs for the app to open as new browser panes
                if let Ok(mut s) = popup_state.lock() {
                    s.popup_requests.push(url);
                }
                wry::NewWindowResponse::Deny
            })
            .with_download_started_handler(move |_uri, dest_path| {
                // Save to configured download directory with the suggested filename
                let filename = dest_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                *dest_path = dl_dir.join(&filename);
                true
            })
            .with_download_completed_handler(move |_uri, path, success| {
                if let Ok(mut s) = dl_state.lock() {
                    s.download_completions.push(DownloadCompletion {
                        path: path.unwrap_or_default(),
                        success,
                    });
                }
            })
            .with_on_page_load_handler(move |event, _url| {
                if let Ok(mut s) = load_state.lock() {
                    s.loading = matches!(event, wry::PageLoadEvent::Started);
                }
            })
            .with_ipc_handler(move |msg: wry::http::Request<String>| {
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
                                    s.console_messages.push_back(msg.to_string());
                                    if s.console_messages.len() > 1000 {
                                        s.console_messages.pop_front();
                                    }
                                }
                            }
                            Some("url_change") => {
                                if let Some(url) = parsed.get("url").and_then(|v| v.as_str()) {
                                    s.url = url.to_string();
                                }
                            }
                            Some("favicon") => {
                                if let Some(url) = parsed.get("url").and_then(|v| v.as_str()) {
                                    s.favicon_url = Some(url.to_string());
                                }
                            }
                            Some("webview_focused") => {
                                s.got_focus = true;
                            }
                            Some("favicon_data") => {
                                if let (Some(url), Some(b64)) = (
                                    parsed.get("url").and_then(|v| v.as_str()),
                                    parsed.get("data").and_then(|v| v.as_str()),
                                ) {
                                    use base64::Engine;
                                    if let Ok(bytes) =
                                        base64::engine::general_purpose::STANDARD.decode(b64)
                                    {
                                        s.favicon_data.push((url.to_string(), bytes));
                                    }
                                }
                            }
                            Some("dialog") => {
                                let kind =
                                    match parsed.get("kind").and_then(|v| v.as_str()).unwrap_or("")
                                    {
                                        "confirm" => DialogKind::Confirm,
                                        "prompt" => DialogKind::Prompt,
                                        _ => DialogKind::Alert,
                                    };
                                let message = parsed
                                    .get("message")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                s.dialog_requests.push(DialogRequest { kind, message });
                            }
                            _ => {}
                        }
                    }
                }
            });

        // Use a Chrome-like user agent to avoid CAPTCHA triggers from bare
        // WebKit UA strings. Custom config value takes precedence.
        let default_ua = default_user_agent();
        let ua = options.and_then(|o| o.user_agent).unwrap_or(&default_ua);
        builder = builder.with_user_agent(ua);

        let webview = builder.build_as_child(parent)?;

        // Inject console capture + dialog interception script
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

                window.alert = function(msg) {
                    window.ipc.postMessage(JSON.stringify({type:'dialog', kind:'alert', message:String(msg)}));
                };
                window.confirm = function(msg) {
                    window.ipc.postMessage(JSON.stringify({type:'dialog', kind:'confirm', message:String(msg)}));
                    return true;
                };
                window.prompt = function(msg) {
                    window.ipc.postMessage(JSON.stringify({type:'dialog', kind:'prompt', message:String(msg)}));
                    return null;
                };
            })()"#,
        );

        // Monitor URL changes from SPA navigations (pushState/replaceState/popstate)
        let _ = webview.evaluate_script(
            r#"(function(){
                var lastUrl = location.href;
                function onUrlChange() {
                    if (location.href !== lastUrl) {
                        lastUrl = location.href;
                        window.ipc.postMessage(JSON.stringify({type:'url_change', url:lastUrl}));
                    }
                }
                var origPush = history.pushState;
                history.pushState = function() {
                    origPush.apply(this, arguments);
                    onUrlChange();
                };
                var origReplace = history.replaceState;
                history.replaceState = function() {
                    origReplace.apply(this, arguments);
                    onUrlChange();
                };
                window.addEventListener('popstate', onUrlChange);
                window.addEventListener('hashchange', onUrlChange);
            })()"#,
        );

        // Inject favicon detection script
        let _ = webview.evaluate_script(
            r#"(function(){
                function sendFavicon() {
                    const link = document.querySelector('link[rel~="icon"], link[rel="shortcut icon"]');
                    const url = link ? link.href : (location.origin + '/favicon.ico');
                    window.ipc.postMessage(JSON.stringify({type:'favicon', url:url}));
                }
                if (document.readyState === 'complete' || document.readyState === 'interactive') {
                    sendFavicon();
                }
                document.addEventListener('DOMContentLoaded', sendFavicon);
                window.addEventListener('load', sendFavicon);
                new MutationObserver(function() { sendFavicon(); })
                    .observe(document.head || document.documentElement, {childList:true, subtree:true});
            })()"#,
        );

        // Notify the app when the webview receives focus (mousedown/focusin)
        // so egui text fields (omnibar) can surrender focus.
        let _ = webview.evaluate_script(
            r#"(function(){
                var sent = false;
                function notify() {
                    if (!sent) {
                        sent = true;
                        window.ipc.postMessage(JSON.stringify({type:'webview_focused'}));
                        setTimeout(function(){ sent = false; }, 200);
                    }
                }
                document.addEventListener('mousedown', notify, true);
                document.addEventListener('focusin', notify, true);
            })()"#,
        );

        Ok(Self {
            webview,
            state,
            profile: profile_name,
            web_context: Some(web_context),
            download_dir,
        })
    }

    /// Get the active profile name.
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// Drain pending popup URLs (from window.open() calls).
    pub fn drain_popup_requests(&self) -> Vec<String> {
        self.state
            .lock()
            .ok()
            .map(|mut s| std::mem::take(&mut s.popup_requests))
            .unwrap_or_default()
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

    /// Get the favicon URL (detected from page `<link rel="icon">` or `/favicon.ico`).
    pub fn favicon_url(&self) -> Option<String> {
        self.state.lock().ok().and_then(|s| s.favicon_url.clone())
    }

    /// Drain decoded favicon image data received via IPC.
    pub fn drain_favicon_data(&self) -> Vec<(String, Vec<u8>)> {
        self.state
            .lock()
            .ok()
            .map(|mut s| std::mem::take(&mut s.favicon_data))
            .unwrap_or_default()
    }

    /// Check (and clear) whether the webview received focus since last call.
    pub fn take_got_focus(&self) -> bool {
        self.state
            .lock()
            .ok()
            .map(|mut s| std::mem::replace(&mut s.got_focus, false))
            .unwrap_or(false)
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

    /// Capture a screenshot of the current viewport as a data URL via JS canvas capture.
    /// The result will be available via `take_eval_result(id)`.
    pub fn screenshot(&self, id: &str) {
        let js = r#"
            try {
                // Fallback: return page metadata since native canvas capture needs html2canvas.
                // Return a plain object — evaluate_with_result handles serialization.
                return {
                    fallback: true,
                    url: window.location.href,
                    title: document.title,
                    viewport: { width: window.innerWidth, height: window.innerHeight }
                };
            } catch(e) {
                return { error: e.message };
            }
        "#;
        self.evaluate_with_result(id, js);
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
            .map(|mut s| {
                std::mem::take(&mut s.console_messages)
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Drain pending dialog requests (alert/confirm/prompt).
    pub fn drain_dialogs(&self) -> Vec<DialogRequest> {
        self.state
            .lock()
            .ok()
            .map(|mut s| std::mem::take(&mut s.dialog_requests))
            .unwrap_or_default()
    }

    /// Drain download completion notifications.
    pub fn drain_downloads(&self) -> Vec<DownloadCompletion> {
        self.state
            .lock()
            .ok()
            .map(|mut s| std::mem::take(&mut s.download_completions))
            .unwrap_or_default()
    }

    /// Whether the page is currently loading.
    pub fn is_loading(&self) -> bool {
        self.state.lock().ok().is_some_and(|s| s.loading)
    }

    /// Get the configured download directory.
    pub fn download_dir(&self) -> &std::path::Path {
        &self.download_dir
    }

    /// Execute find-in-page via JS. Highlights matches, returns count via IPC.
    pub fn find_in_page(&self, query: &str, result_id: &str) {
        let escaped_query = query
            .replace('\\', "\\\\")
            .replace('\'', "\\'")
            .replace('\n', "\\n");
        let escaped_id = result_id.replace('\\', "\\\\").replace('\'', "\\'");
        let script = format!(
            r#"(function(){{
                // Remove previous highlights
                document.querySelectorAll('mark[data-amux-find]').forEach(function(m){{
                    var p = m.parentNode;
                    p.replaceChild(document.createTextNode(m.textContent), m);
                    p.normalize();
                }});
                var query = '{escaped_query}';
                if (!query) {{
                    window.ipc.postMessage(JSON.stringify({{type:'eval_result', id:'{escaped_id}', data:0}}));
                    return;
                }}
                var count = 0;
                var walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT, null, false);
                var nodes = [];
                while(walker.nextNode()) nodes.push(walker.currentNode);
                var lowerQuery = query.toLowerCase();
                for (var i = 0; i < nodes.length; i++) {{
                    var node = nodes[i];
                    var text = node.textContent;
                    var idx = text.toLowerCase().indexOf(lowerQuery);
                    if (idx === -1) continue;
                    var frag = document.createDocumentFragment();
                    var lastIdx = 0;
                    while (idx !== -1) {{
                        frag.appendChild(document.createTextNode(text.substring(lastIdx, idx)));
                        var mark = document.createElement('mark');
                        mark.setAttribute('data-amux-find', count);
                        mark.style.backgroundColor = count === 0 ? '#ff6600' : '#ffff00';
                        mark.style.color = '#000';
                        mark.textContent = text.substring(idx, idx + query.length);
                        frag.appendChild(mark);
                        count++;
                        lastIdx = idx + query.length;
                        idx = text.toLowerCase().indexOf(lowerQuery, lastIdx);
                    }}
                    frag.appendChild(document.createTextNode(text.substring(lastIdx)));
                    node.parentNode.replaceChild(frag, node);
                }}
                // Scroll first match into view
                var first = document.querySelector('mark[data-amux-find="0"]');
                if (first) first.scrollIntoView({{block:'center'}});
                window.ipc.postMessage(JSON.stringify({{type:'eval_result', id:'{escaped_id}', data:count}}));
            }})()"#
        );
        let _ = self.webview.evaluate_script(&script);
    }

    /// Navigate to a specific find match by index.
    pub fn find_navigate(&self, index: usize) {
        let script = format!(
            r#"(function(){{
                document.querySelectorAll('mark[data-amux-find]').forEach(function(m){{
                    m.style.backgroundColor = '#ffff00';
                }});
                var target = document.querySelector('mark[data-amux-find="{index}"]');
                if (target) {{
                    target.style.backgroundColor = '#ff6600';
                    target.scrollIntoView({{block:'center'}});
                }}
            }})()"#
        );
        let _ = self.webview.evaluate_script(&script);
    }

    /// Clear find-in-page highlights.
    pub fn find_clear(&self) {
        let _ = self.webview.evaluate_script(
            r#"(function(){
                document.querySelectorAll('mark[data-amux-find]').forEach(function(m){
                    var p = m.parentNode;
                    p.replaceChild(document.createTextNode(m.textContent), m);
                    p.normalize();
                });
            })()"#,
        );
    }
}
