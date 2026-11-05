use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use dtshim_core::{
    env::{SOCKET_PATH, TCP_ADDR, TRANSPORT},
    socket::{Connection, Transport},
};
use futures::future::BoxFuture;
use tokio::net::{TcpStream, UnixStream};

pub(crate) trait SocketConnector: Send + Sync {
    fn connect(&self) -> BoxFuture<'_, Result<Box<dyn Connection>>>;
}

struct TcpSocket {
    address: String,
}

impl SocketConnector for TcpSocket {
    fn connect(&self) -> BoxFuture<'_, Result<Box<dyn Connection>>> {
        Box::pin(async move {
            let stream = TcpStream::connect(self.address.as_str())
                .await
                .with_context(|| format!("failed to connect to TCP socket {}", self.address))?;
            Ok(Box::new(stream) as Box<dyn Connection>)
        })
    }
}

struct UnixSocket {
    path: PathBuf,
}

impl SocketConnector for UnixSocket {
    fn connect(&self) -> BoxFuture<'_, Result<Box<dyn Connection>>> {
        Box::pin(async move {
            let stream = UnixStream::connect(&self.path).await.with_context(|| {
                format!("failed to connect to Unix socket {}", self.path.display())
            })?;
            Ok(Box::new(stream) as Box<dyn Connection>)
        })
    }
}

pub(crate) fn connector(
    transport: Transport,
    tcp_addr: Option<String>,
    path: Option<PathBuf>,
) -> Result<Box<dyn SocketConnector>> {
    match transport {
        Transport::Tcp => {
            ensure!(
                path.is_none(),
                "{SOCKET_PATH} requires {TRANSPORT}=unix"
            );
            let address = tcp_addr
                .with_context(|| format!("{TCP_ADDR} must be set for TCP transport"))?;
            let (host, port) = address
                .rsplit_once(':')
                .with_context(|| format!("{TCP_ADDR} must be HOST:PORT"))?;
            ensure!(!host.is_empty(), "TCP host must not be empty");
            let port = port
                .parse::<u16>()
                .context("TCP port must be in 1..=65535")?;
            ensure!(port != 0, "TCP port must be in 1..=65535");
            ensure!(
                !host
                    .chars()
                    .any(|ch| ch.is_whitespace() || matches!(ch, ':' | '[' | ']' | '/')),
                "invalid TCP host"
            );
            Ok(Box::new(TcpSocket { address }))
        }
        Transport::Unix => {
            ensure!(
                tcp_addr.is_none(),
                "{TCP_ADDR} requires {TRANSPORT}=tcp"
            );
            let path = path
                .with_context(|| format!("{SOCKET_PATH} must be set for Unix transport"))?;
            ensure!(
                !path.as_os_str().is_empty(),
                "Unix socket path must not be empty"
            );
            Ok(Box::new(UnixSocket { path }))
        }
    }
}
