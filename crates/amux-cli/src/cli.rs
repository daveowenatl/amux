//! Clap argument definitions for the `amux` CLI.
//!
//! Defines the `Cli` root struct and the `Command` enum with all
//! subcommands. Separated from the dispatch logic in `main.rs` to
//! keep argument definitions easy to find and review.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "amux", about = "Terminal multiplexer for AI coding agents")]
pub(crate) struct Cli {
    /// Socket path (auto-detected if omitted)
    #[arg(long, global = true)]
    pub(crate) socket: Option<String>,

    /// Auth token (auto-detected from AMUX_SOCKET_TOKEN or stored token)
    #[arg(long, global = true)]
    pub(crate) token: Option<String>,

    /// Output as JSON
    #[arg(long, global = true)]
    pub(crate) json: bool,

    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Check if the amux server is running
    Ping,
    /// List full hierarchy (workspaces, surfaces, panes)
    Tree,
    /// Send text to a surface
    Send {
        /// Text to send
        text: String,
        /// Target surface ID
        #[arg(long)]
        surface: Option<String>,
    },
    /// Read screen text from a surface
    ReadScreen {
        /// Target surface ID
        #[arg(long)]
        surface: Option<String>,
        /// Include ANSI escape sequences (colors/formatting)
        #[arg(long)]
        ansi: bool,
        /// Line range (e.g. "1-50", "-20" for last 20 lines)
        #[arg(long)]
        lines: Option<String>,
    },
    /// List server capabilities
    Capabilities,
    /// Identify focused workspace/surface
    Identify,
    /// Split the focused pane
    Split {
        /// Split direction: right or down
        #[arg(long, default_value = "right")]
        direction: String,
    },
    /// Close a pane
    ClosePane {
        /// Pane ID to close (defaults to focused)
        #[arg(long)]
        pane: Option<String>,
    },
    /// Open a browser pane
    Browser {
        /// URL to open (defaults to Google)
        url: Option<String>,
    },
    /// Navigate a browser pane to a URL
    #[command(name = "browser-navigate")]
    BrowserNavigate {
        /// URL to navigate to
        url: String,
        /// Target pane ID (defaults to focused browser pane)
        #[arg(long)]
        pane: Option<String>,
    },
    /// Navigate browser back
    #[command(name = "browser-back")]
    BrowserBack {
        /// Target pane ID
        #[arg(long)]
        pane: Option<String>,
    },
    /// Navigate browser forward
    #[command(name = "browser-forward")]
    BrowserForward {
        /// Target pane ID
        #[arg(long)]
        pane: Option<String>,
    },
    /// Reload browser page
    #[command(name = "browser-reload")]
    BrowserReload {
        /// Target pane ID
        #[arg(long)]
        pane: Option<String>,
    },
    /// Get current browser URL
    #[command(name = "browser-url")]
    BrowserUrl {
        /// Target pane ID
        #[arg(long)]
        pane: Option<String>,
    },
    /// Get current browser page title
    #[command(name = "browser-title")]
    BrowserTitle {
        /// Target pane ID
        #[arg(long)]
        pane: Option<String>,
    },
    /// Execute JavaScript in browser pane (fire-and-forget)
    #[command(name = "browser-exec")]
    BrowserExec {
        /// JavaScript code to execute
        script: String,
        /// Target pane ID
        #[arg(long)]
        pane: Option<String>,
    },
    /// Evaluate JavaScript and return result
    #[command(name = "browser-eval")]
    BrowserEval {
        /// JavaScript expression (use 'return <expr>' for a value)
        script: String,
        /// Target pane ID
        #[arg(long)]
        pane: Option<String>,
    },
    /// Get visible page text
    #[command(name = "browser-text")]
    BrowserText {
        /// Target pane ID
        #[arg(long)]
        pane: Option<String>,
    },
    /// Get page DOM snapshot (HTML)
    #[command(name = "browser-snapshot")]
    BrowserSnapshot {
        /// Target pane ID
        #[arg(long)]
        pane: Option<String>,
    },
    /// Capture a screenshot of the browser page
    #[command(name = "browser-screenshot")]
    BrowserScreenshot {
        /// Write to file path (default: print to stdout)
        #[arg(long)]
        output: Option<String>,
        /// Target pane ID
        #[arg(long)]
        pane: Option<String>,
    },
    /// Toggle browser DevTools
    #[command(name = "browser-devtools")]
    BrowserDevtools {
        /// Target pane ID
        #[arg(long)]
        pane: Option<String>,
        /// Explicitly open or close (toggles if omitted)
        #[arg(long)]
        open: Option<bool>,
    },
    /// Drain browser console messages
    #[command(name = "browser-console")]
    BrowserConsole {
        /// Target pane ID
        #[arg(long)]
        pane: Option<String>,
    },
    /// Focus a specific pane
    FocusPane {
        /// Pane ID to focus
        pane_id: String,
    },
    /// List all panes in active surface
    ListPanes,
    /// Create a new workspace
    #[command(name = "workspace-create")]
    WorkspaceCreate {
        /// Workspace title
        #[arg(long)]
        title: Option<String>,
    },
    /// List all workspaces
    #[command(name = "workspace-list")]
    WorkspaceList,
    /// Close a workspace
    #[command(name = "workspace-close")]
    WorkspaceClose {
        /// Workspace ID to close
        workspace_id: Option<String>,
    },
    /// Focus a workspace
    #[command(name = "workspace-focus")]
    WorkspaceFocus {
        /// Workspace ID to focus
        workspace_id: String,
    },
    /// Create a new surface (tab) in a workspace
    #[command(name = "surface-create")]
    SurfaceCreate {
        /// Pane ID to add the surface to (defaults to focused pane)
        #[arg(long)]
        pane: Option<String>,
    },
    /// Close a surface (tab)
    #[command(name = "surface-close")]
    SurfaceClose {
        /// Surface ID to close (defaults to active)
        surface_id: Option<String>,
    },
    /// Focus a surface (tab)
    #[command(name = "surface-focus")]
    SurfaceFocus {
        /// Surface ID to focus
        surface_id: String,
    },
    /// Set the working directory for a surface
    #[command(name = "set-cwd")]
    SetCwd {
        /// Working directory path (omit to clear)
        #[arg(conflicts_with = "clear")]
        cwd: Option<String>,
        /// Clear CWD metadata
        #[arg(long)]
        clear: bool,
        /// Target surface ID (defaults to AMUX_SURFACE_ID)
        #[arg(long)]
        surface: Option<String>,
    },
    /// Set git branch info for a surface
    #[command(name = "set-git")]
    SetGit {
        /// Branch name (omit to clear)
        #[arg(long, conflicts_with = "clear")]
        branch: Option<String>,
        /// Working tree has uncommitted changes
        #[arg(long, conflicts_with = "clear")]
        dirty: bool,
        /// Clear git info
        #[arg(long)]
        clear: bool,
        /// Target surface ID (defaults to AMUX_SURFACE_ID)
        #[arg(long)]
        surface: Option<String>,
    },
    /// Set PR info for a surface
    #[command(name = "set-pr")]
    SetPr {
        /// PR number
        #[arg(long, conflicts_with = "clear")]
        number: Option<u32>,
        /// PR title
        #[arg(long, conflicts_with = "clear")]
        title: Option<String>,
        /// PR state: open, merged, closed
        #[arg(long, conflicts_with = "clear")]
        state: Option<String>,
        /// Clear PR info
        #[arg(long)]
        clear: bool,
        /// Target surface ID (defaults to AMUX_SURFACE_ID)
        #[arg(long)]
        surface: Option<String>,
    },
    /// Set workspace agent status (displayed as a sidebar pill)
    #[command(name = "set-status")]
    SetStatus {
        /// Status state: idle, active, waiting
        state: String,
        /// Optional label text
        label: Option<String>,
        /// Agent's current task description
        #[arg(long)]
        task: Option<String>,
        /// Agent's latest message
        #[arg(long)]
        message: Option<String>,
        /// Target workspace ID (defaults to AMUX_WORKSPACE_ID)
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Publish or replace a keyed status entry.
    ///
    /// Each call writes under its own key (e.g. "claude.tool",
    /// "git.branch"). Re-using a key replaces the prior entry; use
    /// `remove-entry` to expire it. Keys starting with "agent." are
    /// reserved for the legacy sidebar slots owned by `set-status`
    /// and will be rejected.
    #[command(name = "set-entry")]
    SetEntry {
        /// Entry key (e.g. "claude.tool", "git.branch"). Must not
        /// start with "agent." — that namespace is reserved.
        key: String,
        /// Display text
        text: String,
        /// Render priority. Higher sorts first (default 50).
        #[arg(long)]
        priority: Option<i32>,
        /// Optional icon name / emoji (renderer-defined)
        #[arg(long)]
        icon: Option<String>,
        /// Optional RGB/RGBA color, e.g. "#ff8800" or "#ff8800aa"
        #[arg(long)]
        color: Option<String>,
        /// Auto-expire the entry after this many seconds. Useful as a
        /// safety net for scripts that may not get a chance to run
        /// `remove-entry` on exit. Fractional values are rounded up to
        /// the next millisecond (a sub-ms `--ttl` becomes 1ms, so the
        /// entry never expires instantly).
        #[arg(long)]
        ttl: Option<f64>,
        /// Target workspace ID (defaults to AMUX_WORKSPACE_ID)
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Set or clear the workspace progress bar (decoration on the
    /// sidebar row).
    ///
    /// `VALUE` is a fraction in `[0.0, 1.0]` — pass `--clear` instead
    /// to drop the bar entirely. `--label` attaches a short caption
    /// ("compiling 34/120"); it renders next to the bar and only while
    /// a value is present.
    #[command(name = "set-progress")]
    SetProgress {
        /// Progress value between 0.0 and 1.0. Conflicts with `--clear`.
        #[arg(conflicts_with = "clear")]
        value: Option<f32>,
        /// Optional short caption rendered beside the bar.
        #[arg(long)]
        label: Option<String>,
        /// Clear the progress bar (and its label).
        #[arg(long)]
        clear: bool,
        /// Target workspace ID (defaults to AMUX_WORKSPACE_ID)
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Remove a previously-published keyed status entry.
    #[command(name = "remove-entry")]
    RemoveEntry {
        /// Entry key to remove
        key: String,
        /// Target workspace ID (defaults to AMUX_WORKSPACE_ID)
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Send a notification
    Notify {
        /// Notification body
        body: String,
        /// Notification title
        #[arg(long)]
        title: Option<String>,
        /// Notification subtitle (e.g. "Permission Required", "Task Completed")
        #[arg(long)]
        subtitle: Option<String>,
        /// Target workspace ID (defaults to AMUX_WORKSPACE_ID)
        #[arg(long)]
        workspace: Option<String>,
        /// Target pane ID (defaults to focused pane)
        #[arg(long)]
        pane: Option<String>,
    },
    /// List notifications
    #[command(name = "list-notifications")]
    ListNotifications,
    /// Clear all notifications
    #[command(name = "clear-notifications")]
    ClearNotifications,
    /// Save the current session
    #[command(name = "session-save")]
    SessionSave,
    /// Clear saved session data
    #[command(name = "session-clear")]
    SessionClear,
    /// Install shell integration scripts
    #[command(name = "install-shell-integration")]
    InstallShellIntegration,
    /// Handle a Claude Code hook event (reads JSON from stdin)
    #[command(name = "claude-hook")]
    ClaudeHook {
        /// Hook event name (PreToolUse, Stop, UserPromptSubmit, etc.)
        event: String,
    },
    /// Handle a Gemini CLI hook event (reads JSON from stdin)
    #[command(name = "gemini-hook")]
    GeminiHook {
        /// Hook event name (BeforeTool, AfterAgent, SessionStart, etc.)
        event: String,
    },
    /// Handle a Codex CLI hook event (reads JSON from stdin)
    #[command(name = "codex-hook")]
    CodexHook {
        /// Hook event name (SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, Stop)
        event: String,
    },
    /// Subscribe to server events and print them as newline-delimited JSON
    Subscribe {
        /// Event types to subscribe to (e.g. notification, focus_change)
        #[arg(required = true)]
        events: Vec<String>,
    },
}
