use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use amux_layout::PaneTree;
use serde::{Deserialize, Serialize};

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
    pub surfaces: Vec<SavedSurface>,
    pub active_surface_idx: usize,
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
    pub body: String,
    pub source: String,
    pub read: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SavedWorkspaceStatus {
    pub state: String,
    #[serde(default)]
    pub label: Option<String>,
}

// --- File Operations ---

/// Returns the path to the session file: `{data_dir}/amux/session.json`
pub fn session_path() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("amux").join("session.json")
}

/// Save session data to the given path using atomic write (write to .tmp, then rename).
fn save_to_path(data: &SessionData, path: &std::path::Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let json = serde_json::to_string_pretty(data)?;
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

/// Load session data from the given path. Returns `None` if the file does not exist.
fn load_from_path(path: &std::path::Path) -> anyhow::Result<Option<SessionData>> {
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path)?;
    let data: SessionData = serde_json::from_str(&content)?;

    if data.version != 1 {
        anyhow::bail!("unsupported session version: {}", data.version);
    }

    // Reject empty sessions (no workspaces, or all workspaces have no panes)
    if data.workspaces.is_empty() || data.workspaces.iter().all(|ws| ws.panes.is_empty()) {
        return Ok(None);
    }

    Ok(Some(data))
}

/// Delete the given session file.
fn clear_path(path: &std::path::Path) -> anyhow::Result<()> {
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// Save session data to the default session file.
pub fn save(data: &SessionData) -> anyhow::Result<()> {
    save_to_path(data, &session_path())
}

/// Load session data from the default session file.
pub fn load() -> anyhow::Result<Option<SessionData>> {
    load_from_path(&session_path())
}

/// Delete the default session file.
pub fn clear() -> anyhow::Result<()> {
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
    fn corrupt_json_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        fs::write(&path, "not valid json").unwrap();

        let result = load_from_path(&path);
        assert!(result.is_err());
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
    fn load_rejects_empty_workspaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");

        let mut session = minimal_session();
        session.workspaces.clear();
        save_to_path(&session, &path).unwrap();

        let result = load_from_path(&path).unwrap();
        assert!(result.is_none());
    }
}
