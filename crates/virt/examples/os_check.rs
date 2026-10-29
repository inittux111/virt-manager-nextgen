//! Defines a throwaway VM (`vmng-test-os`) marked as one operating system,
//! changes it to another (as Hardware → Boot & firmware does), checks
//! libvirt kept it, and deletes the VM.
//!
//! Usage: `os_check` (`VMNG_TEST_URI` picks the connection; default
//! `qemu:///session`).

use vmng_virt::{
    Connection, ConnectionKind, FirmwareChoice, GuestFamily, InstallSource, NetworkChoice, NewVm, set_os_id,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    vmng_virt::init();
    let uri = std::env::var("VMNG_TEST_URI").unwrap_or_else(|_| "qemu:///session".into());
    let conn = Connection::open(ConnectionKind::from_uri(&uri))?;
    let vm = NewVm {
        name: "vmng-test-os".into(),
        os_id: Some("http://fedoraproject.org/fedora/42".into()),
        family: GuestFamily::Linux,
        arch: "x86_64".into(),
        machine: Some("q35".into()),
        virt_type: "kvm".into(),
        memory: 256 << 20,
        vcpus: 1,
        firmware: FirmwareChoice::Bios,
        tpm: false,
        cpu_model: "host-passthrough".into(),
        install: InstallSource::Manual,
        disk: None,
        existing_disk: None,
        network: NetworkChoice::None,
        graphics: "vnc".into(),
        virtio: true,
        video: "virtio".into(),
    };
    let created = conn.create_vm(&vm, false)?;
    let uuid = created.uuid.clone();
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let wanted = "http://debian.org/debian/13";
        let mut xml = conn.domain_config_xml(&uuid)?;
        set_os_id(&mut xml, wanted);
        conn.save_domain_config(&uuid, &xml)?;
        let got = conn.list_domains()?.into_iter().find(|d| d.uuid == uuid).and_then(|d| d.config.os_id);
        if got.as_deref() != Some(wanted) {
            return Err(format!("operating system after the change: {got:?}").into());
        }
        println!("ok  changed fedora42 -> {wanted}");
        Ok(())
    })();
    println!("cleanup: {:?}", conn.delete_domain(&uuid, &[]));
    result
}
