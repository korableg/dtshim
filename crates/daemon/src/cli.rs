use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "dtshimd",
    version = option_env!("DTSHIM_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
    author,
    about = "Host daemon for executing commands from Docker containers"
)]
pub(crate) struct Cli {
    #[arg(
        long,
        env = "DTSHIM_SOCKET_PATH",
        value_name = "PATH",
        help = "Unix socket path (defaults to a socket in a temporary directory)"
    )]
    pub(crate) socket: Option<PathBuf>,
}
