# Browser IPC Hardening & Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close issues #112 and #123 by wiring up eval result retrieval, fixing robustness bugs, and polishing the browser IPC surface.

**Architecture:** Most browser RPC methods already exist from PR #122. The critical gap is that `browser.evaluate`, `browser.get-text`, and `browser.snapshot` return `{eval_id, status: "pending"}` but there's no way to poll for results. We add a `browser.get-eval-result` method and make the CLI poll automatically. Robustness fixes address panics, stale state, broken shortcuts, and minor quality issues.

**Tech Stack:** Rust, egui, wry (webview), serde_json, tokio (async CLI client)

---

### Task 1: Add `browser.get-eval-result` IPC method

The critical missing piece. Without this, `browser.evaluate`, `browser.get-text`, and `browser.snapshot` are useless — they return an eval_id but clients can't retrieve the result.

**Files:**
- Modify: `crates/amux-ipc/src/methods.rs:4-53` (add to METHODS array)
- Modify: `crates/amux-app/src/ipc_dispatch.rs` (add method handler after line 375)

- [ ] **Step 1: Register the method name**

In `crates/amux-ipc/src/methods.rs`, add `"browser.get-eval-result"` to the `METHODS` array after `"browser.snapshot"`:

```rust
    "browser.snapshot",
    "browser.get-eval-result",
    "browser.click",
```

- [ ] **Step 2: Add the dispatch handler**

In `crates/amux-app/src/ipc_dispatch.rs`, add the handler after the `browser.snapshot` block (after line 375):

```rust
            "browser.get-eval-result" => {
                #[derive(serde::Deserialize)]
                struct GetEvalResultParams {
                    eval_id: String,
                    #[serde(default)]
                    pane_id: Option<String>,
                }
                match serde_json::from_value::<GetEvalResultParams>(req.params.clone()) {
                    Ok(params) => {
                        match self.resolve_browser_pane_ref(params.pane_id.as_deref()) {
                            Some(browser) => {
                                match browser.take_eval_result(&params.eval_id) {
                                    Some(result) => {
                                        // result is a JSON string from the JS side
                                        let value = serde_json::from_str::<serde_json::Value>(&result)
                                            .unwrap_or(serde_json::Value::String(result));
                                        Response::ok(
                                            req.id.clone(),
                                            serde_json::json!({"status": "complete", "result": value}),
                                        )
                                    }
                                    None => Response::ok(
                                        req.id.clone(),
                                        serde_json::json!({"status": "pending"}),
                                    ),
                                }
                            }
                            None => {
                                Response::err(req.id.clone(), "not_found", "no browser pane found")
                            }
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
```

- [ ] **Step 3: Build and verify**

Run: `cargo clippy --workspace -- -D warnings`
Expected: Clean (no warnings)

- [ ] **Step 4: Commit**

```bash
git add crates/amux-ipc/src/methods.rs crates/amux-app/src/ipc_dispatch.rs
git commit -m "feat: add browser.get-eval-result IPC method (refs #112, #123)"
```

---

### Task 2: Add CLI polling for eval-based commands

The CLI commands `browser-eval`, `browser-text`, and `browser-snapshot` currently just print `{eval_id, status: "pending"}`. They should poll `browser.get-eval-result` until the result is ready, then print it.

**Files:**
- Modify: `crates/amux-cli/src/main.rs:320-343` (update BrowserEval, BrowserText, BrowserSnapshot handlers)

- [ ] **Step 1: Add a poll helper function**

In `crates/amux-cli/src/main.rs`, add this helper before the `main` function (or in a suitable location near the top):

```rust
/// Poll `browser.get-eval-result` until the result is ready or timeout.
async fn poll_eval_result(
    client: &amux_ipc::Client,
    eval_id: &str,
    pane_id: Option<&str>,
    timeout_ms: u64,
) -> anyhow::Result<amux_ipc::Response> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(timeout_ms);
    loop {
        let mut params = serde_json::json!({"eval_id": eval_id});
        if let Some(p) = pane_id {
            params["pane_id"] = serde_json::json!(p);
        }
        let resp = client.call("browser.get-eval-result", params).await?;
        if let Some(result) = &resp.result {
            if result.get("status").and_then(|s| s.as_str()) == Some("complete") {
                return Ok(resp);
            }
        }
        if start.elapsed() > timeout {
            return Ok(resp); // Return the "pending" response on timeout
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
```

- [ ] **Step 2: Update BrowserEval handler**

Replace the `Command::BrowserEval` handler (around line 320) with:

```rust
        Command::BrowserEval { script, pane } => {
            let mut params = serde_json::json!({"script": script});
            if let Some(ref p) = pane {
                params["pane_id"] = serde_json::json!(p);
            }
            let resp = client.call("browser.evaluate", params).await?;
            if !resp.ok {
                print_response(&resp, cli.json);
            } else if let Some(eval_id) = resp.result.as_ref()
                .and_then(|r| r.get("eval_id"))
                .and_then(|v| v.as_str())
            {
                let result = poll_eval_result(
                    &client,
                    eval_id,
                    pane.as_deref(),
                    5000,
                ).await?;
                if cli.json {
                    print_response(&result, true);
                } else if let Some(value) = result.result.as_ref()
                    .and_then(|r| r.get("result"))
                {
                    println!("{}", serde_json::to_string_pretty(value)?);
                } else {
                    print_response(&result, false);
                }
            } else {
                print_response(&resp, cli.json);
            }
        }
```

- [ ] **Step 3: Update BrowserText handler**

Replace the `Command::BrowserText` handler (around line 328) with:

```rust
        Command::BrowserText { pane } => {
            let params = match &pane {
                Some(p) => serde_json::json!({"pane_id": p}),
                None => serde_json::json!({}),
            };
            let resp = client.call("browser.get-text", params).await?;
            if !resp.ok {
                print_response(&resp, cli.json);
            } else if let Some(eval_id) = resp.result.as_ref()
                .and_then(|r| r.get("eval_id"))
                .and_then(|v| v.as_str())
            {
                let result = poll_eval_result(
                    &client,
                    eval_id,
                    pane.as_ref().map(|p| p.as_str()),
                    5000,
                ).await?;
                if cli.json {
                    print_response(&result, true);
                } else if let Some(value) = result.result.as_ref()
                    .and_then(|r| r.get("result"))
                    .and_then(|v| v.as_str())
                {
                    println!("{}", value);
                } else {
                    print_response(&result, false);
                }
            } else {
                print_response(&resp, cli.json);
            }
        }
```

- [ ] **Step 4: Update BrowserSnapshot handler**

Replace the `Command::BrowserSnapshot` handler (around line 336). Same pattern as BrowserText:

```rust
        Command::BrowserSnapshot { pane } => {
            let params = match &pane {
                Some(p) => serde_json::json!({"pane_id": p}),
                None => serde_json::json!({}),
            };
            let resp = client.call("browser.snapshot", params).await?;
            if !resp.ok {
                print_response(&resp, cli.json);
            } else if let Some(eval_id) = resp.result.as_ref()
                .and_then(|r| r.get("eval_id"))
                .and_then(|v| v.as_str())
            {
                let result = poll_eval_result(
                    &client,
                    eval_id,
                    pane.as_ref().map(|p| p.as_str()),
                    5000,
                ).await?;
                if cli.json {
                    print_response(&result, true);
                } else if let Some(value) = result.result.as_ref()
                    .and_then(|r| r.get("result"))
                    .and_then(|v| v.as_str())
                {
                    println!("{}", value);
                } else {
                    print_response(&result, false);
                }
            } else {
                print_response(&resp, cli.json);
            }
        }
```

- [ ] **Step 5: Build and verify**

Run: `cargo clippy --workspace -- -D warnings`
Expected: Clean

- [ ] **Step 6: Commit**

```bash
git add crates/amux-cli/src/main.rs
git commit -m "feat: CLI polls for eval results instead of returning pending (refs #112)"
```

---

### Task 3: Add `browser.screenshot` CLI command

Add a CLI subcommand for screenshots. The BrowserPane doesn't have a screenshot method yet, but we can implement it via JS (`document.documentElement.outerHTML` is already `browser.snapshot` — screenshots need a different approach via the webview's native capture or html2canvas). For now, use the wry webview's print-to-PDF or canvas capture.

Actually, looking at the wry API, screenshot capture requires `webview.screenshot()` which returns a PNG. Let's check if wry exposes this.

**Files:**
- Modify: `crates/amux-browser/src/pane.rs` (add screenshot method)
- Modify: `crates/amux-ipc/src/methods.rs` (register method)
- Modify: `crates/amux-app/src/ipc_dispatch.rs` (add handler)
- Modify: `crates/amux-cli/src/cli.rs` (add CLI subcommand)
- Modify: `crates/amux-cli/src/main.rs` (add handler)

- [ ] **Step 1: Check wry screenshot API**

Read `crates/amux-browser/src/pane.rs` to find the webview type and check if `wry::WebView` has a `screenshot` method. If not, implement via JS canvas capture using `html2canvas` or `window.devicePixelRatio` + canvas approach.

- [ ] **Step 2: Add screenshot method to BrowserPane**

In `crates/amux-browser/src/pane.rs`, add:

```rust
    /// Capture a screenshot of the current page as PNG bytes.
    /// Uses evaluate_with_result to run canvas capture in the page context.
    pub fn screenshot(&self, id: &str) {
        let js = r#"
            (async () => {
                try {
                    const canvas = document.createElement('canvas');
                    const ctx = canvas.getContext('2d');
                    canvas.width = window.innerWidth * window.devicePixelRatio;
                    canvas.height = window.innerHeight * window.devicePixelRatio;
                    ctx.scale(window.devicePixelRatio, window.devicePixelRatio);
                    // Use html2canvas if available, otherwise return viewport dimensions
                    if (typeof html2canvas !== 'undefined') {
                        const c = await html2canvas(document.body);
                        return c.toDataURL('image/png');
                    }
                    // Fallback: return page metadata instead of actual screenshot
                    return JSON.stringify({
                        error: 'screenshot requires html2canvas',
                        url: window.location.href,
                        title: document.title,
                        viewport: { width: window.innerWidth, height: window.innerHeight }
                    });
                } catch(e) {
                    return JSON.stringify({ error: e.message });
                }
            })()
        "#;
        self.evaluate_with_result(id, js);
    }
```

Note: This is a JS-based approach. A native wry screenshot (if available in the version we use) would be better. The implementer should check `self.webview` for a `screenshot()` method and prefer it if available.

- [ ] **Step 3: Register the IPC method**

In `crates/amux-ipc/src/methods.rs`, add `"browser.screenshot"` to METHODS after `"browser.get-eval-result"`.

- [ ] **Step 4: Add IPC handler**

In `crates/amux-app/src/ipc_dispatch.rs`, add after the `browser.snapshot` handler:

```rust
            "browser.screenshot" => {
                let eval_id = format!("screenshot_{}", req.id);
                let pane_id_str = Self::pane_id_param(&req.params);
                match self.resolve_browser_pane(pane_id_str.as_deref()) {
                    Some(browser) => {
                        browser.screenshot(&eval_id);
                        Response::ok(
                            req.id.clone(),
                            serde_json::json!({"eval_id": eval_id, "status": "pending"}),
                        )
                    }
                    None => Response::err(req.id.clone(), "not_found", "no browser pane found"),
                }
            }
```

- [ ] **Step 5: Add CLI subcommand**

In `crates/amux-cli/src/cli.rs`, add:

```rust
    /// Capture a screenshot of the browser page.
    BrowserScreenshot {
        /// Write screenshot to file (default: stdout as base64)
        #[arg(long)]
        output: Option<String>,
        /// Target pane ID
        #[arg(long)]
        pane: Option<PaneId>,
    },
```

- [ ] **Step 6: Add CLI handler**

In `crates/amux-cli/src/main.rs`, add handler:

```rust
        Command::BrowserScreenshot { output, pane } => {
            let params = match &pane {
                Some(p) => serde_json::json!({"pane_id": p}),
                None => serde_json::json!({}),
            };
            let resp = client.call("browser.screenshot", params).await?;
            if !resp.ok {
                print_response(&resp, false);
            } else if let Some(eval_id) = resp.result.as_ref()
                .and_then(|r| r.get("eval_id"))
                .and_then(|v| v.as_str())
            {
                let result = poll_eval_result(
                    &client,
                    eval_id,
                    pane.as_ref().map(|p| p.as_str()),
                    10000, // screenshots may take longer
                ).await?;
                if let Some(data) = result.result.as_ref()
                    .and_then(|r| r.get("result"))
                    .and_then(|v| v.as_str())
                {
                    if let Some(path) = output {
                        // Strip data URL prefix if present
                        let b64 = data.strip_prefix("data:image/png;base64,").unwrap_or(data);
                        use base64::Engine;
                        let bytes = base64::engine::general_purpose::STANDARD.decode(b64)?;
                        std::fs::write(&path, &bytes)?;
                        println!("Screenshot saved to {}", path);
                    } else {
                        println!("{}", data);
                    }
                } else {
                    print_response(&result, cli.json);
                }
            }
        }
```

- [ ] **Step 7: Build and verify**

Run: `cargo clippy --workspace -- -D warnings`

- [ ] **Step 8: Commit**

```bash
git add crates/amux-browser/src/pane.rs crates/amux-ipc/src/methods.rs \
  crates/amux-app/src/ipc_dispatch.rs crates/amux-cli/src/cli.rs crates/amux-cli/src/main.rs
git commit -m "feat: add browser.screenshot IPC method and CLI command (refs #112)"
```

---

### Task 4: Fix `active_surface()` panic when no terminal surfaces exist

`ManagedPane::active_surface()` panics with `expect("active_surface() called with no terminal surfaces")` if all terminal tabs are closed and only browser tabs remain.

**Files:**
- Modify: `crates/amux-app/src/managed_pane.rs:234-267` (make active_surface return Option)

- [ ] **Step 1: Change active_surface to return Option**

In `crates/amux-app/src/managed_pane.rs`, change `active_surface()`:

```rust
    pub(crate) fn active_surface(&self) -> Option<&PaneSurface> {
        if let TabEntry::Terminal(s) = &self.tabs[self.active_tab_idx] {
            return Some(s);
        }
        // Fallback: find the last terminal surface
        self.tabs
            .iter()
            .filter_map(|t| t.as_surface())
            .next_back()
    }
```

And `active_surface_mut()`:

```rust
    pub(crate) fn active_surface_mut(&mut self) -> Option<&mut PaneSurface> {
        if let TabEntry::Terminal(s) = &mut self.tabs[self.active_tab_idx] {
            return Some(s);
        }
        self.tabs
            .iter_mut()
            .filter_map(|t| t.as_surface_mut())
            .next_back()
    }
```

- [ ] **Step 2: Fix all call sites**

Search for `.active_surface()` and `.active_surface_mut()` across the codebase. Each call site needs to handle `Option`:
- Some can use `if let Some(sf) = ...` and skip when None
- Some can use `.active_surface()?` in functions that return Option/Result
- The IPC dispatch surface.create path already has the `as_terminal_mut().unwrap()` fix from PR #140

Run: `cargo build --workspace 2>&1` and fix each compile error.

- [ ] **Step 3: Build and verify**

Run: `cargo clippy --workspace -- -D warnings && cargo test --workspace`

- [ ] **Step 4: Commit**

```bash
git add -u
git commit -m "fix: active_surface() returns Option instead of panicking (refs #123)"
```

---

### Task 5: Fix `is_cmd` on non-macOS for browser shortcuts

On non-macOS, `is_cmd = ctrl && shift`. This means `is_cmd && !modifiers.shift` is always `false`, breaking Find (`Cmd+F`), Select All (`Cmd+A`), and Clear Scrollback (`Cmd+Shift+X`) for browser tabs.

**Files:**
- Modify: `crates/amux-app/src/input.rs:62-64`

- [ ] **Step 1: Fix is_cmd definition**

In `crates/amux-app/src/input.rs`, change the non-macOS `is_cmd`:

```rust
                #[cfg(target_os = "macos")]
                let is_cmd = modifiers.mac_cmd || modifiers.command;
                #[cfg(not(target_os = "macos"))]
                let is_cmd = modifiers.ctrl;
```

This makes `Ctrl+C` = Copy, `Ctrl+F` = Find, etc. on Linux/Windows, matching standard platform conventions.

- [ ] **Step 2: Verify terminal shortcuts still work**

The terminal shortcuts (Copy, Paste, Find, Select All) use `is_cmd` which was previously `ctrl+shift`. Changing to just `ctrl` means:
- `Ctrl+C` on non-macOS now triggers Copy (with selection). This conflicts with SIGINT in terminals.
- Need to verify the existing `is_copy` logic: it only fires when there's a selection (`!selection.is_empty()`), so bare `Ctrl+C` still sends SIGINT. This is correct.

Run: `cargo clippy --workspace -- -D warnings`

- [ ] **Step 3: Commit**

```bash
git add crates/amux-app/src/input.rs
git commit -m "fix: is_cmd uses Ctrl (not Ctrl+Shift) on non-macOS platforms (refs #123)"
```

---

### Task 6: Fix deferred browser creation using wrong pane

`queue_browser_pane()` only stores the URL. When `create_pending_browser_panes()` processes it later, it attaches to `focused_pane_id()` which may differ from the pane that triggered the action.

**Files:**
- Modify: `crates/amux-app/src/main.rs:163,286-288,323-394` (queue PaneId alongside URL)

- [ ] **Step 1: Change pending_browser_panes type**

In `crates/amux-app/src/main.rs`, change the field at line 163:

```rust
    pending_browser_panes: Vec<(PaneId, String)>,  // (originating_pane_id, url)
```

- [ ] **Step 2: Update queue_browser_pane**

```rust
    fn queue_browser_pane(&mut self, pane_id: PaneId, url: String) {
        self.pending_browser_panes.push((pane_id, url));
    }
```

- [ ] **Step 3: Update create_pending_browser_panes**

In the processing loop, use the stored `pane_id` instead of `self.focused_pane_id()`:

```rust
    for (originating_pane_id, url) in new_browser_panes {
        // ... use originating_pane_id instead of self.focused_pane_id()
    }
```

- [ ] **Step 4: Update all call sites of queue_browser_pane**

Search for `queue_browser_pane(` and add the pane_id argument. Call sites include:
- Menu actions (main.rs ~line 210): use `self.focused_pane_id()`
- Input shortcuts (input.rs ~line 241): use the focused pane from context
- Pane render right-click (pane_render.rs ~line 552): use `pane_id` from render context

- [ ] **Step 5: Build and verify**

Run: `cargo clippy --workspace -- -D warnings`

- [ ] **Step 6: Commit**

```bash
git add -u
git commit -m "fix: queue_browser_pane stores originating pane ID (refs #123)"
```

---

### Task 7: Fix `is_url_like` to handle `localhost:port` and whitespace

`is_url_like` in `crates/amux-core/src/config.rs` misses `localhost:3000` and allows tabs/newlines.

**Files:**
- Modify: `crates/amux-core/src/config.rs:75-89`

- [ ] **Step 1: Write tests**

Add tests at the bottom of `crates/amux-core/src/config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_url_like_schemes() {
        assert!(is_url_like("http://example.com"));
        assert!(is_url_like("https://example.com"));
        assert!(is_url_like("file:///tmp/test.html"));
    }

    #[test]
    fn is_url_like_domains() {
        assert!(is_url_like("example.com"));
        assert!(is_url_like("docs.rs"));
    }

    #[test]
    fn is_url_like_localhost() {
        assert!(is_url_like("localhost:3000"));
        assert!(is_url_like("localhost:8080/api"));
        assert!(is_url_like("127.0.0.1:9090"));
    }

    #[test]
    fn is_url_like_rejects_search() {
        assert!(!is_url_like("how to write rust"));
        assert!(!is_url_like("hello world"));
        assert!(!is_url_like(""));
        assert!(!is_url_like("   "));
    }

    #[test]
    fn is_url_like_rejects_whitespace() {
        assert!(!is_url_like("example\t.com"));
        assert!(!is_url_like("example\n.com"));
        assert!(!is_url_like("hello\tworld"));
    }
}
```

- [ ] **Step 2: Run tests to see failures**

Run: `cargo test -p amux-core`
Expected: `is_url_like_localhost` and `is_url_like_rejects_whitespace` fail.

- [ ] **Step 3: Fix the function**

```rust
pub fn is_url_like(input: &str) -> bool {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Reject any whitespace (spaces, tabs, newlines)
    if trimmed.chars().any(|c| c.is_whitespace()) {
        return false;
    }
    // Already has a scheme
    if trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("file://")
    {
        return true;
    }
    // localhost or 127.0.0.1 with optional port
    if trimmed.starts_with("localhost") || trimmed.starts_with("127.0.0.1") {
        return true;
    }
    // Has a dot → likely a domain
    trimmed.contains('.')
}
```

- [ ] **Step 4: Run tests to verify**

Run: `cargo test -p amux-core`
Expected: All pass.

- [ ] **Step 5: Commit**

```bash
git add crates/amux-core/src/config.rs
git commit -m "fix: is_url_like handles localhost:port and rejects all whitespace (refs #123)"
```

---

### Task 8: Cap favicon cache size

The `favicon_cache: HashMap<String, egui::TextureHandle>` grows unbounded. Add a simple size cap.

**Files:**
- Modify: `crates/amux-app/src/tab_icons.rs:227` (add eviction after insert)

- [ ] **Step 1: Add size cap after favicon insert**

In `crates/amux-app/src/tab_icons.rs`, after the `self.favicon_cache.insert(url, tex)` line (~227), add:

```rust
                self.favicon_cache.insert(url, tex);

                // Cap cache at 256 entries — evict oldest by arbitrary key.
                // Textures will be re-fetched on next access if needed.
                while self.favicon_cache.len() > 256 {
                    if let Some(key) = self.favicon_cache.keys().next().cloned() {
                        self.favicon_cache.remove(&key);
                    }
                }
```

- [ ] **Step 2: Build and verify**

Run: `cargo clippy --workspace -- -D warnings`

- [ ] **Step 3: Commit**

```bash
git add crates/amux-app/src/tab_icons.rs
git commit -m "fix: cap favicon cache at 256 entries to prevent unbounded growth (refs #123)"
```

---

### Task 9: Fix favicon URL escaping

The favicon fetch JS injects the URL using single-quote escaping only. Backslashes and newlines can break the injected JS.

**Files:**
- Modify: `crates/amux-app/src/tab_icons.rs` (find the favicon fetch JS injection)

- [ ] **Step 1: Find and fix the escaping**

Search for the favicon URL injection in `tab_icons.rs`. Replace manual single-quote escaping with `serde_json::to_string()`:

```rust
let escaped_url = serde_json::to_string(favicon_url).unwrap_or_default();
// escaped_url is already wrapped in quotes, so use it directly in the JS template
let js = format!(r#"
    (async () => {{
        try {{
            const resp = await fetch({escaped_url});
            // ... rest of fetch logic
        }} catch(e) {{}}
    }})()
"#);
```

- [ ] **Step 2: Build and verify**

Run: `cargo clippy --workspace -- -D warnings`

- [ ] **Step 3: Commit**

```bash
git add crates/amux-app/src/tab_icons.rs
git commit -m "fix: use serde_json for favicon URL escaping in JS injection (refs #123)"
```

---

### Task 10: Carry full SavedBrowserTab through restore queue

`pending_browser_restores` only stores `(PaneId, PaneId, String)` — losing `zoom_level` and `profile` from `SavedBrowserTab`.

**Files:**
- Modify: `crates/amux-app/src/main.rs:165` (change tuple type)
- Modify: `crates/amux-app/src/startup.rs:244,313,389` (update the type and push)
- Modify: `crates/amux-app/src/main.rs:374` (update the drain/create loop)

- [ ] **Step 1: Change the type**

In `crates/amux-app/src/main.rs:165`:

```rust
    pending_browser_restores: Vec<(PaneId, amux_session::SavedBrowserTab)>,
```

In `crates/amux-app/src/startup.rs:244`:

```rust
    pub(crate) pending_browser_restores: Vec<(PaneId, amux_session::SavedBrowserTab)>,
```

- [ ] **Step 2: Update the push in startup.rs**

At `startup.rs:389`, change from:

```rust
pending_browser_restores.push((pane_id, bt.pane_id, bt.url.clone()));
```

To:

```rust
pending_browser_restores.push((pane_id, bt.clone()));
```

(May need to derive `Clone` on `SavedBrowserTab` if not already derived.)

- [ ] **Step 3: Update create_pending_browser_panes**

In `main.rs:374`, update the drain loop to use `bt.url`, `bt.zoom_level`, `bt.profile` when creating the BrowserPane.

- [ ] **Step 4: Build and verify**

Run: `cargo clippy --workspace -- -D warnings`

- [ ] **Step 5: Commit**

```bash
git add -u
git commit -m "fix: carry full SavedBrowserTab through restore queue (refs #123)"
```

---

### Task 11: Minor fixes batch

Group the remaining small fixes into one commit.

**Files:**
- Modify: `crates/amux-app/src/ipc_dispatch.rs` (silent create_dir_all)
- Modify: `crates/amux-app/src/pane_render.rs` (stale active_is_browser)

- [ ] **Step 1: Fix silent create_dir_all failures**

Search for `let _ = std::fs::create_dir_all` in the codebase and replace with logged warnings:

```rust
if let Err(e) = std::fs::create_dir_all(&path) {
    tracing::warn!("Failed to create directory {}: {e}", path.display());
}
```

- [ ] **Step 2: Fix stale active_is_browser in pane_render**

In `crates/amux-app/src/pane_render.rs`, the `active_is_browser` snapshot is computed at the top of the method. If click-driven mutations change the active tab, later code uses stale values. Move the snapshot computation to after mutation points, or re-read after mutations.

- [ ] **Step 3: Build and verify**

Run: `cargo clippy --workspace -- -D warnings && cargo test --workspace`

- [ ] **Step 4: Commit**

```bash
git add -u
git commit -m "fix: log create_dir_all failures, recompute active_is_browser after mutations (refs #123)"
```

---

### Task 12: Final verification and PR

- [ ] **Step 1: Full check**

```bash
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

- [ ] **Step 2: Create PR**

```bash
gh pr create --title "Browser IPC hardening and completion (fixes #112, #123)" --body "..."
```

---

## Items from #123 NOT addressed in this plan

These are either already fixed or not worth fixing:

| Item | Status |
|------|--------|
| Console message capping is O(n) Vec::remove(0) | **Already fixed** — uses VecDeque with pop_front() |
| search() recomputes SystemTime::now() per entry | Could not locate this pattern — may have been fixed |
| Legacy saved_pane.browser field | Low priority — backwards compat field, defer |
| Edit > Copy / Select All no-op for egui text fields | Complex egui integration — defer to separate issue |
| Focus change not mirrored on terminal/browser toggle | Needs careful testing — defer to separate issue |

## Items from #112 NOT addressed in this plan

| Item | Status |
|------|--------|
| All 10 RPC methods | **8 already exist.** Screenshot added in Task 3. get-eval-result added in Task 1. |
| CLI wrappers | **Most exist.** Screenshot CLI added in Task 3. Eval/text/snapshot CLIs upgraded to poll in Task 2. |
