//! Bridges blocking libvirt calls to the GTK main loop.
//!
//! Every libvirt call runs on GLib's worker thread pool; the UI awaits the
//! result without ever blocking the main loop.

mod monitor;

pub use monitor::{HISTORY_LEN, HostLive, Monitor, Sample, VmLive};

use std::sync::Arc;

use gtk::gio;
use vmng_virt::{Connection, ConnectionKind, Error, Result};

/// A shareable handle to an open connection.
#[derive(Clone, Debug)]
pub struct Conn(Arc<Connection>);

impl Conn {
    /// Opens a connection without blocking the UI.
    pub async fn open(kind: ConnectionKind) -> Result<Self> {
        run(move || Connection::open(kind)).await.and_then(|r| r).map(|c| Self(Arc::new(c)))
    }

    pub fn kind(&self) -> &ConnectionKind {
        self.0.kind()
    }

    /// Runs `f` with the connection on a worker thread.
    pub async fn call<R, F>(&self, f: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&Connection) -> Result<R> + Send + 'static,
    {
        let conn = self.0.clone();
        run(move || f(&conn)).await.and_then(|r| r)
    }
}

/// Runs a blocking closure on the GLib thread pool.
async fn run<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> Result<R> {
    gio::spawn_blocking(f)
        .await
        .map_err(|_| Error::Invalid("An internal error occurred while talking to libvirt.".into()))
}
