# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

amux is a cross-platform terminal multiplexer for AI coding agents (Claude Code, Gemini CLI, Codex CLI). Built in Rust with GPU-accelerated rendering via wgpu (Metal on macOS, DX12/Vulkan on Windows, Vulkan on Linux) and wezterm-term for VT state machine.

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

## Workspace Structure

Cargo workspace with 9 crates under `crates/`:

| Crate | Type | Purpose |
|---|---|---|
| `amux-term` | lib | Terminal pane abstraction (wezterm-term + portable-pty). Key/mouse encoders, OSC handling, color resolution. |
| `amux-app` | bin | Main binary: GUI + event loop (eframe/winit) |
| `amux-cli` | bin | CLI binary (socket client) |
| `amux-render-soft` | lib | Softbuffer renderer (Phase 1–7) |
| `amux-render-gpu` | lib | wgpu + cosmic-text GPU renderer (Phase 8) |
| `amux-ipc` | lib | Socket server + JSON-RPC protocol |
| `amux-layout` | lib | PaneTree binary tree layout engine |
| `amux-notify` | lib | OSC notification parsing + in-app store |
| `amux-session` | lib | Session persistence (save/restore JSON) |

Key dependency: `wezterm-term` is a git dependency pinned to rev `05343b3` from the wezterm monorepo.

## Architecture

### Rendering
wgpu for GPU rendering with platform-specific backends. wezterm-term handles VT/terminal state machine. PTY streams are monitored for OSC 9/99/777 sequences.

### Agent Integrations
Three first-class agent integrations, each using the agent's native event system:
- **Claude Code**: Hooks into all 9 hook events (`PreToolUse`, `Stop`, etc.)
- **Gemini CLI**: Hooks into 7/11 events + window title state machine parsing + OSC 9 notifications (sets `TERM_PROGRAM=wezterm` to trigger Gemini's OSC output)
- **Codex CLI**: JSON-RPC via `app-server` subprocess for real-time events and approval interception; falls back to hooks when Codex runs in TUI mode
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
