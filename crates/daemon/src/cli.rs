use std::path::PathBuf;

use clap::Parser;
use dtshim_core::{env, socket::Transport};

#[derive(Debug, Parser)]
#[command(
    name = "dtshimd",
    version = option_env!("DTSHIM_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
    author,
    about = "Host daemon for executing commands from Docker containers"
)]
pub(crate) struct Cli {
    #[arg(long, env = env::TRANSPORT, default_value = "tcp")]
    pub(crate) transport: Transport,

    #[arg(
        long,
        env = env::SOCKET_PATH,
        value_name = "PATH",
        help = "Unix socket path; requires unix transport (defaults to a temporary directory)"
    )]
    pub(crate) socket: Option<PathBuf>,
}
