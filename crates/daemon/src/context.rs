use anyhow::Result;
use dtshim_core::model::Start;

use crate::policy::Policy;
use crate::volume::Volumes;

pub(crate) struct Ctx {
    pub(crate) token: Option<String>,
    pub(crate) policy: Policy,
    pub(crate) volumes: Volumes,
}

impl Ctx {
    pub(crate) fn authenticates(&self, token: Option<&str>) -> bool {
        self.token
            .as_deref()
            .is_none_or(|expected| token == Some(expected))
    }

    pub(crate) fn prepare(&self, start: Start) -> Result<Start> {
        let start = self.volumes.map_start(start);
        self.policy.prepare(start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authentication_distinguishes_disabled_empty_missing_and_exact_tokens() {
        for (expected, provided, accepted) in [
            (None, None, true),
            (None, Some("anything"), true),
            (Some(""), Some(""), true),
            (Some(""), None, false),
            (Some("secret"), Some("secret"), true),
            (Some("secret"), None, false),
            (Some("secret"), Some(""), false),
            (Some("secret"), Some("Secret"), false),
            (Some("secret"), Some("secret "), false),
        ] {
            let context = Ctx {
                token: expected.map(str::to_owned),
                policy: Policy::default(),
                volumes: Volumes::default(),
            };
            assert_eq!(
                context.authenticates(provided),
                accepted,
                "{expected:?}, {provided:?}"
            );
        }
    }

    #[test]
    fn paths_are_mapped_before_policy_matching_and_host_overrides_are_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("policy.yaml");
        std::fs::write(&path, "commands: {tool: {executable: /bin/tool, env: {host: [{FIXED: /app/fixed}]}, allow: [{prefix: [/host/file]}]}}").unwrap();
        let context = Ctx {
            token: None,
            policy: Policy::new(Some(&path)).unwrap(),
            volumes: Volumes::new(vec!["/host:/app".parse().unwrap()]).unwrap(),
        };
        let request = Start {
            token: Some("/app/token".into()),
            command: "tool".into(),
            args: vec!["/app/file".into()],
            env: vec![
                ("PATH_VALUE".into(), "/app/value".into()),
                ("FIXED".into(), "client".into()),
            ],
            cwd: "/app/work".into(),
        };
        let prepared = context.prepare(request).unwrap();
        assert_eq!(prepared.token.as_deref(), Some("/app/token"));
        assert_eq!(prepared.command, "/bin/tool");
        assert_eq!(prepared.args, ["/host/file"]);
        assert_eq!(
            prepared.env,
            [
                ("PATH_VALUE".to_owned(), "/host/value".to_owned()),
                ("FIXED".to_owned(), "/app/fixed".to_owned())
            ]
        );
        assert_eq!(prepared.cwd, "/host/work");
    }

    #[test]
    fn prepare_propagates_policy_denial() {
        let context = Ctx {
            token: None,
            policy: Policy::default(),
            volumes: Volumes::default(),
        };
        let error = context
            .prepare(Start {
                token: None,
                command: String::new(),
                args: vec![],
                env: vec![],
                cwd: "/".into(),
            })
            .unwrap_err();
        assert_eq!(error.to_string(), "command is empty");
    }
}
