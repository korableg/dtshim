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
    #[arg(long, env = env::TOKEN, hide_env_values = true, help = "Require this token from clients; authentication is disabled when omitted")]
    pub(crate) token: Option<String>,

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

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, FromArgMatches, error::ErrorKind};

    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        let mut command = Cli::command();
        for name in ["token", "transport", "socket", "policy", "volume_mapping"] {
            command = command.mut_arg(name, |arg| arg.env(None::<&str>));
        }
        Cli::from_arg_matches(&command.try_get_matches_from(args.iter().copied())?)
    }

    #[test]
    fn defaults_to_tcp_with_authentication_and_policy_disabled() {
        let cli = parse(&["dtshimd"]).unwrap();
        assert_eq!(cli.transport, Transport::Tcp);
        assert!(cli.token.is_none());
        assert!(cli.socket.is_none());
        assert!(cli.policy.is_none());
        assert!(cli.volume_mapping.is_empty());
    }

    #[test]
    fn accepts_token_socket_policy_and_repeated_comma_separated_volumes() {
        let cli = parse(&[
            "dtshimd",
            "--transport",
            "unix",
            "--token",
            "secret with spaces",
            "--socket",
            "/tmp/socket",
            "--policy",
            "/tmp/policy.yaml",
            "--volume_mapping",
            "/host:/app,/config:/config",
            "--volume_mapping",
            "/other:/other",
        ])
        .unwrap();
        assert_eq!(cli.transport, Transport::Unix);
        assert_eq!(cli.token.as_deref(), Some("secret with spaces"));
        assert_eq!(cli.socket, Some(PathBuf::from("/tmp/socket")));
        assert_eq!(cli.policy, Some(PathBuf::from("/tmp/policy.yaml")));
        let volumes = crate::volume::Volumes::new(cli.volume_mapping).unwrap();
        assert_eq!(
            volumes.map_to_host("/app/file /config/file /other/file"),
            "/host/file /config/file /other/file"
        );
    }

    #[test]
    fn invalid_settings_are_rejected_and_help_version_are_available() {
        for args in [
            vec!["dtshimd", "--transport", "udp"],
            vec!["dtshimd", "--volume_mapping", "relative:/app"],
            vec!["dtshimd", "--token"],
            vec!["dtshimd", "--unknown"],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
        assert_eq!(
            parse(&["dtshimd", "--help"]).unwrap_err().kind(),
            ErrorKind::DisplayHelp
        );
        assert_eq!(
            parse(&["dtshimd", "--version"]).unwrap_err().kind(),
            ErrorKind::DisplayVersion
        );
    }
}
