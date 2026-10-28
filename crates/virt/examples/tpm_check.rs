//! Creates a throwaway Windows-11-style VM (`vmng-test-tpm`: UEFI with
//! Secure Boot, TPM 2.0, no disk) on `qemu:///session`, starts it from the
//! given installer, shows the TPM the running VM has, waits, and deletes it.
//!
//! Usage: `tpm_check <installer.iso> [seconds to keep it running]`

use vmng_virt::{
    Connection, ConnectionKind, FirmwareChoice, GuestFamily, InstallSource, NetworkChoice, NewVm, PowerAction,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    vmng_virt::init();
    let mut args = std::env::args().skip(1);
    let iso = args.next().ok_or("usage: tpm_check <installer.iso> [seconds]")?;
    let wait: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(3);
    // VMNG_TEST_URI picks another libvirt (the test pipeline's Debian
    // container); by default the user session.
    let uri = std::env::var("VMNG_TEST_URI").unwrap_or_else(|_| "qemu:///session".into());
    let conn = Connection::open(ConnectionKind::from_uri(&uri))?;
    let vm = NewVm {
        name: "vmng-test-tpm".into(),
        os_id: Some("http://microsoft.com/win/11".into()),
        family: GuestFamily::Windows,
        arch: "x86_64".into(),
        machine: Some("q35".into()),
        virt_type: "kvm".into(),
        memory: 4 << 30,
        vcpus: 2,
        firmware: FirmwareChoice::Uefi { secure_boot: true },
        tpm: true,
        cpu_model: "host-passthrough".into(),
        install: InstallSource::Media(iso),
        disk: None,
        network: NetworkChoice::None,
        graphics: "spice".into(),
        virtio: false,
        video: "virtio".into(),
    };
    let created = conn.create_vm(&vm, true)?;
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        if let Some(e) = &created.start_error {
            return Err(format!("start failed: {e}").into());
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
        let live = vmng_xml::parse(&conn.domain_xml(&created.uuid, false)?)?;
        let tpm = live.find("devices/tpm").ok_or("the running VM has no TPM")?;
        println!("running TPM: {}", tpm.to_xml());
        println!("firmware: {:?}", live.find_text("os/loader"));
        std::thread::sleep(std::time::Duration::from_secs(wait));
        conn.power(&created.uuid, PowerAction::ForceOff)?;
        Ok(())
    })();
    println!("cleanup: {:?}", conn.delete_domain(&created.uuid, &[]));
    result
}
