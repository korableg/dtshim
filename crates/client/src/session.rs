use std::{io::ErrorKind, process::ExitCode};

use anyhow::{Context, Result, bail, ensure};
use dtshim_core::{
    codec::PostcardCodec,
    model::{Message, Start},
};
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::codec::{FramedRead, FramedWrite};

pub(crate) async fn run<S>(stream: S, start: Start) -> Result<ExitCode>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    run_with_io(
        stream,
        start,
        tokio::io::stdin(),
        tokio::io::stdout(),
        tokio::io::stderr(),
    )
    .await
}

async fn run_with_io<S, I, O, E>(
    stream: S,
    start: Start,
    mut stdin: I,
    mut stdout: O,
    mut stderr: E,
) -> Result<ExitCode>
where
    S: AsyncRead + AsyncWrite + Unpin,
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
{
    let (rd, wr) = tokio::io::split(stream);
    let mut rx = FramedRead::new(rd, PostcardCodec::new());
    let mut tx = FramedWrite::new(wr, PostcardCodec::new());

    tx.send(Message::Start(start))
        .await
        .context("failed to send start message")?;

    let input = async {
        let mut buf = vec![0u8; 8192];

        loop {
            let n = stdin.read(&mut buf).await.context("failed to read stdin")?;

            match tx.send(Message::Stdin(buf[..n].to_vec())).await {
                Ok(()) => {}
                Err(err)
                    if matches!(
                        err.kind(),
                        ErrorKind::BrokenPipe | ErrorKind::ConnectionReset
                    ) =>
                {
                    return anyhow::Ok(());
                }
                Err(err) => return Err(err).context("failed to send stdin"),
            }

            if n == 0 {
                return anyhow::Ok(());
            }
        }
    };

    let output = async {
        while let Some(msg) = rx.next().await {
            match msg.context("failed to receive server message")? {
                Message::Stdout(data) => {
                    stdout
                        .write_all(&data)
                        .await
                        .context("failed to write stdout")?;
                    stdout.flush().await.context("failed to flush stdout")?;
                }
                Message::Stderr(data) => {
                    stderr
                        .write_all(&data)
                        .await
                        .context("failed to write stderr")?;
                    stderr.flush().await.context("failed to flush stderr")?;
                }
                Message::Code(code) => {
                    let code = u8::try_from(code).context("invalid command exit code")?;
                    return Ok(ExitCode::from(code));
                }
                Message::Signaled(signal) => {
                    ensure!(
                        (1..128).contains(&signal),
                        "invalid termination signal: {signal}"
                    );
                    return Ok(ExitCode::from(128 + signal as u8));
                }
                Message::Failed(message) => bail!("command failed: {message}"),
                other => bail!("unexpected message from server: {other:?}"),
            }
        }

        bail!("server disconnected before sending command status")
    };

    tokio::pin!(input, output);

    tokio::select! {
        biased;

        result = &mut output => result,
        result = &mut input => {
            result?;
            output.await
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        pin::Pin,
        task::{Context as TaskContext, Poll},
        time::Duration,
    };

    use tokio::{
        io::{ReadBuf, duplex},
        time::timeout,
    };
    use tokio_util::codec::Framed;

    use super::*;

    fn start() -> Start {
        Start {
            token: Some("secret".into()),
            command: "tool".into(),
            args: vec!["argument".into()],
            env: vec![("NAME".into(), "value".into())],
            cwd: "/work".into(),
        }
    }

    async fn response(message: Option<Message>) -> Result<ExitCode> {
        let (client, server) = duplex(65536);
        let server = async {
            let mut server = Framed::new(server, PostcardCodec::new());
            assert!(matches!(
                server.next().await.unwrap().unwrap(),
                Message::Start(_)
            ));
            if let Some(message) = message {
                server.send(message).await.unwrap();
            }
        };
        timeout(Duration::from_secs(2), async {
            let (result, ()) = tokio::join!(
                run_with_io(
                    client,
                    start(),
                    tokio::io::empty(),
                    tokio::io::sink(),
                    tokio::io::sink()
                ),
                server
            );
            result
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn forwards_start_stdin_eof_and_both_output_streams() {
        let (client, server) = duplex(65536);
        let input = vec![0xff; 20000];
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let server = async {
            let mut server = Framed::new(server, PostcardCodec::new());
            let Message::Start(request) = server.next().await.unwrap().unwrap() else {
                panic!("expected start");
            };
            assert_eq!(request.token.as_deref(), Some("secret"));
            assert_eq!(request.command, "tool");
            assert_eq!(request.args, ["argument"]);
            assert_eq!(request.env, [("NAME".to_owned(), "value".to_owned())]);
            assert_eq!(request.cwd, "/work");
            let mut received = Vec::new();
            loop {
                match server.next().await.unwrap().unwrap() {
                    Message::Stdin(data) if data.is_empty() => break,
                    Message::Stdin(data) => received.extend(data),
                    other => panic!("unexpected message: {other:?}"),
                }
            }
            assert_eq!(received, input);
            for message in [
                Message::Stdout(b"first\0".to_vec()),
                Message::Stderr(vec![0xff, b'\n']),
                Message::Stdout(b"last".to_vec()),
                Message::Code(7),
            ] {
                server.send(message).await.unwrap();
            }
        };
        let result = timeout(Duration::from_secs(2), async {
            let (result, ()) = tokio::join!(
                run_with_io(client, start(), input.as_slice(), &mut stdout, &mut stderr),
                server
            );
            result
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result, ExitCode::from(7));
        assert_eq!(stdout, b"first\0last");
        assert_eq!(stderr, [0xff, b'\n']);
    }

    #[tokio::test]
    async fn exit_codes_and_signals_are_validated() {
        for code in [0, 1, 255] {
            assert_eq!(
                response(Some(Message::Code(code))).await.unwrap(),
                ExitCode::from(code as u8)
            );
        }
        for code in [-1, 256, i32::MAX] {
            assert_eq!(
                response(Some(Message::Code(code)))
                    .await
                    .unwrap_err()
                    .to_string(),
                "invalid command exit code"
            );
        }
        for signal in [1, 15, 127] {
            assert_eq!(
                response(Some(Message::Signaled(signal))).await.unwrap(),
                ExitCode::from(128 + signal as u8)
            );
        }
        for signal in [-1, 0, 128, i32::MAX] {
            assert_eq!(
                response(Some(Message::Signaled(signal)))
                    .await
                    .unwrap_err()
                    .to_string(),
                format!("invalid termination signal: {signal}")
            );
        }
    }

    #[tokio::test]
    async fn server_failures_unexpected_messages_and_early_disconnect_are_reported() {
        assert_eq!(
            response(Some(Message::Failed("authentication failed".into())))
                .await
                .unwrap_err()
                .to_string(),
            "command failed: authentication failed"
        );
        for message in [Message::Start(start()), Message::Stdin(vec![])] {
            assert!(
                response(Some(message))
                    .await
                    .unwrap_err()
                    .to_string()
                    .starts_with("unexpected message from server:")
            );
        }
        assert_eq!(
            response(None).await.unwrap_err().to_string(),
            "server disconnected before sending command status"
        );
    }

    #[tokio::test]
    async fn closed_connection_reports_failure_to_send_start() {
        let (client, server) = duplex(1024);
        drop(server);
        let error = run_with_io(
            client,
            start(),
            tokio::io::empty(),
            tokio::io::sink(),
            tokio::io::sink(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "failed to send start message");
    }

    #[tokio::test]
    async fn corrupt_server_frame_is_reported() {
        let (client, mut server) = duplex(1024);
        server.write_all(&[0, 0, 0, 1, 255]).await.unwrap();
        let error = timeout(
            Duration::from_secs(2),
            run_with_io(
                client,
                start(),
                tokio::io::empty(),
                tokio::io::sink(),
                tokio::io::sink(),
            ),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(error.to_string(), "failed to receive server message");
    }

    struct FailedReader;

    impl AsyncRead for FailedReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut TaskContext<'_>,
            _: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::other("read failed")))
        }
    }

    struct FailedWriter {
        flush: bool,
    }

    impl AsyncWrite for FailedWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut TaskContext<'_>,
            data: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(if self.flush {
                Ok(data.len())
            } else {
                Err(io::Error::other("write failed"))
            })
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::other("flush failed")))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn stdin_read_errors_are_reported() {
        let (client, _server) = duplex(1024);
        let error = timeout(
            Duration::from_secs(2),
            run_with_io(
                client,
                start(),
                FailedReader,
                tokio::io::sink(),
                tokio::io::sink(),
            ),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(error.to_string(), "failed to read stdin");
    }

    #[tokio::test]
    async fn output_write_and_flush_errors_identify_the_stream() {
        for (is_stderr, flush, expected) in [
            (false, false, "failed to write stdout"),
            (false, true, "failed to flush stdout"),
            (true, false, "failed to write stderr"),
            (true, true, "failed to flush stderr"),
        ] {
            let (client, server) = duplex(1024);
            let mut server = Framed::new(server, PostcardCodec::new());
            let message = if is_stderr {
                Message::Stderr(vec![42])
            } else {
                Message::Stdout(vec![42])
            };
            server.send(message).await.unwrap();
            let error = timeout(
                Duration::from_secs(2),
                run_with_io(
                    client,
                    start(),
                    tokio::io::empty(),
                    FailedWriter { flush },
                    FailedWriter { flush },
                ),
            )
            .await
            .unwrap()
            .unwrap_err();
            assert_eq!(error.to_string(), expected);
        }
    }

    struct StdinWriteFailure {
        kind: ErrorKind,
        writes: usize,
        status: &'static [u8],
        reader: Option<std::task::Waker>,
    }

    impl AsyncRead for StdinWriteFailure {
        fn poll_read(
            mut self: Pin<&mut Self>,
            context: &mut TaskContext<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if self.writes < 2 {
                self.reader = Some(context.waker().clone());
                return Poll::Pending;
            }
            let length = buffer.remaining().min(self.status.len());
            buffer.put_slice(&self.status[..length]);
            self.status = &self.status[length..];
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for StdinWriteFailure {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _: &mut TaskContext<'_>,
            data: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.writes += 1;
            if let Some(reader) = self.reader.take() {
                reader.wake();
            }
            Poll::Ready(if self.writes == 1 {
                Ok(data.len())
            } else {
                Err(io::Error::from(self.kind))
            })
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn broken_stdin_connections_still_allow_receiving_command_status() {
        for kind in [
            ErrorKind::BrokenPipe,
            ErrorKind::ConnectionReset,
            ErrorKind::Other,
        ] {
            let stream = StdinWriteFailure {
                kind,
                writes: 0,
                status: &[0, 0, 0, 2, 4, 0],
                reader: None,
            };
            let result = timeout(
                Duration::from_secs(2),
                run_with_io(
                    stream,
                    start(),
                    b"input".as_slice(),
                    tokio::io::sink(),
                    tokio::io::sink(),
                ),
            )
            .await
            .unwrap();
            if kind == ErrorKind::Other {
                assert_eq!(result.unwrap_err().to_string(), "failed to send stdin");
            } else {
                assert_eq!(result.unwrap(), ExitCode::SUCCESS);
            }
        }
    }

    #[tokio::test]
    async fn command_status_does_not_wait_for_stdin_to_finish() {
        let (client, server) = duplex(1024);
        let (stdin, _input_writer) = duplex(1024);
        let mut server = Framed::new(server, PostcardCodec::new());
        server.send(Message::Code(0)).await.unwrap();
        let result = timeout(
            Duration::from_secs(2),
            run_with_io(client, start(), stdin, tokio::io::sink(), tokio::io::sink()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result, ExitCode::SUCCESS);
    }
}
