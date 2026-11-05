use anyhow::{Context, Result};
use tokio::signal::unix::{Signal, SignalKind, signal};

pub(crate) struct Shutdown {
    interrupt: Signal,
    terminate: Signal,
}

impl Shutdown {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())
                .context("failed to register SIGINT handler")?,
            terminate: signal(SignalKind::terminate())
                .context("failed to register SIGTERM handler")?,
        })
    }

    pub(crate) async fn wait(&mut self) {
        tokio::select! {
            _ = self.interrupt.recv() => {},
            _ = self.terminate.recv() => {},
        }
    }
}
