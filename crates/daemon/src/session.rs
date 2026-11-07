use std::{os::unix::process::ExitStatusExt, process::Stdio, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use dtshim_core::{codec::PostcardCodec, model::Message};
use futures::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    process::{Child, Command},
    time::timeout,
};
use tokio_util::{
    codec::{FramedRead, FramedWrite},
    sync::CancellationToken,
};

use crate::context::Ctx;

const START_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) async fn run<S>(
    stream: S,
    cancellation: CancellationToken,
    context: Arc<Ctx>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let volumes = &context.volumes;
    let mut child: Option<Child> = None;

    let result = {
        let connection = async {
            let (rd, wr) = tokio::io::split(stream);
            let mut rx = FramedRead::new(rd, PostcardCodec::new());
            let mut tx = FramedWrite::new(wr, PostcardCodec::new());

            let message = match timeout(START_TIMEOUT, rx.next()).await {
                Ok(Some(message)) => message.context("failed to receive start message")?,
                Ok(None) => bail!("client disconnected before sending start message"),
                Err(_) => {
                    tx.send(Message::Failed("start message timed out".into()))
                        .await?;
                    return Ok(());
                }
            };
            let Message::Start(start) = message else {
                bail!("the first message should be a start");
            };
            if !context.authenticates(start.token.as_deref()) {
                tx.send(Message::Failed("authentication failed".into()))
                    .await
                    .context("failed to send authentication denial")?;
                return Ok(());
            }
            let start = match context.prepare(start) {
                Ok(start) => start,
                Err(err) => {
                    let message = volumes.map_from_host(&format!("policy denied: {err:#}"));
                    tx.send(Message::Failed(message))
                        .await
                        .context("failed to send policy denial")?;
                    return Ok(());
                }
            };
            eprintln!("start: {:?} {:?}", &start.command, &start.args);
            child = match Command::new(&start.command)
                .args(&start.args)
                .envs(start.env)
                .current_dir(&start.cwd)
                .stderr(Stdio::piped())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .with_context(|| format!("failed to spawn command {:?}", start.command))
            {
                Ok(child) => Some(child),
                Err(err) => {
                    eprintln!("failed: {err:#}");
                    let message = volumes.map_from_host(&format!("{err:#}"));
                    let _ = tx.send(Message::Failed(message)).await;
                    return Err(err);
                }
            };

            let child = child.as_mut().context("child was not initialized")?;
            let mut stdin = Some(child.stdin.take().context("failed to take child stdin")?);
            let mut stdout = child.stdout.take().context("failed to take child stdout")?;
            let mut stderr = child.stderr.take().context("failed to take child stderr")?;
            let mut out_buf = vec![0u8; 8192];
            let mut err_buf = vec![0u8; 8192];

            let input = async {
                let mut mapper = volumes.stream_to_host();
                while let Some(msg) = rx.next().await {
                    match msg.context("failed to receive client message")? {
                        Message::Stdin(data) => {
                            if !data.is_empty() {
                                eprintln!("stdin: {:?}", String::from_utf8_lossy(&data));
                            }
                            let Some(pipe) = stdin.as_mut() else {
                                continue;
                            };
                            let eof = data.is_empty();
                            let data = if eof {
                                mapper.finish()
                            } else {
                                mapper.push(&data)
                            };
                            if let Err(err) = pipe.write_all(&data).await {
                                eprintln!("stdin error: {err}");
                                stdin.take();
                            } else if eof {
                                stdin.take();
                                eprintln!("stdin: closed");
                            }
                        }
                        other => bail!("unexpected message from client: {other:?}"),
                    }
                }
                bail!("client disconnected")
            };

            let output = async {
                let mut stdout_done = false;
                let mut stderr_done = false;
                let mut stdout_mapper = volumes.stream_from_host();
                let mut stderr_mapper = volumes.stream_from_host();

                while !stdout_done || !stderr_done {
                    let message = tokio::select! {
                        result = stdout.read(&mut out_buf), if !stdout_done => {
                            match result.context("failed to read child stdout")? {
                                0 => {
                                    stdout_done = true;
                                    eprintln!("stdout: closed");
                                    let data = stdout_mapper.finish();
                                    (!data.is_empty()).then_some(Message::Stdout(data))
                                }
                                n => {
                                    eprintln!("stdout: {:?}", String::from_utf8_lossy(&out_buf[..n]));
                                    let data = stdout_mapper.push(&out_buf[..n]);
                                    (!data.is_empty()).then_some(Message::Stdout(data))
                                }
                            }
                        }

                        result = stderr.read(&mut err_buf), if !stderr_done => {
                            match result.context("failed to read child stderr")? {
                                0 => {
                                    stderr_done = true;
                                    eprintln!("stderr: closed");
                                    let data = stderr_mapper.finish();
                                    (!data.is_empty()).then_some(Message::Stderr(data))
                                }
                                n => {
                                    eprintln!("stderr: {:?}", String::from_utf8_lossy(&err_buf[..n]));
                                    let data = stderr_mapper.push(&err_buf[..n]);
                                    (!data.is_empty()).then_some(Message::Stderr(data))
                                }
                            }
                        }
                    };

                    if let Some(message) = message {
                        tx.send(message)
                            .await
                            .context("failed to send child output")?;
                    }
                }

                let status = match child
                    .wait()
                    .await
                    .context("failed to wait for child process")
                {
                    Ok(status) => status,
                    Err(err) => {
                        eprintln!("failed: {err:#}");
                        let _ = tx.send(Message::Failed(format!("{err:#}"))).await;
                        return Err(err);
                    }
                };

                if let Some(code) = status.code() {
                    eprintln!("code: {code}");
                    tx.send(Message::Code(code))
                        .await
                        .context("failed to send child exit code")?;
                } else if let Some(signal) = status.signal() {
                    eprintln!("signaled: {signal}");
                    tx.send(Message::Signaled(signal))
                        .await
                        .context("failed to send child termination signal")?;
                }

                anyhow::Ok(())
            };

            tokio::select! {
                result = input => result,
                result = output => result,
            }
        };

        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                eprintln!("cancelled");
                Ok(())
            },
            result = connection => result,
        }
    };

    if let Some(child) = child.as_mut() {
        if let Err(err) = child
            .kill()
            .await
            .context("failed to terminate child process")
        {
            if result.is_ok() {
                return Err(err);
            }
            eprintln!("{err:#}");
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use dtshim_core::model::Start;
    use tokio::{
        io::{DuplexStream, duplex},
        task::JoinHandle,
    };
    use tokio_util::codec::Framed;

    use crate::{policy::Policy, volume::Volumes};

    use super::*;

    fn start(script: &str) -> Start {
        Start {
            token: None,
            command: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            env: Vec::new(),
            cwd: "/".into(),
        }
    }

    fn connect(
        context: Ctx,
    ) -> (
        Framed<DuplexStream, PostcardCodec>,
        CancellationToken,
        JoinHandle<Result<()>>,
    ) {
        let (client, server) = duplex(65536);
        let cancellation = CancellationToken::new();
        let task = tokio::spawn(run(server, cancellation.clone(), Arc::new(context)));
        (
            Framed::new(client, PostcardCodec::new()),
            cancellation,
            task,
        )
    }

    fn context(token: Option<&str>) -> Ctx {
        Ctx {
            token: token.map(str::to_owned),
            policy: Policy::default(),
            volumes: Volumes::default(),
        }
    }

    async fn receive(client: &mut Framed<DuplexStream, PostcardCodec>) -> Message {
        timeout(Duration::from_secs(5), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn authentication_rejects_missing_or_wrong_tokens_before_policy_and_spawn() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("spawned");
        for token in [None, Some("wrong")] {
            let (mut client, _, task) = connect(context(Some("required")));
            let mut request = start(&format!("touch '{}'", marker.display()));
            request.token = token.map(str::to_owned);
            request.cwd = "/does-not-exist".into();
            client.send(Message::Start(request)).await.unwrap();
            assert!(
                matches!(receive(&mut client).await, Message::Failed(message) if message == "authentication failed")
            );
            task.await.unwrap().unwrap();
            assert!(!marker.exists());
        }
    }

    #[tokio::test]
    async fn token_is_optional_when_disabled_and_compared_before_volume_mapping() {
        for (expected, provided) in [
            (None, None),
            (None, Some("anything")),
            (Some("/app/token"), Some("/app/token")),
        ] {
            let mut ctx = context(expected);
            ctx.volumes = Volumes::new(vec!["/host:/app".parse().unwrap()]).unwrap();
            let (mut client, _, task) = connect(ctx);
            let mut request = start("exit 0");
            request.token = provided.map(str::to_owned);
            client.send(Message::Start(request)).await.unwrap();
            assert!(matches!(receive(&mut client).await, Message::Code(0)));
            task.await.unwrap().unwrap();
        }
    }

    #[tokio::test]
    async fn explicit_stdin_eof_keeps_session_alive_and_drains_both_outputs_before_status() {
        let (mut client, _, task) = connect(context(None));
        client
            .send(Message::Start(start(
                "cat >/dev/null; printf 'final stdout'; printf 'final stderr' >&2; exit 7",
            )))
            .await
            .unwrap();
        client
            .send(Message::Stdin(b"input".to_vec()))
            .await
            .unwrap();
        client.send(Message::Stdin(Vec::new())).await.unwrap();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        loop {
            match receive(&mut client).await {
                Message::Stdout(data) => stdout.extend(data),
                Message::Stderr(data) => stderr.extend(data),
                Message::Code(7) => break,
                other => panic!("unexpected message: {other:?}"),
            }
        }
        assert_eq!(stdout, b"final stdout");
        assert_eq!(stderr, b"final stderr");
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn disconnect_after_stdin_eof_or_broken_pipe_ends_the_session() {
        for (script, eof) in [
            ("cat >/dev/null; echo ready; exec sleep 60", true),
            ("exec 0<&-; echo ready; exec sleep 60", false),
        ] {
            let (mut client, _, task) = connect(context(None));
            client.send(Message::Start(start(script))).await.unwrap();
            if eof {
                client.send(Message::Stdin(Vec::new())).await.unwrap();
            }
            let mut output = String::new();
            while !output.contains('\n') {
                match receive(&mut client).await {
                    Message::Stdout(data) => output.push_str(std::str::from_utf8(&data).unwrap()),
                    other => panic!("unexpected message: {other:?}"),
                }
            }
            assert_eq!(output, "ready\n");
            if !eof {
                client
                    .send(Message::Stdin(b"write to closed stdin".to_vec()))
                    .await
                    .unwrap();
                tokio::task::yield_now().await;
            }
            drop(client);
            let result = timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap();
            assert!(result.is_err());
        }
    }

    #[tokio::test]
    async fn handshake_deadline_does_not_limit_a_running_command_and_shutdown_ends_the_session() {
        let (mut client, cancellation, task) = connect(context(None));
        client
            .send(Message::Start(start("echo ready; exec sleep 60")))
            .await
            .unwrap();
        let mut output = String::new();
        while !output.contains('\n') {
            match receive(&mut client).await {
                Message::Stdout(data) => output.push_str(std::str::from_utf8(&data).unwrap()),
                other => panic!("unexpected message: {other:?}"),
            }
        }
        assert_eq!(output, "ready\n");
        tokio::time::pause();
        tokio::time::advance(START_TIMEOUT + Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert!(!task.is_finished());
        tokio::time::resume();
        cancellation.cancel();
        timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn first_complete_start_frame_has_a_deadline() {
        let (client, _, task) = connect(context(None));
        let mut stream = client.into_inner();
        stream.write_all(&[0, 0]).await.unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(START_TIMEOUT + Duration::from_secs(1)).await;
        let mut client = Framed::new(stream, PostcardCodec::new());
        assert!(
            matches!(receive(&mut client).await, Message::Failed(message) if message == "start message timed out")
        );
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn handshake_rejects_disconnect_wrong_message_and_invalid_encoding() {
        let (client, _, task) = connect(context(None));
        drop(client);
        let error = timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "client disconnected before sending start message"
        );

        for message in [Message::Code(0), Message::Stdin(vec![])] {
            let (mut client, _, task) = connect(context(None));
            client.send(message).await.unwrap();
            let error = timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err();
            assert_eq!(error.to_string(), "the first message should be a start");
        }

        let (client, _, task) = connect(context(None));
        client
            .into_inner()
            .write_all(&[0, 0, 0, 1, 255])
            .await
            .unwrap();
        let error = timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.to_string(), "failed to receive start message");
    }

    #[tokio::test]
    async fn cancelled_session_does_not_wait_for_a_start_message() {
        let (_client, cancellation, task) = connect(context(None));
        cancellation.cancel();
        timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn policy_denial_is_sent_without_spawning_a_child() {
        let (mut client, _, task) = connect(context(None));
        let mut request = start("exit 0");
        request.command.clear();
        client.send(Message::Start(request)).await.unwrap();
        assert!(
            matches!(receive(&mut client).await, Message::Failed(message) if message == "policy denied: command is empty")
        );
        timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn spawn_failures_are_sent_with_container_paths() {
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let mut ctx = context(None);
        ctx.volumes = Volumes::new(vec![
            format!("{}:/app", directory.path().display())
                .parse()
                .unwrap(),
        ])
        .unwrap();
        let (mut client, _, task) = connect(ctx);
        let mut request = start("exit 0");
        request.command = directory
            .path()
            .join("missing-command")
            .to_str()
            .unwrap()
            .into();
        client.send(Message::Start(request)).await.unwrap();
        let Message::Failed(message) = receive(&mut client).await else {
            panic!("expected spawn failure");
        };
        assert!(message.contains("/app/missing-command"), "{message}");
        assert!(
            !message.contains(directory.path().to_str().unwrap()),
            "{message}"
        );
        let error = timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().starts_with("failed to spawn command"));
    }

    #[tokio::test]
    async fn child_signal_is_reported_as_a_signal_not_an_exit_code() {
        let (mut client, _, task) = connect(context(None));
        client
            .send(Message::Start(start("kill -TERM $$")))
            .await
            .unwrap();
        assert!(matches!(receive(&mut client).await, Message::Signaled(15)));
        timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn non_stdin_messages_after_start_end_the_session() {
        let (mut client, _, task) = connect(context(None));
        client
            .send(Message::Start(start("printf ready; exec cat")))
            .await
            .unwrap();
        let mut ready = Vec::new();
        while ready.len() < b"ready".len() {
            match receive(&mut client).await {
                Message::Stdout(data) => ready.extend(data),
                other => panic!("unexpected message: {other:?}"),
            }
        }
        assert_eq!(ready, b"ready");
        client.send(Message::Code(0)).await.unwrap();
        let error = timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.to_string(), "unexpected message from client: Code(0)");
    }

    #[tokio::test]
    async fn maps_stdin_across_frames_and_flushes_pending_output_at_eof() {
        let mut ctx = context(None);
        ctx.volumes = Volumes::new(vec!["/host:/app".parse().unwrap()]).unwrap();
        let (mut client, _, task) = connect(ctx);
        let mut request = start(
            "read -r value; [ \"$value\" = \"$EXPECTED\" ] || exit 42; printf '%s' \"$EXPECTED\"; printf '%s' \"$EXPECTED\" >&2",
        );
        request.env = vec![("EXPECTED".into(), "/host".into())];
        client.send(Message::Start(request)).await.unwrap();
        client.send(Message::Stdin(b"/ap".to_vec())).await.unwrap();
        client.send(Message::Stdin(b"p\n".to_vec())).await.unwrap();
        client.send(Message::Stdin(vec![])).await.unwrap();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        loop {
            match receive(&mut client).await {
                Message::Stdout(data) => stdout.extend(data),
                Message::Stderr(data) => stderr.extend(data),
                Message::Code(0) => break,
                other => panic!("unexpected message: {other:?}"),
            }
        }
        assert_eq!(stdout, b"/app");
        assert_eq!(stderr, b"/app");
        timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn stdin_eof_flushes_an_exact_pending_path_and_ignores_later_stdin() {
        let mut ctx = context(None);
        ctx.volumes = Volumes::new(vec!["/host:/app".parse().unwrap()]).unwrap();
        let (mut client, _, task) = connect(ctx);
        let mut request = start("value=$(cat); [ \"$value\" = \"$EXPECTED\" ]");
        request.env = vec![("EXPECTED".into(), "/host".into())];
        client.send(Message::Start(request)).await.unwrap();
        client.send(Message::Stdin(b"/app".to_vec())).await.unwrap();
        client.send(Message::Stdin(vec![])).await.unwrap();
        client
            .send(Message::Stdin(b"ignored".to_vec()))
            .await
            .unwrap();
        assert!(matches!(receive(&mut client).await, Message::Code(0)));
        timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
