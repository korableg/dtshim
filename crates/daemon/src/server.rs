use std::future::Future;
use std::sync::Arc;

use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::context::Ctx;
use crate::session;
use crate::socket::SocketListener;

const MAX_SESSIONS: usize = 128;

pub(crate) async fn run<L: SocketListener>(
    listener: &L,
    shutdown: impl Future<Output = ()>,
    context: Arc<Ctx>,
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
            res = listener.accept(), if sessions.len() < MAX_SESSIONS => match res {
                Ok(connection) => {
                    let token = cancellation.child_token();
                    let context = Arc::clone(&context);
                    sessions.spawn(async move {
                        if let Err(e) = session::run(connection, token, context).await {
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

#[cfg(test)]
mod tests {
    use std::{
        io,
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    use anyhow::Result;
    use tokio::{
        io::{DuplexStream, duplex},
        sync::{Mutex, Notify, mpsc},
        time::timeout,
    };

    use crate::{policy::Policy, volume::Volumes};

    use super::*;

    struct Listener {
        incoming: Mutex<mpsc::Receiver<DuplexStream>>,
        accepted: AtomicUsize,
        changed: Notify,
    }

    impl SocketListener for Listener {
        type Stream = DuplexStream;

        fn address(&self) -> Result<String> {
            Ok("test".into())
        }

        async fn accept(&self) -> io::Result<Self::Stream> {
            let stream = self.incoming.lock().await.recv().await.unwrap();
            self.accepted.fetch_add(1, Ordering::SeqCst);
            self.changed.notify_one();
            Ok(stream)
        }
    }

    #[tokio::test]
    async fn accepts_at_most_128_sessions_and_reuses_slots_after_disconnect() {
        let (sender, receiver) = mpsc::channel(MAX_SESSIONS + 1);
        let listener = Arc::new(Listener {
            incoming: Mutex::new(receiver),
            accepted: AtomicUsize::new(0),
            changed: Notify::new(),
        });
        let mut clients = Vec::new();
        for _ in 0..=MAX_SESSIONS {
            let (client, server) = duplex(1024);
            clients.push(client);
            sender.send(server).await.unwrap();
        }
        let cancellation = CancellationToken::new();
        let context = Arc::new(Ctx {
            token: None,
            policy: Policy::default(),
            volumes: Volumes::default(),
        });
        let server = run(listener.as_ref(), cancellation.cancelled(), context);
        let exercise = async {
            while listener.accepted.load(Ordering::SeqCst) < MAX_SESSIONS {
                listener.changed.notified().await;
            }
            tokio::task::yield_now().await;
            assert_eq!(listener.accepted.load(Ordering::SeqCst), MAX_SESSIONS);
            drop(clients.remove(0));
            while listener.accepted.load(Ordering::SeqCst) <= MAX_SESSIONS {
                listener.changed.notified().await;
            }
            cancellation.cancel();
        };
        timeout(Duration::from_secs(5), async {
            tokio::join!(server, exercise);
        })
        .await
        .unwrap();
    }

    struct FailingListener {
        listener: Listener,
        attempts: AtomicUsize,
    }

    impl SocketListener for FailingListener {
        type Stream = DuplexStream;

        fn address(&self) -> Result<String> {
            self.listener.address()
        }

        async fn accept(&self) -> io::Result<Self::Stream> {
            if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(io::Error::other("transient accept failure"));
            }
            self.listener.accept().await
        }
    }

    #[tokio::test]
    async fn accept_errors_are_retried_and_shutdown_closes_active_connections() {
        use tokio::io::AsyncReadExt;

        let (sender, receiver) = mpsc::channel(1);
        let listener = FailingListener {
            listener: Listener {
                incoming: Mutex::new(receiver),
                accepted: AtomicUsize::new(0),
                changed: Notify::new(),
            },
            attempts: AtomicUsize::new(0),
        };
        let (mut client, connection) = duplex(1024);
        sender.send(connection).await.unwrap();
        let cancellation = CancellationToken::new();
        let context = Arc::new(Ctx {
            token: None,
            policy: Policy::default(),
            volumes: Volumes::default(),
        });
        let exercise = async {
            while listener.listener.accepted.load(Ordering::SeqCst) == 0 {
                listener.listener.changed.notified().await;
            }
            cancellation.cancel();
            assert_eq!(client.read(&mut [0; 1]).await.unwrap(), 0);
        };
        timeout(Duration::from_secs(5), async {
            tokio::join!(run(&listener, cancellation.cancelled(), context), exercise);
        })
        .await
        .unwrap();
        assert!(listener.attempts.load(Ordering::SeqCst) >= 2);
        assert_eq!(listener.listener.accepted.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn already_requested_shutdown_takes_priority_over_accepting_connections() {
        let (sender, receiver) = mpsc::channel(1);
        let listener = Listener {
            incoming: Mutex::new(receiver),
            accepted: AtomicUsize::new(0),
            changed: Notify::new(),
        };
        let (_client, connection) = duplex(1024);
        sender.send(connection).await.unwrap();
        let context = Arc::new(Ctx {
            token: None,
            policy: Policy::default(),
            volumes: Volumes::default(),
        });
        timeout(
            Duration::from_secs(5),
            run(&listener, std::future::ready(()), context),
        )
        .await
        .unwrap();
        assert_eq!(listener.accepted.load(Ordering::SeqCst), 0);
    }
}
