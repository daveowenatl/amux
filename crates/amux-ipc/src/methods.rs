use serde::{Deserialize, Serialize};

/// All methods the server can handle.
pub const METHODS: &[&str] = &[
    "system.ping",
    "system.capabilities",
    "system.identify",
    "workspace.create",
    "workspace.list",
    "workspace.close",
    "workspace.focus",
    "surface.create",
    "surface.close",
    "surface.focus",
    "surface.send_text",
    "surface.read_text",
    "surface.list",
    "surface.set_cwd",
    "surface.set_git",
    "surface.set_pr",
    "pane.split",
    "pane.close",
    "pane.focus",
    "pane.list",
    "status.set",
    "status.upsert_entry",
    "status.remove_entry",
    "notify.send",
    "notify.list",
    "notify.clear",
    "pane.create-browser",
    "browser.navigate",
    "browser.go-back",
    "browser.go-forward",
    "browser.reload",
    "browser.stop",
    "browser.get-url",
    "browser.get-title",
    "browser.execute-script",
    "browser.evaluate",
    "browser.get-text",
    "browser.snapshot",
    "browser.get-eval-result",
    "browser.screenshot",
    "browser.click",
    "browser.type",
    "browser.scroll",
    "browser.console",
    "browser.toggle-devtools",
    "browser.zoom",
    "browser.list-profiles",
    "browser.delete-profile",
    "browser.get-profile",
    "session.save",
    "subscribe",
    "unsubscribe",
];

// --- Params ---

#[derive(Debug, Deserialize)]
pub struct SendTextParams {
    pub surface_id: String,
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct ReadTextParams {
    pub surface_id: String,
    /// If true, include ANSI escape sequences in the output.
    #[serde(default)]
    pub ansi: bool,
    /// Line range string: "1-50", "-20" (last 20), or None for visible screen.
    #[serde(default)]
    pub lines: Option<String>,
}

// --- Results ---

#[derive(Debug, Serialize)]
pub struct PingResult {
    pub pong: bool,
}

#[derive(Debug, Serialize)]
pub struct CapabilitiesResult {
    pub methods: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct IdentifyResult {
    pub workspace_id: String,
    pub surface_id: String,
}

#[derive(Debug, Serialize)]
pub struct SurfaceInfo {
    pub id: String,
    pub title: String,
    pub cols: usize,
    pub rows: usize,
    pub alive: bool,
}

#[derive(Debug, Serialize)]
pub struct SurfaceListResult {
    pub surfaces: Vec<SurfaceInfo>,
}

#[derive(Debug, Serialize)]
pub struct ReadTextResult {
    pub text: String,
}

// --- Status / Notify Params ---

#[derive(Debug, Deserialize)]
pub struct StatusSetParams {
    pub workspace_id: String,
    pub state: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

/// Parameters for `status.upsert_entry`: publish / replace a keyed status
/// entry under a user-defined key. Must not begin with `agent.` — those
/// slots are owned by `status.set`.
#[derive(Debug, Deserialize)]
pub struct UpsertEntryParams {
    pub workspace_id: String,
    pub key: String,
    pub text: String,
    /// Render priority. Higher sorts first. Defaults to
    /// [`amux_notify::priority::USER_GENERIC`] server-side if omitted.
    #[serde(default)]
    pub priority: Option<i32>,
    #[serde(default)]
    pub icon: Option<String>,
    /// RGBA tuple. Wire format is a 4-element array of bytes.
    #[serde(default)]
    pub color: Option<[u8; 4]>,
}

/// Parameters for `status.remove_entry`: expire a previously-published
/// keyed entry. Returns `{ "removed": bool }` so callers can tell whether
/// the key actually existed.
#[derive(Debug, Deserialize)]
pub struct RemoveEntryParams {
    pub workspace_id: String,
    pub key: String,
}

#[derive(Debug, Deserialize)]
pub struct SetCwdParams {
    pub surface_id: String,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SetGitParams {
    pub surface_id: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub dirty: bool,
}

#[derive(Debug, Deserialize)]
pub struct SetPrParams {
    pub surface_id: String,
    #[serde(default)]
    pub number: Option<u32>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct NotifySendParams {
    pub workspace_id: String,
    #[serde(default)]
    pub pane_id: Option<String>,
    #[serde(default)]
    pub surface_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub subtitle: Option<String>,
    pub body: String,
}

// --- Status / Notify Results ---

#[derive(Debug, Serialize)]
pub struct NotifySendResult {
    pub notification_id: u64,
}

#[derive(Debug, Serialize)]
pub struct NotifyListEntry {
    pub id: u64,
    pub workspace_id: String,
    pub pane_id: String,
    pub title: String,
    pub subtitle: String,
    pub body: String,
    pub source: String,
    pub read: bool,
    pub created_at_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct NotifyListResult {
    pub notifications: Vec<NotifyListEntry>,
}
