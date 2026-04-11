# amux

amux is a terminal multiplexer for AI coding agents (Claude Code, Gemini CLI, Codex CLI). It's a clone of [cmux](https://github.com/manaflow-ai/cmux), rebuilt in Rust to run on Windows and Linux alongside macOS. The sidebar, notification ring, and "which pane needs my attention" model are cmux's design; credit goes there.

**Status: MVP.** Pre-1.0. The UI, CLI, and socket protocol still move between commits; don't depend on them being stable yet. Windows is the newest platform and the least tested. Codex-on-Windows is not wired up yet (Claude Code and Gemini CLI work on Windows; Codex still passthroughs).

[![Latest release](https://img.shields.io/github/v/release/daveowenatl/amux?color=555&label=latest)](https://github.com/daveowenatl/amux/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-555)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20macOS%20%7C%20Linux-555)
![Built with Rust](https://img.shields.io/badge/built%20with-Rust-555?logo=rust)

## What it does

- Runs Claude Code, Gemini CLI, and Codex CLI in panes. A blue ring appears on any pane whose agent needs input.
- Sidebar shows per-workspace status: which agent is active, which tool it's running, which pane is waiting on you.
- Hook integration is auto-injected. No `install-hooks` step. Your `~/.claude/settings.json`, `~/.gemini/settings.json`, and `~/.codex/` are not touched.
- Workspaces, horizontal and vertical splits, surface tabs within a workspace.
- GPU-rendered via wgpu (Metal / DX12 / Vulkan) backed by wezterm-term for the VT state machine.
- CLI and Unix / named-pipe socket for driving it from scripts. tmux-compat shim so agent scripts calling `tmux` route to amux.

## Install

### GitHub Releases

Grab the archive for your platform from the [Releases page](https://github.com/daveowenatl/amux/releases/latest):

| Platform | File |
|---|---|
| macOS (Apple Silicon) | `amux-aarch64-apple-darwin.tar.gz` |
| macOS (Intel) | `amux-x86_64-apple-darwin.tar.gz` |
| Linux (x86_64) | `amux-x86_64-unknown-linux-gnu.tar.gz` |
| Windows (x86_64) | `amux-x86_64-pc-windows-msvc.zip` |

Extract and put the contents on your `PATH`. Each archive contains:

- `amux` — CLI (shell integration, status, notifications, session control)
- `amux-app` — the GUI terminal multiplexer itself
- `amux-agent-wrapper` *(Windows only)* — compiled agent hook injector; copied to `~/.config/amux/bin/{claude,gemini}.exe` on first launch and used by `amux-app` to wire hooks into new panes

### From source

```bash
git clone https://github.com/daveowenatl/amux
cd amux
cargo build --release
./target/release/amux-app
```

Requirements: Rust 1.80+, a C compiler, and platform graphics drivers. Windows needs the MSVC toolchain (`rustup default stable-x86_64-pc-windows-msvc`). Homebrew / Winget / `cargo install amux` are not set up yet — build from source or use the release archive.

## Keyboard Shortcuts

Defaults differ between macOS and Windows/Linux. On Windows/Linux, workspace / tab / edit operations use `Ctrl+Shift` instead of bare `Ctrl` so the terminal's own `Ctrl+C` (SIGINT), `Ctrl+N`, `Ctrl+W`, `Ctrl+S` (XOFF), etc. still reach the shell. Every binding is overridable in `config.toml` under `[keybindings]`.

### Workspaces

| Action | macOS | Windows / Linux |
|---|---|---|
| New workspace | `Cmd+N` | `Ctrl+Shift+N` |
| Next workspace | `Cmd+Shift+]` | `Ctrl+Shift+]` |
| Previous workspace | `Cmd+Shift+[` | `Ctrl+Shift+[` |
| Jump to workspace 1–8 | `Cmd+1`…`Cmd+8` | `Ctrl+1`…`Ctrl+8` |
| Jump to last workspace | `Cmd+9` | `Ctrl+9` |
| Toggle sidebar | `Cmd+B` | `Ctrl+B` |

### Surfaces (tabs within a workspace)

| Action | macOS | Windows / Linux |
|---|---|---|
| New tab | `Cmd+T` | `Ctrl+Shift+T` |
| Close tab | `Cmd+W` | `Ctrl+Shift+W` |
| New browser tab | `Cmd+Shift+L` | `Ctrl+Shift+L` |
| Next tab in focused pane | `Ctrl+Tab` | `Ctrl+Tab` |
| Previous tab in focused pane | `Ctrl+Shift+Tab` | `Ctrl+Shift+Tab` |

### Panes (splits)

| Action | macOS | Windows / Linux |
|---|---|---|
| Split right | `Cmd+D` | `Ctrl+D` |
| Split down | `Cmd+Shift+D` | `Ctrl+Shift+D` |
| Focus pane left | `Cmd+Alt+←` | `Ctrl+Alt+←` |
| Focus pane right | `Cmd+Alt+→` | `Ctrl+Alt+→` |
| Focus pane up | `Cmd+Alt+↑` | `Ctrl+Alt+↑` |
| Focus pane down | `Cmd+Alt+↓` | `Ctrl+Alt+↓` |
| Zoom focused pane | `Cmd+Shift+Enter` | `Ctrl+Shift+Enter` |

### Terminal

| Action | macOS | Windows / Linux |
|---|---|---|
| Copy | `Cmd+C` | `Ctrl+Shift+C` |
| Paste | `Cmd+V` | `Ctrl+Shift+V` |
| Select all | `Cmd+A` | `Ctrl+Shift+A` |
| Find | `Cmd+F` | `Ctrl+F` |
| Scrollback / copy mode | `Cmd+Shift+X` | `Ctrl+Shift+X` |
| Clear scrollback | `Cmd+K` | `Ctrl+Shift+K` |
| Zoom in (font) | `Cmd+=` | `Ctrl+=` |
| Zoom out (font) | `Cmd+-` | `Ctrl+-` |
| Reset font size | `Cmd+0` | `Ctrl+0` |

### Notifications

| Action | macOS | Windows / Linux |
|---|---|---|
| Toggle notification panel | `Cmd+I` | `Ctrl+I` |
| Jump to latest unread | `Cmd+Shift+U` | `Ctrl+Shift+U` |

### Session / dev

| Action | macOS | Windows / Linux |
|---|---|---|
| Save session | `Cmd+S` | `Ctrl+Shift+S` |
| Open dev tools | `Cmd+Alt+I` | `Ctrl+Shift+I` |

## Agent Integration

amux detects which agent is running in a pane by `argv[0]` and wires its hook events into the sidebar. Wrappers are installed to `~/.config/amux/bin/` and that directory is prepended to `PATH` for every pane, so launching `claude`, `gemini`, or `codex` inside amux finds the wrapper first. The wrapper injects hooks for the current session and execs the real agent binary.

### Claude Code

All 9 hook events: `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Notification`, `Stop`, `SubagentStart`, `SubagentStop`, `SessionEnd`. Hooks are injected via `--settings` at launch; nothing is persisted to `~/.claude/`.

### Gemini CLI

Six hook events: `BeforeAgent`, `AfterAgent`, `BeforeTool`, `Notification`, `SessionStart`, `SessionEnd`. Requires Gemini CLI `v0.26.0` or newer for hook support. Older versions fall back to parsing Gemini's window-title state machine (`◇ Ready` / `✦ Working` / `✋ Action Required` / `⏲ Working…`) as a best-effort status signal. Hook injection uses `GEMINI_CLI_SYSTEM_SETTINGS_PATH`; `~/.gemini/settings.json` is untouched and any user-defined hooks still fire alongside amux's.

### Codex CLI

Five hook events: `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Stop`. The wrapper creates a per-session `CODEX_HOME` tempdir that symlinks your real `~/.codex/` and overlays amux's hook config. Your Codex config, credentials, and history are never touched. amux observes Codex state for the sidebar but does not intercept approvals or drive the model.

Codex on Windows is not wired up yet. The wrapper needs a symlinked `CODEX_HOME` overlay, which requires [Windows Developer Mode](https://learn.microsoft.com/en-us/windows/apps/get-started/enable-your-device-for-development) (or admin) to create symlinks. Until that's wired, `codex` on Windows runs passthrough — you get a working Codex session, just without sidebar status or tool indicators. Claude Code and Gemini CLI are unaffected on Windows.

### Any other agent

amux injects `AMUX_WORKSPACE_ID`, `AMUX_SURFACE_ID`, and `AMUX_SOCKET_PATH` into every pane. Any script can report state back:

```bash
amux set-status active "Running evaluations..."
amux set-status idle
amux notify "Tests passed"
```

OSC 9 / 99 / 777 sequences on the PTY stream also surface as in-app notifications without a CLI call.

## tmux Compatibility

```bash
amux install-tmux-shim                              # route `tmux` calls to amux
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

On relaunch amux restores window, workspace, and pane layout; working directories; terminal scrollback (up to 4,000 lines per surface, best-effort); status pills; and notification history. Live agent process state is **not** restored — Claude, Gemini, and Codex sessions have to be restarted manually after an amux restart.

## Configuration

Config lives at `~/.config/amux/config.toml` on Unix and `%APPDATA%\amux\config.toml` on Windows. Every section and key is optional.

```toml
# Shell to spawn in new panes. Bare name ("pwsh", "bash", "fish") is
# resolved against PATH, or an absolute path. Defaults to $SHELL on Unix
# and prefers pwsh.exe on Windows, falling back to $COMSPEC (cmd.exe).
# shell = "pwsh"

[appearance]
sidebar_width = 220
font_family = "IBM Plex Mono"
font_size = 14.0
theme = "dark"             # dark | light | system

[notifications]
sound = true
system_notifications = true
ring = true
auto_reorder_workspaces = true

[keybindings]
# Override any default. Use "cmd+…" on macOS or "ctrl+…" on Win/Linux.
# Full list of actions: see `KeybindingsConfig` in
# crates/amux-core/src/config.rs.
new_workspace = "cmd+n"
toggle_sidebar = "cmd+b"
```

## Building from Source

```bash
git clone https://github.com/daveowenatl/amux
cd amux
cargo build --workspace
cargo test --workspace
```

Requirements: Rust 1.80+, a C compiler, platform graphics drivers. Windows needs the MSVC toolchain. Before pushing, run `cargo fmt --check` and `cargo clippy --workspace -- -D warnings`; CI enforces both.

## Contributing

- Bugs and feature requests: [GitHub Issues](https://github.com/daveowenatl/amux/issues)
- Questions and ideas: [Discussions](https://github.com/daveowenatl/amux/discussions)
- PRs welcome.

## License

MIT. See [LICENSE](./LICENSE).
