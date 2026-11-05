mod cli;
mod server;
mod session;
mod signal;
mod socket;

use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;

use crate::cli::Cli;
use crate::signal::Shutdown;
use crate::socket::BoundSocket;

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
    let socket = BoundSocket::bind(cli.socket)?;
    let mut shutdown = Shutdown::new()?;

    println!("{}", socket.path().to_str().context("invalid socket path")?);

    server::run(socket.listener(), shutdown.wait()).await;

    Ok(())
}
