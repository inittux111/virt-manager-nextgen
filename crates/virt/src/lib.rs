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
mod cpu;
mod devices;
mod domain;
mod error;
mod firmware;
mod guest;
mod host;
mod manage;
mod memory;
mod networks;
mod new_vm;
mod remote;
mod screenshot;
mod snapshot;
mod stats;
mod storage;
pub mod units;
mod video;

pub use config::{DiskConfig, DomainConfig, Firmware, GraphicsConfig, InterfaceConfig, set_os_id};
pub use connection::{Connection, ConnectionKind, GuestDetails, HostInfo, PoolSummary};
pub use console::ConsoleStream;
pub use cpu::{CpuMode, SECURITY_FEATURES, Topology, mitigations_on, set_cpu_mode, set_mitigations, set_topology};
pub use devices::{DeviceKind, DeviceRef, disk_prefix, list_devices, next_disk_target, normalize_mac, replace_device};
pub use domain::{DomainState, DomainSummary, PowerAction};
pub use error::{Error, Result};
pub use firmware::{FirmwareChoice, loader_is_secure, set_firmware};
pub use guest::{GuestFilesystem, GuestInfo};
pub use host::{DomainCaps, GuestArch, HostCaps, InstallImage, NetworkSummary, VolumeOptions, volume_xml};
pub use manage::{CloneDiskMode, unique_file_name, unique_name, validate_vm_name};
pub use memory::{MemorySize, set_memory};
pub use networks::{
    Ipv4Config, NetworkDetails, NetworkMode, NetworkSettings, apply_network_settings, check_network_xml, suggest_subnet,
};
pub use new_vm::{
    BOOT_POOL, Created, GuestFamily, InstallSource, NetworkChoice, NetworkInstall, NewDisk, NewVm, build_domain_xml,
};
pub use remote::{SshTarget, display_address};
pub use screenshot::Screenshot;
pub use snapshot::{
    SnapshotInfo, SnapshotLocation, SnapshotModes, SnapshotOptions, validate_snapshot_name, validate_snapshot_name_for,
};
pub use stats::{BlockStats, DomainStats, HostStats, NetStats, StatsWanted};
pub use storage::{NewPool, PoolDetails, PoolKind, VolumeInfo, pool_xml};
pub use video::{set_video_3d, video_3d};

/// Silences libvirt's default behaviour of printing every error to stderr.
/// Errors are still reported through [`Error`].
pub fn init() {
    virt::error::clear_error_callback();
}
