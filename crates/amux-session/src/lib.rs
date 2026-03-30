use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use amux_layout::PaneTree;
use serde::{Deserialize, Serialize};

/// Typed errors for session persistence operations.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// Load-time deserialization failure (corrupt or incompatible file).
    #[error("corrupted session file: {0}")]
    Corrupted(serde_json::Error),

    /// Save-time serialization failure.
    #[error("failed to serialize session: {0}")]
    Serialize(serde_json::Error),

    #[error("unsupported session version {version} (expected {expected})")]
    VersionMismatch { version: u32, expected: u32 },

    #[error("session file I/O error: {0}")]
    Io(#[from] std::io::Error),
}

// --- Limits ---

/// Maximum scrollback lines saved per surface.
pub const MAX_SCROLLBACK_LINES: usize = 4_000;

/// Maximum total bytes of scrollback saved per surface.
/// Prevents unbounded session file growth from long lines (e.g., minified JSON).
pub const MAX_SCROLLBACK_BYTES: usize = 400_000;

/// Truncate scrollback text to fit within `max_bytes`, keeping the most recent
/// output (truncating from the top). Avoids cutting mid-line when possible.
/// Safe for multi-byte UTF-8: advances to the next char boundary if needed.
pub fn truncate_scrollback(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut excess = text.len() - max_bytes;
    // Advance to a valid UTF-8 char boundary.
    while excess < text.len() && !text.is_char_boundary(excess) {
        excess += 1;
    }
    // Find the next newline after the truncation point to avoid mid-line cut.
    match text[excess..].find('\n') {
        Some(i) => &text[excess + i + 1..],
        None => &text[excess..],
    }
}

// --- Data Model ---

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionData {
    pub version: u32,
    pub saved_at: String,
    pub workspaces: Vec<SavedWorkspace>,
    pub active_workspace_idx: usize,
    pub next_pane_id: u64,
    pub next_workspace_id: u64,
    pub next_surface_id: u64,
    pub sidebar: SavedSidebar,
    #[serde(default)]
    pub notifications: Vec<SavedNotification>,
    #[serde(default)]
    pub workspace_statuses: HashMap<u64, SavedWorkspaceStatus>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SavedWorkspace {
    pub id: u64,
    pub title: String,
    pub tree: PaneTree,
    pub focused_pane: u64,
    #[serde(default)]
    pub zoomed: Option<u64>,
    pub panes: HashMap<u64, SavedManagedPane>,
    #[serde(default)]
    pub color: Option<[u8; 4]>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SavedManagedPane {
    /// Panel type identifier (default "terminal"). Future-proofing for
    /// non-terminal panels (e.g., markdown, browser).
    #[serde(default = "default_panel_type")]
    pub panel_type: String,
    pub surfaces: Vec<SavedSurface>,
    pub active_surface_idx: usize,
}

/// The panel type identifier for terminal panes.
pub const PANEL_TYPE_TERMINAL: &str = "terminal";

fn default_panel_type() -> String {
    PANEL_TYPE_TERMINAL.to_string()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SavedSurface {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub scrollback: String,
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub git_dirty: bool,
    #[serde(default)]
    pub pr_number: Option<u32>,
    #[serde(default)]
    pub pr_title: Option<String>,
    #[serde(default)]
    pub pr_state: Option<String>,
    #[serde(default)]
    pub user_title: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SavedSidebar {
    pub visible: bool,
    pub width: f32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SavedNotification {
    pub id: u64,
    pub workspace_id: u64,
    pub pane_id: u64,
    pub surface_id: u64,
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    pub body: String,
    pub source: String,
    pub read: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SavedWorkspaceStatus {
    pub state: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

// --- File Operations ---

/// Returns the path to the session file: `{data_dir}/amux/session.json`
pub fn session_path() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("amux").join("session.json")
}

/// Save session data to the given path using atomic write (write to .tmp, then rename).
fn save_to_path(data: &SessionData, path: &std::path::Path) -> Result<(), SessionError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let json = serde_json::to_string_pretty(data).map_err(SessionError::Serialize)?;
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, &json)?;

    // On Windows, fs::rename fails if the destination exists.
    #[cfg(windows)]
    {
        let _ = fs::remove_file(path);
    }

    fs::rename(&tmp_path, path)?;

    Ok(())
}

/// Lightweight header for version checking before full deserialization.
#[derive(Deserialize)]
struct SessionHeader {
    version: u32,
}

/// Load session data from the given path. Returns `None` if the file does not exist.
fn load_from_path(path: &std::path::Path) -> Result<Option<SessionData>, SessionError> {
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path)?;

    // Check version before full deserialization so incompatible schemas
    // produce VersionMismatch instead of Corrupted.
    let header: SessionHeader = serde_json::from_str(&content).map_err(SessionError::Corrupted)?;
    if header.version != 1 {
        return Err(SessionError::VersionMismatch {
            version: header.version,
            expected: 1,
        });
    }

    let data: SessionData = serde_json::from_str(&content).map_err(SessionError::Corrupted)?;

    // Reject empty sessions (no workspaces, or all workspaces have no panes)
    if data.workspaces.is_empty() || data.workspaces.iter().all(|ws| ws.panes.is_empty()) {
        return Ok(None);
    }

    Ok(Some(data))
}

/// Delete the given session file.
fn clear_path(path: &std::path::Path) -> Result<(), SessionError> {
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// Save session data to the default session file.
pub fn save(data: &SessionData) -> Result<(), SessionError> {
    save_to_path(data, &session_path())
}

/// Load session data from the default session file.
pub fn load() -> Result<Option<SessionData>, SessionError> {
    load_from_path(&session_path())
}

/// Delete the default session file.
pub fn clear() -> Result<(), SessionError> {
    clear_path(&session_path())
}

#[cfg(test)]
mod tests {
    use super::*;
    use amux_layout::PaneTree;

    fn minimal_session() -> SessionData {
        let tree = PaneTree::new(0);
        let mut panes = HashMap::new();
        panes.insert(
            0,
            SavedManagedPane {
                panel_type: "terminal".to_string(),
                surfaces: vec![SavedSurface {
                    id: 0,
                    title: "zsh".to_string(),
                    working_dir: Some("/tmp".to_string()),
                    scrollback: "$ echo hello\nhello\n".to_string(),
                    cols: 80,
                    rows: 24,
                    git_branch: None,
                    git_dirty: false,
                    pr_number: None,
                    pr_title: None,
                    pr_state: None,
                    user_title: None,
                }],
                active_surface_idx: 0,
            },
        );

        SessionData {
            version: 1,
            saved_at: "2026-03-24T00:00:00Z".to_string(),
            workspaces: vec![SavedWorkspace {
                id: 0,
                title: "default".to_string(),
                tree,
                focused_pane: 0,
                zoomed: None,
                panes,
                color: None,
            }],
            active_workspace_idx: 0,
            next_pane_id: 1,
            next_workspace_id: 1,
            next_surface_id: 1,
            sidebar: SavedSidebar {
                visible: true,
                width: 200.0,
            },
            notifications: vec![],
            workspace_statuses: HashMap::new(),
        }
    }

    #[test]
    fn round_trip_serde() {
        let session = minimal_session();
        let json = serde_json::to_string(&session).unwrap();
        let restored: SessionData = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.version, 1);
        assert_eq!(restored.workspaces.len(), 1);
        assert_eq!(restored.workspaces[0].title, "default");
        assert_eq!(
            restored.workspaces[0].panes[&0].surfaces[0].scrollback,
            "$ echo hello\nhello\n"
        );
    }

    #[test]
    fn load_missing_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let result = load_from_path(&path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");

        let session = minimal_session();
        save_to_path(&session, &path).unwrap();

        let restored = load_from_path(&path).unwrap().unwrap();
        assert_eq!(restored.workspaces[0].panes.len(), 1);
        assert_eq!(
            restored.workspaces[0].panes[&0].surfaces[0].scrollback,
            "$ echo hello\nhello\n"
        );
    }

    #[test]
    fn corrupt_json_returns_corrupted_variant() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        fs::write(&path, "not valid json").unwrap();

        let err = load_from_path(&path).unwrap_err();
        assert!(
            matches!(err, SessionError::Corrupted(_)),
            "expected Corrupted, got: {err:?}"
        );
    }

    #[test]
    fn wrong_version_returns_version_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");

        let mut session = minimal_session();
        session.version = 99;
        // Write directly to bypass version check in save_to_path
        let json = serde_json::to_string(&session).unwrap();
        fs::write(&path, &json).unwrap();

        let err = load_from_path(&path).unwrap_err();
        assert!(
            matches!(
                err,
                SessionError::VersionMismatch {
                    version: 99,
                    expected: 1
                }
            ),
            "expected VersionMismatch, got: {err:?}"
        );
    }

    #[test]
    fn clear_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");

        save_to_path(&minimal_session(), &path).unwrap();
        assert!(path.exists());

        clear_path(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn deserialize_without_user_title_defaults_to_none() {
        // Simulate an older session file that predates the user_title field.
        let json = r#"{
            "id": 0, "title": "zsh", "working_dir": "/tmp", "scrollback": "",
            "cols": 80, "rows": 24, "git_branch": null, "git_dirty": false,
            "pr_number": null, "pr_title": null, "pr_state": null
        }"#;
        let surface: SavedSurface = serde_json::from_str(json).unwrap();
        assert!(surface.user_title.is_none());
    }

    #[test]
    fn deserialize_without_panel_type_defaults_to_terminal() {
        let json = r#"{
            "surfaces": [
                { "id": 0, "title": "zsh", "cols": 80, "rows": 24 }
            ],
            "active_surface_idx": 0
        }"#;
        let pane: SavedManagedPane = serde_json::from_str(json).unwrap();
        assert_eq!(pane.panel_type, "terminal");
    }

    #[test]
    fn load_rejects_empty_workspaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");

        let mut session = minimal_session();
        session.workspaces.clear();
        save_to_path(&session, &path).unwrap();

        let result = load_from_path(&path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn truncate_scrollback_noop_when_under_limit() {
        let text = "line1\nline2\nline3\n";
        assert_eq!(truncate_scrollback(text, 1000), text);
    }

    #[test]
    fn truncate_scrollback_cuts_from_top() {
        let text = "aaaa\nbbbb\ncccc\ndddd\n";
        // 20 chars, limit=15: excess=5, text[5..]="bbbb\ncccc\ndddd\n",
        // find('\n')=Some(4), start=5+4+1=10, text[10..]="cccc\ndddd\n"
        let result = truncate_scrollback(text, 15);
        assert_eq!(result, "cccc\ndddd\n");
    }

    #[test]
    fn truncate_scrollback_avoids_mid_line_cut() {
        // "abc\ndef\nghi\n" = 12 bytes, limit 8 → excess 4
        // text[4..] = "def\nghi\n", first \n at index 3 → skip to "ghi\n"
        let text = "abc\ndef\nghi\n";
        let result = truncate_scrollback(text, 8);
        assert_eq!(result, "ghi\n");
    }

    #[test]
    fn truncate_scrollback_multibyte_utf8() {
        // "你好\n世界\n" = 6+1+6+1 = 14 bytes
        let text = "你好\n世界\n";
        assert_eq!(text.len(), 14);
        // limit=8 → excess=6, byte 6 is '\n', find('\n')=Some(0), start=6+0+1=7
        let result = truncate_scrollback(text, 8);
        assert_eq!(result, "世界\n");
    }

    #[test]
    fn truncate_scrollback_mid_codepoint_boundary() {
        // "café\ndata\n" — 'é' is 2 bytes (0xC3 0xA9), total = 5+1+5 = 11 bytes
        let text = "café\ndata\n";
        assert_eq!(text.len(), 11);
        // limit=7 → excess=4, byte 4 is inside 'é' (0xA9), advance to 5 ('\n')
        // find('\n')=Some(0), start=5+0+1=6
        let result = truncate_scrollback(text, 7);
        assert_eq!(result, "data\n");
    }

    #[test]
    fn truncate_scrollback_no_newline() {
        // Single long line with no newlines — falls through to &text[excess..]
        let text = "abcdefghijklmnop";
        let result = truncate_scrollback(text, 10);
        // excess=6, no newline found, returns text[6..] = "ghijklmnop"
        assert_eq!(result, "ghijklmnop");
    }

    #[test]
    fn deserialize_notification_without_subtitle_defaults_empty() {
        let json = r#"{
            "id": 1,
            "workspace_id": 1,
            "pane_id": 10,
            "surface_id": 100,
            "title": "T",
            "body": "B",
            "source": "cli",
            "read": false
        }"#;
        let n: SavedNotification = serde_json::from_str(json).unwrap();
        assert_eq!(n.subtitle, "");
    }
}
