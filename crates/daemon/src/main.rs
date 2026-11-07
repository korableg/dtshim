mod cli;
mod context;
mod policy;
mod policy_config;
mod server;
mod session;
mod signal;
mod socket;
mod volume;

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Result, ensure};
use clap::Parser;
use dtshim_core::{
    env::{SOCKET_PATH, TRANSPORT},
    socket::Transport,
};

use crate::cli::Cli;
use crate::context::Ctx;
use crate::policy::Policy;
use crate::signal::Shutdown;
use crate::socket::{SocketListener, TcpSocket, UnixSocket};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<ExitCode> {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            err.print()?;
            return Ok(ExitCode::from(err.exit_code() as u8));
        }
    };

    run(cli).await?;

    Ok(ExitCode::SUCCESS)
}

async fn run(cli: Cli) -> Result<()> {
    let context = Arc::new(Ctx {
        token: cli.token,
        policy: Policy::new(cli.policy.as_deref())?,
        volumes: volume::Volumes::new(cli.volume_mapping)?,
    });
    match cli.transport {
        Transport::Tcp => {
            ensure!(
                cli.socket.is_none(),
                "--socket / {SOCKET_PATH} requires unix transport (--transport unix or {TRANSPORT}=unix)"
            );
            let socket = TcpSocket::bind().await?;
            run_server(socket, context).await
        }
        Transport::Unix => {
            let socket = UnixSocket::bind(cli.socket)?;
            run_server(socket, context).await
        }
    }
}

async fn run_server<L: SocketListener>(socket: L, context: Arc<Ctx>) -> Result<()> {
    let mut shutdown = Shutdown::new()?;

    println!("{}", socket.address()?);

    server::run(&socket, shutdown.wait(), context).await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli() -> Cli {
        Cli {
            token: None,
            transport: Transport::Tcp,
            socket: None,
            policy: None,
            volume_mapping: vec![],
        }
    }

    #[tokio::test]
    async fn tcp_rejects_unix_socket_settings_before_starting_a_server() {
        let mut cli = cli();
        cli.socket = Some("socket".into());
        assert_eq!(
            run(cli).await.unwrap_err().to_string(),
            "--socket / DTSHIM_SOCKET_PATH requires unix transport (--transport unix or DTSHIM_TRANSPORT=unix)"
        );
    }

    #[tokio::test]
    async fn startup_propagates_policy_volume_and_socket_errors() {
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let mut invalid_policy = cli();
        invalid_policy.policy = Some(directory.path().join("missing.yaml"));
        assert!(
            run(invalid_policy)
                .await
                .unwrap_err()
                .to_string()
                .starts_with("failed to read policy ")
        );

        let mut invalid_volumes = cli();
        invalid_volumes.volume_mapping = vec![
            "/host:/app".parse().unwrap(),
            "/host:/other".parse().unwrap(),
        ];
        assert_eq!(
            run(invalid_volumes).await.unwrap_err().to_string(),
            "host path \"/host\" has multiple volume mappings"
        );

        let mut invalid_socket = cli();
        invalid_socket.transport = Transport::Unix;
        invalid_socket.socket = Some(directory.path().join("missing/socket"));
        assert!(
            run(invalid_socket)
                .await
                .unwrap_err()
                .to_string()
                .starts_with("failed to bind socket ")
        );
    }
}
