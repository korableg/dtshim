use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use dtshim_core::{
    env::{SOCKET_PATH, TCP_ADDR, TRANSPORT},
    socket::Transport,
};

pub(crate) enum Endpoint {
    Tcp(String),
    Unix(PathBuf),
}

impl Endpoint {
    pub(crate) fn new(
        transport: Transport,
        tcp_addr: Option<String>,
        path: Option<PathBuf>,
    ) -> Result<Self> {
        match transport {
            Transport::Tcp => {
                ensure!(path.is_none(), "{SOCKET_PATH} requires {TRANSPORT}=unix");
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
                Ok(Self::Tcp(address))
            }
            Transport::Unix => {
                ensure!(tcp_addr.is_none(), "{TCP_ADDR} requires {TRANSPORT}=tcp");
                let path =
                    path.with_context(|| format!("{SOCKET_PATH} must be set for Unix transport"))?;
                ensure!(
                    !path.as_os_str().is_empty(),
                    "Unix socket path must not be empty"
                );
                Ok(Self::Unix(path))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcp_accepts_hostnames_ipv4_and_nonzero_ports() {
        for address in [
            "localhost:1",
            "127.0.0.1:20001",
            "host.docker.internal:65535",
        ] {
            assert!(matches!(
                Endpoint::new(Transport::Tcp, Some(address.into()), None).unwrap(),
                Endpoint::Tcp(actual) if actual == address
            ));
        }
    }

    #[test]
    fn tcp_rejects_missing_addresses_invalid_hosts_ports_and_unix_options() {
        assert_eq!(
            Endpoint::new(Transport::Tcp, None, None)
                .err()
                .unwrap()
                .to_string(),
            "DTSHIM_TCP_ADDR must be set for TCP transport"
        );
        assert_eq!(
            Endpoint::new(Transport::Tcp, Some("host:1".into()), Some("socket".into()))
                .err()
                .unwrap()
                .to_string(),
            "DTSHIM_SOCKET_PATH requires DTSHIM_TRANSPORT=unix"
        );
        for (address, expected) in [
            ("host", "DTSHIM_TCP_ADDR must be HOST:PORT"),
            (":80", "TCP host must not be empty"),
            ("host:0", "TCP port must be in 1..=65535"),
            ("host:65536", "TCP port must be in 1..=65535"),
            ("host:-1", "TCP port must be in 1..=65535"),
            ("host:", "TCP port must be in 1..=65535"),
            ("host:abc", "TCP port must be in 1..=65535"),
            ("host:80 ", "TCP port must be in 1..=65535"),
            ("host name:80", "invalid TCP host"),
            ("host\tname:80", "invalid TCP host"),
            ("host/path:80", "invalid TCP host"),
            ("[::1]:80", "invalid TCP host"),
            ("::1:80", "invalid TCP host"),
            ("host]:80", "invalid TCP host"),
        ] {
            assert_eq!(
                Endpoint::new(Transport::Tcp, Some(address.into()), None)
                    .err()
                    .unwrap()
                    .to_string(),
                expected,
                "{address}"
            );
        }
    }

    #[test]
    fn unix_preserves_the_path_and_rejects_missing_empty_and_tcp_options() {
        for path in ["relative.sock", "/tmp/socket with spaces"] {
            assert!(
                matches!(Endpoint::new(Transport::Unix, None, Some(path.into())).unwrap(), Endpoint::Unix(actual) if actual == PathBuf::from(path))
            );
        }
        for (address, path, expected) in [
            (
                None,
                None,
                "DTSHIM_SOCKET_PATH must be set for Unix transport",
            ),
            (
                None,
                Some(PathBuf::new()),
                "Unix socket path must not be empty",
            ),
            (
                Some("host:1".into()),
                Some("socket".into()),
                "DTSHIM_TCP_ADDR requires DTSHIM_TRANSPORT=tcp",
            ),
        ] {
            assert_eq!(
                Endpoint::new(Transport::Unix, address, path)
                    .err()
                    .unwrap()
                    .to_string(),
                expected
            );
        }
    }
}
