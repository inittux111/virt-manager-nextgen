//! Names, descriptions and icons for devices, as shown in the hardware list.

use vmng_virt::{DeviceKind, DeviceRef};
use vmng_xml::Element;

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn bus_name(bus: Option<&str>) -> &str {
    match bus {
        Some("virtio") => "VirtIO",
        Some("sata") => "SATA",
        Some("scsi") => "SCSI",
        Some("ide") => "IDE",
        Some("usb") => "USB",
        Some("nvme") => "NVMe",
        Some("ps2") => "PS/2",
        Some(other) => other,
        None => "",
    }
}

/// Whether a device gets its own row in the hardware list. Plumbing that
/// libvirt manages by itself (PCI bridges, the emulator path, …) is hidden.
pub fn is_listed(d: &DeviceRef) -> bool {
    match &d.kind {
        DeviceKind::Emulator | DeviceKind::Memballoon => false,
        // Renders 3D acceleration; set through the video card's switch.
        DeviceKind::Graphics => d.element.attr("type") != Some("egl-headless"),
        // The audio backend is plumbing for the sound card.
        DeviceKind::Other(name) if name == "audio" => false,
        DeviceKind::Controller => !matches!(d.element.attr("type"), Some("pci" | "virtio-serial" | "sata" | "ide")),
        _ => true,
    }
}

/// Where a disk's data comes from (file, block device or pool volume).
pub fn disk_source(el: &Element) -> Option<String> {
    let src = el.child("source")?;
    src.attr("file")
        .or_else(|| src.attr("dev"))
        .map(str::to_owned)
        .or_else(|| Some(format!("{} in {}", src.attr("volume")?, src.attr("pool")?)))
}

/// Title and subtitle of a device row.
pub fn describe(d: &DeviceRef) -> (String, String) {
    let e = &d.element;
    let attr = |name: &str| e.attr(name).unwrap_or_default().to_owned();
    match &d.kind {
        DeviceKind::Disk => {
            let target = e.find_attr("target", "dev").unwrap_or_default();
            let bus = bus_name(e.find_attr("target", "bus"));
            let mut sub = Vec::new();
            if let Some(f) = e.find_attr("driver", "type") {
                sub.push(f.to_owned());
            }
            if let Some(s) = disk_source(e) {
                sub.push(file_name(&s).to_owned());
            }
            (format!("{bus} disk ({target})").trim().to_owned(), sub.join(" · "))
        }
        DeviceKind::Cdrom => {
            let target = e.find_attr("target", "dev").unwrap_or_default();
            let media = disk_source(e).map_or_else(|| "Empty".to_owned(), |s| file_name(&s).to_owned());
            (format!("CD-ROM ({target})"), media)
        }
        DeviceKind::Floppy => {
            ("Floppy".into(), disk_source(e).map_or_else(|| "Empty".into(), |s| file_name(&s).to_owned()))
        }
        DeviceKind::Interface => {
            let kind = attr("type");
            let source = match kind.as_str() {
                "network" => e.find_attr("source", "network").map(str::to_owned),
                "bridge" => e.find_attr("source", "bridge").map(|b| format!("{b} (bridge)")),
                "direct" => e.find_attr("source", "dev").map(|d| format!("{d} (macvtap)")),
                "user" => Some("User networking".into()),
                _ => None,
            };
            let model = e.find_attr("model", "type").unwrap_or_default();
            let sub =
                [source.unwrap_or(kind), model.to_owned()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>();
            ("Network".into(), sub.join(" · "))
        }
        DeviceKind::Graphics => ("Display".into(), attr("type").to_uppercase()),
        DeviceKind::Video => ("Video".into(), e.find_attr("model", "type").unwrap_or_default().to_owned()),
        DeviceKind::Sound => ("Sound".into(), attr("model")),
        DeviceKind::Input => {
            let title = match attr("type").as_str() {
                "tablet" => "Tablet",
                "keyboard" => "Keyboard",
                "mouse" => "Mouse",
                other => return (format!("Input ({other})"), bus_name(e.attr("bus")).to_owned()),
            };
            (title.into(), bus_name(e.attr("bus")).to_owned())
        }
        DeviceKind::Filesystem => ("Shared folder".into(), e.find_attr("source", "dir").unwrap_or_default().to_owned()),
        DeviceKind::Channel => match e.find_attr("target", "name") {
            Some("org.qemu.guest_agent.0") => ("Guest agent channel".into(), "qemu-guest-agent".into()),
            Some("com.redhat.spice.0") => ("SPICE agent channel".into(), "Clipboard and display resizing".into()),
            Some(name) => ("Channel".into(), name.to_owned()),
            None => ("Channel".into(), attr("type")),
        },
        DeviceKind::Serial => ("Serial port".into(), attr("type")),
        DeviceKind::Console => ("Console".into(), e.find_attr("target", "type").unwrap_or_default().to_owned()),
        DeviceKind::Controller => {
            let t = attr("type");
            let title = match t.as_str() {
                "usb" => "USB controller".to_owned(),
                "scsi" => "SCSI controller".to_owned(),
                other => format!("{} controller", other.to_uppercase()),
            };
            (title, attr("model"))
        }
        DeviceKind::Tpm => {
            ("TPM".into(), format!("{} · {}", e.find_attr("backend", "version").unwrap_or("2.0"), attr("model")))
        }
        DeviceKind::Rng => ("Random number generator".into(), e.find_text("backend").unwrap_or_default()),
        DeviceKind::Redirdev => ("USB redirector".into(), attr("type")),
        DeviceKind::Hostdev => ("Host device".into(), attr("type")),
        DeviceKind::Watchdog => ("Watchdog".into(), attr("model")),
        DeviceKind::Memballoon => ("Memory balloon".into(), attr("model")),
        DeviceKind::Emulator => ("Emulator".into(), e.text()),
        DeviceKind::Other(name) => (name.clone(), String::new()),
    }
}

/// A symbolic icon for a device kind.
pub fn icon(kind: &DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Disk => "drive-harddisk-symbolic",
        DeviceKind::Cdrom | DeviceKind::Floppy => "media-optical-symbolic",
        DeviceKind::Interface => "network-wired-symbolic",
        DeviceKind::Graphics => "video-display-symbolic",
        DeviceKind::Video => "video-display-symbolic",
        DeviceKind::Sound => "audio-speakers-symbolic",
        DeviceKind::Input => "input-keyboard-symbolic",
        DeviceKind::Filesystem => "folder-symbolic",
        DeviceKind::Channel | DeviceKind::Serial | DeviceKind::Console => "utilities-terminal-symbolic",
        DeviceKind::Controller => "drive-multidisk-symbolic",
        DeviceKind::Tpm => "security-high-symbolic",
        DeviceKind::Rng => "dialog-password-symbolic",
        DeviceKind::Redirdev | DeviceKind::Hostdev => "media-removable-symbolic",
        _ => "applications-system-symbolic",
    }
}

#[cfg(test)]
mod tests {
    use vmng_virt::list_devices;

    use super::*;

    #[test]
    fn describes_common_devices() {
        let d = vmng_xml::parse(
            "<domain><devices>\
             <disk type='file' device='disk'><driver type='qcow2'/><source file='/i/fedora.qcow2'/><target dev='vda' bus='virtio'/></disk>\
             <disk type='file' device='cdrom'><target dev='sda' bus='sata'/></disk>\
             <interface type='bridge'><source bridge='br0'/><model type='virtio'/></interface>\
             <channel type='unix'><target type='virtio' name='org.qemu.guest_agent.0'/></channel>\
             <controller type='pci' model='pcie-root'/>\
             </devices></domain>",
        )
        .unwrap();
        let devs = list_devices(&d);
        assert_eq!(describe(&devs[0]), ("VirtIO disk (vda)".into(), "qcow2 · fedora.qcow2".into()));
        assert_eq!(describe(&devs[1]), ("CD-ROM (sda)".into(), "Empty".into()));
        assert_eq!(describe(&devs[2]).1, "br0 (bridge) · virtio");
        assert_eq!(describe(&devs[3]).0, "Guest agent channel");
        assert!(!is_listed(&devs[4]));
    }
}
