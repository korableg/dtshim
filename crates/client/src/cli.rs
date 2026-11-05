use std::path::PathBuf;

use clap::Parser;
use dtshim_core::{env, socket::Transport};

#[derive(Debug, Parser)]
#[command(name = "dtshim", disable_help_flag = true, disable_version_flag = true)]
pub(crate) struct Cli {
    #[arg(env = env::TRANSPORT, default_value = "tcp", hide = true)]
    pub(crate) transport: Transport,

    #[arg(env = env::TCP_ADDR, hide = true)]
    pub(crate) tcp_addr: Option<String>,

    #[arg(env = env::SOCKET_PATH, hide = true)]
    pub(crate) socket: Option<PathBuf>,
}

impl Cli {
    pub(crate) fn from_env() -> Result<Self, clap::Error> {
        Self::try_parse_from(["dtshim"])
    }
}
