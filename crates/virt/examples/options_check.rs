//! Checks the machine options on a throwaway VM (`vmng-test-opts`) on
//! `qemu:///session`: a new machine on an existing disk image, title and
//! description, current and maximum memory (also changed while running),
//! shared folders (9p and virtiofs, read-only), disk options (detect
//! zeroes, I/O mode, a removable USB disk) and a chosen MAC address.
//! The pool `vmng-test-optpool` in `<empty-dir>` and everything in it is
//! removed again.
//!
//! Usage: `options_check <empty-dir>`

use vmng_virt::{
    Connection, ConnectionKind, FirmwareChoice, GuestFamily, InstallSource, MemorySize, NetworkChoice, NewPool, NewVm,
    PoolKind, PowerAction, VolumeOptions, set_memory,
};
use vmng_xml::Element;

type Res<T> = Result<T, Box<dyn std::error::Error>>;

const POOL: &str = "vmng-test-optpool";
const MIB: u64 = 1 << 20;

fn main() -> Res<()> {
    vmng_virt::init();
    let dir = std::env::args().nth(1).ok_or("usage: options_check <empty-dir>")?;
    let uri = std::env::var("VMNG_TEST_URI").unwrap_or_else(|_| "qemu:///session".into());
    let conn = Connection::open(ConnectionKind::from_uri(&uri))?;
    conn.create_pool(&NewPool {
        name: POOL.into(),
        kind: PoolKind::Folder,
        path: dir.clone(),
        host: String::new(),
        source: String::new(),
    })?;
    let result = run(&conn, &dir);
    if let Some(vm) = conn.list_domains().ok().and_then(|l| l.into_iter().find(|d| d.config.name == "vmng-test-opts")) {
        let _ = conn.power(&vm.uuid, PowerAction::ForceOff);
        println!("cleanup vm: {:?}", conn.delete_domain(&vm.uuid, &[]));
    }
    for v in conn.list_volumes(POOL).unwrap_or_default() {
        let _ = conn.delete_volume(POOL, &v.name);
    }
    println!("cleanup pool: {:?}", conn.remove_pool(POOL));
    result?;
    println!("all checks passed");
    Ok(())
}

fn check(ok: bool, what: &str) -> Res<()> {
    if ok {
        println!("ok  {what}");
        Ok(())
    } else {
        Err(format!("FAILED: {what}").into())
    }
}

fn run(conn: &Connection, dir: &str) -> Res<()> {
    // An existing image whose name doesn't say it is qcow2: the format
    // must come from libvirt, not the file name.
    let image = conn.create_named_volume(POOL, "existing.disk", 1 << 30, "qcow2", &VolumeOptions::default())?;
    let usb = conn.create_named_volume(POOL, "usb.img", 64 * MIB, "raw", &VolumeOptions::default())?;
    let vm = NewVm {
        name: "vmng-test-opts".into(),
        os_id: None,
        family: GuestFamily::Linux,
        arch: "x86_64".into(),
        machine: Some("q35".into()),
        virt_type: "kvm".into(),
        memory: 256 * MIB,
        vcpus: 1,
        firmware: FirmwareChoice::Bios,
        tpm: false,
        cpu_model: "host-passthrough".into(),
        install: InstallSource::Manual,
        disk: None,
        existing_disk: Some(image.clone()),
        network: NetworkChoice::User,
        graphics: "vnc".into(),
        virtio: true,
        video: "virtio".into(),
    };
    let uuid = conn.create_vm(&vm, false)?.uuid;
    let xml = conn.domain_config_xml(&uuid)?;
    let disk = xml.find("devices/disk").ok_or("no disk")?;
    check(disk.find_attr("source", "file") == Some(image.as_str()), "new VM uses the existing image")?;
    check(disk.find_attr("driver", "type") == Some("qcow2"), "existing image format read from libvirt")?;
    check(conn.list_volumes(POOL)?.len() == 2, "no extra disk was created")?;

    // Everything at once in the saved configuration.
    let mut xml = conn.domain_config_xml(&uuid)?;
    let pos = xml.children.iter().position(|n| matches!(n, vmng_xml::Node::Element(e) if e.name == "uuid"));
    let at = pos.map_or(1, |p| p + 1);
    xml.children.insert(at, vmng_xml::Node::Element(Element::new("title").with_text("Options test")));
    xml.children.insert(at + 1, vmng_xml::Node::Element(Element::new("description").with_text("Line one\nLine two")));
    set_memory(&mut xml, MemorySize { current: 256 * MIB, maximum: 512 * MIB });
    let devices = xml.child_mut("devices").ok_or("no devices")?;
    let disk = devices.children.iter_mut().find_map(|n| match n {
        vmng_xml::Node::Element(e) if e.name == "disk" => Some(e),
        _ => None,
    });
    let disk = disk.ok_or("no disk")?;
    let driver = disk.child_or_insert("driver");
    driver.set_attr("discard", "unmap");
    driver.set_attr("detect_zeroes", "unmap");
    driver.set_attr("io", "threads");
    devices.push(
        Element::new("disk")
            .with_attr("type", "file")
            .with_attr("device", "disk")
            .with_child(Element::new("driver").with_attr("name", "qemu").with_attr("type", "raw"))
            .with_child(Element::new("source").with_attr("file", &usb))
            .with_child(
                Element::new("target").with_attr("dev", "sda").with_attr("bus", "usb").with_attr("removable", "on"),
            ),
    );
    devices.push(
        Element::new("filesystem")
            .with_attr("type", "mount")
            .with_attr("accessmode", "mapped")
            .with_child(Element::new("driver").with_attr("type", "path"))
            .with_child(Element::new("source").with_attr("dir", dir))
            .with_child(Element::new("target").with_attr("dir", "share9p"))
            .with_child(Element::new("readonly")),
    );
    if let Some(iface) = devices.children.iter_mut().find_map(|n| match n {
        vmng_xml::Node::Element(e) if e.name == "interface" => Some(e),
        _ => None,
    }) {
        iface.child_or_insert("mac").set_attr("address", "52:54:00:aa:bb:01");
    }
    conn.save_domain_config(&uuid, &xml)?;
    conn.power(&uuid, PowerAction::Start)?;
    std::thread::sleep(std::time::Duration::from_secs(1));
    let live = vmng_xml::parse(&conn.domain_xml(&uuid, false)?)?;
    check(live.find_text("title").as_deref() == Some("Options test"), "title")?;
    check(live.find_text("description").as_deref() == Some("Line one\nLine two"), "description")?;
    // The running size is the guest's balloon, which an empty machine
    // can't shrink, so the saved sizes are checked.
    let mem = MemorySize::of(&conn.domain_config_xml(&uuid)?);
    check(mem.maximum == 512 * MIB && mem.current == 256 * MIB, &format!("memory 256 of max 512 MiB ({mem:?})"))?;
    let disks: Vec<&Element> = live.find("devices").ok_or("no devices")?.children_named("disk").collect();
    check(
        disks[0].find_attr("driver", "detect_zeroes") == Some("unmap")
            && disks[0].find_attr("driver", "io") == Some("threads"),
        "detect zeroes and I/O mode",
    )?;
    check(disks.iter().any(|d| d.find_attr("target", "removable") == Some("on")), "removable USB disk")?;
    let fs = live.find("devices/filesystem").ok_or("no shared folder")?;
    check(fs.child("readonly").is_some() && fs.find_attr("target", "dir") == Some("share9p"), "read-only 9p folder")?;
    check(live.find_attr("devices/interface/mac", "address") == Some("52:54:00:aa:bb:01"), "chosen MAC address")?;

    // While running: memory up to the maximum, and a new title.
    conn.set_live_memory(&uuid, 384 * MIB)?;
    conn.set_live_title(&uuid, Some("Renamed live"), None)?;
    let live = vmng_xml::parse(&conn.domain_xml(&uuid, false)?)?;
    // Accepted (the guest's balloon driver would carry it out).
    check(live.find_text("title").as_deref() == Some("Renamed live"), "title changed while running")?;
    check(live.find_text("description").is_none(), "description cleared while running")?;
    check(conn.set_live_memory(&uuid, 1024 * MIB).is_err(), "memory above the maximum is refused")?;
    conn.power(&uuid, PowerAction::ForceOff)?;
    std::thread::sleep(std::time::Duration::from_millis(500));

    // virtiofs, read-only (needs shared memory and virtiofsd).
    let mut xml = conn.domain_config_xml(&uuid)?;
    let mb = xml.child_or_insert("memoryBacking");
    mb.child_or_insert("source").set_attr("type", "memfd");
    mb.child_or_insert("access").set_attr("mode", "shared");
    let devices = xml.child_mut("devices").ok_or("no devices")?;
    devices.remove_children(|c| c.name == "filesystem");
    devices.push(
        Element::new("filesystem")
            .with_attr("type", "mount")
            .with_attr("accessmode", "passthrough")
            .with_child(Element::new("driver").with_attr("type", "virtiofs"))
            .with_child(Element::new("source").with_attr("dir", dir))
            .with_child(Element::new("target").with_attr("dir", "sharefs"))
            .with_child(Element::new("readonly")),
    );
    conn.save_domain_config(&uuid, &xml)?;
    match conn.power(&uuid, PowerAction::Start) {
        Ok(()) => {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let live = vmng_xml::parse(&conn.domain_xml(&uuid, false)?)?;
            let fs = live.find("devices/filesystem").ok_or("no shared folder")?;
            check(
                fs.child("readonly").is_some() && fs.find_attr("driver", "type") == Some("virtiofs"),
                "read-only virtiofs folder",
            )?;
            conn.power(&uuid, PowerAction::ForceOff)?;
        }
        Err(e) => return Err(format!("FAILED: virtiofs folder didn't start: {}", e.message()).into()),
    }
    Ok(())
}
