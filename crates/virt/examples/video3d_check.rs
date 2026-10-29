//! Starts a throwaway VM (`vmng-test-3d`, no disk) with 3D acceleration on
//! its VirtIO video card (as the Hardware tab sets it), checks the running
//! machine has it, turns it off again, and deletes the VM.
//!
//! Usage: `video3d_check` (`VMNG_TEST_URI` picks the connection; default
//! `qemu:///session`).

use vmng_virt::{
    Connection, ConnectionKind, FirmwareChoice, GuestFamily, InstallSource, NetworkChoice, NewVm, PowerAction,
    set_video_3d, video_3d,
};
use vmng_xml::Node;

fn video_position(xml: &vmng_xml::Element) -> Option<usize> {
    xml.child("devices")?.children.iter().position(|n| matches!(n, Node::Element(e) if e.name == "video"))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    vmng_virt::init();
    let uri = std::env::var("VMNG_TEST_URI").unwrap_or_else(|_| "qemu:///session".into());
    let conn = Connection::open(ConnectionKind::from_uri(&uri))?;
    let vm = NewVm {
        name: "vmng-test-3d".into(),
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
        graphics: "spice".into(),
        virtio: true,
        video: "virtio".into(),
    };
    let created = conn.create_vm(&vm, false)?;
    let uuid = created.uuid.clone();
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        for on in [true, false] {
            let mut xml = conn.domain_config_xml(&uuid)?;
            let v = video_position(&xml).ok_or("no video card")?;
            set_video_3d(&mut xml, v, on);
            conn.save_domain_config(&uuid, &xml)?;
            conn.power(&uuid, PowerAction::Start)?;
            std::thread::sleep(std::time::Duration::from_secs(2));
            let live = vmng_xml::parse(&conn.domain_xml(&uuid, false)?)?;
            let video = live.find("devices/video").ok_or("no video card")?;
            let headless = live.to_xml().contains("egl-headless");
            if video_3d(video) != on || headless != on {
                return Err(format!(
                    "3D {on}: running machine has accel3d={} egl-headless={headless}",
                    video_3d(video)
                )
                .into());
            }
            let shot = conn.screenshot(&uuid);
            println!(
                "ok  3D {}: screenshot {:?}",
                if on { "on" } else { "off" },
                shot.map(|s| matches!(s, vmng_virt::Screenshot::Rgb { .. } | vmng_virt::Screenshot::Png(_)))
            );
            conn.power(&uuid, PowerAction::ForceOff)?;
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = conn.power(&uuid, PowerAction::ForceOff);
    }
    println!("cleanup: {:?}", conn.delete_domain(&uuid, &[]));
    result
}
