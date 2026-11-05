mod request;
mod session;

use std::process::ExitCode;

use anyhow::{Context, Result};
use tokio::runtime::Builder;

use crate::request::Request;

fn main() -> Result<ExitCode> {
    let request = Request::new()?;
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to create runtime")?;
    let result = runtime.block_on(session::run(request));
    runtime.shutdown_background();

    result
}
