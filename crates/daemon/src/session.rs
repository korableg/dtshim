use std::{os::unix::process::ExitStatusExt, process::Stdio, sync::Arc};

use anyhow::{Context, Result, bail};
use dtshim_core::{codec::PostcardCodec, model::Message, socket::Connection};
use futures::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Child,
};
use tokio_util::{
    codec::{FramedRead, FramedWrite},
    sync::CancellationToken,
};

use crate::context::Ctx;

pub(crate) async fn run(
    stream: Box<dyn Connection>,
    cancellation: CancellationToken,
    context: Arc<Ctx>,
) -> Result<()> {
    let volumes = &context.volumes;
    let mut child: Option<Child> = None;

    let result = {
        let connection = async {
            let (rd, wr) = tokio::io::split(stream);
            let mut rx = FramedRead::new(rd, PostcardCodec::new());
            let mut tx = FramedWrite::new(wr, PostcardCodec::new());

            if let Some(msg) = rx.next().await {
                match msg.context("failed to receive start message")? {
                    Message::Start(start) => {
                        let start = match context.prepare(start) {
                            Ok(start) => start,
                            Err(err) => {
                                let message =
                                    volumes.map_from_host(&format!("policy denied: {err:#}"));
                                tx.send(Message::Failed(message))
                                    .await
                                    .context("failed to send policy denial")?;
                                return Ok(());
                            }
                        };
                        eprintln!("start: {:?} {:?}", &start.command, &start.args);

                        child = match tokio::process::Command::new(&start.command)
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
                                let err_msg = volumes.map_from_host(&format!("{err:#}"));
                                let _ = tx.send(Message::Failed(err_msg)).await;
                                return Err(err);
                            }
                        };
                    }
                    _ => bail!("the first message should be a start"),
                }
            }

            let child = child.as_mut().context("child was not initialized")?;
            let mut stdin = child.stdin.take().context("failed to take child stdin")?;
            let mut stdout = child.stdout.take().context("failed to take child stdout")?;
            let mut stderr = child.stderr.take().context("failed to take child stderr")?;
            let mut out_buf = vec![0u8; 8192];
            let mut err_buf = vec![0u8; 8192];

            let input = async {
                let mut mapper = volumes.stream_to_host();
                while let Some(msg) = rx.next().await {
                    match msg.context("failed to receive client message")? {
                        Message::Stdin(data) if data.is_empty() => break,
                        Message::Stdin(data) => {
                            eprintln!("stdin: {:?}", String::from_utf8_lossy(&data));
                            let data = mapper.push(&data);
                            if let Err(err) = stdin.write_all(&data).await {
                                eprintln!("stdin error: {err}");
                                drop(stdin);
                                return anyhow::Ok(());
                            }
                        }
                        other => bail!("unexpected message from client: {other:?}"),
                    }
                }

                if let Err(err) = stdin.write_all(&mapper.finish()).await {
                    eprintln!("stdin error: {err}");
                }

                drop(stdin);
                eprintln!("stdin: closed");

                anyhow::Ok(())
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

            tokio::pin!(input, output);

            tokio::select! {
                result = &mut input => {
                    result?;
                    output.await
                }
                result = &mut output => result,
            }
        };

        tokio::select! {
            biased;

            _ = cancellation.cancelled() => {
                eprintln!("cancelled");
                Ok(())
            }
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
