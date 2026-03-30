pub mod client;
pub mod methods;
pub mod protocol;
pub mod server;
pub mod socket_path;

pub use client::IpcClient;
pub use protocol::ServerEvent;
pub use protocol::{Request, Response, RpcError};
pub use server::{start_server, EventBroadcaster, IpcCommand, EVENT_TYPES};
pub use socket_path::{read_last_addr, read_last_token, write_last_token, IpcAddr};

/// Typed errors for IPC operations.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("IPC server bind failed: {0}")]
    BindFailed(String),

    #[error("server thread exited before binding")]
    ServerThreadDied,

    #[error("IPC auth token must not be empty")]
    EmptyToken,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("protocol error: {0}")]
    Protocol(#[from] serde_json::Error),

    #[error("authentication failed: {0}")]
    AuthFailed(String),

    #[error("connection closed")]
    ConnectionClosed,
}
