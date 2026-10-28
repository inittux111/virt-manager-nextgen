//! Creating virtual machines: the request, the XML it becomes, and the
//! steps to create disks, define and start the machine.

use virt::domain::Domain;
use vmng_xml::Element;

use crate::config::Firmware;
use crate::connection::Connection;
use crate::error::{Error, Result};
use crate::manage::{unique_name, validate_vm_name};
use crate::units::KIB;

/// How the guest OS gets onto the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallSource {
    /// Boot an installer from an ISO image or optical drive.
    Media(String),
    /// Use an existing disk image as the system disk.
    ImportDisk(String),
    /// No installer: an empty machine (network boot or set up later).
    Manual,
}

/// The disk to create for the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewDisk {
    pub pool: String,
    /// Size in bytes.
    pub size: u64,
    /// `qcow2` or `raw`.
    pub format: String,
}

/// Where the network card is connected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetworkChoice {
    /// A libvirt virtual network, e.g. `default`.
    Network(String),
    /// A host bridge, e.g. `br0`.
    Bridge(String),
    /// User-mode networking (no setup needed; used by user sessions).
    User,
    None,
}

/// Broad guest family, which decides drivers and defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuestFamily {
    Linux,
    Windows,
    Other,
}

/// Everything needed to create a VM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewVm {
    pub name: String,
    pub os_id: Option<String>,
    pub family: GuestFamily,
    pub arch: String,
    /// Machine type; `None` lets libvirt choose.
    pub machine: Option<String>,
    /// `kvm` or `qemu`.
    pub virt_type: String,
    /// Memory in bytes.
    pub memory: u64,
    pub vcpus: u32,
    pub firmware: Firmware,
    pub tpm: bool,
    /// `host-passthrough`, `host-model` or `default`.
    pub cpu_model: String,
    pub install: InstallSource,
    /// The system disk to create (not used when importing).
    pub disk: Option<NewDisk>,
    pub network: NetworkChoice,
    /// `spice`, `vnc` or `none`.
    pub graphics: String,
    /// Paravirtual (virtio) devices; Windows needs extra drivers for these.
    pub virtio: bool,
    /// Video model, e.g. `virtio`, `qxl`, `vga`, `bochs`.
    pub video: String,
}

/// The result of creating a VM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Created {
    pub uuid: String,
    /// Set when the machine was created but could not be started.
    pub start_error: Option<Error>,
}

fn el(name: &str) -> Element {
    Element::new(name)
}

fn is_x86(arch: &str) -> bool {
    matches!(arch, "x86_64" | "i686")
}

/// Builds the domain XML. `disk_path` is the created or imported disk.
pub fn build_domain_xml(vm: &NewVm, disk_path: Option<&str>) -> Element {
    let x86 = is_x86(&vm.arch);
    let windows = vm.family == GuestFamily::Windows;
    let q35 = vm.machine.as_deref().is_none_or(|m| m.contains("q35"));
    let mut d = el("domain").with_attr("type", &vm.virt_type);
    d.push(el("name").with_text(&vm.name));
    if let Some(id) = &vm.os_id {
        d.push(
            el("metadata").with_child(
                el("libosinfo:libosinfo")
                    .with_attr("xmlns:libosinfo", "http://libosinfo.org/xmlns/libvirt/domain/1.0")
                    .with_child(el("libosinfo:os").with_attr("id", id)),
            ),
        );
    }
    let kib = (vm.memory / KIB).to_string();
    d.push(el("memory").with_attr("unit", "KiB").with_text(&kib));
    d.push(el("currentMemory").with_attr("unit", "KiB").with_text(&kib));
    d.push(el("vcpu").with_attr("placement", "static").with_text(vm.vcpus.max(1).to_string()));

    // Firmware and boot.
    let mut os = el("os");
    let mut ty = el("type").with_attr("arch", &vm.arch).with_text("hvm");
    if let Some(m) = &vm.machine {
        ty.set_attr("machine", m);
    }
    os.push(ty);
    let secure = matches!(vm.firmware, Firmware::Uefi { secure_boot: true });
    if let Firmware::Uefi { secure_boot } = vm.firmware {
        os.set_attr("firmware", "efi");
        let yes_no = if secure_boot { "yes" } else { "no" };
        os.push(
            el("firmware")
                .with_child(el("feature").with_attr("enabled", yes_no).with_attr("name", "enrolled-keys"))
                .with_child(el("feature").with_attr("enabled", yes_no).with_attr("name", "secure-boot")),
        );
    }
    d.push(os);

    let mut features = el("features").with_child(el("acpi"));
    if x86 {
        features.push(el("apic"));
        if windows {
            let mut hv = el("hyperv").with_attr("mode", "custom");
            for f in ["relaxed", "vapic", "vpindex", "runtime", "synic", "stimer", "frequencies", "tlbflush", "ipi"] {
                hv.push(el(f).with_attr("state", "on"));
            }
            hv.push(el("spinlocks").with_attr("state", "on").with_attr("retries", "8191"));
            features.push(hv);
        }
        if vm.virt_type == "kvm" {
            features.push(el("vmport").with_attr("state", "off"));
        }
        if secure {
            features.push(el("smm").with_attr("state", "on"));
        }
    }
    d.push(features);

    if vm.virt_type == "kvm" && vm.cpu_model != "default" {
        let mut cpu = el("cpu").with_attr("mode", &vm.cpu_model);
        if vm.cpu_model == "host-passthrough" {
            cpu.set_attr("check", "none");
            cpu.set_attr("migratable", "on");
        }
        d.push(cpu);
    }

    let mut clock = el("clock").with_attr("offset", if windows { "localtime" } else { "utc" });
    if x86 {
        clock.push(el("timer").with_attr("name", "rtc").with_attr("tickpolicy", "catchup"));
        clock.push(el("timer").with_attr("name", "pit").with_attr("tickpolicy", "delay"));
        clock.push(el("timer").with_attr("name", "hpet").with_attr("present", "no"));
        if windows {
            clock.push(el("timer").with_attr("name", "hypervclock").with_attr("present", "yes"));
        }
    }
    d.push(clock);
    d.push(el("on_poweroff").with_text("destroy"));
    d.push(el("on_reboot").with_text("restart"));
    d.push(el("on_crash").with_text("destroy"));
    if x86 {
        d.push(
            el("pm")
                .with_child(el("suspend-to-mem").with_attr("enabled", "no"))
                .with_child(el("suspend-to-disk").with_attr("enabled", "no")),
        );
    }

    // Devices.
    let mut dev = el("devices");
    let (disk_bus, disk_prefix) = if vm.virtio { ("virtio", "vd") } else { ("sata", "sd") };
    let cdrom_bus = if x86 { "sata" } else { "scsi" };
    let mut sd_index = 0u8;
    let mut next_sd = || {
        let name = format!("sd{}", char::from(b'a' + sd_index));
        sd_index += 1;
        name
    };
    // The system disk boots first; with nothing installed yet the firmware
    // falls through to the installer, and after installation the new
    // system starts without having to remove the installer.
    if let Some(path) = disk_path {
        let format = match &vm.install {
            InstallSource::ImportDisk(p) if !p.ends_with(".qcow2") => "raw",
            _ => vm.disk.as_ref().map_or("qcow2", |d| d.format.as_str()),
        };
        let target = if disk_bus == "virtio" { format!("{disk_prefix}a") } else { next_sd() };
        let mut driver = el("driver").with_attr("name", "qemu").with_attr("type", format);
        if vm.virtio {
            driver.set_attr("discard", "unmap");
        }
        dev.push(
            el("disk")
                .with_attr("type", "file")
                .with_attr("device", "disk")
                .with_child(driver)
                .with_child(el("source").with_attr("file", path))
                .with_child(el("target").with_attr("dev", target).with_attr("bus", disk_bus))
                .with_child(el("boot").with_attr("order", "1")),
        );
    }
    if let InstallSource::Media(media) = &vm.install {
        let block = media.starts_with("/dev/");
        let mut cd = el("disk")
            .with_attr("type", if block { "block" } else { "file" })
            .with_attr("device", "cdrom")
            .with_child(el("driver").with_attr("name", "qemu").with_attr("type", "raw"))
            .with_child(el("source").with_attr(if block { "dev" } else { "file" }, media))
            .with_child(el("target").with_attr("dev", next_sd()).with_attr("bus", cdrom_bus))
            .with_child(el("readonly"));
        cd.push(el("boot").with_attr("order", "2"));
        dev.push(cd);
    }

    dev.push(el("controller").with_attr("type", "usb").with_attr("model", "qemu-xhci").with_attr("ports", "15"));
    if !x86 || cdrom_bus == "scsi" {
        dev.push(el("controller").with_attr("type", "scsi").with_attr("model", "virtio-scsi"));
    }

    let model = if vm.virtio {
        "virtio"
    } else if q35 {
        "e1000e"
    } else {
        "e1000"
    };
    let iface = match &vm.network {
        NetworkChoice::Network(n) => {
            Some(el("interface").with_attr("type", "network").with_child(el("source").with_attr("network", n)))
        }
        NetworkChoice::Bridge(b) => {
            Some(el("interface").with_attr("type", "bridge").with_child(el("source").with_attr("bridge", b)))
        }
        NetworkChoice::User => Some(el("interface").with_attr("type", "user")),
        NetworkChoice::None => None,
    };
    if let Some(iface) = iface {
        dev.push(iface.with_child(el("model").with_attr("type", model)));
    }

    dev.push(el("console").with_attr("type", "pty").with_child(el("target").with_attr("type", "serial")));
    // qemu-guest-agent channel: the guest reports its IP, hostname, and can
    // be shut down cleanly.
    dev.push(
        el("channel")
            .with_attr("type", "unix")
            .with_child(el("target").with_attr("type", "virtio").with_attr("name", "org.qemu.guest_agent.0")),
    );
    if vm.graphics == "spice" {
        dev.push(
            el("channel")
                .with_attr("type", "spicevmc")
                .with_child(el("target").with_attr("type", "virtio").with_attr("name", "com.redhat.spice.0")),
        );
    }
    dev.push(el("input").with_attr("type", "tablet").with_attr("bus", "usb"));
    if !x86 {
        dev.push(el("input").with_attr("type", "keyboard").with_attr("bus", "usb"));
    }
    if vm.graphics != "none" {
        dev.push(el("graphics").with_attr("type", &vm.graphics).with_attr("autoport", "yes"));
        if x86 {
            dev.push(el("sound").with_attr("model", if q35 { "ich9" } else { "ich6" }));
        }
        dev.push(el("video").with_child(el("model").with_attr("type", &vm.video)));
        if vm.graphics == "spice" {
            dev.push(el("redirdev").with_attr("bus", "usb").with_attr("type", "spicevmc"));
            dev.push(el("redirdev").with_attr("bus", "usb").with_attr("type", "spicevmc"));
        }
    }
    if vm.tpm {
        let model = if x86 { "tpm-crb" } else { "tpm-tis" };
        dev.push(
            el("tpm")
                .with_attr("model", model)
                .with_child(el("backend").with_attr("type", "emulator").with_attr("version", "2.0")),
        );
    }
    if vm.virtio {
        dev.push(el("memballoon").with_attr("model", "virtio"));
        dev.push(
            el("rng")
                .with_attr("model", "virtio")
                .with_child(el("backend").with_attr("model", "random").with_text("/dev/urandom")),
        );
    }
    d.push(dev);
    d
}

impl Connection {
    /// Creates (and optionally starts) a VM. If anything fails before the
    /// machine is defined, created disks are removed again.
    pub fn create_vm(&self, vm: &NewVm, start: bool) -> Result<Created> {
        validate_vm_name(&vm.name).map_err(|m| Error::Invalid(m.to_owned()))?;
        if Domain::lookup_by_name(self.raw(), &vm.name).is_ok() {
            return Err(Error::Invalid(format!("A machine called {} already exists.", vm.name)));
        }
        let mut created_disk: Option<String> = None;
        let disk_path = match (&vm.install, &vm.disk) {
            (InstallSource::ImportDisk(path), _) => Some(path.clone()),
            (_, Some(disk)) => {
                let ext = if disk.format == "qcow2" { ".qcow2" } else { ".img" };
                let name = unique_name(&format!("{}{ext}", vm.name), |n| !self.volume_name_free(&disk.pool, n));
                let path = self.create_volume(&disk.pool, &name, disk.size, &disk.format)?;
                created_disk = Some(path.clone());
                Some(path)
            }
            _ => None,
        };
        let xml = build_domain_xml(vm, disk_path.as_deref()).to_xml();
        let dom = match Domain::define_xml(self.raw(), &xml) {
            Ok(d) => d,
            Err(e) => {
                if let Some(path) = created_disk
                    && let Ok(v) = virt::storage_vol::StorageVol::lookup_by_path(self.raw(), &path)
                {
                    let _ = v.delete(0);
                }
                return Err(e.into());
            }
        };
        let uuid = dom.get_uuid_string()?;
        let start_error = if start { dom.create().err().map(Error::from) } else { None };
        Ok(Created { uuid, start_error })
    }

    /// The domain XML a request would produce (for review and testing).
    pub fn preview_vm_xml(&self, vm: &NewVm) -> String {
        let disk = match &vm.install {
            InstallSource::ImportDisk(p) => Some(p.clone()),
            _ => vm.disk.as_ref().map(|d| format!("<{}>/{}.qcow2", d.pool, vm.name)),
        };
        build_domain_xml(vm, disk.as_deref()).to_xml()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DomainConfig;

    fn linux() -> NewVm {
        NewVm {
            name: "fedora44".into(),
            os_id: Some("http://fedoraproject.org/fedora/unknown".into()),
            family: GuestFamily::Linux,
            arch: "x86_64".into(),
            machine: Some("q35".into()),
            virt_type: "kvm".into(),
            memory: 4 << 30,
            vcpus: 2,
            firmware: Firmware::Uefi { secure_boot: true },
            tpm: false,
            cpu_model: "host-passthrough".into(),
            install: InstallSource::Media("/isos/Fedora.iso".into()),
            disk: Some(NewDisk { pool: "default".into(), size: 20 << 30, format: "qcow2".into() }),
            network: NetworkChoice::Bridge("br0".into()),
            graphics: "spice".into(),
            virtio: true,
            video: "virtio".into(),
        }
    }

    #[test]
    fn linux_vm_round_trips_through_config() {
        let xml = build_domain_xml(&linux(), Some("/images/fedora44.qcow2"));
        let c = DomainConfig::from_xml(&vmng_xml::parse(&xml.to_xml()).unwrap());
        assert_eq!(c.name, "fedora44");
        assert_eq!(c.os_id.as_deref(), Some("http://fedoraproject.org/fedora/unknown"));
        assert_eq!(c.memory, 4 << 30);
        assert_eq!(c.vcpus, 2);
        assert_eq!(c.firmware, Some(Firmware::Uefi { secure_boot: true }));
        assert_eq!(c.disks.len(), 2);
        assert_eq!(c.disks[0].bus.as_deref(), Some("virtio"));
        assert_eq!(c.disks[0].target, "vda");
        assert!(c.disks[1].is_cdrom());
        assert_eq!(c.interfaces[0].kind, "bridge");
        assert_eq!(c.interfaces[0].source.as_deref(), Some("br0"));
        assert_eq!(c.graphics[0].kind, "spice");
        assert!(c.has_guest_agent_channel);
        assert!(xml.find("features/smm").is_some());
        let boots: Vec<_> = xml
            .child("devices")
            .unwrap()
            .children_named("disk")
            .map(|d| d.find_attr("boot", "order").unwrap())
            .collect();
        assert_eq!(boots, ["1", "2"]);
    }

    #[test]
    fn windows_vm_uses_compatible_devices() {
        let mut vm = linux();
        vm.family = GuestFamily::Windows;
        vm.virtio = false;
        vm.tpm = true;
        vm.video = "qxl".into();
        let xml = build_domain_xml(&vm, Some("/images/win.qcow2"));
        let c = DomainConfig::from_xml(&xml);
        assert_eq!(c.disks[0].bus.as_deref(), Some("sata"));
        assert_eq!(c.disks[0].target, "sda");
        assert_eq!(c.disks[1].target, "sdb");
        assert_eq!(c.interfaces[0].model.as_deref(), Some("e1000e"));
        assert_eq!(xml.find_attr("clock", "offset"), Some("localtime"));
        assert!(xml.find("features/hyperv").is_some());
        assert!(xml.find("devices/tpm").is_some());
        assert!(xml.find("devices/rng").is_none());
    }

    #[test]
    fn bios_manual_vm_without_disk() {
        let mut vm = linux();
        vm.firmware = Firmware::Bios;
        vm.install = InstallSource::Manual;
        vm.disk = None;
        vm.network = NetworkChoice::None;
        vm.graphics = "none".into();
        let xml = build_domain_xml(&vm, None);
        assert!(xml.find_attr("os", "firmware").is_none());
        assert!(xml.find("features/smm").is_none());
        assert!(xml.find("devices/disk").is_none());
        assert!(xml.find("devices/interface").is_none());
        assert!(xml.find("devices/graphics").is_none());
    }
}
