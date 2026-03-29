use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};

use crate::protocol::{Request, Response};
use crate::socket_path::IpcAddr;

/// IPC client for connecting to the amux server.
pub struct IpcClient {
    #[cfg(unix)]
    reader: Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    #[cfg(unix)]
    writer: tokio::net::unix::OwnedWriteHalf,

    #[cfg(windows)]
    reader: Lines<BufReader<tokio::io::ReadHalf<tokio::net::windows::named_pipe::NamedPipeClient>>>,
    #[cfg(windows)]
    writer: tokio::io::WriteHalf<tokio::net::windows::named_pipe::NamedPipeClient>,
}

impl IpcClient {
    /// Connect to the amux IPC server at the given address.
    pub async fn connect(addr: &IpcAddr) -> anyhow::Result<Self> {
        #[cfg(unix)]
        {
            let IpcAddr::Unix(ref path) = addr;
            let stream = tokio::net::UnixStream::connect(path).await?;
            let (read_half, write_half) = stream.into_split();
            Ok(Self {
                reader: BufReader::new(read_half).lines(),
                writer: write_half,
            })
        }

        #[cfg(windows)]
        {
            let IpcAddr::NamedPipe(ref name) = addr;
            let client = tokio::net::windows::named_pipe::ClientOptions::new().open(name)?;
            let (read_half, write_half) = tokio::io::split(client);
            Ok(Self {
                reader: BufReader::new(read_half).lines(),
                writer: write_half,
            })
        }
    }

    /// Send a JSON-RPC request and wait for the response.
    pub async fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<Response> {
        let req = Request {
            id: uuid::Uuid::new_v4().to_string(),
            method: method.to_string(),
            params,
        };
        let mut json = serde_json::to_string(&req)?;
        json.push('\n');
        self.writer.write_all(json.as_bytes()).await?;
        self.writer.flush().await?;

        let line = self
            .reader
            .next_line()
            .await?
            .ok_or_else(|| anyhow::anyhow!("connection closed"))?;
        let resp: Response = serde_json::from_str(&line)?;
        Ok(resp)
    }

    /// Read the next line from the server (event or response).
    /// Returns `None` if the connection is closed.
    pub async fn read_line(&mut self) -> anyhow::Result<Option<String>> {
        Ok(self.reader.next_line().await?)
    }
}
