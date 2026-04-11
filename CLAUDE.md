# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

amux is a cross-platform terminal multiplexer for AI coding agents (Claude Code, Gemini CLI, Codex CLI). Built in Rust with GPU-accelerated rendering via wgpu (Metal on macOS, DX12/Vulkan on Windows, Vulkan on Linux) and `libghostty-vt` for the VT state machine.

## Build Commands

```bash
cargo build --workspace        # Build all crates
cargo build --release          # Release build
cargo test --workspace         # Run all tests
cargo test -p amux-term        # Run tests for a single crate
cargo test <test_name>         # Run a single test by name
cargo clippy --workspace -- -D warnings  # Lint (treat warnings as errors)
cargo fmt                      # Format code
cargo fmt --check              # Check formatting without modifying
```

Requirements: Rust 1.80+, C compiler, platform graphics drivers. Windows needs MSVC toolchain.

**Before pushing any commit**, always run `cargo fmt --check` and `cargo clippy --workspace -- -D warnings` to catch lint and formatting issues locally. CI will reject PRs that fail these checks.

## Workspace Structure

Cargo workspace with 9 crates under `crates/`:

| Crate | Type | Purpose |
|---|---|---|
| `amux-term` | lib | Terminal pane abstraction (`libghostty-vt` + portable-pty). Key/mouse encoders, OSC handling, color resolution. |
| `amux-app` | bin | Main binary: GUI + event loop (eframe/winit) |
| `amux-cli` | bin | CLI binary (socket client) |
| `amux-render-soft` | lib | Softbuffer renderer (Phase 1–7) |
| `amux-render-gpu` | lib | wgpu + cosmic-text GPU renderer (Phase 8) |
| `amux-ipc` | lib | Socket server + JSON-RPC protocol |
| `amux-layout` | lib | PaneTree binary tree layout engine |
| `amux-notify` | lib | OSC notification parsing + in-app store |
| `amux-session` | lib | Session persistence (save/restore JSON) |

Key dependency: `libghostty-vt` is patched to a fork at `github.com/daveowenatl/libghostty-rs` rev `cabcfb81cc3f4f20ef9b62312df6bb04c929abb5`. The fork cherry-picks unpublished fixes for Windows — upstream's `build.rs` hardcodes `libghostty-vt.so.0.1.0` as the expected shared-library filename, which panics on Windows where Zig emits `ghostty-vt.dll` + `ghostty-vt.lib`.

## Architecture

### Rendering
wgpu for GPU rendering with platform-specific backends. `libghostty-vt` handles the VT / terminal state machine. PTY streams are monitored for OSC 9/99/777 sequences.

### Agent Integrations
Three first-class agent integrations, each using the agent's native event system:
- **Claude Code**: Hooks into all 9 hook events (`SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Notification`, `Stop`, `SubagentStart`, `SubagentStop`, `SessionEnd`) via a wrapper at `~/.config/amux/bin/claude` that injects hooks through Claude's `--settings` CLI flag. Each event is handled by `amux claude-hook <event>` in `crates/amux-cli/src/claude_hook.rs`, with a pure `status_update_for` helper for unit-testable coverage. `Notification` events surface the payload's `message` field as the status message rather than a generic label. `PostToolUse` / `SubagentStop` clear the per-tool message while keeping the agent in the active state so the next hook overwrites cleanly.
- **Gemini CLI**: Hooks into 6 events (`BeforeAgent`, `AfterAgent`, `BeforeTool`, `Notification`, `SessionStart`, `SessionEnd`) via a wrapper at `~/.config/amux/bin/gemini` that injects hooks using `GEMINI_CLI_SYSTEM_SETTINGS_PATH`. Because Gemini's hook arrays use `CONCAT` merging, injection is additive — user's `~/.gemini/settings.json` is untouched. Requires Gemini ≥ v0.26.0 for hooks; older versions fall back to parsing the dynamic window title state machine (◇ Ready / ✦ Working / ✋ Action Required) as a coarse status signal, captured via `NotificationEvent::TitleChanged` and `gemini_title::parse_gemini_title`.
- **Codex CLI**: Hooks into 5 events (`SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Stop`) via a wrapper at `~/.config/amux/bin/codex` that builds a per-session `CODEX_HOME` tempdir. The tempdir symlinks the user's real `~/.codex/` and overlays amux's own `hooks.json` and `managed_config.toml` (the latter sets `[features] codex_hooks = true` via Codex's system-managed config layer). User's real `~/.codex/` is never touched. Full `codex app-server` integration with approval interception is deferred.
- **Generic**: Any agent can integrate via environment variables (`AMUX_WORKSPACE_ID`, `AMUX_SURFACE_ID`, `AMUX_SOCKET_PATH`) and the `amux set-status` / `amux notify` CLI

### Core Concepts
- **Workspace**: Top-level container, shown in sidebar with status pills
- **Surface**: Tabs within a workspace
- **Pane**: Split views within a surface, each running a PTY
- **Notification ring**: Blue ring on panes + sidebar tab highlight when agents need attention
- **Notification panel**: Centralized view of all pending notifications across workspaces

### APIs
- **CLI**: `amux` subcommands for workspace/pane management, hook installation, status reporting
- **Socket API**: Unix domain socket at `$AMUX_SOCKET_PATH`
- **tmux compat shim**: `amux install-tmux-shim` routes `tmux` calls to amux for agent scripts written for tmux

### Configuration
Config file: `~/.config/amux/config.toml` (or `%APPDATA%\amux\config.toml` on Windows). Covers appearance, notifications, keybindings, and agent overrides.

### Session Restore
Restores layout, working directories, scrollback (up to 4k lines/surface), status pills, and notification history. Does **not** restore live agent process state.
