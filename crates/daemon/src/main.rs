mod cli;
mod server;
mod session;
mod signal;
mod socket;

use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;

use crate::cli::Cli;
use crate::signal::Shutdown;

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
    let socket = socket::bind(cli.transport, cli.socket).await?;
    let mut shutdown = Shutdown::new()?;

    println!("{}", socket.address()?);

    server::run(socket.as_ref(), shutdown.wait()).await;

    Ok(())
}
