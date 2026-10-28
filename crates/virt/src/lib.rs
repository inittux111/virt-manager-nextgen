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
mod console;
mod devices;
mod domain;
mod error;
mod guest;
mod host;
mod manage;
mod networks;
mod new_vm;
mod snapshot;
mod stats;
mod storage;
pub mod units;

pub use config::{DiskConfig, DomainConfig, Firmware, GraphicsConfig, InterfaceConfig};
pub use connection::{Connection, ConnectionKind, GuestDetails, HostInfo, PoolSummary};
pub use console::ConsoleStream;
pub use devices::{DeviceKind, DeviceRef, disk_prefix, list_devices, next_disk_target, replace_device};
pub use domain::{DomainState, DomainSummary, PowerAction};
pub use error::{Error, Result};
pub use guest::{GuestFilesystem, GuestInfo};
pub use host::{DomainCaps, GuestArch, HostCaps, InstallImage, NetworkSummary};
pub use manage::{CloneDiskMode, unique_file_name, unique_name, validate_vm_name};
pub use networks::{Ipv4Config, NetworkDetails, NetworkMode, NetworkSettings, apply_network_settings, suggest_subnet};
pub use new_vm::{Created, GuestFamily, InstallSource, NetworkChoice, NewDisk, NewVm, build_domain_xml};
pub use snapshot::{SnapshotInfo, SnapshotLocation, validate_snapshot_name};
pub use stats::{BlockStats, DomainStats, HostStats, NetStats, StatsWanted};
pub use storage::{NewPool, PoolDetails, PoolKind, VolumeInfo, pool_xml};

/// Silences libvirt's default behaviour of printing every error to stderr.
/// Errors are still reported through [`Error`].
pub fn init() {
    virt::error::clear_error_callback();
}
