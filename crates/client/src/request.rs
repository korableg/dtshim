use std::path::Path;

use anyhow::{Context, Result, anyhow};
use dtshim_core::model::Start;

use crate::socket::SocketConnector;

pub(crate) struct Request {
    pub(crate) socket: Box<dyn SocketConnector>,
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
    pub(crate) fn new(socket: Box<dyn SocketConnector>) -> Result<Self> {
        let (command, args) = command_line()?;
        let cwd = std::env::current_dir().context("failed to get current working directory")?;
        let cwd = std::path::absolute(cwd)
            .context("failed to resolve absolute working directory")?;
        let start = Start {
            command,
            args,
            env: std::env::vars().collect(),
            cwd: cwd
                .to_str()
                .context("working directory is not valid UTF-8")?
                .to_string(),
        };

        Ok(Self { socket, start })
    }
}
