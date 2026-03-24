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
    "pane.split",
    "pane.close",
    "pane.focus",
    "pane.list",
    "status.set",
    "notify.send",
    "notify.list",
    "notify.clear",
    "session.save",
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
}

#[derive(Debug, Deserialize)]
pub struct NotifySendParams {
    pub workspace_id: String,
    pub pane_id: String,
    #[serde(default)]
    pub title: Option<String>,
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
    pub body: String,
    pub source: String,
    pub read: bool,
    pub created_at_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct NotifyListResult {
    pub notifications: Vec<NotifyListEntry>,
}
