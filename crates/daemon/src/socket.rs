use std::{
    io::{self, ErrorKind},
    net::Ipv4Addr,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
};

use anyhow::{Context, Result, bail};
use tempfile::TempDir;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream, UnixListener, UnixStream},
};

pub(crate) trait SocketListener: Send + Sync {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    fn address(&self) -> Result<String>;
    async fn accept(&self) -> io::Result<Self::Stream>;
}

pub(crate) struct TcpSocket {
    listener: TcpListener,
}

impl TcpSocket {
    pub(crate) async fn bind() -> Result<Self> {
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
    type Stream = TcpStream;

    fn address(&self) -> Result<String> {
        Ok(self.listener.local_addr()?.to_string())
    }

    async fn accept(&self) -> io::Result<Self::Stream> {
        self.listener.accept().await.map(|(stream, _)| stream)
    }
}

pub(crate) struct UnixSocket {
    listener: UnixListener,
    path: PathBuf,
    _temp_dir: Option<TempDir>,
}

impl UnixSocket {
    pub(crate) fn bind(path: Option<PathBuf>) -> Result<Self> {
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
    type Stream = UnixStream;

    fn address(&self) -> Result<String> {
        Ok(self.path.to_str().context("invalid socket path")?.into())
    }

    async fn accept(&self) -> io::Result<Self::Stream> {
        self.listener.accept().await.map(|(stream, _)| stream)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        os::unix::{ffi::OsStringExt, fs::FileTypeExt},
        time::Duration,
    };

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        time::timeout,
    };

    use super::*;

    #[tokio::test]
    async fn tcp_binds_loopback_skips_occupied_ports_and_accepts_connections() {
        let first = TcpSocket::bind().await.unwrap();
        let second = TcpSocket::bind().await.unwrap();
        let address: std::net::SocketAddr = first.address().unwrap().parse().unwrap();
        assert_eq!(address.ip(), Ipv4Addr::LOCALHOST);
        assert!(address.port() >= 20001);
        assert_ne!(first.address().unwrap(), second.address().unwrap());
        timeout(Duration::from_secs(2), async {
            let mut client = TcpStream::connect(address).await.unwrap();
            let mut accepted = first.accept().await.unwrap();
            client.write_all(b"ping").await.unwrap();
            let mut data = [0; 4];
            accepted.read_exact(&mut data).await.unwrap();
            assert_eq!(&data, b"ping");
            accepted.write_all(b"pong").await.unwrap();
            client.read_exact(&mut data).await.unwrap();
            assert_eq!(&data, b"pong");
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn unix_accepts_connections_and_removes_only_its_socket_on_drop() {
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let path = directory.path().join("socket");
        let socket = UnixSocket::bind(Some(path.clone())).unwrap();
        assert_eq!(socket.address().unwrap(), path.to_str().unwrap());
        assert!(std::fs::metadata(&path).unwrap().file_type().is_socket());
        timeout(Duration::from_secs(2), async {
            let mut client = UnixStream::connect(&path).await.unwrap();
            let mut accepted = socket.accept().await.unwrap();
            client.write_all(b"unix").await.unwrap();
            let mut data = [0; 4];
            accepted.read_exact(&mut data).await.unwrap();
            assert_eq!(&data, b"unix");
        })
        .await
        .unwrap();
        drop(socket);
        assert!(!path.exists());
        assert!(directory.path().exists());
    }

    #[tokio::test]
    async fn temporary_unix_directory_is_private_and_removed_with_the_socket() {
        let socket = UnixSocket::bind(None).unwrap();
        let path = PathBuf::from(socket.address().unwrap());
        let directory = path.parent().unwrap().to_owned();
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(path.exists());
        drop(socket);
        assert!(!path.exists());
        assert!(!directory.exists());
    }

    #[tokio::test]
    async fn unix_reports_invalid_paths_and_does_not_replace_existing_files() {
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let path = directory.path().join("occupied");
        std::fs::write(&path, "keep me").unwrap();
        let error = UnixSocket::bind(Some(path.clone())).err().unwrap();
        assert_eq!(
            error.to_string(),
            format!("failed to bind socket {}", path.display())
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep me");
        assert!(UnixSocket::bind(Some(directory.path().join("missing/socket"))).is_err());
        let non_utf8 = directory.path().join(OsString::from_vec(vec![255]));
        let socket = UnixSocket::bind(Some(non_utf8)).unwrap();
        assert_eq!(
            socket.address().unwrap_err().to_string(),
            "invalid socket path"
        );
    }
}
