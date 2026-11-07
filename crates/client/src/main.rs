mod cli;
mod request;
mod session;
mod socket;

use std::process::ExitCode;

use anyhow::{Context, Result};
use dtshim_core::model::Start;
use tokio::{
    net::{TcpStream, UnixStream},
    runtime::Builder,
};

use crate::cli::Cli;
use crate::socket::Endpoint;

fn main() -> Result<ExitCode> {
    let cli = Cli::from_env()?;
    let endpoint = Endpoint::new(cli.transport, cli.tcp_addr, cli.socket)?;
    let start = request::from_env(cli.token)?;
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to create runtime")?;
    let result = runtime.block_on(run(endpoint, start));
    runtime.shutdown_background();

    result
}

async fn run(endpoint: Endpoint, start: Start) -> Result<ExitCode> {
    match endpoint {
        Endpoint::Tcp(address) => {
            let stream = TcpStream::connect(address.as_str())
                .await
                .with_context(|| format!("failed to connect to TCP socket {address}"))?;
            session::run(stream, start).await
        }
        Endpoint::Unix(path) => {
            let stream = UnixStream::connect(&path)
                .await
                .with_context(|| format!("failed to connect to Unix socket {}", path.display()))?;
            session::run(stream, start).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start() -> Start {
        Start {
            token: None,
            command: "tool".into(),
            args: vec![],
            env: vec![],
            cwd: "/".into(),
        }
    }

    #[tokio::test]
    async fn connection_errors_identify_the_transport_and_endpoint() {
        let error = run(Endpoint::Tcp("invalid address".into()), start())
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "failed to connect to TCP socket invalid address"
        );
        let error = run(Endpoint::Unix(std::path::PathBuf::new()), start())
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "failed to connect to Unix socket ");
    }
}
