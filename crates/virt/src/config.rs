//! A typed, read-only view of a domain's XML configuration.
//!
//! Only the facts the UI shows are extracted here. Editing always goes
//! through the full [`vmng_xml::Element`] tree so that nothing unknown is
//! lost.

use vmng_xml::Element;

use crate::units::{self, KIB};

/// Boot firmware of a domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Firmware {
    Bios,
    Uefi { secure_boot: bool },
}

/// A `<disk>` device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskConfig {
    /// `disk`, `cdrom`, `floppy` or `lun`.
    pub device: String,
    /// Guest target name, e.g. `vda`.
    pub target: String,
    /// Bus, e.g. `virtio`, `sata`, `scsi`.
    pub bus: Option<String>,
    /// Image format from `<driver type=…>`, e.g. `qcow2`.
    pub format: Option<String>,
    /// Path of the backing file or block device, if any.
    pub source_path: Option<String>,
    /// Storage pool and volume for `type="volume"` disks.
    pub source_volume: Option<(String, String)>,
    pub read_only: bool,
}

impl DiskConfig {
    pub fn is_cdrom(&self) -> bool {
        self.device == "cdrom"
    }
}

/// An `<interface>` device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterfaceConfig {
    /// `network`, `bridge`, `direct`, `user`, …
    pub kind: String,
    /// Network name, bridge name or host device, depending on `kind`.
    pub source: Option<String>,
    pub model: Option<String>,
    pub mac: Option<String>,
    /// Host-side device name while the domain runs, e.g. `vnet3`.
    pub target_dev: Option<String>,
}

/// A `<graphics>` device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphicsConfig {
    /// `spice`, `vnc`, …
    pub kind: String,
    /// TCP port while running (`-1` or absent when automatic and stopped).
    pub port: Option<i32>,
    pub listen: Option<String>,
}

/// Facts extracted from a domain XML document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DomainConfig {
    pub name: String,
    pub uuid: String,
    pub title: Option<String>,
    pub description: Option<String>,
    /// osinfo id from the libosinfo metadata, if the domain has one.
    pub os_id: Option<String>,
    pub vcpus: u32,
    /// Current (boot) memory in bytes.
    pub memory: u64,
    /// Maximum memory in bytes.
    pub max_memory: u64,
    pub arch: Option<String>,
    pub machine: Option<String>,
    /// Hypervisor type, e.g. `kvm` or `qemu`.
    pub domain_type: String,
    pub firmware: Option<Firmware>,
    /// Path and format of the per-VM UEFI variable store, if any.
    pub nvram_path: Option<String>,
    pub nvram_format: Option<String>,
    pub disks: Vec<DiskConfig>,
    pub interfaces: Vec<InterfaceConfig>,
    pub graphics: Vec<GraphicsConfig>,
    /// Whether a qemu-guest-agent channel is configured.
    pub has_guest_agent_channel: bool,
}

impl DomainConfig {
    /// Extracts the configuration from a parsed `<domain>` element.
    pub fn from_xml(dom: &Element) -> Self {
        let memory = |path: &str| {
            dom.find(path).map_or(0, |e| {
                let v = e.text().parse().unwrap_or(0);
                units::to_bytes(v, e.attr("unit"), KIB)
            })
        };
        let max_memory = memory("memory");
        let current = memory("currentMemory");

        let devices = dom.child("devices");
        let devs = |name: &'static str| devices.into_iter().flat_map(move |d| d.children_named(name));

        Self {
            name: dom.find_text("name").unwrap_or_default(),
            uuid: dom.find_text("uuid").unwrap_or_default(),
            title: dom.find_text("title").filter(|s| !s.is_empty()),
            description: dom.find_text("description").filter(|s| !s.is_empty()),
            os_id: os_id(dom),
            vcpus: dom.find("vcpu").map_or(1, |v| {
                v.attr("current").and_then(|c| c.parse().ok()).unwrap_or_else(|| v.text().parse().unwrap_or(1))
            }),
            memory: if current > 0 { current } else { max_memory },
            max_memory,
            arch: dom.find_attr("os/type", "arch").map(str::to_owned),
            machine: dom.find_attr("os/type", "machine").map(str::to_owned),
            domain_type: dom.attr("type").unwrap_or("qemu").to_owned(),
            firmware: firmware(dom),
            nvram_path: dom.find_text("os/nvram").filter(|p| !p.is_empty()),
            nvram_format: dom.find_attr("os/nvram", "format").map(str::to_owned),
            disks: devs("disk").map(disk).collect(),
            interfaces: devs("interface").map(interface).collect(),
            graphics: devs("graphics")
                .map(|g| GraphicsConfig {
                    kind: g.attr("type").unwrap_or_default().to_owned(),
                    port: g.attr("port").and_then(|p| p.parse().ok()).filter(|&p| p > 0),
                    listen: g.attr("listen").map(str::to_owned),
                })
                .collect(),
            has_guest_agent_channel: devs("channel")
                .any(|c| c.find_attr("target", "name") == Some("org.qemu.guest_agent.0")),
        }
    }
}

fn os_id(dom: &Element) -> Option<String> {
    let meta = dom.child("metadata")?;
    // The namespace prefix is chosen by whoever wrote the XML; match on the
    // local name instead of assuming "libosinfo:".
    let local = |e: &&Element, name: &str| e.name.rsplit(':').next() == Some(name);
    let info = meta.elements().find(|e| local(e, "libosinfo"))?;
    let os = info.elements().find(|e| local(e, "os"))?;
    os.attr("id").map(str::to_owned)
}

fn firmware(dom: &Element) -> Option<Firmware> {
    let os = dom.child("os")?;
    let loader = os.child("loader");
    let efi = os.attr("firmware") == Some("efi")
        || loader.is_some_and(|l| {
            l.attr("type") == Some("pflash") || l.text().contains("OVMF") || l.text().contains("edk2")
        });
    if !efi {
        return Some(Firmware::Bios);
    }
    let secure_boot = loader.is_some_and(|l| l.attr("secure") == Some("yes"))
        || os.child("firmware").is_some_and(|f| {
            f.children_named("feature")
                .any(|feat| feat.attr("name") == Some("secure-boot") && feat.attr("enabled") == Some("yes"))
        });
    Some(Firmware::Uefi { secure_boot })
}

fn disk(d: &Element) -> DiskConfig {
    let source = d.child("source");
    let attr = |name| source.and_then(|s| s.attr(name)).map(str::to_owned);
    DiskConfig {
        device: d.attr("device").unwrap_or("disk").to_owned(),
        target: d.find_attr("target", "dev").unwrap_or_default().to_owned(),
        bus: d.find_attr("target", "bus").map(str::to_owned),
        format: d.find_attr("driver", "type").map(str::to_owned),
        source_path: attr("file").or_else(|| attr("dev")),
        source_volume: attr("pool").zip(attr("volume")),
        read_only: d.child("readonly").is_some(),
    }
}

fn interface(i: &Element) -> InterfaceConfig {
    let kind = i.attr("type").unwrap_or_default().to_owned();
    let src = i.child("source");
    let source = src.and_then(|s| match kind.as_str() {
        "network" => s.attr("network"),
        "bridge" => s.attr("bridge"),
        _ => s.attr("dev"),
    });
    InterfaceConfig {
        source: source.map(str::to_owned),
        model: i.find_attr("model", "type").map(str::to_owned),
        mac: i.find_attr("mac", "address").map(str::to_owned),
        target_dev: i.find_attr("target", "dev").map(str::to_owned),
        kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<domain type="kvm">
  <name>fedora</name>
  <uuid>8e1d1c84-0000-4000-8000-000000000001</uuid>
  <metadata>
    <osi:libosinfo xmlns:osi="http://libosinfo.org/xmlns/libvirt/domain/1.0">
      <osi:os id="http://fedoraproject.org/fedora/40"/>
    </osi:libosinfo>
  </metadata>
  <memory unit="KiB">8388608</memory>
  <currentMemory unit="GiB">4</currentMemory>
  <vcpu placement="static" current="2">4</vcpu>
  <os firmware="efi">
    <type arch="x86_64" machine="pc-q35-9.0">hvm</type>
    <firmware><feature enabled="yes" name="secure-boot"/></firmware>
  </os>
  <devices>
    <disk type="file" device="disk">
      <driver name="qemu" type="qcow2"/>
      <source file="/var/lib/libvirt/images/fedora.qcow2"/>
      <target dev="vda" bus="virtio"/>
    </disk>
    <disk type="file" device="cdrom"><target dev="sda" bus="sata"/><readonly/></disk>
    <interface type="network">
      <mac address="52:54:00:aa:bb:cc"/>
      <source network="default"/>
      <model type="virtio"/>
    </interface>
    <channel type="unix"><target type="virtio" name="org.qemu.guest_agent.0"/></channel>
    <graphics type="spice" autoport="yes" port="5900"/>
  </devices>
</domain>"#;

    #[test]
    fn extracts_config() {
        let c = DomainConfig::from_xml(&vmng_xml::parse(XML).unwrap());
        assert_eq!(c.name, "fedora");
        assert_eq!(c.os_id.as_deref(), Some("http://fedoraproject.org/fedora/40"));
        assert_eq!(c.max_memory, 8 * units::GIB);
        assert_eq!(c.memory, 4 * units::GIB);
        assert_eq!(c.vcpus, 2);
        assert_eq!(c.firmware, Some(Firmware::Uefi { secure_boot: true }));
        assert_eq!(c.disks.len(), 2);
        assert_eq!(c.disks[0].format.as_deref(), Some("qcow2"));
        assert!(c.disks[1].is_cdrom() && c.disks[1].read_only);
        assert_eq!(c.interfaces[0].source.as_deref(), Some("default"));
        assert_eq!(c.graphics[0].kind, "spice");
        assert_eq!(c.graphics[0].port, Some(5900));
        assert!(c.has_guest_agent_channel);
    }

    #[test]
    fn minimal_domain_defaults() {
        let c = DomainConfig::from_xml(
            &vmng_xml::parse("<domain type='qemu'><name>x</name><os><type>hvm</type></os></domain>").unwrap(),
        );
        assert_eq!(c.vcpus, 1);
        assert_eq!(c.firmware, Some(Firmware::Bios));
        assert!(c.disks.is_empty());
    }
}
