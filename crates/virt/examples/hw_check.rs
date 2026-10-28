//! Exercises device editing on a throwaway VM named `vmng-test-hw`.
//!
//! Usage: `hw_check <uri> <pool> <iso>`. Everything is removed at the end.

use vmng_virt::{
    Connection, ConnectionKind, DeviceKind, FirmwareChoice, GuestFamily, InstallSource, NetworkChoice, NewVm,
    PowerAction, list_devices, next_disk_target,
};
use vmng_xml::Element;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    vmng_virt::init();
    let mut a = std::env::args().skip(1);
    let (uri, pool, iso) = match (a.next(), a.next(), a.next()) {
        (Some(u), Some(p), Some(i)) => (u, p, i),
        _ => return Err("usage: hw_check <uri> <pool> <iso>".into()),
    };
    let conn = Connection::open(ConnectionKind::from_uri(&uri))?;
    let vm = NewVm {
        name: "vmng-test-hw".into(),
        os_id: None,
        family: GuestFamily::Linux,
        arch: "x86_64".into(),
        machine: Some("q35".into()),
        virt_type: "kvm".into(),
        memory: 512 << 20,
        vcpus: 1,
        firmware: FirmwareChoice::Bios,
        tpm: false,
        cpu_model: "host-passthrough".into(),
        install: InstallSource::Media(iso.clone()),
        disk: None,
        network: NetworkChoice::None,
        graphics: "vnc".into(),
        virtio: true,
        video: "virtio".into(),
    };
    let uuid = conn.create_vm(&vm, false)?.uuid;
    let mut created_disk = None;
    let mut run = || -> Result<(), Box<dyn std::error::Error>> {
        // Add a new 1 GiB disk.
        let xml = conn.domain_config_xml(&uuid)?;
        let target = next_disk_target(&xml, "vd");
        let path = conn.create_disk(&pool, "vmng-test-hw-extra", 1 << 30, "qcow2")?;
        created_disk = Some(path.clone());
        let disk = Element::new("disk")
            .with_attr("type", "file")
            .with_attr("device", "disk")
            .with_child(Element::new("driver").with_attr("name", "qemu").with_attr("type", "qcow2"))
            .with_child(Element::new("source").with_attr("file", &path))
            .with_child(Element::new("target").with_attr("dev", &target).with_attr("bus", "virtio"));
        assert!(conn.add_device(&uuid, &disk)?);
        // Add a NIC on user networking.
        let nic = Element::new("interface")
            .with_attr("type", "user")
            .with_child(Element::new("model").with_attr("type", "virtio"));
        conn.add_device(&uuid, &nic)?;
        // Change memory through a full save.
        let mut xml = conn.domain_config_xml(&uuid)?;
        for n in ["memory", "currentMemory"] {
            let m = xml.child_or_insert(n);
            m.set_attr("unit", "KiB");
            m.set_text((768 * 1024).to_string());
        }
        conn.save_domain_config(&uuid, &xml)?;
        // A changed UUID must be refused.
        let mut bad = xml.clone();
        bad.child_or_insert("uuid").set_text("00000000-0000-4000-8000-000000000000");
        assert!(conn.save_domain_config(&uuid, &bad).is_err());

        let xml = conn.domain_config_xml(&uuid)?;
        let devs = list_devices(&xml);
        assert_eq!(devs.iter().filter(|d| d.kind == DeviceKind::Disk).count(), 1);
        assert_eq!(devs.iter().filter(|d| d.kind == DeviceKind::Interface).count(), 1);
        let cfg = vmng_virt::DomainConfig::from_xml(&xml);
        assert_eq!(cfg.memory, 768 << 20);
        println!("added disk {target} at {path}, nic, memory 768 MiB");

        // Start, eject the CD live, unplug the cable live, then stop.
        conn.power(&uuid, PowerAction::Start)?;
        let mut cd = devs.iter().find(|d| d.kind == DeviceKind::Cdrom).ok_or("no cdrom")?.element.clone();
        cd.remove_children(|c| c.name == "source");
        println!("eject live: {}", conn.update_device(&uuid, &cd)?);
        let mut nic = devs.iter().find(|d| d.kind == DeviceKind::Interface).unwrap().element.clone();
        nic.child_or_insert("link").set_attr("state", "down");
        println!("unplug cable live: {}", conn.update_device(&uuid, &nic)?);
        // Hot-unplug the extra disk while running.
        let disk = list_devices(&conn.domain_config_xml(&uuid)?)
            .into_iter()
            .find(|d| d.kind == DeviceKind::Disk)
            .unwrap()
            .element;
        println!("remove disk while running (live={})", conn.remove_device(&uuid, &disk)?);
        conn.power(&uuid, PowerAction::ForceOff)?;
        let xml = conn.domain_config_xml(&uuid)?;
        assert!(list_devices(&xml).iter().all(|d| d.kind != DeviceKind::Disk));
        assert!(xml.find("devices/disk/source").is_none(), "cd should be empty");
        assert_eq!(xml.find_attr("devices/interface/link", "state"), Some("down"));
        println!("all checks passed");
        Ok(())
    };
    let result = run();
    let paths: Vec<String> = created_disk.into_iter().collect();
    println!("cleanup: {:?}", conn.delete_domain(&uuid, &paths)?);
    result
}
