use std::future::Future;
use std::sync::Arc;

use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::session;
use crate::socket::SocketListener;
use crate::volume::Volumes;

pub(crate) async fn run(
    listener: &dyn SocketListener,
    shutdown: impl Future<Output = ()>,
    volumes: Arc<Volumes>,
    host_env: Arc<Vec<String>>,
) {
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
                    let volumes = Arc::clone(&volumes);
                    let host_env = Arc::clone(&host_env);
                    sessions.spawn(async move {
                        if let Err(e) = session::run(connection, token, volumes, host_env).await {
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
