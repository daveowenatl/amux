use amux_ipc::{read_last_addr, IpcAddr, IpcClient};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "amux", about = "Terminal multiplexer for AI coding agents")]
struct Cli {
    /// Socket path (auto-detected if omitted)
    #[arg(long, global = true)]
    socket: Option<String>,

    /// Output as JSON
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check if the amux server is running
    Ping,
    /// List surfaces (hierarchical view)
    Tree,
    /// Send text to a surface
    Send {
        /// Text to send
        text: String,
        /// Target surface ID
        #[arg(long)]
        surface: Option<String>,
    },
    /// Read screen text from a surface
    ReadScreen {
        /// Target surface ID
        #[arg(long)]
        surface: Option<String>,
    },
    /// List server capabilities
    Capabilities,
    /// Identify focused workspace/surface
    Identify,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let addr = resolve_addr(&cli)?;
    let mut client = IpcClient::connect(&addr).await?;

    match cli.command {
        Command::Ping => {
            let resp = client.call("system.ping", serde_json::json!({})).await?;
            print_response(&resp, cli.json);
        }
        Command::Tree => {
            let resp = client.call("surface.list", serde_json::json!({})).await?;
            if cli.json {
                print_response(&resp, true);
            } else if let Some(result) = &resp.result {
                print_tree(result);
            }
        }
        Command::Send { text, surface } => {
            let surface_id = surface
                .or_else(|| std::env::var("AMUX_SURFACE_ID").ok())
                .unwrap_or_else(|| "default".to_string());
            let resp = client
                .call(
                    "surface.send_text",
                    serde_json::json!({
                        "surface_id": surface_id,
                        "text": text,
                    }),
                )
                .await?;
            print_response(&resp, cli.json);
        }
        Command::ReadScreen { surface } => {
            let surface_id = surface
                .or_else(|| std::env::var("AMUX_SURFACE_ID").ok())
                .unwrap_or_else(|| "default".to_string());
            let resp = client
                .call(
                    "surface.read_text",
                    serde_json::json!({
                        "surface_id": surface_id,
                    }),
                )
                .await?;
            if cli.json {
                print_response(&resp, true);
            } else if let Some(result) = &resp.result {
                if let Some(text) = result.get("text").and_then(|t| t.as_str()) {
                    println!("{}", text);
                }
            }
        }
        Command::Capabilities => {
            let resp = client
                .call("system.capabilities", serde_json::json!({}))
                .await?;
            print_response(&resp, cli.json);
        }
        Command::Identify => {
            let resp = client
                .call("system.identify", serde_json::json!({}))
                .await?;
            print_response(&resp, cli.json);
        }
    }
    Ok(())
}

fn resolve_addr(cli: &Cli) -> anyhow::Result<IpcAddr> {
    if let Some(ref socket) = cli.socket {
        return Ok(IpcAddr::from_stored(socket));
    }

    // Check environment variable
    if let Ok(path) = std::env::var("AMUX_SOCKET_PATH") {
        return Ok(IpcAddr::from_stored(&path));
    }

    // Fall back to last-known address
    read_last_addr()
}

fn print_response(resp: &amux_ipc::Response, json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(resp).unwrap());
    } else if resp.ok {
        if let Some(result) = &resp.result {
            println!("{}", serde_json::to_string_pretty(result).unwrap());
        }
    } else if let Some(err) = &resp.error {
        eprintln!("error [{}]: {}", err.code, err.message);
        std::process::exit(1);
    }
}

fn print_tree(result: &serde_json::Value) {
    if let Some(surfaces) = result.get("surfaces").and_then(|s| s.as_array()) {
        println!("workspace: default");
        for (i, surface) in surfaces.iter().enumerate() {
            let prefix = if i == surfaces.len() - 1 {
                "└──"
            } else {
                "├──"
            };
            let id = surface.get("id").and_then(|v| v.as_str()).unwrap_or("?");
            let title = surface.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let cols = surface.get("cols").and_then(|v| v.as_u64()).unwrap_or(0);
            let rows = surface.get("rows").and_then(|v| v.as_u64()).unwrap_or(0);
            let alive = surface
                .get("alive")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let status = if alive { "running" } else { "exited" };
            println!(
                "  {} surface:{} \"{}\" {}x{} [{}]",
                prefix, id, title, cols, rows, status
            );
        }
    }
}
