use std::str::FromStr;

use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpStream, UnixStream},
};

pub trait Connection: AsyncRead + AsyncWrite + Unpin + Send {}
impl Connection for TcpStream {}
impl Connection for UnixStream {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Tcp,
    Unix,
}

impl FromStr for Transport {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "tcp" => Ok(Self::Tcp),
            "unix" => Ok(Self::Unix),
            _ => Err("transport must be tcp or unix"),
        }
    }
}
