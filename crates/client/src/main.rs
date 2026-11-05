mod cli;
mod request;
mod session;
mod socket;

use std::process::ExitCode;

use anyhow::{Context, Result};
use tokio::runtime::Builder;

use crate::cli::Cli;
use crate::request::Request;

fn main() -> Result<ExitCode> {
    let cli = Cli::from_env()?;
    let socket = socket::connector(cli.transport, cli.tcp_addr, cli.socket)?;
    let request = Request::new(socket)?;
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to create runtime")?;
    let result = runtime.block_on(session::run(request));
    runtime.shutdown_background();

    result
}
