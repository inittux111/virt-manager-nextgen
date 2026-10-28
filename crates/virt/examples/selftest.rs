//! End-to-end check of snapshot, clone and delete against a real libvirt.
//!
//! Creates a throwaway VM with a 16 MiB disk on `qemu:///session` (never the
//! system connection), exercises the operations and removes everything again.
//!
//! The disk lives in a transient storage pool in a temporary directory.
//!
//! Usage: `selftest <empty-directory>`

use virt::connect::Connect;
use virt::domain::Domain;
use virt::storage_pool::StoragePool;
use virt::storage_vol::StorageVol;
use vmng_virt::{CloneDiskMode, Connection, ConnectionKind};

const NAME: &str = "vmng-selftest";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    vmng_virt::init();
    let raw = Connect::open(Some("qemu:///session"))?;
    let dir = std::env::args().nth(1).ok_or("usage: selftest <empty-directory>")?;
    let pool = StoragePool::create_xml(
        &raw,
        &format!("<pool type='dir'><name>{NAME}-pool</name><target><path>{dir}</path></target></pool>"),
        0,
    )?;
    let vol = StorageVol::create_xml(
        &pool,
        &format!(
            "<volume><name>{NAME}.qcow2</name><capacity unit='MiB'>16</capacity><target><format type='qcow2'/></target></volume>"
        ),
        0,
    )?;
    let path = vol.get_path()?;
    let dom = Domain::define_xml(
        &raw,
        &format!(
            "<domain type='kvm'><name>{NAME}</name><memory unit='MiB'>128</memory><vcpu>1</vcpu>\
             <os><type arch='x86_64' machine='q35'>hvm</type></os><devices>\
             <disk type='file' device='disk'><driver name='qemu' type='qcow2'/><source file='{path}'/><target dev='vda' bus='virtio'/></disk>\
             <interface type='user'><mac address='52:54:00:12:34:56'/><model type='virtio'/></interface>\
             </devices></domain>"
        ),
    )?;
    let uuid = dom.get_uuid_string()?;
    let conn = Connection::open(ConnectionKind::Session)?;

    let run = || -> Result<(), Box<dyn std::error::Error>> {
        conn.create_snapshot(&uuid, "first", "made by selftest")?;
        conn.set_snapshot_description(&uuid, "first", "edited")?;
        let snaps = conn.list_snapshots(&uuid)?;
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].description.as_deref(), Some("edited"));
        assert!(snaps[0].is_current);
        println!("snapshot ok: {:?}", snaps[0]);
        conn.revert_snapshot(&uuid, "first")?;
        conn.delete_snapshot(&uuid, "first")?;
        assert!(conn.list_snapshots(&uuid)?.is_empty());

        // A running machine is captured with its memory.
        conn.power(&uuid, vmng_virt::PowerAction::Start)?;
        conn.create_snapshot(&uuid, "live", "")?;
        let live = conn.list_snapshots(&uuid)?;
        assert!(live[0].has_memory(), "{live:?}");
        conn.revert_snapshot(&uuid, "live")?;
        conn.power(&uuid, vmng_virt::PowerAction::ForceOff)?;
        conn.delete_snapshot(&uuid, "live")?;
        println!("live snapshot ok");

        let clone_uuid = conn.clone_domain(
            &uuid,
            &format!("{NAME}-clone"),
            &[("vda".into(), CloneDiskMode::Copy)],
            &|done, total| println!("clone progress {done}/{total}"),
        )?;
        let clone = conn.list_domains()?.into_iter().find(|d| d.uuid == clone_uuid).expect("clone listed");
        let clone_disk = clone.config.disks[0].source_path.clone().expect("clone disk path");
        assert_ne!(clone_disk, path);
        assert_ne!(clone.config.interfaces[0].mac.as_deref(), Some("52:54:00:12:34:56"));
        println!("clone ok: {} -> {clone_disk}", clone.config.name);

        conn.rename_domain(&clone_uuid, &format!("{NAME}-renamed"))?;
        let failed = conn.delete_domain(&clone_uuid, std::slice::from_ref(&clone_disk))?;
        assert!(failed.is_empty(), "{failed:?}");
        assert!(StorageVol::lookup_by_path(&raw, &clone_disk).is_err());
        println!("delete ok");
        Ok(())
    };
    let result = run();

    // Clean up whatever is left.
    for name in [format!("{NAME}-clone"), format!("{NAME}-renamed")] {
        if let Ok(d) = Domain::lookup_by_name(&raw, &name) {
            let _ = d.undefine_flags(2);
        }
    }
    let leftover = conn.delete_domain(&uuid, &[path]);
    println!("cleanup: {leftover:?}");
    let _ = pool.destroy();
    result
}
