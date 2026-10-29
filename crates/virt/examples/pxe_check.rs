//! Creates a throwaway VM (`vmng-test-pxe`, no disk) that boots from the
//! network (PXE), as the New VM wizard's Manual install with "Boot from the
//! network" does, starts it, checks the network card is its boot device,
//! and deletes it.
//!
//! Usage: `pxe_check` (`VMNG_TEST_URI` picks the connection; default
//! `qemu:///session`).

use vmng_virt::{
    Connection, ConnectionKind, FirmwareChoice, GuestFamily, InstallSource, NetworkChoice, NewVm, PowerAction,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    vmng_virt::init();
    let uri = std::env::var("VMNG_TEST_URI").unwrap_or_else(|_| "qemu:///session".into());
    let conn = Connection::open(ConnectionKind::from_uri(&uri))?;
    // User-mode networking works on any connection, without a virtual
    // network having to be active.
    let network = NetworkChoice::User;
    let vm = NewVm {
        name: "vmng-test-pxe".into(),
        os_id: None,
        family: GuestFamily::Linux,
        arch: "x86_64".into(),
        machine: Some("q35".into()),
        virt_type: "kvm".into(),
        memory: 256 << 20,
        vcpus: 1,
        firmware: FirmwareChoice::Bios,
        tpm: false,
        cpu_model: "host-passthrough".into(),
        install: InstallSource::NetworkBoot,
        disk: None,
        network,
        graphics: "vnc".into(),
        virtio: true,
        video: "virtio".into(),
    };
    let created = conn.create_vm(&vm, true)?;
    let uuid = created.uuid.clone();
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        if let Some(e) = &created.start_error {
            return Err(format!("start failed: {e}").into());
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
        let live = vmng_xml::parse(&conn.domain_xml(&uuid, false)?)?;
        let order = live.find_attr("devices/interface/boot", "order");
        if order != Some("1") {
            return Err(format!("network card boot order: {order:?}").into());
        }
        println!("ok  boots from the network card");
        conn.power(&uuid, PowerAction::ForceOff)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = conn.power(&uuid, PowerAction::ForceOff);
    }
    println!("cleanup: {:?}", conn.delete_domain(&uuid, &[]));
    result
}
