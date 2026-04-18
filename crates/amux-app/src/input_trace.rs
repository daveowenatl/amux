//! Env-gated file logging for diagnosing keyboard/input routing issues
//! (see amux #297, Windows keystroke drop). Enabled when the
//! `AMUX_LOG_INPUT` environment variable is set to a non-empty value.
//!
//! Writes append-only lines to `<temp_dir>/amux-input.log` so the log
//! survives on Windows GUI-subsystem builds that have no visible stdout.
//! Each line is prefixed with a millisecond timestamp (from process
//! start) and a site tag. Zero-cost when the env var is unset: the
//! check is a single `OnceLock` read returning a cached bool.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("AMUX_LOG_INPUT").is_some_and(|v| !v.is_empty()))
}

fn log_path() -> &'static PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| std::env::temp_dir().join("amux-input.log"))
}

fn epoch() -> &'static Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now)
}

pub(crate) fn log(site: &str, msg: impl std::fmt::Display) {
    if !enabled() {
        return;
    }
    let ms = epoch().elapsed().as_millis();
    let line = format!("{ms:>8} {site:<16} {msg}\n");
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
    {
        let _ = f.write_all(line.as_bytes());
    }
}
