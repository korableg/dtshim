use std::future::Future;

use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::session;
use crate::socket::SocketListener;

pub(crate) async fn run(listener: &dyn SocketListener, shutdown: impl Future<Output = ()>) {
    let cancellation = CancellationToken::new();
    let mut sessions = JoinSet::new();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            biased;

            _ = &mut shutdown => break,
            Some(result) = sessions.join_next(), if !sessions.is_empty() => {
                if let Err(e) = result {
                    eprintln!("session task error: {e}");
                }
            }
            res = listener.accept() => match res {
                Ok(connection) => {
                    let token = cancellation.child_token();
                    sessions.spawn(async move {
                        if let Err(e) = session::run(connection, token).await {
                            eprintln!("connection error: {e:#}");
                        }
                    });
                }
                Err(e) => eprintln!("accept error: {e}"),
            },
        }
    }

    cancellation.cancel();

    while let Some(result) = sessions.join_next().await {
        if let Err(e) = result {
            eprintln!("session task error: {e}");
        }
    }
}
