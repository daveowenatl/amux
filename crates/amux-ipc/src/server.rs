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
pub fn start_server() -> anyhow::Result<(std_mpsc::Receiver<IpcCommand>, IpcAddr)> {
    let addr = default_addr();
    cleanup_stale(&addr);
    write_last_addr(&addr)?;

    let (cmd_tx, cmd_rx) = std_mpsc::channel::<IpcCommand>();
    let addr_clone = addr.clone();

    std::thread::Builder::new()
        .name("ipc-server".to_string())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(run_server(addr_clone, cmd_tx));
        })?;

    Ok((cmd_rx, addr))
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
async fn run_server(addr: IpcAddr, cmd_tx: std_mpsc::Sender<IpcCommand>) {
    let IpcAddr::Unix(ref path) = addr;
    let listener = tokio::net::UnixListener::bind(path).expect("bind unix socket");
    tracing::info!("IPC server listening on {}", path.display());

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
async fn run_server(addr: IpcAddr, cmd_tx: std_mpsc::Sender<IpcCommand>) {
    use tokio::net::windows::named_pipe::ServerOptions;

    let IpcAddr::NamedPipe(ref name) = addr;
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(name)
        .expect("create named pipe");
    tracing::info!("IPC server listening on {}", name);

    loop {
        // Wait for a client to connect to the current pipe instance.
        if let Err(e) = server.connect().await {
            tracing::error!("named pipe connect error: {}", e);
            continue;
        }

        let connected = server;
        // Create a new pipe instance for the next client BEFORE handling this one.
        server = ServerOptions::new()
            .create(name)
            .expect("create next named pipe instance");

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
