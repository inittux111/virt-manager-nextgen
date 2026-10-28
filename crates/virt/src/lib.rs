//! The libvirt access layer.
//!
//! Everything the application knows about hypervisors goes through this
//! crate. It turns libvirt handles and XML into plain, typed Rust values
//! that are cheap to send between threads, so the UI never touches libvirt
//! directly.
//!
//! All calls here are **blocking** (libvirt talks to a daemon over a
//! socket). The UI runs them on worker threads.

mod config;
mod connection;
mod domain;
mod error;
mod stats;
pub mod units;

pub use config::{DiskConfig, DomainConfig, Firmware, GraphicsConfig, InterfaceConfig};
pub use connection::{Connection, ConnectionKind, GuestDetails, HostInfo, PoolSummary};
pub use domain::{DomainState, DomainSummary, PowerAction};
pub use error::{Error, Result};
pub use stats::{BlockStats, DomainStats, HostStats, NetStats};

/// Silences libvirt's default behaviour of printing every error to stderr.
/// Errors are still reported through [`Error`].
pub fn init() {
    virt::error::clear_error_callback();
}
