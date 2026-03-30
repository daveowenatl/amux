use std::collections::HashSet;
use std::sync::mpsc as std_mpsc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, oneshot};

use crate::protocol::{AuthMessage, AuthResponse, Request, Response, ServerEvent};
use crate::socket_path::{default_addr, write_last_addr, IpcAddr};

/// A command sent from the IPC server to the main (eframe) thread.
pub struct IpcCommand {
    pub request: Request,
    pub reply_tx: oneshot::Sender<Response>,
}

/// Handle returned from `start_server` for sending events to connected clients.
#[derive(Clone)]
pub struct EventBroadcaster {
    tx: broadcast::Sender<ServerEvent>,
}

impl EventBroadcaster {
    /// Broadcast an event to all subscribed clients. Silently drops if no receivers.
    pub fn send(&self, event: ServerEvent) {
        let _ = self.tx.send(event);
    }
}

/// Start the IPC server on a background thread.
///
/// Returns the command receiver (for the main thread to drain), the IPC address,
/// and an `EventBroadcaster` for pushing events to subscribed clients.
pub fn start_server(
    token: String,
) -> anyhow::Result<(std_mpsc::Receiver<IpcCommand>, IpcAddr, EventBroadcaster)> {
    let addr = default_addr();
    cleanup_stale(&addr);

    let (cmd_tx, cmd_rx) = std_mpsc::channel::<IpcCommand>();
    let (bind_tx, bind_rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let (event_tx, _) = broadcast::channel::<ServerEvent>(256);
    let broadcaster = EventBroadcaster {
        tx: event_tx.clone(),
    };
    let addr_clone = addr.clone();

    std::thread::Builder::new()
        .name("ipc-server".to_string())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(run_server(addr_clone, cmd_tx, bind_tx, event_tx, token));
        })?;

    // Wait for the server thread to report bind success/failure
    match bind_rx.recv() {
        Ok(Ok(())) => {
            write_last_addr(&addr)?;
            Ok((cmd_rx, addr, broadcaster))
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
    event_tx: broadcast::Sender<ServerEvent>,
    token: String,
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
                let event_rx = event_tx.subscribe();
                let tok = token.clone();
                let (reader, writer) = stream.into_split();
                tokio::spawn(handle_connection(
                    BufReader::new(reader),
                    writer,
                    tx,
                    event_rx,
                    tok,
                ));
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
    event_tx: broadcast::Sender<ServerEvent>,
    token: String,
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
        let event_rx = event_tx.subscribe();
        let tok = token.clone();
        let (reader, writer) = tokio::io::split(connected);
        tokio::spawn(handle_connection(
            BufReader::new(reader),
            writer,
            tx,
            event_rx,
            tok,
        ));
    }
}

// ---------------------------------------------------------------------------
// Shared connection handler
// ---------------------------------------------------------------------------

/// Known event types that clients can subscribe to.
pub const EVENT_TYPES: &[&str] = &[
    "notification",
    "surface_exit",
    "focus_change",
    "status_change",
];

async fn handle_connection<R, W>(
    reader: BufReader<R>,
    mut writer: W,
    cmd_tx: std_mpsc::Sender<IpcCommand>,
    mut event_rx: broadcast::Receiver<ServerEvent>,
    expected_token: String,
) where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut lines = reader.lines();

    // --- Auth handshake: first message must be a valid token ---
    let auth_line = match lines.next_line().await {
        Ok(Some(line)) => line,
        _ => return,
    };
    let auth_ok = match serde_json::from_str::<AuthMessage>(&auth_line) {
        Ok(msg) => msg.token == expected_token,
        Err(_) => false,
    };
    if !auth_ok {
        let resp = AuthResponse {
            ok: false,
            error: Some("unauthorized".to_string()),
        };
        let _ = write_json(&mut writer, &resp).await;
        return;
    }
    let resp = AuthResponse {
        ok: true,
        error: None,
    };
    if write_json(&mut writer, &resp).await.is_err() {
        return;
    }

    // --- Authenticated: proceed with normal request handling ---
    let mut subscriptions: HashSet<String> = HashSet::new();

    loop {
        // If we have subscriptions, use select! to handle both requests and events.
        // Otherwise, just wait for the next request line.
        if subscriptions.is_empty() {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if !handle_request_line(&line, &cmd_tx, &mut writer, &mut subscriptions).await {
                        break;
                    }
                }
                _ => break,
            }
        } else {
            tokio::select! {
                line_result = lines.next_line() => {
                    match line_result {
                        Ok(Some(line)) => {
                            if !handle_request_line(
                                &line,
                                &cmd_tx,
                                &mut writer,
                                &mut subscriptions,
                            ).await {
                                break;
                            }
                        }
                        _ => break,
                    }
                }
                event_result = event_rx.recv() => {
                    match event_result {
                        Ok(event) => {
                            if subscriptions.contains(&event.event)
                                && write_event(&mut writer, &event).await.is_err()
                            {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("IPC client lagged, dropped {} events", n);
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    }
}

/// Handle a single request line. Returns `false` if the connection should close.
async fn handle_request_line<W: tokio::io::AsyncWrite + Unpin>(
    line: &str,
    cmd_tx: &std_mpsc::Sender<IpcCommand>,
    writer: &mut W,
    subscriptions: &mut HashSet<String>,
) -> bool {
    let request: Request = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => {
            let err_resp = Response::err(String::new(), "parse_error", &e.to_string());
            let _ = write_response(writer, &err_resp).await;
            return true;
        }
    };

    // Handle subscribe/unsubscribe locally in the server (no main thread round-trip).
    if request.method == "subscribe" {
        let response = handle_subscribe(&request, subscriptions);
        let _ = write_response(writer, &response).await;
        return true;
    }
    if request.method == "unsubscribe" {
        let response = handle_unsubscribe(&request, subscriptions);
        let _ = write_response(writer, &response).await;
        return true;
    }

    let (reply_tx, reply_rx) = oneshot::channel();
    let cmd = IpcCommand { request, reply_tx };

    if cmd_tx.send(cmd).is_err() {
        return false; // main thread gone
    }

    match reply_rx.await {
        Ok(response) => {
            let _ = write_response(writer, &response).await;
            true
        }
        Err(_) => false,
    }
}

fn handle_subscribe(request: &Request, subscriptions: &mut HashSet<String>) -> Response {
    let events: Vec<String> = match serde_json::from_value(
        request
            .params
            .get("events")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    ) {
        Ok(v) => v,
        Err(_) => {
            return Response::err(
                request.id.clone(),
                "invalid_params",
                "params.events must be an array of strings",
            );
        }
    };

    let mut subscribed = Vec::new();
    let mut unknown = Vec::new();
    for event in &events {
        if EVENT_TYPES.contains(&event.as_str()) {
            subscriptions.insert(event.clone());
            subscribed.push(event.clone());
        } else {
            unknown.push(event.clone());
        }
    }

    let mut result = serde_json::json!({ "subscribed": subscribed });
    if !unknown.is_empty() {
        result["unknown"] = serde_json::json!(unknown);
    }
    Response::ok(request.id.clone(), result)
}

fn handle_unsubscribe(request: &Request, subscriptions: &mut HashSet<String>) -> Response {
    let events: Vec<String> = match serde_json::from_value(
        request
            .params
            .get("events")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    ) {
        Ok(v) => v,
        Err(_) => {
            return Response::err(
                request.id.clone(),
                "invalid_params",
                "params.events must be an array of strings",
            );
        }
    };

    let mut unsubscribed = Vec::new();
    for event in &events {
        if subscriptions.remove(event) {
            unsubscribed.push(event.clone());
        }
    }

    Response::ok(
        request.id.clone(),
        serde_json::json!({ "unsubscribed": unsubscribed }),
    )
}

async fn write_json<W: tokio::io::AsyncWrite + Unpin, T: serde::Serialize>(
    writer: &mut W,
    value: &T,
) -> anyhow::Result<()> {
    let mut json = serde_json::to_string(value)?;
    json.push('\n');
    writer.write_all(json.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

async fn write_response<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    response: &Response,
) -> anyhow::Result<()> {
    write_json(writer, response).await
}

async fn write_event<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    event: &ServerEvent,
) -> anyhow::Result<()> {
    write_json(writer, event).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Request;

    fn make_request(id: &str, method: &str, params: serde_json::Value) -> Request {
        Request {
            id: id.to_string(),
            method: method.to_string(),
            params,
        }
    }

    #[test]
    fn subscribe_valid_events() {
        let mut subs = HashSet::new();
        let req = make_request(
            "1",
            "subscribe",
            serde_json::json!({"events": ["notification", "focus_change"]}),
        );
        let resp = handle_subscribe(&req, &mut subs);
        assert!(resp.ok);
        assert!(subs.contains("notification"));
        assert!(subs.contains("focus_change"));
        assert_eq!(subs.len(), 2);
    }

    #[test]
    fn subscribe_reports_unknown_events() {
        let mut subs = HashSet::new();
        let req = make_request(
            "2",
            "subscribe",
            serde_json::json!({"events": ["notification", "bogus"]}),
        );
        let resp = handle_subscribe(&req, &mut subs);
        assert!(resp.ok);
        assert!(subs.contains("notification"));
        assert!(!subs.contains("bogus"));
        assert_eq!(subs.len(), 1);
        let result = resp.result.unwrap();
        let unknown = result.get("unknown").unwrap().as_array().unwrap();
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].as_str().unwrap(), "bogus");
    }

    #[test]
    fn subscribe_no_unknown_field_when_all_valid() {
        let mut subs = HashSet::new();
        let req = make_request(
            "2b",
            "subscribe",
            serde_json::json!({"events": ["notification"]}),
        );
        let resp = handle_subscribe(&req, &mut subs);
        assert!(resp.ok);
        let result = resp.result.unwrap();
        assert!(result.get("unknown").is_none());
    }

    #[test]
    fn subscribe_invalid_params() {
        let mut subs = HashSet::new();
        let req = make_request(
            "3",
            "subscribe",
            serde_json::json!({"events": "not_an_array"}),
        );
        let resp = handle_subscribe(&req, &mut subs);
        assert!(!resp.ok);
        assert!(subs.is_empty());
    }

    #[test]
    fn unsubscribe_removes_events() {
        let mut subs = HashSet::new();
        subs.insert("notification".to_string());
        subs.insert("focus_change".to_string());
        let req = make_request(
            "4",
            "unsubscribe",
            serde_json::json!({"events": ["notification"]}),
        );
        let resp = handle_unsubscribe(&req, &mut subs);
        assert!(resp.ok);
        assert!(!subs.contains("notification"));
        assert!(subs.contains("focus_change"));
        let result = resp.result.unwrap();
        let unsubscribed = result.get("unsubscribed").unwrap().as_array().unwrap();
        assert_eq!(unsubscribed.len(), 1);
        assert_eq!(unsubscribed[0].as_str().unwrap(), "notification");
    }

    #[test]
    fn unsubscribe_nonexistent_returns_empty() {
        let mut subs = HashSet::new();
        let req = make_request("5", "unsubscribe", serde_json::json!({"events": ["bogus"]}));
        let resp = handle_unsubscribe(&req, &mut subs);
        assert!(resp.ok);
        let result = resp.result.unwrap();
        let unsubscribed = result.get("unsubscribed").unwrap().as_array().unwrap();
        assert!(unsubscribed.is_empty());
    }

    #[test]
    fn event_types_contains_expected() {
        assert!(EVENT_TYPES.contains(&"notification"));
        assert!(EVENT_TYPES.contains(&"surface_exit"));
        assert!(EVENT_TYPES.contains(&"focus_change"));
        assert!(EVENT_TYPES.contains(&"status_change"));
    }
}
