use std::{
    io::{self, ErrorKind},
    net::Ipv4Addr,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
};

use anyhow::{Context, Result, bail, ensure};
use dtshim_core::{
    env::{SOCKET_PATH, TRANSPORT},
    socket::{Connection, Transport},
};
use futures::future::BoxFuture;
use tempfile::TempDir;
use tokio::net::{TcpListener, UnixListener};

pub(crate) trait SocketListener: Send + Sync {
    fn address(&self) -> Result<String>;
    fn accept(&self) -> BoxFuture<'_, io::Result<Box<dyn Connection>>>;
}

struct TcpSocket {
    listener: TcpListener,
}

impl TcpSocket {
    async fn bind() -> Result<Self> {
        for port in 20001..=u16::MAX {
            match TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
                Ok(listener) => return Ok(Self { listener }),
                Err(err) if err.kind() == ErrorKind::AddrInUse => continue,
                Err(err) => {
                    return Err(err)
                        .with_context(|| format!("failed to bind TCP socket 127.0.0.1:{port}"));
                }
            }
        }

        bail!("no free TCP port on 127.0.0.1 in range 20001..=65535")
    }
}

impl SocketListener for TcpSocket {
    fn address(&self) -> Result<String> {
        Ok(self.listener.local_addr()?.to_string())
    }

    fn accept(&self) -> BoxFuture<'_, io::Result<Box<dyn Connection>>> {
        Box::pin(async move {
            self.listener
                .accept()
                .await
                .map(|(stream, _)| Box::new(stream) as Box<dyn Connection>)
        })
    }
}

struct UnixSocket {
    listener: UnixListener,
    path: PathBuf,
    _temp_dir: Option<TempDir>,
}

impl UnixSocket {
    fn bind(path: Option<PathBuf>) -> Result<Self> {
        let (path, temp_dir) = match path {
            Some(path) => (path, None),
            None => {
                let temp_dir = tempfile::Builder::new()
                    .prefix("dtshim_")
                    .permissions(std::fs::Permissions::from_mode(0o700))
                    .tempdir()
                    .context("failed to create socket directory")?;
                (temp_dir.path().join("dtshim.sock"), Some(temp_dir))
            }
        };
        let listener = UnixListener::bind(&path)
            .with_context(|| format!("failed to bind socket {}", path.display()))?;

        Ok(Self {
            listener,
            path,
            _temp_dir: temp_dir,
        })
    }
}

impl Drop for UnixSocket {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_file(&self.path) {
            eprintln!("failed to remove socket {}: {}", self.path.display(), err)
        }
    }
}

impl SocketListener for UnixSocket {
    fn address(&self) -> Result<String> {
        Ok(self.path.to_str().context("invalid socket path")?.into())
    }

    fn accept(&self) -> BoxFuture<'_, io::Result<Box<dyn Connection>>> {
        Box::pin(async move {
            self.listener
                .accept()
                .await
                .map(|(stream, _)| Box::new(stream) as Box<dyn Connection>)
        })
    }
}

pub(crate) async fn bind(
    transport: Transport,
    path: Option<PathBuf>,
) -> Result<Box<dyn SocketListener>> {
    match transport {
        Transport::Tcp => {
            ensure!(
                path.is_none(),
                "--socket / {SOCKET_PATH} requires unix transport (--transport unix or {TRANSPORT}=unix)"
            );
            Ok(Box::new(TcpSocket::bind().await?))
        }
        Transport::Unix => Ok(Box::new(UnixSocket::bind(path)?)),
    }
}
