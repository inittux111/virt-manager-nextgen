//! Reading and changing a domain's devices and settings.
//!
//! Edits work on the domain's full persistent XML (`vmng_xml::Element`), so
//! anything the app does not understand is kept as is. Devices are
//! addressed by their position among the children of `<devices>`.

use virt::domain::Domain;
use vmng_xml::{Element, Node};

use crate::connection::Connection;
use crate::error::{Error, Result};

// virDomainXMLFlags
const XML_SECURE: u32 = 1;
const XML_INACTIVE: u32 = 2;
// virDomainModificationImpact
const AFFECT_LIVE: u32 = 1;
const AFFECT_CONFIG: u32 = 2;

/// The kinds of devices the app distinguishes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    Disk,
    Cdrom,
    Floppy,
    Interface,
    Graphics,
    Video,
    Sound,
    Input,
    Filesystem,
    Channel,
    Serial,
    Console,
    Controller,
    Tpm,
    Rng,
    Redirdev,
    Hostdev,
    Watchdog,
    Memballoon,
    Emulator,
    Other(String),
}

impl DeviceKind {
    pub fn of(el: &Element) -> Self {
        match el.name.as_str() {
            "disk" => match el.attr("device") {
                Some("cdrom") => Self::Cdrom,
                Some("floppy") => Self::Floppy,
                _ => Self::Disk,
            },
            "interface" => Self::Interface,
            "graphics" => Self::Graphics,
            "video" => Self::Video,
            "sound" => Self::Sound,
            "input" => Self::Input,
            "filesystem" => Self::Filesystem,
            "channel" => Self::Channel,
            "serial" => Self::Serial,
            "console" => Self::Console,
            "controller" => Self::Controller,
            "tpm" => Self::Tpm,
            "rng" => Self::Rng,
            "redirdev" => Self::Redirdev,
            "hostdev" => Self::Hostdev,
            "watchdog" => Self::Watchdog,
            "memballoon" => Self::Memballoon,
            "emulator" => Self::Emulator,
            other => Self::Other(other.to_owned()),
        }
    }

    /// Devices the guest can't work without or libvirt re-adds on its own;
    /// they are shown but cannot be removed.
    pub fn removable(&self, el: &Element) -> bool {
        match self {
            Self::Emulator | Self::Memballoon => false,
            // The serial console pair is managed together by libvirt.
            Self::Console => false,
            // PCI/PCIe root and bridge controllers are created by libvirt.
            Self::Controller => !matches!(el.attr("type"), Some("pci" | "sata" | "virtio-serial")),
            // PS/2 keyboard and mouse come with the machine type.
            Self::Input => el.attr("bus") != Some("ps2"),
            _ => true,
        }
    }
}

/// One device of a domain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceRef {
    pub kind: DeviceKind,
    /// Position among the children of `<devices>`.
    pub index: usize,
    pub element: Element,
}

/// Lists the devices of a domain in document order.
pub fn list_devices(domain: &Element) -> Vec<DeviceRef> {
    let Some(devices) = domain.child("devices") else { return Vec::new() };
    devices
        .children
        .iter()
        .enumerate()
        .filter_map(|(index, n)| match n {
            Node::Element(e) => Some(DeviceRef { kind: DeviceKind::of(e), index, element: e.clone() }),
            _ => None,
        })
        .collect()
}

/// Replaces the device at `index` in `domain`.
pub fn replace_device(domain: &mut Element, index: usize, device: Element) -> Result<()> {
    let devices = domain.child_mut("devices").ok_or_else(|| Error::Invalid("The machine has no devices.".into()))?;
    match devices.children.get_mut(index) {
        Some(n @ Node::Element(_)) => {
            *n = Node::Element(device);
            Ok(())
        }
        _ => Err(Error::NotFound("That device no longer exists.".into())),
    }
}

/// Picks the first free target name with `prefix` (e.g. `vd` → `vdb`).
pub fn next_disk_target(domain: &Element, prefix: &str) -> String {
    let used: Vec<String> = list_devices(domain)
        .iter()
        .filter(|d| d.element.name == "disk")
        .filter_map(|d| d.element.find_attr("target", "dev").map(str::to_owned))
        .collect();
    // vda … vdz, then vdaa … vdzz.
    let letters = (b'a'..=b'z').map(|c| (c as char).to_string());
    let two = (b'a'..=b'z').flat_map(|a| (b'a'..=b'z').map(move |b| format!("{}{}", a as char, b as char)));
    letters
        .chain(two)
        .map(|s| format!("{prefix}{s}"))
        .find(|t| !used.contains(t))
        .unwrap_or_else(|| format!("{prefix}zz"))
}

/// The disk-name prefix for a bus.
pub fn disk_prefix(bus: &str) -> &'static str {
    match bus {
        "virtio" => "vd",
        "ide" => "hd",
        "fdc" => "fd",
        _ => "sd",
    }
}

impl Connection {
    /// The persistent configuration of a domain, including secrets
    /// (graphics passwords), for editing.
    pub fn domain_config_xml(&self, uuid: &str) -> Result<Element> {
        Ok(vmng_xml::parse(&self.domain(uuid)?.get_xml_desc(XML_SECURE | XML_INACTIVE)?)?)
    }

    /// Saves an edited configuration. The UUID must stay the same, so an
    /// edit can never create a second machine.
    pub fn save_domain_config(&self, uuid: &str, xml: &Element) -> Result<()> {
        if xml.name != "domain" {
            return Err(Error::Invalid("The XML must describe a <domain>.".into()));
        }
        match xml.find_text("uuid") {
            Some(u) if u.eq_ignore_ascii_case(uuid) => {}
            _ => return Err(Error::Invalid("The machine's <uuid> can't be changed or removed.".into())),
        }
        let current = self.domain(uuid)?.get_name()?;
        if xml.find_text("name").as_deref() != Some(current.as_str()) {
            return Err(Error::Invalid("Rename the machine with Rename… instead of editing <name>.".into()));
        }
        Domain::define_xml(self.raw(), &xml.to_xml())?;
        Ok(())
    }

    fn affect(&self, dom: &Domain) -> u32 {
        let active = dom.is_active().unwrap_or(false);
        // A transient machine (started without being defined) has no saved
        // configuration to change; asking for one fails the whole change.
        let persistent = dom.is_persistent().unwrap_or(true);
        match (active, persistent) {
            (true, true) => AFFECT_LIVE | AFFECT_CONFIG,
            (true, false) => AFFECT_LIVE,
            (false, _) => AFFECT_CONFIG,
        }
    }

    /// Adds a device. On a running machine it is hot-plugged when possible;
    /// returns `false` if it only takes effect after a restart.
    pub fn add_device(&self, uuid: &str, device: &Element) -> Result<bool> {
        let dom = self.domain(uuid)?;
        let xml = device.to_xml();
        let flags = self.affect(&dom);
        match dom.attach_device_flags(&xml, flags) {
            Ok(_) => Ok(true),
            Err(_) if flags & AFFECT_LIVE != 0 => {
                dom.attach_device_flags(&xml, AFFECT_CONFIG)?;
                Ok(false)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Removes a device (as found in the persistent configuration).
    /// Returns `false` if it stays in the running machine until a restart.
    pub fn remove_device(&self, uuid: &str, device: &Element) -> Result<bool> {
        let dom = self.domain(uuid)?;
        let xml = device.to_xml();
        let flags = self.affect(&dom);
        match dom.detach_device_flags(&xml, flags) {
            Ok(_) => Ok(true),
            Err(_) if flags & AFFECT_LIVE != 0 => {
                dom.detach_device_flags(&xml, AFFECT_CONFIG)?;
                Ok(false)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Changes a device in place, live when the machine runs and the change
    /// allows it (e.g. inserting a CD or unplugging a network cable).
    /// Returns `false` if the running machine only sees it after a restart.
    pub fn update_device(&self, uuid: &str, device: &Element) -> Result<bool> {
        let dom = self.domain(uuid)?;
        let xml = device.to_xml();
        let flags = self.affect(&dom);
        match dom.update_device_flags(&xml, flags) {
            Ok(_) => Ok(true),
            Err(_) if flags & AFFECT_LIVE != 0 => {
                dom.update_device_flags(&xml, AFFECT_CONFIG)?;
                Ok(false)
            }
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = "<domain><name>a</name><uuid>u</uuid><devices>\
        <emulator>/usr/bin/qemu</emulator>\
        <disk type='file' device='disk'><target dev='vda' bus='virtio'/></disk>\
        <disk type='file' device='cdrom'><target dev='sda' bus='sata'/></disk>\
        <controller type='pci' model='pcie-root'/>\
        <input type='mouse' bus='ps2'/><input type='tablet' bus='usb'/>\
        <interface type='network'><source network='default'/></interface>\
        </devices></domain>";

    #[test]
    fn lists_and_classifies_devices() {
        let d = vmng_xml::parse(XML).unwrap();
        let devs = list_devices(&d);
        let kinds: Vec<_> = devs.iter().map(|d| d.kind.clone()).collect();
        assert_eq!(
            kinds,
            [
                DeviceKind::Emulator,
                DeviceKind::Disk,
                DeviceKind::Cdrom,
                DeviceKind::Controller,
                DeviceKind::Input,
                DeviceKind::Input,
                DeviceKind::Interface
            ]
        );
        assert!(!devs[0].kind.removable(&devs[0].element));
        assert!(!devs[3].kind.removable(&devs[3].element));
        assert!(!devs[4].kind.removable(&devs[4].element));
        assert!(devs[5].kind.removable(&devs[5].element));
    }

    #[test]
    fn replaces_by_index_and_finds_targets() {
        let mut d = vmng_xml::parse(XML).unwrap();
        let idx = list_devices(&d).iter().find(|x| x.kind == DeviceKind::Interface).unwrap().index;
        let new = Element::new("interface").with_attr("type", "bridge");
        replace_device(&mut d, idx, new).unwrap();
        assert_eq!(d.find_attr("devices/interface", "type"), Some("bridge"));
        assert!(replace_device(&mut d, 999, Element::new("x")).is_err());
        assert_eq!(next_disk_target(&d, "vd"), "vdb");
        assert_eq!(next_disk_target(&d, "sd"), "sdb");
        assert_eq!(disk_prefix("virtio"), "vd");
    }
}
