//! Per-agent wrapper logic. Each sub-module handles the injection
//! mechanism specific to its agent (`--settings` for Claude,
//! `GEMINI_CLI_SYSTEM_SETTINGS_PATH` for Gemini) and the passthrough
//! path when the wrapper isn't running inside an amux pane.

pub(crate) mod claude;
pub(crate) mod gemini;
