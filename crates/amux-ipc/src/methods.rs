use serde::{Deserialize, Serialize};

/// All methods the server can handle.
pub const METHODS: &[&str] = &[
    "system.ping",
    "system.capabilities",
    "system.identify",
    "surface.send_text",
    "surface.read_text",
    "surface.list",
    "pane.split",
    "pane.close",
    "pane.focus",
    "pane.list",
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
