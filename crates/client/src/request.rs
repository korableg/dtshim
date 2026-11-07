use std::{ffi::OsString, path::Path};

use anyhow::{Context, Result, anyhow};
use dtshim_core::model::Start;

fn command_line(args: impl IntoIterator<Item = OsString>) -> Result<(String, Vec<String>)> {
    let mut args = args.into_iter();
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

fn environment(
    values: impl IntoIterator<Item = (OsString, OsString)>,
) -> Result<Vec<(String, String)>> {
    values
        .into_iter()
        .map(|(name, value)| {
            let name = name
                .into_string()
                .map_err(|_| anyhow!("environment variable name is not valid UTF-8"))?;
            let value = value
                .into_string()
                .map_err(|_| anyhow!("environment variable {name:?} value is not valid UTF-8"))?;
            Ok((name, value))
        })
        .collect()
}

pub(crate) fn from_env(token: Option<String>) -> Result<Start> {
    let (command, args) = command_line(std::env::args_os())?;
    let cwd = std::env::current_dir().context("failed to get current working directory")?;
    let cwd = std::path::absolute(cwd).context("failed to resolve absolute working directory")?;
    Ok(Start {
        token,
        command,
        args,
        env: environment(std::env::vars_os())?,
        cwd: cwd
            .to_str()
            .context("working directory is not valid UTF-8")?
            .to_string(),
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStringExt;

    use super::*;

    #[test]
    fn command_line_uses_the_basename_and_preserves_arguments() {
        for executable in ["tool", "/usr/local/bin/tool", "./tool", "../tool"] {
            let (command, args) = command_line(
                [executable, "", "with spaces", "--help", "юникод"].map(OsString::from),
            )
            .unwrap();
            assert_eq!(command, "tool");
            assert_eq!(args, ["", "with spaces", "--help", "юникод"]);
        }
    }

    #[test]
    fn command_line_reports_missing_invalid_and_non_utf8_names() {
        for (args, expected) in [
            (vec![], "missing command name"),
            (vec![OsString::from("/")], "invalid command name"),
            (vec![OsString::from("")], "invalid command name"),
            (
                vec![OsString::from_vec(vec![255])],
                "command name is not valid UTF-8",
            ),
            (
                vec!["tool".into(), "valid".into(), OsString::from_vec(vec![255])],
                "command argument 2 is not valid UTF-8",
            ),
        ] {
            assert_eq!(command_line(args).unwrap_err().to_string(), expected);
        }
    }

    #[test]
    fn environment_preserves_order_duplicates_empty_values_and_unicode() {
        assert!(environment(Vec::new()).unwrap().is_empty());
        assert_eq!(
            environment([
                ("A".into(), "one".into()),
                ("A".into(), "".into()),
                ("имя".into(), "значение".into()),
            ])
            .unwrap(),
            [
                ("A".to_owned(), "one".to_owned()),
                ("A".to_owned(), "".to_owned()),
                ("имя".to_owned(), "значение".to_owned())
            ]
        );
    }

    #[test]
    fn environment_identifies_non_utf8_names_and_values() {
        assert_eq!(
            environment([(OsString::from_vec(vec![255]), "value".into())])
                .unwrap_err()
                .to_string(),
            "environment variable name is not valid UTF-8"
        );
        assert_eq!(
            environment([("NAME".into(), OsString::from_vec(vec![255]))])
                .unwrap_err()
                .to_string(),
            "environment variable \"NAME\" value is not valid UTF-8"
        );
    }
}
