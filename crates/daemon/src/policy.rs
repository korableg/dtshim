use std::{collections::HashMap, path::Path};

use anyhow::{Context, Result, bail, ensure};
use dtshim_core::model::Start;
use regex::Regex;

use crate::policy_config::{HostEnvConfig, InvocationConfig, PolicyConfig};

struct AllowedOption {
    value: Option<Regex>,
    required: bool,
    repeat: bool,
}

struct Invocation {
    prefix: Vec<String>,
    options: Vec<AllowedOption>,
    option_names: HashMap<String, usize>,
    positionals: Vec<Regex>,
}

struct AllowedCommand {
    executable: String,
    host_env: Vec<String>,
    env_overrides: Vec<(String, String)>,
    allow: Vec<Invocation>,
}

#[derive(Default)]
pub(crate) struct Policy {
    commands: Option<HashMap<String, AllowedCommand>>,
}

impl AllowedOption {
    fn consume<'a>(
        &self,
        inline_value: Option<&'a str>,
        args: &mut std::slice::Iter<'a, String>,
        seen: &mut bool,
    ) -> bool {
        if *seen && !self.repeat {
            return false;
        }
        *seen = true;
        let Some(pattern) = &self.value else {
            return inline_value.is_none();
        };
        let Some(value) = inline_value.or_else(|| args.next().map(String::as_str)) else {
            return false;
        };
        pattern.is_match(value)
    }
}

fn full_regex(pattern: &str) -> Result<Regex> {
    Regex::new(pattern).context("invalid regular expression")?;
    Regex::new(&format!(r"\A(?:{pattern})\z")).context("invalid full-match regular expression")
}

impl Invocation {
    fn compile(config: InvocationConfig) -> Result<Self> {
        let mut option_names = HashMap::new();
        let mut options = Vec::with_capacity(config.options.len());
        for (index, option) in config.options.into_iter().enumerate() {
            for name in &option.names {
                option_names.insert(name.clone(), index);
            }
            let value = option
                .value
                .as_deref()
                .map(full_regex)
                .transpose()
                .with_context(|| format!("option {:?}", option.names[0]))?;
            options.push(AllowedOption {
                value,
                required: option.required,
                repeat: option.repeat,
            });
        }
        let positionals = config
            .positionals
            .iter()
            .enumerate()
            .map(|(index, pattern)| {
                full_regex(pattern).with_context(|| format!("positional argument {}", index + 1))
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            prefix: config.prefix,
            options,
            option_names,
            positionals,
        })
    }

    fn matches(&self, args: &[String]) -> bool {
        let Some(args) = args.strip_prefix(self.prefix.as_slice()) else {
            return false;
        };
        if self.options.is_empty() && self.positionals.is_empty() {
            return args.is_empty();
        }
        let mut seen = vec![false; self.options.len()];
        let mut positionals = self.positionals.iter();
        let mut parse_options = true;
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            if parse_options && arg == "--" {
                parse_options = false;
                continue;
            }
            if !parse_options || !arg.starts_with('-') || arg == "-" {
                let Some(pattern) = positionals.next() else {
                    return false;
                };
                if !pattern.is_match(arg) {
                    return false;
                }
                continue;
            }
            if arg.starts_with("--") {
                let (name, inline_value) = match arg.split_once('=') {
                    Some((name, value)) => (name, Some(value)),
                    None => (arg.as_str(), None),
                };
                let Some(&index) = self.option_names.get(name) else {
                    return false;
                };
                if !self.options[index].consume(inline_value, &mut args, &mut seen[index]) {
                    return false;
                }
                continue;
            }
            let mut cluster = &arg[1..];
            while !cluster.is_empty() {
                let name = [b'-', cluster.as_bytes()[0]];
                let Ok(name) = std::str::from_utf8(&name) else {
                    return false;
                };
                let Some(&index) = self.option_names.get(name) else {
                    return false;
                };
                cluster = &cluster[1..];
                let option = &self.options[index];
                let inline_value = if option.value.is_some() && !cluster.is_empty() {
                    Some(cluster.strip_prefix('=').unwrap_or(cluster))
                } else {
                    None
                };
                if !option.consume(inline_value, &mut args, &mut seen[index]) {
                    return false;
                }
                if option.value.is_some() {
                    break;
                }
            }
        }
        positionals.next().is_none()
            && self
                .options
                .iter()
                .zip(seen)
                .all(|(option, seen)| !option.required || seen)
    }
}

impl Policy {
    pub(crate) fn new(path: Option<&Path>) -> Result<Self> {
        match path {
            Some(path) => Self::load(path),
            None => Ok(Self::default()),
        }
    }

    fn load(path: &Path) -> Result<Self> {
        let config = PolicyConfig::load(path)?;
        Self::compile(config).with_context(|| format!("invalid policy {}", path.display()))
    }

    fn compile(config: PolicyConfig) -> Result<Self> {
        let mut commands = HashMap::with_capacity(config.commands.len());
        for (name, command) in config.commands {
            let mut host_env = Vec::with_capacity(command.host_env.len());
            let mut env_overrides = Vec::new();
            for entry in command.host_env {
                match entry {
                    HostEnvConfig::Name(variable) => host_env.push(variable),
                    HostEnvConfig::Value(values) => {
                        for (variable, value) in values {
                            host_env.push(variable.clone());
                            env_overrides.push((variable, value));
                        }
                    }
                }
            }
            let allow = command
                .allow
                .into_iter()
                .enumerate()
                .map(|(index, config)| {
                    Invocation::compile(config)
                        .with_context(|| format!("command {name:?}, allow[{}]", index))
                })
                .collect::<Result<_>>()?;
            commands.insert(
                name,
                AllowedCommand {
                    executable: command.executable,
                    host_env,
                    env_overrides,
                    allow,
                },
            );
        }
        Ok(Self {
            commands: Some(commands),
        })
    }

    pub(crate) fn prepare(&self, mut start: Start) -> Result<Start> {
        let Some(commands) = &self.commands else {
            ensure!(!start.command.is_empty(), "command is empty");
            return Ok(start);
        };
        let command = commands
            .get(&start.command)
            .with_context(|| format!("command {:?} is not allowed", start.command))?;
        if !command
            .allow
            .iter()
            .any(|invocation| invocation.matches(&start.args))
        {
            bail!(
                "arguments for command {:?} do not match any allowed invocation",
                start.command
            );
        }
        start.command.clone_from(&command.executable);
        start
            .env
            .retain(|(name, _)| !command.host_env.contains(name));
        start.env.extend(command.env_overrides.iter().cloned());
        Ok(start)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn compile_policy(yaml: &str) -> Result<Policy> {
        Policy::compile(PolicyConfig::parse(yaml).unwrap())
    }

    fn start(command: &str, args: &[&str]) -> Start {
        Start {
            command: command.to_owned(),
            args: args.iter().map(|value| (*value).to_owned()).collect(),
            env: Vec::new(),
            cwd: "/work".to_owned(),
        }
    }

    fn policy(invocation: &str) -> Policy {
        compile_policy(&format!(
            "commands:\n  tool:\n    executable: /usr/bin/tool\n    allow:\n      - {invocation}\n"
        ))
        .unwrap()
    }

    #[test]
    fn unrestricted_policy_rejects_empty_command() {
        let policy = Policy::default();
        for values in [&[][..], &["arg"]] {
            assert!(policy.prepare(start("", values)).is_err());
        }
    }

    #[test]
    fn unrestricted_policy_preserves_the_entire_start() {
        let policy = Policy::new(None).unwrap();
        for name in [
            "tool",
            "/usr/bin/tool",
            "./tool",
            "../tool",
            ".",
            "..",
            " ",
            "tool name",
            "tool\\name",
            "tool\0name",
        ] {
            for values in [
                &[][..],
                &["--unknown", "", "arg with spaces", "\0", "--", "-x"],
            ] {
                let mut request = start(name, values);
                request.env = vec![("HOME".to_owned(), "/container/home".to_owned())];
                let expected_args = request.args.clone();
                let expected_env = request.env.clone();
                let expected_cwd = request.cwd.clone();
                let prepared = policy.prepare(request).unwrap();
                assert_eq!(prepared.command, name);
                assert_eq!(prepared.args, expected_args);
                assert_eq!(prepared.env, expected_env);
                assert_eq!(prepared.cwd, expected_cwd);
            }
        }
    }

    #[test]
    fn command_names_and_empty_invocations_are_exact() {
        let policy = policy("{}");
        assert_eq!(
            policy.prepare(start("tool", &[])).unwrap().command,
            "/usr/bin/tool"
        );
        for name in ["other", "/usr/bin/tool", "./tool", "", "TOOL"] {
            assert!(policy.prepare(start(name, &[])).is_err(), "{name}");
        }
        for values in [&["--"][..], &["arg"], &[""]] {
            assert!(policy.prepare(start("tool", values)).is_err());
        }
        assert!(
            compile_policy("commands: {}")
                .unwrap()
                .prepare(start("tool", &[]))
                .is_err()
        );
    }

    #[test]
    fn host_env_filters_only_the_authorized_commands_names() {
        let policy = compile_policy(
            "commands:
  first:
    executable: /bin/first
    host_env: [HOME, MISSING]
    allow: [{}]
  second:
    executable: /bin/second
    host_env: [PATH]
    allow: [{}]
  empty:
    executable: /bin/empty
    host_env: []
    allow: [{}]
  omitted:
    executable: /bin/omitted
    allow: [{}]
",
        )
        .unwrap();
        let original: Vec<_> = ["HOME", "PATH", "MISSING", "HOME", "home", "OTHER"]
            .into_iter()
            .map(|name| (name.to_owned(), format!("client-{name}")))
            .collect();
        for (name, expected) in [
            ("first", vec!["PATH", "home", "OTHER"]),
            ("second", vec!["HOME", "MISSING", "HOME", "home", "OTHER"]),
            (
                "empty",
                vec!["HOME", "PATH", "MISSING", "HOME", "home", "OTHER"],
            ),
            (
                "omitted",
                vec!["HOME", "PATH", "MISSING", "HOME", "home", "OTHER"],
            ),
        ] {
            let mut request = start(name, &[]);
            request.env = original.clone();
            let prepared = policy.prepare(request).unwrap();
            let expected: Vec<_> = expected
                .into_iter()
                .map(|name| (name.to_owned(), format!("client-{name}")))
                .collect();
            assert_eq!(prepared.command, format!("/bin/{name}"));
            assert_eq!(prepared.env, expected, "{name}");
            assert_eq!(prepared.cwd, "/work");
        }
    }

    #[test]
    fn prepare_rewrites_command_and_env_without_changing_args_or_cwd() {
        let policy = compile_policy(
            "commands: {tool: {executable: /bin/tool, host_env: [HOME, {FIXED: '/container/fixed'}, {EMPTY: ''}], allow: [{prefix: [status]}]}}",
        )
        .unwrap();
        let mut request = start("tool", &["status"]);
        request.cwd = "/host/work".to_owned();
        request.env = ["HOME", "FIXED", "EMPTY", "OTHER"]
            .into_iter()
            .map(|name| (name.to_owned(), "/host/client".to_owned()))
            .collect();
        let prepared = policy.prepare(request).unwrap();
        assert_eq!(prepared.command, "/bin/tool");
        assert_eq!(prepared.args, ["status"]);
        assert_eq!(prepared.cwd, "/host/work");
        assert_eq!(
            prepared.env,
            vec![
                ("OTHER".to_owned(), "/host/client".to_owned()),
                ("FIXED".to_owned(), "/container/fixed".to_owned()),
                ("EMPTY".to_owned(), String::new()),
            ]
        );
    }

    #[test]
    fn host_env_filter_accepts_permissive_environment_names() {
        let policy = compile_policy(
            "commands: {tool: {executable: /bin/tool, host_env: ['1', 'a-b', 'a.b', 'with space', 'имя'], allow: [{}]}}",
        )
        .unwrap();
        let mut request = start("tool", &[]);
        request.env = ["1", "a-b", "a.b", "with space", "имя"]
            .into_iter()
            .map(|name| (name.to_owned(), String::new()))
            .collect();
        assert!(policy.prepare(request).unwrap().env.is_empty());
    }

    #[test]
    fn options_match_in_any_order_without_reordering_argv() {
        let policy = policy(
            "prefix: [list]\n        options:\n          - names: [--json, -j]\n          - names: [--limit, -n]\n            value: '[1-9][0-9]?'\n",
        );
        for values in [
            vec!["list"],
            vec!["list", "--json", "--limit", "10"],
            vec!["list", "--limit=10", "--json"],
            vec!["list", "-n", "1", "-j"],
            vec!["list", "-jn", "10"],
            vec!["list", "-n10"],
            vec!["list", "-n=10"],
            vec!["list", "-jn10"],
            vec!["list", "-jn=10"],
        ] {
            let request = start("tool", &values);
            let expected = request.args.clone();
            let prepared = policy.prepare(request).unwrap();
            assert_eq!(prepared.args, expected, "{values:?}");
        }
        for values in [
            vec!["--json", "list"],
            vec!["other"],
            vec!["list", "--unknown"],
            vec!["list", "--json=true"],
            vec!["list", "-jn"],
            vec!["list", "-jjn10"],
            vec!["list", "-jxn10"],
            vec!["list", "-n10j"],
            vec!["list", "-n10", "--limit=20"],
            vec!["list", "--limit"],
            vec!["list", "--limit="],
            vec!["list", "--limit", "100"],
            vec!["list", "--limit", "a10"],
            vec!["list", "--limit", "10\n"],
            vec!["list", "--json", "-j"],
            vec!["list", "-n", "1", "--limit=2"],
            vec!["list", "--json", "extra"],
        ] {
            assert!(
                policy.prepare(start("tool", &values)).is_err(),
                "{values:?}"
            );
        }
    }

    #[test]
    fn short_flag_groups_share_required_and_repeat_rules_with_aliases() {
        let policy = policy(
            "options:\n          - names: [--alpha, -a, -A]\n            required: true\n          - names: [--beta, -b]\n            repeat: true\n          - names: [-c]\n",
        );
        for values in [
            vec!["-abc"],
            vec!["-cbab"],
            vec!["-ab", "-b", "-c"],
            vec!["--alpha", "-bb", "-c"],
            vec!["-A"],
        ] {
            let request = start("tool", &values);
            let expected_args = request.args.clone();
            let prepared = policy.prepare(request).unwrap();
            assert_eq!(prepared.args, expected_args, "{values:?}");
        }
        for values in [
            vec!["-bc"],
            vec!["-aac"],
            vec!["-aA"],
            vec!["-abx"],
            vec!["-axc"],
            vec!["-ab", "--alpha"],
            vec!["-a=true"],
            vec!["-éa"],
            vec!["-aé"],
        ] {
            assert!(policy.prepare(start("tool", &values)).is_err(), "{values:?}");
        }
    }

    #[test]
    fn short_value_option_consumes_the_remainder_of_its_group() {
        let policy = policy(
            "options:\n          - names: [-j]\n            required: true\n          - names: [--name, -n]\n            value: '(j|имя|=tail)'\n            required: true\n",
        );
        for values in [
            vec!["-jnj"],
            vec!["-jnимя"],
            vec!["-jn=имя"],
            vec!["-j", "-n=имя"],
            vec!["-jn==tail"],
            vec!["-jn", "имя"],
            vec!["-nj", "-j"],
        ] {
            let request = start("tool", &values);
            let expected_args = request.args.clone();
            let prepared = policy.prepare(request).unwrap();
            assert_eq!(prepared.args, expected_args, "{values:?}");
        }
        for values in [
            vec!["-nj"],
            vec!["-jnимяj"],
            vec!["-jn"],
            vec!["-jn=tail"],
            vec!["-jnимя", "--name=j"],
            vec!["--", "-jnимя"],
        ] {
            assert!(policy.prepare(start("tool", &values)).is_err(), "{values:?}");
        }
    }

    #[test]
    fn required_repeatable_options_keep_values_attached() {
        let policy = policy(
            "options:\n          - names: [--tag, -t]\n            value: '(-draft|release)'\n            required: true\n            repeat: true\n",
        );
        for values in [
            vec!["--tag", "-draft"],
            vec!["-t", "release", "--tag=-draft"],
            vec!["-trelease", "-t=-draft"],
        ] {
            assert!(policy.prepare(start("tool", &values)).is_ok());
        }
        for values in [
            vec![],
            vec!["--tag"],
            vec!["--tag", "--unknown"],
            vec!["--tag", "release", "orphan"],
        ] {
            assert!(policy.prepare(start("tool", &values)).is_err());
        }
    }

    #[test]
    fn positionals_remain_ordered_and_double_dash_ends_options() {
        let policy = policy(
            "options:\n          - names: [--json]\n        positionals: ['first', '(-last|last)']",
        );
        for values in [
            vec!["first", "--json", "last"],
            vec!["--json", "first", "last"],
            vec!["first", "last", "--json"],
            vec!["first", "--", "-last"],
        ] {
            assert!(policy.prepare(start("tool", &values)).is_ok(), "{values:?}");
        }
        for values in [
            vec!["last", "first"],
            vec!["first"],
            vec!["first", "-last"],
            vec!["first", "--", "--json"],
            vec!["first", "last", "extra"],
        ] {
            assert!(
                policy.prepare(start("tool", &values)).is_err(),
                "{values:?}"
            );
        }
    }

    #[test]
    fn option_values_preserve_empty_strings_and_double_dash() {
        let policy = policy(
            "options:\n          - names: [--value, -v]\n            value: '(--|)'\n            required: true\n          - names: [--flag]\n",
        );
        for values in [
            vec!["--value", "--", "--flag"],
            vec!["--value=--", "--flag"],
            vec!["--value="],
            vec!["-v", ""],
            vec!["-v="],
            vec!["-v--", "--flag"],
            vec!["-v=--", "--flag"],
            vec!["-v", "--", "--flag"],
        ] {
            assert!(policy.prepare(start("tool", &values)).is_ok(), "{values:?}");
        }
        for values in [
            vec!["--value"],
            vec!["-v"],
            vec!["--value", "--", "--unknown"],
            vec!["--flag=", "--value="],
        ] {
            assert!(policy.prepare(start("tool", &values)).is_err(), "{values:?}");
        }
    }

    #[test]
    fn double_dash_is_positional_after_the_option_terminator() {
        let policy = policy("positionals: ['--', '-', '']");
        assert!(policy.prepare(start("tool", &["--", "--", "-", ""])).is_ok());
        for values in [
            vec!["--", "-", ""],
            vec!["--", "--", "-"],
            vec!["--unknown", "--", "--", "-", ""],
        ] {
            assert!(policy.prepare(start("tool", &values)).is_err(), "{values:?}");
        }
    }

    #[test]
    fn variants_do_not_combine_their_options() {
        let policy = compile_policy("commands:\n  tool:\n    executable: /usr/bin/tool\n    allow:\n      - prefix: [status]\n        options: [{names: [--short]}]\n      - prefix: [log]\n        options: [{names: [--oneline]}]\n").unwrap();
        assert!(
            policy
                .prepare(start("tool", &["status", "--short"]))
                .is_ok()
        );
        assert!(policy.prepare(start("tool", &["log", "--oneline"])).is_ok());
        assert!(
            policy
                .prepare(start("tool", &["status", "--oneline"]))
                .is_err()
        );
        assert!(policy.prepare(start("tool", &["log", "--short"])).is_err());
    }

    #[test]
    fn regular_expressions_match_whole_values() {
        let policy = policy("positionals: ['(?m)^yes$|no']");
        for value in ["yes", "no"] {
            assert!(policy.prepare(start("tool", &[value])).is_ok());
        }
        for value in ["yes\n", "yes\nother", "other\nyes", "nobody", "ohno"] {
            assert!(
                policy.prepare(start("tool", &[value])).is_err(),
                "{value:?}"
            );
        }
        assert!(full_regex("a)|(?:b").is_err());
    }

    #[test]
    fn invalid_regular_expressions_are_rejected() {
        for yaml in [
            "commands: {tool: {executable: /bin/tool, allow: [{positionals: ['[']}]}}",
            "commands: {tool: {executable: /bin/tool, allow: [{options: [{names: [-a], value: '['}]}]}}",
        ] {
            assert!(
                compile_policy(yaml).is_err(),
                "accepted invalid policy: {yaml}"
            );
        }
    }

    #[test]
    fn loaded_policy_keeps_a_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("policy.yaml");
        fs::write(
            &path,
            "commands: {tool: {executable: /bin/tool, allow: [{}]}}",
        )
        .unwrap();
        let policy = Policy::load(&path).unwrap();
        fs::write(&path, "commands: {}").unwrap();
        assert!(policy.prepare(start("tool", &[])).is_ok());
        assert!(
            Policy::load(&path)
                .unwrap()
                .prepare(start("tool", &[]))
                .is_err()
        );
    }
}
