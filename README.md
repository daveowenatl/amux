<h1 align="center">amux</h1>
<p align="center">A cross-platform terminal multiplexer for AI coding agents — Claude Code, Gemini CLI, and Codex CLI, all first-class</p>

<p align="center">
  <a href="https://github.com/yourusername/amux/releases"><img src="https://img.shields.io/github/v/release/yourusername/amux?color=555&label=latest" alt="Latest release" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-555" alt="License: MIT" /></a>
  <img src="https://img.shields.io/badge/platforms-Windows%20%7C%20macOS%20%7C%20Linux-555" alt="Platforms" />
  <img src="https://img.shields.io/badge/built%20with-Rust-555?logo=rust" alt="Built with Rust" />
</p>

## Features

### Notification rings
Panes get a blue ring and sidebar tabs light up when agents need your attention — works across Claude Code, Gemini CLI, and Codex CLI.

### Notification panel
See all pending notifications in one place. Jump to the most recent unread across all workspaces and agents.

### Agent status sidebar
Live status pills per workspace — which agent is thinking, which tool it's running, and which pane needs your input right now.

### Vertical + horizontal tabs
Sidebar shows git branch, PR status, working directory, listening ports, and latest notification text. Split panes horizontally and vertically.

---

- **All three agentic CLIs, first-class** — Claude Code, Gemini CLI, and Codex CLI each get hook integration, live status indicators, and tool visibility in the sidebar. Zero-setup everywhere — hooks inject automatically when the agent launches inside an amux pane, without touching the user's native agent config. Unix uses bash wrappers; Windows uses a compiled Rust wrapper (`amux-agent-wrapper.exe`) that ships alongside `amux.exe`. Codex on Windows is still a follow-up — it needs a symlinked `CODEX_HOME` which requires Developer Mode.
- **Scriptable** — CLI and socket API to create workspaces, split panes, send keystrokes, and drive agents programmatically. tmux-compat shim included for agent scripts that call tmux directly.
- **Native on every platform** — Built in Rust with wgpu for GPU-accelerated rendering. Runs natively on Windows (DX12/Vulkan), macOS (Metal), and Linux (Vulkan). Not Electron. Not Tauri.
- **Cross-platform config** — Reads `~/.config/amux/config.toml`. No platform-specific config format.

## Install

### GitHub Releases (recommended)

Download the latest binary for your platform from the [Releases page](https://github.com/yourusername/amux/releases/latest):

| Platform | File |
|---|---|
| macOS (Apple Silicon) | `amux-aarch64-apple-darwin.tar.gz` |
| macOS (Intel) | `amux-x86_64-apple-darwin.tar.gz` |
| Linux (x86_64) | `amux-x86_64-unknown-linux-gnu.tar.gz` |
| Windows (x86_64) | `amux-x86_64-pc-windows-msvc.zip` |

amux auto-updates on launch. You only need to download once.

### Homebrew (macOS)

```bash
brew tap yourusername/amux
brew install --cask amux
```

### Cargo (all platforms)

```bash
cargo install amux
```

### Winget (Windows)

```powershell
winget install amux
```

## Why amux?

I run a lot of parallel agent sessions — Claude Code for some projects, Gemini CLI for others, Codex for the rest. No single agentic CLI has won yet and I don't think one will. I wanted a multiplexer that treats all of them the same.

Ghostty and WezTerm are excellent terminals but neither was built for multi-agent workflows. The notification problem is real: agent CLIs fire OS notifications that all say roughly "agent needs input" with no context, and with ten panes open you can't tell which one needs you without tabbing through them all.

cmux solved this beautifully on macOS — the blue ring and sidebar model is exactly right. But it's macOS-only and optimized for Claude Code specifically. amux is the same idea built cross-platform in Rust, with first-class support for all three major agentic CLIs from day one.

The sidebar hooks into each agent's native event system: Claude Code's `PreToolUse`/`Stop` hooks, Gemini CLI's `BeforeTool`/`AfterAgent` hooks, and Codex CLI's `PreToolUse`/`Stop` hooks. You get live "Running: `cargo test`" tool indicators in the sidebar — without writing any glue code. All three agents inject hooks automatically per pane; no manual install step. Richer Codex integration via `codex app-server` — including approval interception from the sidebar — is tracked as future work.

The rendering is wgpu (Metal on macOS, DX12/Vulkan on Windows/Linux) backed by wezterm-term for the VT state machine. It's fast and it's not Electron.

## The Zen of amux

amux is not prescriptive about how developers hold their tools. Which is why it runs on every major operating system and integrates with every major agentic CLI out of the box.

amux is a primitive, not a solution. It gives you a terminal, notifications, workspaces, splits, tabs, agent status indicators, and a CLI to control all of it. It doesn't tell you which AI to use, which OS to run, or how to structure your projects. What you build with the primitives is yours.

The best developers have always built their own tools. Nobody has figured out the best way to work with agents yet, and the teams building closed products definitely haven't either. The developers closest to their own codebases will figure it out first.

Give a million developers composable primitives across every platform and they'll collectively find the most efficient workflows faster than any product team could design top-down.

## Agent Integration

amux auto-detects which agent is running in each pane and activates the appropriate integration. No manual setup for Claude Code or Gemini CLI — launching them inside an amux pane is enough. Wrappers installed to `~/.config/amux/bin/` inject hooks at runtime without touching your global agent settings.

### Claude Code

Hooks into all 9 Claude Code hook events (`SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Notification`, `Stop`, `SubagentStart`, `SubagentStop`, `SessionEnd`). The sidebar shows the current tool name during `PreToolUse` and clears on `PostToolUse`/`Stop`. `Notification` events surface the underlying permission-prompt or error message text. Completion fires an in-app notification and optional OS notification. Hooks are injected via `--settings` per session — your `~/.claude/settings.json` is untouched.

### Gemini CLI

Hooks into Gemini CLI's BeforeAgent, AfterAgent, BeforeTool, Notification, SessionStart, and SessionEnd events. Status updates, tool indicators, and the "needs input" ring flow to the sidebar automatically. Requires Gemini CLI v0.26.0 or newer for hook support; older versions fall back to parsing Gemini's dynamic window title (◇ Ready / ✦ Working / ✋ Action Required / ⏲ Working…) as a best-effort status signal. Hook injection uses `GEMINI_CLI_SYSTEM_SETTINGS_PATH` pointing at a per-pane temp file, so your `~/.gemini/settings.json` is untouched and any user-defined hooks still fire alongside amux's.

### Codex CLI

Hooks into Codex CLI's SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, and Stop events via a wrapper script that creates a per-session `CODEX_HOME` tempdir symlinking your real `~/.codex/` and overlaying amux hook config. No modification of your real Codex config, credentials, or history. Launching `codex` inside an amux pane is enough — no manual install step. Amux observes Codex state for the sidebar but does not intercept approvals or drive the model; full `codex app-server` integration with approval interception is tracked as future work.

### Any other agent

Any script or agent can integrate using environment variables that amux injects into every pane:

```bash
# Read from environment — amux sets these automatically
echo $AMUX_WORKSPACE_ID
echo $AMUX_SURFACE_ID
echo $AMUX_SOCKET_PATH

# Report state to amux sidebar
amux set-status active "Running evaluations..."
amux set-status idle
amux notify "Tests passed — ready for review"
```

OSC 9/99/777 sequences are intercepted from the PTY stream and shown as in-app notifications without any CLI call.

## Keyboard Shortcuts

Keys shown as `Ctrl` map to `Cmd` on macOS.

### Workspaces

| Shortcut | Action |
|---|---|
| `Ctrl N` | New workspace |
| `Ctrl 1–8` | Jump to workspace 1–8 |
| `Ctrl 9` | Jump to last workspace |
| `Ctrl Shift ]` | Next workspace |
| `Ctrl Shift [` | Previous workspace |
| `Ctrl Shift W` | Close workspace |
| `Ctrl Shift R` | Rename workspace |
| `Ctrl B` | Toggle sidebar |

### Surfaces (tabs)

| Shortcut | Action |
|---|---|
| `Ctrl T` | New surface |
| `Ctrl Shift ]` | Next surface |
| `Ctrl Shift [` | Previous surface |
| `Ctrl Tab` | Next surface |
| `Ctrl Shift Tab` | Previous surface |
| `Ctrl 1–8` | Jump to surface 1–8 |
| `Ctrl W` | Close surface |

### Split Panes

| Shortcut | Action |
|---|---|
| `Ctrl D` | Split right |
| `Ctrl Shift D` | Split down |
| `Alt ← → ↑ ↓` | Focus pane directionally |
| `Ctrl Shift H` | Flash focused pane |

### Notifications

| Shortcut | Action |
|---|---|
| `Ctrl I` | Show notification panel |
| `Ctrl Shift U` | Jump to latest unread |

### Find

| Shortcut | Action |
|---|---|
| `Ctrl F` | Find |
| `Ctrl G` / `Ctrl Shift G` | Find next / previous |
| `Ctrl Shift F` | Hide find bar |
| `Ctrl E` | Use selection for find |

### Terminal

| Shortcut | Action |
|---|---|
| `Ctrl K` | Clear scrollback |
| `Ctrl Shift C` | Copy |
| `Ctrl Shift V` | Paste |
| `Ctrl +` / `Ctrl -` | Increase / decrease font size |
| `Ctrl 0` | Reset font size |

### Window

| Shortcut | Action |
|---|---|
| `Ctrl Shift N` | New window |
| `Ctrl ,` | Settings |
| `Ctrl Shift ,` | Reload configuration |

## tmux Compatibility

Agent scripts written for tmux work with amux via the built-in shim:

```bash
# Install the shim so `tmux` calls route to amux
amux install-tmux-shim
```

Or call directly:
```bash
amux __tmux-compat new-session -s myproject
amux __tmux-compat send-keys -t myproject "claude" Enter
```

## CLI Reference

```
amux new-workspace [--title <name>] [--color <hex>]
amux list-workspaces [--json]
amux close-workspace [--id <id>]

amux split [--right | --down] [-- <command>]
amux focus-pane [--pane <id>]
amux resize-pane -L|-R|-U|-D [N]

amux send-keys [--pane <id>] <keys>
amux read-screen [--pane <id>] [--lines <start:end>] [--ansi]

amux set-status <active|idle|waiting> [--label <text>]
amux notify <message> [--workspace <id>]

amux tree [--json]
amux socket-path
```

Full reference: `amux help`.

## Session Restore

On relaunch, amux restores:
- Window, workspace, and pane layout
- Working directories
- Terminal scrollback (up to 4,000 lines per surface, best-effort)
- Agent status pills and notification history

amux does **not** restore live process state (active Claude Code / Gemini / Codex sessions are not resumed after restart).

## Configuration

Config lives at `~/.config/amux/config.toml` (or `%APPDATA%\amux\config.toml` on Windows):

```toml
# Shell to spawn in new panes. Accepts a bare name ("pwsh", "bash", "fish")
# that amux resolves against PATH, or an absolute path.
# When unset, amux uses $SHELL on Unix and prefers pwsh.exe on Windows if
# installed, otherwise falls back to $COMSPEC (cmd.exe).
# shell = "pwsh"

[appearance]
sidebar_width = 220
font_family = "JetBrains Mono"
font_size = 13.0
theme = "dark"            # dark | light | system

[notifications]
sound = true
system_notifications = true
ring = true
auto_reorder_workspaces = true

[keybindings]
new_workspace = "ctrl+n"
toggle_sidebar = "ctrl+b"
# ... full list in docs

[agents]
# Override auto-detection if needed
# default_agent = "claude"   # claude | gemini | codex
```

## Building from Source

Requirements: Rust 1.80+, a C compiler, and platform graphics drivers.

```bash
git clone https://github.com/yourusername/amux
cd amux
cargo build --release
./target/release/amux
```

On Windows, the MSVC toolchain is required (`rustup default stable-x86_64-pc-windows-msvc`).

## Contributing

- Open [GitHub Issues](https://github.com/yourusername/amux/issues) for bugs and feature requests
- Start a [Discussion](https://github.com/yourusername/amux/discussions) for questions and ideas
- PRs welcome — see [CONTRIBUTING.md](./CONTRIBUTING.md)

## License

MIT — see [LICENSE](./LICENSE) for the full text.
