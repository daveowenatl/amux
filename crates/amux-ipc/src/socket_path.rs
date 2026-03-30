use std::fmt;
use std::path::PathBuf;

/// Platform-abstracted IPC address.
#[derive(Debug, Clone)]
pub enum IpcAddr {
    /// Unix domain socket path (macOS/Linux).
    #[cfg(unix)]
    Unix(PathBuf),

    /// Windows named pipe name (e.g. `\\.\pipe\amux-1234`).
    #[cfg(windows)]
    NamedPipe(String),
}

impl fmt::Display for IpcAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(unix)]
            IpcAddr::Unix(p) => write!(f, "{}", p.display()),
            #[cfg(windows)]
            IpcAddr::NamedPipe(name) => write!(f, "{}", name),
        }
    }
}

impl IpcAddr {
    /// Serialize to string for storage.
    pub fn to_string_lossy(&self) -> String {
        self.to_string()
    }

    /// Parse from stored string.
    pub fn from_stored(s: &str) -> Self {
        #[cfg(unix)]
        {
            IpcAddr::Unix(PathBuf::from(s))
        }
        #[cfg(windows)]
        {
            IpcAddr::NamedPipe(s.to_string())
        }
    }
}

/// Compute the IPC address for this process.
pub fn default_addr() -> IpcAddr {
    let pid = std::process::id();

    #[cfg(unix)]
    {
        let dir = std::env::var("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        IpcAddr::Unix(dir.join(format!("amux-{}.sock", pid)))
    }

    #[cfg(windows)]
    {
        IpcAddr::NamedPipe(format!(r"\\.\pipe\amux-{}", pid))
    }
}

/// Write the IPC address to `{data_dir}/amux/last-socket-path`
/// so the CLI can discover it without knowing the PID.
pub fn write_last_addr(addr: &IpcAddr) -> anyhow::Result<()> {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("amux");
    std::fs::create_dir_all(&data_dir)?;
    std::fs::write(data_dir.join("last-socket-path"), addr.to_string())?;
    Ok(())
}

/// Read the last-known IPC address (for CLI auto-discovery).
pub fn read_last_addr() -> anyhow::Result<IpcAddr> {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("amux");
    let content = std::fs::read_to_string(data_dir.join("last-socket-path"))?;
    Ok(IpcAddr::from_stored(content.trim()))
}

/// Write the auth token to `{data_dir}/amux/last-socket-token`
/// so the CLI can discover it alongside the socket address.
pub fn write_last_token(token: &str) -> anyhow::Result<()> {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("amux");
    std::fs::create_dir_all(&data_dir)?;
    std::fs::write(data_dir.join("last-socket-token"), token)?;
    Ok(())
}

/// Read the last-known auth token (for CLI auto-discovery).
pub fn read_last_token() -> anyhow::Result<String> {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("amux");
    let content = std::fs::read_to_string(data_dir.join("last-socket-token"))?;
    Ok(content.trim().to_string())
}
