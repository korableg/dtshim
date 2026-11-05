use std::{io::ErrorKind, process::ExitCode};

use anyhow::{Context, Result, bail, ensure};
use dtshim_core::{codec::PostcardCodec, model::Message};
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::codec::{FramedRead, FramedWrite};

use crate::request::Request;

pub(crate) async fn run(request: Request) -> Result<ExitCode> {
    let Request { socket, start } = request;
    let stream = socket.connect().await?;

    let (rd, wr) = tokio::io::split(stream);
    let mut rx = FramedRead::new(rd, PostcardCodec::new());
    let mut tx = FramedWrite::new(wr, PostcardCodec::new());

    tx.send(Message::Start(start))
        .await
        .context("failed to send start message")?;

    let input = async {
        let mut stdin = tokio::io::stdin();
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
        let mut stdout = tokio::io::stdout();
        let mut stderr = tokio::io::stderr();

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
