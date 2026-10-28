//! Starts a throwaway VM (`vmng-test-firmware`, no disk) with each kind of
//! firmware choice, on `qemu:///session`, then switches an existing VM from
//! one firmware file to another; everything is removed again.
//!
//! Usage: `firmware_check` (uses the firmware files libvirt reports).

use vmng_virt::{
    Connection, ConnectionKind, FirmwareChoice, GuestFamily, InstallSource, NetworkChoice, NewVm, PowerAction,
};

fn vm(firmware: FirmwareChoice) -> NewVm {
    NewVm {
        name: "vmng-test-firmware".into(),
        os_id: None,
        family: GuestFamily::Linux,
        arch: "x86_64".into(),
        machine: Some("q35".into()),
        virt_type: "kvm".into(),
        memory: 256 << 20,
        vcpus: 1,
        firmware,
        tpm: false,
        cpu_model: "host-passthrough".into(),
        install: InstallSource::Manual,
        disk: None,
        network: NetworkChoice::None,
        graphics: "vnc".into(),
        virtio: true,
        video: "virtio".into(),
    }
}

/// Creates the VM, checks it runs, and (unless `keep`) deletes it.
fn run(conn: &Connection, fw: FirmwareChoice, keep: bool) -> Result<String, Box<dyn std::error::Error>> {
    let created = conn.create_vm(&vm(fw.clone()), true)?;
    let check = (|| -> Result<(), Box<dyn std::error::Error>> {
        if let Some(e) = &created.start_error {
            return Err(format!("start failed: {e}").into());
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
        let d = conn.list_domains()?.into_iter().find(|d| d.uuid == created.uuid).ok_or("not listed")?;
        if !d.state.is_active() {
            return Err("not running".into());
        }
        let xml = conn.domain_config_xml(&created.uuid)?;
        println!(
            "ok  {:<70} loader={:?} nvram={:?}",
            fw.label("x86_64"),
            xml.find_text("os/loader"),
            xml.find_text("os/nvram")
        );
        conn.power(&created.uuid, PowerAction::ForceOff)?;
        Ok(())
    })();
    if !keep || check.is_err() {
        conn.delete_domain(&created.uuid, &[])?;
    }
    check.map(|()| created.uuid)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    vmng_virt::init();
    // VMNG_TEST_URI picks another libvirt (the test pipeline's Debian
    // container); by default the user session.
    let uri = std::env::var("VMNG_TEST_URI").unwrap_or_else(|_| "qemu:///session".into());
    let conn = Connection::open(ConnectionKind::from_uri(&uri))?;
    let dc = conn.domain_caps("x86_64", Some("q35"), "kvm")?;
    println!("firmware files: {:#?}", dc.loaders);
    let mut failed = 0;
    let mut choices = vec![FirmwareChoice::Bios, FirmwareChoice::Uefi { secure_boot: true }];
    // The ones a normal VM uses (not the confidential-computing builds).
    choices.extend(dc.loaders.iter().filter(|p| p.contains("OVMF_CODE")).map(|p| FirmwareChoice::Loader(p.clone())));
    for fw in choices {
        if let Err(e) = run(&conn, fw.clone(), false) {
            failed += 1;
            println!("ERR {}: {e}", fw.label("x86_64"));
        }
    }

    // Switching an existing VM to another firmware file, as the Hardware
    // tab does, must give it a fitting variable store.
    let files: Vec<&String> = dc.loaders.iter().filter(|p| p.contains("OVMF_CODE")).collect();
    if let [first, .., last] = files.as_slice() {
        let uuid = run(&conn, FirmwareChoice::Loader((*first).clone()), true)?;
        let mut xml = conn.domain_config_xml(&uuid)?;
        vmng_virt::set_firmware(&mut xml, &FirmwareChoice::Loader((*last).clone()));
        let switched = conn.save_domain_config(&uuid, &xml).and_then(|()| conn.power(&uuid, PowerAction::Start));
        match switched {
            Ok(()) => {
                let xml = conn.domain_config_xml(&uuid)?;
                println!("ok  switched to {last}: nvram={:?}", xml.find_text("os/nvram"));
                conn.power(&uuid, PowerAction::ForceOff)?;
            }
            Err(e) => {
                failed += 1;
                println!("ERR switching to {last}: {e}");
            }
        }
        conn.delete_domain(&uuid, &[])?;
    }
    if failed > 0 {
        return Err(format!("{failed} firmware check(s) failed").into());
    }
    Ok(())
}
