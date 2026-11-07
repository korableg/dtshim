use std::{
    cmp::Reverse,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Result, bail, ensure};
use dtshim_core::model::Start;

#[derive(Clone, Debug)]
pub(crate) struct Volume {
    host: String,
    docker: String,
}

impl FromStr for Volume {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (host, docker) = value
            .split_once(':')
            .ok_or("expected HOST:DOCKER with absolute paths")?;

        if !Path::new(host).is_absolute() || !Path::new(docker).is_absolute() {
            return Err("volume paths must be absolute".into());
        }
        if value.contains(['\0', ',']) || docker.contains(':') {
            return Err("volume paths cannot contain NUL, ':' or ','; use HOST:DOCKER".into());
        }
        Ok(Self {
            host: Path::new(host)
                .components()
                .collect::<PathBuf>()
                .to_string_lossy()
                .into_owned(),
            docker: Path::new(docker)
                .components()
                .collect::<PathBuf>()
                .to_string_lossy()
                .into_owned(),
        })
    }
}

#[derive(Debug)]
struct Replacement {
    from: String,
    to: String,
}

fn left_boundary(byte: u8) -> bool {
    right_boundary(byte)
        || matches!(
            byte,
            b'=' | b':'
                | b','
                | b';'
                | b'('
                | b')'
                | b'['
                | b']'
                | b'{'
                | b'}'
                | b'<'
                | b'>'
                | b'|'
                | b'&'
                | b'?'
                | b'#'
        )
}

fn right_boundary(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t'..=b'\r' | 0 | b'\'' | b'"')
}

enum PrefixMatch {
    Pending,
    Unmatched,
    Mapped { prefix: PathBuf, consumed: usize },
}

fn match_prefix(candidate: &[u8], replacements: &[Replacement], complete: bool) -> PrefixMatch {
    for replacement in replacements {
        let source = replacement.from.as_bytes();
        if candidate.len() < source.len() {
            if !complete && source.starts_with(candidate) {
                return PrefixMatch::Pending;
            }
            continue;
        }
        if !candidate.starts_with(source) {
            continue;
        }
        let next = candidate.get(source.len()).copied();
        if next.is_none() && !complete {
            return PrefixMatch::Pending;
        }
        if source != b"/" && next.is_some_and(|byte| byte != b'/') {
            continue;
        }
        let needs_slash = if source == b"/" {
            next.is_some()
        } else {
            next == Some(b'/')
        };
        let prefix = if needs_slash {
            Path::new(&replacement.to).join("")
        } else {
            PathBuf::from(&replacement.to)
        };
        let consumed = source.len() + usize::from(source != b"/" && next == Some(b'/'));
        return PrefixMatch::Mapped { prefix, consumed };
    }
    PrefixMatch::Unmatched
}

pub(crate) struct StreamMapper<'a> {
    replacements: &'a [Replacement],
    pending: Vec<u8>,
    pending_limit: usize,
    previous: Option<u8>,
    passthrough: bool,
}

impl<'a> StreamMapper<'a> {
    fn new(replacements: &'a [Replacement]) -> Self {
        Self {
            replacements,
            pending: Vec::new(),
            pending_limit: replacements.first().map_or(0, |item| item.from.len() + 1),
            previous: None,
            passthrough: false,
        }
    }

    pub(crate) fn push(&mut self, data: &[u8]) -> Vec<u8> {
        if self.replacements.is_empty() {
            return data.to_vec();
        }
        let mut output = Vec::with_capacity(data.len());
        for &byte in data {
            if self.passthrough {
                output.push(byte);
                if right_boundary(byte) {
                    self.passthrough = false;
                }
            } else if !self.pending.is_empty()
                || (byte == b'/' && self.previous.is_none_or(left_boundary))
            {
                self.buffer(byte);
                self.resolve(&mut output, false);
            } else {
                output.push(byte);
            }
            self.previous = Some(byte);
        }
        output
    }

    pub(crate) fn finish(&mut self) -> Vec<u8> {
        let mut output = Vec::new();
        self.resolve(&mut output, true);
        output
    }

    fn buffer(&mut self, byte: u8) {
        if self.pending.len() == self.pending.capacity() {
            let additional = self
                .pending
                .capacity()
                .max(256)
                .min(self.pending_limit - self.pending.len());
            self.pending.reserve_exact(additional);
        }
        self.pending.push(byte);
    }

    fn resolve(&mut self, output: &mut Vec<u8>, eof: bool) {
        if self.pending.is_empty() {
            return;
        }
        let terminated = self
            .pending
            .last()
            .is_some_and(|&byte| right_boundary(byte));
        let candidate_len = self.pending.len() - usize::from(terminated);
        let candidate = &self.pending[..candidate_len];
        match match_prefix(candidate, self.replacements, eof || terminated) {
            PrefixMatch::Pending => return,
            PrefixMatch::Unmatched => output.extend_from_slice(&self.pending),
            PrefixMatch::Mapped { prefix, consumed } => {
                output.extend_from_slice(prefix.as_os_str().as_bytes());
                output.extend_from_slice(&self.pending[consumed..]);
            }
        }
        self.passthrough = !terminated;
        self.pending.clear();
    }
}

#[derive(Debug, Default)]
pub(crate) struct Volumes {
    from_host: Vec<Replacement>,
    to_host: Vec<Replacement>,
}

impl Volumes {
    pub(crate) fn new(volumes: Vec<Volume>) -> Result<Self> {
        let mut result = Self::default();
        for volume in volumes {
            if let Some(existing) = result
                .from_host
                .iter()
                .find(|item| item.from == volume.host)
            {
                ensure!(
                    existing.to == volume.docker,
                    "host path {:?} has multiple volume mappings",
                    volume.host
                );
                continue;
            }
            if result.to_host.iter().any(|item| item.from == volume.docker) {
                bail!(
                    "Docker path {:?} has multiple volume mappings",
                    volume.docker
                );
            }
            result.from_host.push(Replacement {
                from: volume.host.clone(),
                to: volume.docker.clone(),
            });
            result.to_host.push(Replacement {
                from: volume.docker,
                to: volume.host,
            });
        }
        result
            .from_host
            .sort_by_key(|item| Reverse(item.from.len()));
        result.to_host.sort_by_key(|item| Reverse(item.from.len()));
        Ok(result)
    }

    pub(crate) fn map_from_host(&self, data: &str) -> String {
        Self::map(data, &self.from_host)
    }

    pub(crate) fn map_to_host(&self, data: &str) -> String {
        Self::map(data, &self.to_host)
    }

    pub(crate) fn map_start(&self, mut start: Start) -> Start {
        start.cwd = self.map_to_host(&start.cwd);
        for arg in &mut start.args {
            *arg = self.map_to_host(arg);
        }
        for (_, value) in &mut start.env {
            *value = self.map_to_host(value);
        }
        start
    }

    fn map(data: &str, replacements: &[Replacement]) -> String {
        if replacements.is_empty() {
            return data.to_owned();
        }
        let mut output = String::with_capacity(data.len());
        let mut copied = 0;
        let mut offset = 0;
        let bytes = data.as_bytes();
        while offset < bytes.len() {
            let previous = offset.checked_sub(1).map(|index| bytes[index]);
            if bytes[offset] != b'/' || !previous.is_none_or(left_boundary) {
                offset += 1;
                continue;
            }
            let end = bytes[offset..]
                .iter()
                .position(|&byte| right_boundary(byte))
                .map_or(bytes.len(), |length| offset + length);
            if let PrefixMatch::Mapped { prefix, consumed } =
                match_prefix(&bytes[offset..end], replacements, true)
            {
                output.push_str(&data[copied..offset]);
                output.push_str(&prefix.to_string_lossy());
                output.push_str(&data[offset + consumed..end]);
                copied = end;
            }
            offset = end;
        }
        output.push_str(&data[copied..]);
        output
    }

    pub(crate) fn stream_from_host(&self) -> StreamMapper<'_> {
        StreamMapper::new(&self.from_host)
    }

    pub(crate) fn stream_to_host(&self) -> StreamMapper<'_> {
        StreamMapper::new(&self.to_host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volumes(values: &[&str]) -> Volumes {
        Volumes::new(values.iter().map(|value| value.parse().unwrap()).collect()).unwrap()
    }

    fn fixtures() -> Volumes {
        volumes(&[
            "/host:/app",
            "/special:/app/nested",
            "/config.json:/etc/config.json",
            "/other:/host",
            "/данные:/контейнер",
        ])
    }

    #[test]
    fn maps_start_paths_without_changing_command_or_env_names() {
        let start = Start {
            command: "/app/tool".to_owned(),
            args: vec!["--file=/app/input".to_owned(), "unchanged".to_owned()],
            env: vec![
                ("/app/name".to_owned(), "/app/value".to_owned()),
                ("OTHER".to_owned(), "unchanged".to_owned()),
            ],
            cwd: "/app/work".to_owned(),
        };
        let mapped = volumes(&["/host:/app"]).map_start(start);
        assert_eq!(mapped.command, "/app/tool");
        assert_eq!(mapped.args, ["--file=/host/input", "unchanged"]);
        assert_eq!(
            mapped.env,
            vec![
                ("/app/name".to_owned(), "/host/value".to_owned()),
                ("OTHER".to_owned(), "unchanged".to_owned()),
            ]
        );
        assert_eq!(mapped.cwd, "/host/work");
    }

    #[test]
    fn parses_and_normalizes_directories_files_and_roots() {
        for (input, host, docker) in [
            ("/host:/app", "/host", "/app"),
            ("/host//./dir/:/app//./dir/", "/host/dir", "/app/dir"),
            (
                "/config.json:/etc/config.json",
                "/config.json",
                "/etc/config.json",
            ),
            ("/:/app/", "/", "/app"),
            ("/host/:/", "/host", "/"),
            ("/host/../dir:/app", "/host/../dir", "/app"),
        ] {
            let volume: Volume = input.parse().unwrap();
            assert_eq!(
                (volume.host.as_str(), volume.docker.as_str()),
                (host, docker),
                "{input}"
            );
        }
    }

    #[test]
    fn rejects_invalid_and_ambiguous_mappings() {
        for input in [
            "",
            "/host",
            ":",
            "host:/app",
            "/host:app",
            ":/app",
            "/host:",
            "/host:/app:ro",
            "/host:/ap,p",
            "/ho,st:/app",
            "/ho\0st:/app",
        ] {
            assert!(input.parse::<Volume>().is_err(), "{input:?}");
        }
        for inputs in [
            ["/host:/app", "/host/:/other"],
            ["/host:/app", "/other:/app/"],
        ] {
            let parsed = inputs.iter().map(|value| value.parse().unwrap()).collect();
            assert!(Volumes::new(parsed).is_err(), "{inputs:?}");
        }
        let duplicate = volumes(&["/host:/app", "/host/:/app/"]);
        assert_eq!(duplicate.from_host.len(), 1);
        assert_eq!(duplicate.to_host.len(), 1);
    }

    #[test]
    fn maps_strings_in_both_directions_without_cascading() {
        let volumes = fixtures();
        for (docker, host) in [
            ("/app", "/host"),
            ("/app/", "/host/"),
            ("--file='/etc/config.json'", "--file='/config.json'"),
            ("\"/app/a\" '/app/nested/b'", "\"/host/a\" '/special/b'"),
            ("/app/nested", "/special"),
            ("/app/nest", "/host/nest"),
            ("/app/nestedly/file", "/host/nestedly/file"),
            ("/app//./nested/file", "/host//./nested/file"),
            ("/host/file", "/other/file"),
            ("/контейнер/файл.txt", "/данные/файл.txt"),
        ] {
            assert_eq!(volumes.map_to_host(docker), host, "{docker}");
            assert_eq!(volumes.map_from_host(host), docker, "{host}");
        }
    }

    #[test]
    fn leaves_relative_sibling_and_nonliteral_prefixes_unchanged() {
        let volumes = fixtures();
        for input in [
            "foo/app ./app ../app /apple /app.txt /app-other",
            "foo/host ./host ../host /hosted /host.txt /host-other",
            "//app /./app //host /./host",
            "foo\\/app foo\\/host",
            "/etc/config.json.bak /config.json.bak",
        ] {
            assert_eq!(volumes.map_to_host(input), input);
            assert_eq!(volumes.map_from_host(input), input);
        }
    }

    #[test]
    fn treats_all_ascii_whitespace_quotes_and_nul_as_boundaries() {
        let volumes = fixtures();
        for delimiter in [' ', '\t', '\n', '\x0b', '\x0c', '\r', '\'', '"', '\0'] {
            let input = format!("/app{delimiter}/app/nested{delimiter}/etc/config.json");
            let expected = format!("/host{delimiter}/special{delimiter}/config.json");
            assert_eq!(volumes.map_to_host(&input), expected, "{delimiter:?}");
            assert_eq!(volumes.map_from_host(&expected), input, "{delimiter:?}");
        }
    }

    #[test]
    fn joins_root_mappings_without_extra_slashes() {
        for (mapping, docker, host) in [
            ("/host:/", "/", "/host"),
            ("/host:/", "/a/b", "/host/a/b"),
            ("/host:/", "//a", "/host//a"),
            ("/:/app", "/app", "/"),
            ("/:/app", "/app/a/b", "/a/b"),
            ("/:/", "/a/b", "/a/b"),
        ] {
            let volumes = volumes(&[mapping]);
            assert_eq!(volumes.map_to_host(docker), host, "{mapping}: {docker}");
            assert_eq!(volumes.map_from_host(host), docker, "{mapping}: {host}");
            for (from_host, input, expected) in [(false, docker, host), (true, host, docker)] {
                for split in 0..=input.len() {
                    let mut stream = if from_host {
                        volumes.stream_from_host()
                    } else {
                        volumes.stream_to_host()
                    };
                    let mut actual = stream.push(&input.as_bytes()[..split]);
                    actual.extend(stream.push(&input.as_bytes()[split..]));
                    actual.extend(stream.finish());
                    assert_eq!(
                        actual,
                        expected.as_bytes(),
                        "{mapping}: {input}, split {split}"
                    );
                }
            }
        }
        let volumes = volumes(&["/:/app"]);
        assert_eq!(volumes.map_to_host("/app/"), "/");
    }

    #[test]
    fn streams_preserve_expected_output_at_every_split_and_bytewise() {
        let volumes = fixtures();
        let cases: &[(bool, &[u8], &[u8])] = &[
            (
                false,
                b"'/app/nested/a' /app/nest /apple ./app",
                b"'/special/a' /host/nest /apple ./app",
            ),
            (
                false,
                b"/app/nest\xff/./app\0/app",
                b"/host/nest\xff/./app\0/host",
            ),
            (false, b"/app//./nested/x /ap", b"/host//./nested/x /ap"),
            (
                false,
                "/контейнер/файл /app/nested".as_bytes(),
                "/данные/файл /special".as_bytes(),
            ),
            (
                true,
                b"/special/\xff\xfe '/config.json' /host",
                b"/app/nested/\xff\xfe '/etc/config.json' /app",
            ),
            (
                true,
                "/данные/файл /other/app".as_bytes(),
                "/контейнер/файл /host/app".as_bytes(),
            ),
        ];
        for &(from_host, input, expected) in cases {
            let make_stream = || {
                if from_host {
                    volumes.stream_from_host()
                } else {
                    volumes.stream_to_host()
                }
            };
            for split in 0..=input.len() {
                let mut stream = make_stream();
                let mut actual = stream.push(&input[..split]);
                actual.extend(stream.push(&input[split..]));
                actual.extend(stream.finish());
                assert_eq!(actual, expected, "{input:?}, split {split}");
            }
            let mut stream = make_stream();
            let mut actual = Vec::new();
            for &byte in input {
                actual.extend(stream.push(&[byte]));
                assert!(stream.pending.len() <= stream.pending_limit);
            }
            actual.extend(stream.finish());
            assert_eq!(actual, expected, "{input:?}, bytewise");
        }
    }

    #[test]
    fn flushes_exact_and_incomplete_prefixes_at_eof() {
        let volumes = fixtures();
        for (input, expected) in [
            ("/", "/"),
            ("/ap", "/ap"),
            ("/app", "/host"),
            ("/app/nest", "/host/nest"),
            ("/app/nested", "/special"),
        ] {
            let mut stream = volumes.stream_to_host();
            assert!(stream.push(input.as_bytes()).is_empty(), "{input}");
            assert_eq!(stream.finish(), expected.as_bytes(), "{input}");
            assert!(stream.finish().is_empty());
        }
    }

    #[test]
    fn emits_unmatched_prompts_and_resolved_prefixes_immediately() {
        let volumes = volumes(&["/Users/me:/workspace"]);
        let mut stream = volumes.stream_from_host();
        assert_eq!(stream.push(b"prompt> /etc"), b"prompt> /etc");
        assert_eq!(stream.push(b" "), b" ");
        assert!(stream.push(b"/Us").is_empty());
        assert_eq!(stream.pending, b"/Us");
        assert_eq!(stream.push(b"X"), b"/UsX");
        assert_eq!(stream.push(b"/Users/me\n"), b"/Users/me\n");
        assert_eq!(stream.push(b"/Users/me/"), b"/workspace/");
        assert_eq!(stream.push(b"Users/me\xff"), b"Users/me\xff");
        assert!(stream.finish().is_empty());

        let empty = Volumes::default();
        let mut stream = empty.stream_to_host();
        assert_eq!(stream.push(b"/\xffprompt"), b"/\xffprompt");
        assert!(stream.pending.is_empty());
        assert!(stream.finish().is_empty());
    }

    #[test]
    fn bounds_pending_and_passes_long_suffixes_without_an_os_limit() {
        let volumes = volumes(&["/host:/app", "/special:/app/nested"]);
        let mut stream = volumes.stream_to_host();
        assert_eq!(stream.pending_limit, b"/app/nested".len() + 1);
        assert!(stream.push(b"/app/nested").is_empty());
        assert_eq!(stream.pending, b"/app/nested");
        assert_eq!(stream.push(b"/"), b"/special/");
        assert!(stream.pending.is_empty());

        let suffix = format!("{}//./app/nested", "x".repeat(100_000));
        assert_eq!(stream.push(suffix.as_bytes()), suffix.as_bytes());
        assert!(stream.pending.is_empty());
        assert!(stream.finish().is_empty());
        assert_eq!(
            volumes.map_to_host(&format!("/app/nested/{suffix}")),
            format!("/special/{suffix}")
        );
        assert_eq!(
            volumes.map_from_host(&format!("/special/{suffix}")),
            format!("/app/nested/{suffix}")
        );
    }
}
