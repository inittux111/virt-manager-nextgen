//! Creates, starts and deletes a throwaway VM named `vmng-test-newvm`.
//!
//! Usage: `newvm_check <uri> <pool> <installer.iso>`. The VM has no network
//! card and a 1 GiB disk; everything is removed again at the end.

use vmng_virt::{
    Connection, ConnectionKind, FirmwareChoice, GuestFamily, InstallSource, NetworkChoice, NewDisk, NewVm, PowerAction,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    vmng_virt::init();
    let mut args = std::env::args().skip(1);
    let (uri, pool, iso) = match (args.next(), args.next(), args.next()) {
        (Some(u), Some(p), Some(i)) => (u, p, i),
        _ => return Err("usage: newvm_check <uri> <pool> <installer.iso>".into()),
    };
    let conn = Connection::open(ConnectionKind::from_uri(&uri))?;
    let caps = conn.host_caps()?;
    let x86 = caps.guest("x86_64").ok_or("no x86_64 guests")?;
    println!("host {} kvm={} machines: {:?}", caps.host_arch, x86.kvm, &x86.machines[..x86.machines.len().min(6)]);
    let dc = conn.domain_caps("x86_64", Some("q35"), "kvm")?;
    println!("domcaps: {dc:?}");
    println!("networks: {:?}", conn.list_networks()?);
    println!("bridges: {:?}", conn.list_bridges());
    println!("images: {:?}", conn.list_install_images()?.iter().map(|i| &i.name).collect::<Vec<_>>());

    let vm = NewVm {
        name: "vmng-test-newvm".into(),
        os_id: Some("http://fedoraproject.org/fedora/unknown".into()),
        family: GuestFamily::Linux,
        arch: "x86_64".into(),
        machine: Some("q35".into()),
        virt_type: "kvm".into(),
        memory: 1 << 30,
        vcpus: 1,
        firmware: FirmwareChoice::Uefi { secure_boot: dc.secure_boot },
        tpm: false,
        cpu_model: "host-passthrough".into(),
        install: InstallSource::Media(iso),
        disk: Some(NewDisk { pool, size: 1 << 30, format: "qcow2".into() }),
        existing_disk: None,
        network: NetworkChoice::None,
        graphics: "spice".into(),
        virtio: true,
        video: "virtio".into(),
    };
    let created = conn.create_vm(&vm, true)?;
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        if let Some(e) = &created.start_error {
            return Err(format!("start failed: {e}").into());
        }
        std::thread::sleep(std::time::Duration::from_secs(3));
        let d = conn.list_domains()?.into_iter().find(|d| d.uuid == created.uuid).ok_or("not listed")?;
        println!(
            "created {} state={:?} disks={:?}",
            d.config.name,
            d.state,
            d.config.disks.iter().map(|x| (&x.target, &x.source_path)).collect::<Vec<_>>()
        );
        if !d.state.is_active() {
            return Err("not running".into());
        }
        conn.power(&created.uuid, PowerAction::ForceOff)?;
        Ok(())
    })();
    let disk: Vec<String> = conn
        .list_domains()?
        .into_iter()
        .find(|d| d.uuid == created.uuid)
        .map(|d| d.config.disks.iter().filter(|x| !x.is_cdrom()).filter_map(|x| x.source_path.clone()).collect())
        .unwrap_or_default();
    println!("cleanup: {:?}", conn.delete_domain(&created.uuid, &disk)?);
    result
}
