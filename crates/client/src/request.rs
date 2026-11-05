use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, ensure};
use dtshim_core::model::Start;

pub(crate) struct Request {
    pub(crate) socket: PathBuf,
    pub(crate) start: Start,
}

fn command_line() -> Result<(String, Vec<String>)> {
    let mut args = std::env::args_os();
    let command = args.next().context("missing command name")?;
    let command = Path::new(&command)
        .file_name()
        .context("invalid command name")?
        .to_str()
        .context("command name is not valid UTF-8")?
        .to_string();

    let args = args
        .enumerate()
        .map(|(index, arg)| {
            arg.into_string()
                .map_err(|_| anyhow!("command argument {} is not valid UTF-8", index + 1))
        })
        .collect::<Result<Vec<String>>>()?;

    Ok((command, args))
}

impl Request {
    pub(crate) fn new() -> Result<Self> {
        let socket =
            std::env::var_os("DTSHIM_SOCKET_PATH").context("DTSHIM_SOCKET_PATH must be set")?;
        ensure!(!socket.is_empty(), "DTSHIM_SOCKET_PATH must not be empty");
        let socket = PathBuf::from(socket);

        let (command, args) = command_line()?;
        let start = Start {
            command,
            args,
            env: std::env::vars().collect(),
        };

        Ok(Self { socket, start })
    }
}
