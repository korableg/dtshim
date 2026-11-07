use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum HostEnvConfig {
    Name(String),
    Value(HashMap<String, String>),
}

#[derive(Default, Deserialize, garde::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnvConfig {
    #[serde(default)]
    #[garde(custom(validate_host_env))]
    pub(crate) host: Vec<HostEnvConfig>,
    #[serde(default)]
    #[garde(custom(validate_env_allow))]
    pub(crate) allow: Option<Vec<String>>,
}

#[derive(Deserialize, garde::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct OptionConfig {
    #[garde(
        length(min = 1),
        inner(pattern(r"\A(?:--[A-Za-z0-9_-]+|-[A-Za-z0-9])\z"))
    )]
    pub(crate) names: Vec<String>,
    #[garde(skip)]
    pub(crate) value: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub(crate) required: bool,
    #[serde(default)]
    #[garde(skip)]
    pub(crate) repeat: bool,
}

#[derive(Deserialize, garde::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct AllowedArgsConfig {
    #[serde(default)]
    #[garde(inner(pattern(r"\A[^\x00]*\z")))]
    pub(crate) prefix: Vec<String>,
    #[serde(default)]
    #[garde(dive, custom(validate_option_names))]
    pub(crate) options: Vec<OptionConfig>,
    #[serde(default)]
    #[garde(skip)]
    pub(crate) positionals: Vec<String>,
}

#[derive(Deserialize, garde::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommandConfig {
    #[garde(custom(validate_executable))]
    pub(crate) executable: String,
    #[serde(default)]
    #[garde(dive, custom(validate_env))]
    pub(crate) env: EnvConfig,
    #[garde(length(min = 1), dive)]
    pub(crate) allow: Vec<AllowedArgsConfig>,
}

#[derive(Deserialize, garde::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct PolicyConfig {
    #[garde(dive, custom(validate_command_names))]
    pub(crate) commands: HashMap<String, CommandConfig>,
}

fn validate_option_names(options: &[OptionConfig], _: &()) -> garde::Result {
    let mut seen = HashSet::new();
    for option in options {
        for name in &option.names {
            if !seen.insert(name) {
                return Err(garde::Error::new(format!("duplicate option name {name:?}")));
            }
        }
    }
    Ok(())
}

fn validate_executable(executable: &str, _: &()) -> garde::Result {
    if !Path::new(executable).is_absolute() || executable.contains('\0') {
        return Err(garde::Error::new(
            "executable must be an absolute host path without NUL bytes",
        ));
    }
    Ok(())
}

fn validate_host_env(entries: &[HostEnvConfig], _: &()) -> garde::Result {
    let mut seen = HashSet::new();
    for entry in entries {
        let (variable, value) = match entry {
            HostEnvConfig::Name(variable) => (variable, None),
            HostEnvConfig::Value(values) => {
                let mut values = values.iter();
                match (values.next(), values.next()) {
                    (Some((variable, value)), None) => (variable, Some(value)),
                    _ => {
                        return Err(garde::Error::new(
                            "each env.host mapping must contain exactly one variable",
                        ));
                    }
                }
            }
        };
        if variable.is_empty() || variable.contains(['=', '\0']) {
            return Err(garde::Error::new(format!(
                "invalid env.host name {variable:?}"
            )));
        }
        if value.is_some_and(|value| value.contains('\0')) {
            return Err(garde::Error::new(format!(
                "env.host value for {variable:?} contains a NUL byte"
            )));
        }
        if !seen.insert(variable) {
            return Err(garde::Error::new(format!(
                "duplicate env.host name {variable:?}"
            )));
        }
    }
    Ok(())
}

fn validate_env_allow(allow: &Option<Vec<String>>, _: &()) -> garde::Result {
    if let Some(names) = allow {
        let mut seen = HashSet::new();
        for name in names {
            if name.is_empty() || name.contains(['=', '\0']) {
                return Err(garde::Error::new(format!(
                    "invalid env.allow name {name:?}"
                )));
            }
            if !seen.insert(name) {
                return Err(garde::Error::new(format!(
                    "duplicate env.allow name {name:?}"
                )));
            }
        }
    }
    Ok(())
}

fn validate_env(env: &EnvConfig, _: &()) -> garde::Result {
    let Some(allow) = &env.allow else {
        return Ok(());
    };
    for entry in &env.host {
        let names = match entry {
            HostEnvConfig::Name(name) => vec![name],
            HostEnvConfig::Value(values) => values.keys().collect(),
        };
        for name in names {
            if !allow.contains(name) {
                return Err(garde::Error::new(format!(
                    "env.host name {name:?} is not included in env.allow"
                )));
            }
        }
    }
    Ok(())
}

fn validate_command_names(commands: &HashMap<String, CommandConfig>, _: &()) -> garde::Result {
    for name in commands.keys() {
        if name.is_empty()
            || name == "."
            || name == ".."
            || name
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, '/' | '\\' | '\0'))
        {
            return Err(garde::Error::new(format!("invalid command name {name:?}")));
        }
    }
    Ok(())
}

impl PolicyConfig {
    pub(crate) fn parse(yaml: &str) -> Result<Self> {
        let options = serde_saphyr::options! {
            duplicate_keys: serde_saphyr::DuplicateKeyPolicy::Error,
            merge_keys: serde_saphyr::MergeKeyPolicy::Error,
            strict_booleans: true,
            reject_unsupported_tags: true,
        };
        serde_saphyr::from_str_with_options_valid(yaml, options)
            .context("failed to parse or validate YAML policy")
    }

    pub(crate) fn load(path: &Path) -> Result<Self> {
        let yaml = fs::read_to_string(path)
            .with_context(|| format!("failed to read policy {}", path.display()))?;
        Self::parse(&yaml).with_context(|| format!("invalid policy {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_allow_list_validates_names_and_all_host_entries() {
        for env in [
            "{host: [HOME], allow: []}",
            "{host: [{HOME: fixed}], allow: [PATH]}",
            "{allow: ['']}",
            "{allow: ['A=B']}",
            r#"{allow: ["A\0B"]}"#,
            "{allow: [HOME, HOME]}",
            "{allow: HOME}",
            "{allow: [1]}",
            "{typo: []}",
        ] {
            let yaml =
                format!("commands: {{tool: {{executable: /bin/tool, env: {env}, allow: [{{}}]}}}}");
            assert!(PolicyConfig::parse(&yaml).is_err(), "{env}");
        }
        for env in [
            "{}",
            "{allow: null}",
            "{allow: []}",
            "{host: [HOME, {LANG: C}], allow: [HOME, LANG]}",
        ] {
            let yaml =
                format!("commands: {{tool: {{executable: /bin/tool, env: {env}, allow: [{{}}]}}}}");
            assert!(PolicyConfig::parse(&yaml).is_ok(), "{env}");
        }
        assert!(
            PolicyConfig::parse(
                "commands: {tool: {executable: /bin/tool, host_env: [], allow: [{}]}}"
            )
            .is_err()
        );
    }

    #[test]
    fn malformed_host_env_entries_are_rejected() {
        for entries in [
            "[{HOME: one, HOME: two}]",
            "[{A: 1}]",
            "[{A: true}]",
            "[{A: null}]",
            "[{A: []}]",
            "[{1: value}]",
            "[1]",
            "[true]",
            "[null]",
            "null",
            "HOME",
        ] {
            let yaml = format!(
                "commands: {{tool: {{executable: /bin/tool, env: {{host: {entries}}}, allow: [{{}}]}}}}"
            );
            assert!(PolicyConfig::parse(&yaml).is_err(), "{entries}");
        }
    }

    #[test]
    fn invalid_and_ambiguous_host_env_entries_are_rejected() {
        for entries in [
            "[{}]",
            "[{A: a, B: b}]",
            "[HOME, HOME]",
            "[HOME, {HOME: value}]",
            "[{HOME: value}, HOME]",
            "[{HOME: one}, {HOME: two}]",
            "[{'': value}]",
            "[{'A=B': value}]",
            r#"[{"A\0B": value}]"#,
            r#"[{A: "bad\0value"}]"#,
        ] {
            let yaml = format!(
                "commands: {{tool: {{executable: /bin/tool, env: {{host: {entries}}}, allow: [{{}}]}}}}"
            );
            assert!(PolicyConfig::parse(&yaml).is_err(), "{entries}");
        }
    }

    #[test]
    fn host_env_names_reject_only_invalid_environment_names() {
        for names in [r#"['']"#, r#"['A=B']"#, r#"["A\0B"]"#] {
            let yaml = format!(
                "commands: {{tool: {{executable: /bin/tool, env: {{host: {names}}}, allow: [{{}}]}}}}"
            );
            assert!(PolicyConfig::parse(&yaml).is_err(), "{names}");
        }
        assert!(
            PolicyConfig::parse(
                "commands: {tool: {executable: /bin/tool, env: {host: ['1', 'a-b', 'a.b', 'with space', 'имя']}, allow: [{}]}}"
            )
            .is_ok()
        );
    }

    #[test]
    fn invalid_command_names_executables_and_aliases_are_rejected() {
        for yaml in [
            "commands: {tool: {executable: tool, allow: [{}]}}",
            "commands: {'/bin/tool': {executable: /bin/tool, allow: [{}]}}",
            "commands: {'': {executable: /bin/tool, allow: [{}]}}",
            "commands: {tool: {executable: /bin/tool, allow: [{options: [{names: [-a, -a]}]}]}}",
            "commands: {tool: {executable: /bin/tool, allow: [{options: [{names: [-a]}, {names: [-a]}]}]}}",
            "commands: {'.': {executable: /bin/tool, allow: [{}]}}",
            "commands: {'..': {executable: /bin/tool, allow: [{}]}}",
            "commands: {'tool name': {executable: /bin/tool, allow: [{}]}}",
            "commands: {'tool\u{2003}name': {executable: /bin/tool, allow: [{}]}}",
            r#"commands: {"tool\\name": {executable: /bin/tool, allow: [{}]}}"#,
            r#"commands: {"tool\0name": {executable: /bin/tool, allow: [{}]}}"#,
            r#"commands: {tool: {executable: "/bin/tool\0", allow: [{}]}}"#,
        ] {
            assert!(
                PolicyConfig::parse(yaml).is_err(),
                "accepted invalid policy: {yaml}"
            );
        }
    }

    #[test]
    fn prefix_rejects_nul_but_accepts_empty_and_multiline_arguments() {
        assert!(
            PolicyConfig::parse(
                r#"commands: {tool: {executable: /bin/tool, allow: [{prefix: ["bad\0argument"]}]}}"#
            )
            .is_err()
        );
        assert!(
            PolicyConfig::parse(
                r#"commands: {tool: {executable: /bin/tool, allow: [{prefix: ["", "first\nsecond"]}]}}"#
            )
            .is_ok()
        );
    }

    #[test]
    fn malformed_and_ambiguous_yaml_is_rejected() {
        for yaml in [
            "",
            "{}",
            "commands: null",
            "commands: [tool]",
            "commands: {}\n---\ncommands: {}",
            "commands: {}\nunknown: true",
            "commands: {}\ncommands: {}",
            "commands: {tool: {executable: /bin/tool, allow: [{}]}, tool: {executable: /bin/other, allow: [{}]}}",
            "commands: {tool: {executable: /bin/tool, executable: /bin/other, allow: [{}]}}",
            "commands: {tool: {executable: /bin/tool, allow: [{prefix: [], prefix: []}]}}",
            "commands: {tool: {executable: /bin/tool, allow: [{options: [{names: [-a], names: [-b]}]}]}}",
            "commands: {tool: {executable: /bin/tool, allow: [{options: [{names: [-a], required: true, required: false}]}]}}",
            "commands: {tool: {executable: /bin/tool, typo: true, allow: [{}]}}",
            "commands: {tool: {executable: /bin/tool, allow: [{typo: true}]}}",
            "commands: {tool: {executable: /bin/tool, allow: [{options: [{names: [-a], typo: true}]}]}}",
            "commands: {tool: {executable: /bin/tool, allow: [{options: [{names: [-a], required: yes}]}]}}",
            "commands: {tool: {<<: {executable: /bin/tool}, allow: [{}]}}",
        ] {
            assert!(
                PolicyConfig::parse(yaml).is_err(),
                "accepted invalid policy: {yaml}"
            );
        }
    }

    #[test]
    fn empty_rules_and_invalid_option_names_are_rejected() {
        for yaml in [
            "commands: {tool: {executable: /bin/tool, allow: []}}",
            "commands: {tool: {executable: /bin/tool, allow: [{options: [{names: []}]}]}}",
            "commands: {tool: {executable: /bin/tool, allow: [{options: [{names: [-ab]}]}]}}",
            "commands: {tool: {executable: /bin/tool, allow: [{options: [{names: [--]}]}]}}",
            "commands: {tool: {executable: /bin/tool, allow: [{options: [{names: [--x=y]}]}]}}",
        ] {
            assert!(
                PolicyConfig::parse(yaml).is_err(),
                "accepted invalid policy: {yaml}"
            );
        }
    }

    #[test]
    fn option_names_preserve_the_existing_alphabet() {
        assert!(
            PolicyConfig::parse(
                "commands: {tool: {executable: /bin/tool, allow: [{options: [{names: [---, --_, --a-b_1, -A, -9]}]}]}}"
            )
            .is_ok()
        );
    }

    #[test]
    fn invalid_option_name_reports_its_yaml_path_and_location() {
        let yaml = "commands:\n  tool:\n    executable: /bin/tool\n    allow:\n      - options:\n          - names: [--ok, --bad=name]\n";
        let error = PolicyConfig::parse(yaml).err().unwrap();
        let error = error.downcast_ref::<serde_saphyr::Error>().unwrap();
        let location = error.location().unwrap();
        assert_eq!(location.line(), 6);
        assert_eq!(location.column(), 27);
        let rendered = error.to_string();
        assert!(
            rendered.contains("commands.tool.allow[0].options[0].names[1]"),
            "{rendered}"
        );
    }

    #[test]
    fn loading_reports_file_and_yaml_errors() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("policy.yaml");
        assert!(PolicyConfig::load(&path).is_err());
        fs::write(&path, "commands: [").unwrap();
        assert!(PolicyConfig::load(&path).is_err());
    }
}
