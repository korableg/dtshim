use std::path::PathBuf;

use clap::Parser;
use dtshim_core::{env, socket::Transport};

#[derive(Debug, Parser)]
#[command(name = "dtshim", disable_help_flag = true, disable_version_flag = true)]
pub(crate) struct Cli {
    #[arg(env = env::TOKEN, hide = true)]
    pub(crate) token: Option<String>,

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

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, FromArgMatches, error::ErrorKind};

    use super::*;

    fn command() -> clap::Command {
        let mut command = Cli::command();
        for name in ["token", "transport", "tcp_addr", "socket"] {
            command = command.mut_arg(name, |arg| arg.env(None::<&str>));
        }
        command
    }

    #[test]
    fn defaults_to_tcp_without_optional_settings() {
        let matches = command().try_get_matches_from(["dtshim"]).unwrap();
        let cli = Cli::from_arg_matches(&matches).unwrap();
        assert_eq!(cli.transport, Transport::Tcp);
        assert!(cli.token.is_none());
        assert!(cli.tcp_addr.is_none());
        assert!(cli.socket.is_none());
    }

    #[test]
    fn does_not_intercept_the_wrapped_commands_help_or_version_flags() {
        for flag in ["--help", "--version", "--transport"] {
            assert_eq!(
                command()
                    .try_get_matches_from(["dtshim", flag])
                    .unwrap_err()
                    .kind(),
                ErrorKind::UnknownArgument
            );
        }
    }
}
