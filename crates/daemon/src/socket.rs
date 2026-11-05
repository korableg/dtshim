use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use tempfile::TempDir;
use tokio::net::UnixListener;

pub(crate) struct BoundSocket {
    listener: UnixListener,
    path: PathBuf,
    _temp_dir: Option<TempDir>,
}

impl BoundSocket {
    pub(crate) fn bind(path: Option<PathBuf>) -> Result<Self> {
        let (path, temp_dir) = match path {
            Some(path) => (path, None),
            None => {
                let temp_dir = tempfile::Builder::new()
                    .prefix("dtshim_")
                    .permissions(std::fs::Permissions::from_mode(0o700))
                    .tempdir()
                    .context("failed to create socket directory")?;
                (temp_dir.path().join("dtshim.sock"), Some(temp_dir))
            }
        };
        let listener = UnixListener::bind(&path)
            .with_context(|| format!("failed to bind socket {}", path.display()))?;

        Ok(Self {
            listener,
            path,
            _temp_dir: temp_dir,
        })
    }

    pub(crate) fn listener(&self) -> &UnixListener {
        &self.listener
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for BoundSocket {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_file(&self.path) {
            eprintln!("failed to remove socket {}: {}", self.path.display(), err)
        }
    }
}
