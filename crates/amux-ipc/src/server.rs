use std::sync::mpsc as std_mpsc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

use crate::protocol::{Request, Response};
use crate::socket_path::{default_addr, write_last_addr, IpcAddr};

/// A command sent from the IPC server to the main (eframe) thread.
pub struct IpcCommand {
    pub request: Request,
    pub reply_tx: oneshot::Sender<Response>,
}

/// Start the IPC server on a background thread.
///
/// Returns the command receiver (for the main thread to drain) and the IPC address.
/// The socket path is only written after the bind succeeds, avoiding a race where
/// clients could discover an address that isn't ready yet.
pub fn start_server() -> anyhow::Result<(std_mpsc::Receiver<IpcCommand>, IpcAddr)> {
    let addr = default_addr();
    cleanup_stale(&addr);

    let (cmd_tx, cmd_rx) = std_mpsc::channel::<IpcCommand>();
    let (bind_tx, bind_rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let addr_clone = addr.clone();

    std::thread::Builder::new()
        .name("ipc-server".to_string())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(run_server(addr_clone, cmd_tx, bind_tx));
        })?;

    // Wait for the server thread to report bind success/failure
    match bind_rx.recv() {
        Ok(Ok(())) => {
            write_last_addr(&addr)?;
            Ok((cmd_rx, addr))
        }
        Ok(Err(e)) => anyhow::bail!("IPC server bind failed: {}", e),
        Err(_) => anyhow::bail!("IPC server thread exited before binding"),
    }
}

/// Remove a stale socket file (Unix only).
fn cleanup_stale(addr: &IpcAddr) {
    match addr {
        #[cfg(unix)]
        IpcAddr::Unix(path) => {
            let _ = std::fs::remove_file(path);
        }
        #[cfg(windows)]
        IpcAddr::NamedPipe(_) => {
            // Named pipes are kernel objects; no file to clean up.
        }
    }
}

// ---------------------------------------------------------------------------
// Unix implementation
// ---------------------------------------------------------------------------

#[cfg(unix)]
async fn run_server(
    addr: IpcAddr,
    cmd_tx: std_mpsc::Sender<IpcCommand>,
    bind_tx: std_mpsc::Sender<Result<(), String>>,
) {
    let IpcAddr::Unix(ref path) = addr;
    let listener = match tokio::net::UnixListener::bind(path) {
        Ok(l) => {
            tracing::info!("IPC server listening on {}", path.display());
            let _ = bind_tx.send(Ok(()));
            l
        }
        Err(e) => {
            tracing::error!("failed to bind IPC socket at {}: {}", path.display(), e);
            let _ = bind_tx.send(Err(e.to_string()));
            return;
        }
    };

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let tx = cmd_tx.clone();
                let (reader, writer) = stream.into_split();
                tokio::spawn(handle_connection(BufReader::new(reader), writer, tx));
            }
            Err(e) => {
                tracing::error!("accept error: {}", e);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Windows implementation
// ---------------------------------------------------------------------------

#[cfg(windows)]
async fn run_server(
    addr: IpcAddr,
    cmd_tx: std_mpsc::Sender<IpcCommand>,
    bind_tx: std_mpsc::Sender<Result<(), String>>,
) {
    use tokio::net::windows::named_pipe::ServerOptions;

    let IpcAddr::NamedPipe(ref name) = addr;
    let mut server = match ServerOptions::new().first_pipe_instance(true).create(name) {
        Ok(s) => {
            tracing::info!("IPC server listening on {}", name);
            let _ = bind_tx.send(Ok(()));
            s
        }
        Err(e) => {
            tracing::error!("failed to create named pipe {}: {}", name, e);
            let _ = bind_tx.send(Err(e.to_string()));
            return;
        }
    };

    loop {
        // Wait for a client to connect to the current pipe instance.
        if let Err(e) = server.connect().await {
            tracing::error!("named pipe connect error: {}", e);
            continue;
        }

        let connected = server;
        // Create a new pipe instance for the next client BEFORE handling this one.
        server = match ServerOptions::new().create(name) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("failed to create next named pipe instance: {}", e);
                return;
            }
        };

        let tx = cmd_tx.clone();
        let (reader, writer) = tokio::io::split(connected);
        tokio::spawn(handle_connection(BufReader::new(reader), writer, tx));
    }
}

// ---------------------------------------------------------------------------
// Shared connection handler
// ---------------------------------------------------------------------------

async fn handle_connection<R, W>(
    reader: BufReader<R>,
    mut writer: W,
    cmd_tx: std_mpsc::Sender<IpcCommand>,
) where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut lines = reader.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let request: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let err_resp = Response::err(String::new(), "parse_error", &e.to_string());
                let _ = write_response(&mut writer, &err_resp).await;
                continue;
            }
        };

        let (reply_tx, reply_rx) = oneshot::channel();
        let cmd = IpcCommand { request, reply_tx };

        if cmd_tx.send(cmd).is_err() {
            break; // main thread gone
        }

        // Wait for the main thread to process and reply.
        match reply_rx.await {
            Ok(response) => {
                let _ = write_response(&mut writer, &response).await;
            }
            Err(_) => break,
        }
    }
}

async fn write_response<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    response: &Response,
) -> anyhow::Result<()> {
    let mut json = serde_json::to_string(response)?;
    json.push('\n');
    writer.write_all(json.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}
