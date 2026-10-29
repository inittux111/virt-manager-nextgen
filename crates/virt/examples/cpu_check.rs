//! Starts a throwaway VM (`vmng-test-cpu`, no disk) on `qemu:///session`
//! with each kind of CPU choice (host CPU, host model, a named model with
//! the security mitigations, QEMU default) and a manual CPU layout, and
//! checks the running machine got them; everything is removed again.
//!
//! Usage: `cpu_check`

use vmng_virt::{
    Connection, ConnectionKind, CpuMode, FirmwareChoice, GuestFamily, InstallSource, NetworkChoice, NewVm, PowerAction,
    Topology, set_cpu_mode, set_mitigations, set_topology,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    vmng_virt::init();
    // VMNG_TEST_URI picks another libvirt (the test pipeline's Debian
    // container); by default the user session.
    let uri = std::env::var("VMNG_TEST_URI").unwrap_or_else(|_| "qemu:///session".into());
    let conn = Connection::open(ConnectionKind::from_uri(&uri))?;
    let caps = conn.domain_caps("x86_64", Some("q35"), "kvm")?;
    println!("security mitigations: {:?}", caps.cpu_security_features);
    // A model the host runs; qemu64 is always there.
    let model = ["EPYC", "Skylake-Client", "qemu64"]
        .into_iter()
        .find(|m| caps.cpu_models.iter().any(|c| c == m))
        .ok_or("no usable named CPU model")?;
    let vm = NewVm {
        name: "vmng-test-cpu".into(),
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
    let mut failed = 0;
    let layout = Topology { sockets: 1, dies: 1, cores: 2, threads: 2 };
    for mode in [CpuMode::HostPassthrough, CpuMode::HostModel, CpuMode::Model(model.into()), CpuMode::Default] {
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let mut xml = conn.domain_config_xml(&uuid)?;
            set_cpu_mode(&mut xml, &mode);
            set_mitigations(&mut xml, &caps.cpu_security_features, true);
            set_topology(&mut xml, Some(layout));
            conn.save_domain_config(&uuid, &xml)?;
            conn.power(&uuid, PowerAction::Start)?;
            std::thread::sleep(std::time::Duration::from_secs(1));
            let live = vmng_xml::parse(&conn.domain_xml(&uuid, false)?)?;
            let got = Topology::of(&live).ok_or("the running machine has no layout")?;
            if got.total() != 4 || live.find_text("vcpu").as_deref() != Some("4") {
                return Err(format!("layout {got:?}, vcpus {:?}", live.find_text("vcpu")).into());
            }
            let cpu = live.find("cpu").map(|c| c.to_xml()).unwrap_or_default();
            println!("ok  {mode:?}: {}", cpu.split_whitespace().collect::<Vec<_>>().join(" "));
            conn.power(&uuid, PowerAction::ForceOff)?;
            std::thread::sleep(std::time::Duration::from_millis(500));
            Ok(())
        })();
        if let Err(e) = result {
            failed += 1;
            println!("ERR {mode:?}: {e}");
            let _ = conn.power(&uuid, PowerAction::ForceOff);
        }
    }
    println!("cleanup: {:?}", conn.delete_domain(&uuid, &[]));
    if failed > 0 {
        return Err(format!("{failed} CPU check(s) failed").into());
    }
    Ok(())
}
