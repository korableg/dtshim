use std::path::PathBuf;

use clap::Parser;
use dtshim_core::{env, socket::Transport};

use crate::volume::Volume;

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

    #[arg(
        long,
        env = env::POLICY,
        value_name = "FILE",
        help = "Allow only commands and arguments declared in this YAML policy"
    )]
    pub(crate) policy: Option<PathBuf>,

    #[arg(
        long = "volume_mapping",
        env = env::VOLUME_MAPPING,
        value_delimiter = ',',
        value_name = "HOST:DOCKER",
        help = "Map absolute host and Docker paths; repeat the flag or separate mappings with commas"
    )]
    pub(crate) volume_mapping: Vec<Volume>,
}
