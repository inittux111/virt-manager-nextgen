//! What the host offers: guest architectures, machine types, firmware,
//! networks, bridges, storage pools and install images.

use virt::storage_pool::StoragePool;
use vmng_xml::Element;

use crate::connection::{Connection, PoolSummary};
use crate::error::Result;

/// One guest architecture the hypervisor can run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuestArch {
    pub arch: String,
    pub emulator: Option<String>,
    /// Machine types and aliases (e.g. `q35`, `pc`), deprecated ones left out.
    pub machines: Vec<String>,
    /// Whether hardware virtualization (KVM) is available.
    pub kvm: bool,
}

/// Host capabilities relevant to creating VMs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostCaps {
    pub host_arch: String,
    pub guests: Vec<GuestArch>,
}

impl HostCaps {
    pub fn guest(&self, arch: &str) -> Option<&GuestArch> {
        self.guests.iter().find(|g| g.arch == arch)
    }

    pub(crate) fn from_xml(xml: &Element) -> Self {
        let mut guests: Vec<GuestArch> = Vec::new();
        for guest in xml.children_named("guest").filter(|g| g.find_text("os_type").as_deref() == Some("hvm")) {
            let Some(arch) = guest.child("arch") else { continue };
            let name = arch.attr("name").unwrap_or_default().to_owned();
            let machines = arch
                .children_named("machine")
                .filter(|m| m.attr("deprecated") != Some("yes"))
                .map(Element::text)
                .filter(|m| !m.starts_with("xen") && m != "x-remote" && m != "none")
                .collect();
            let kvm = arch.children_named("domain").any(|d| d.attr("type") == Some("kvm"));
            guests.push(GuestArch { arch: name, emulator: arch.find_text("emulator"), machines, kvm });
        }
        Self { host_arch: xml.find_text("host/cpu/arch").unwrap_or_default(), guests }
    }
}

/// What a specific arch/machine/virt-type combination supports.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DomainCaps {
    /// The canonical machine type, e.g. `pc-q35-10.2`.
    pub machine: String,
    pub uefi: bool,
    pub secure_boot: bool,
    /// UEFI firmware files that can be chosen by path (virt-manager lists
    /// them as "UEFI x86_64: /usr/share/…").
    pub loaders: Vec<String>,
    pub tpm: bool,
    pub video_models: Vec<String>,
    pub graphics_types: Vec<String>,
}

fn enum_values(el: Option<&Element>, name: &str) -> Vec<String> {
    el.and_then(|e| e.children_named("enum").find(|en| en.attr("name") == Some(name)))
        .map(|en| en.children_named("value").map(Element::text).collect())
        .unwrap_or_default()
}

impl DomainCaps {
    pub(crate) fn from_xml(xml: &Element) -> Self {
        let os = xml.child("os");
        let loader = os.and_then(|o| o.child("loader"));
        let firmware = enum_values(os, "firmware");
        let tpm = xml.find("devices/tpm");
        Self {
            machine: xml.find_text("machine").unwrap_or_default(),
            uefi: firmware.iter().any(|f| f == "efi"),
            secure_boot: enum_values(loader, "secure").iter().any(|v| v == "yes"),
            loaders: loader
                .filter(|l| l.attr("supported") == Some("yes"))
                .map(|l| l.children_named("value").map(Element::text).filter(|v| v.starts_with('/')).collect())
                .unwrap_or_default(),
            tpm: tpm.is_some_and(|t| t.attr("supported") == Some("yes"))
                && enum_values(tpm, "backendModel").iter().any(|b| b == "emulator"),
            video_models: enum_values(xml.find("devices/video"), "modelType"),
            graphics_types: enum_values(xml.find("devices/graphics"), "type"),
        }
    }
}

/// A libvirt virtual network.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkSummary {
    pub name: String,
    pub active: bool,
    pub autostart: bool,
    /// `nat`, `route`, `bridge`, `open`, … or `isolated` when there is no forward.
    pub forward: String,
    pub bridge: Option<String>,
}

/// An installer image found in a storage pool.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstallImage {
    pub name: String,
    pub path: String,
    pub pool: String,
    pub size: u64,
}

impl Connection {
    pub fn host_caps(&self) -> Result<HostCaps> {
        Ok(HostCaps::from_xml(&vmng_xml::parse(&self.raw().get_capabilities()?)?))
    }

    pub fn domain_caps(&self, arch: &str, machine: Option<&str>, virt_type: &str) -> Result<DomainCaps> {
        let xml = self.raw().get_domain_capabilities(None, Some(arch), machine, Some(virt_type), 0)?;
        Ok(DomainCaps::from_xml(&vmng_xml::parse(&xml)?))
    }

    pub fn list_networks(&self) -> Result<Vec<NetworkSummary>> {
        let mut out = Vec::new();
        for net in self.raw().list_all_networks(0)? {
            let xml = vmng_xml::parse(&net.get_xml_desc(0)?)?;
            out.push(NetworkSummary {
                name: net.get_name()?,
                active: net.is_active().unwrap_or(false),
                autostart: net.get_autostart().unwrap_or(false),
                forward: xml.find_attr("forward", "mode").unwrap_or("isolated").to_owned(),
                bridge: xml.find_attr("bridge", "name").map(str::to_owned),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Active host bridges VMs can be attached to directly (e.g. `br0`).
    /// Empty if the host does not expose interfaces to libvirt.
    pub fn list_bridges(&self) -> Vec<String> {
        const ACTIVE: u32 = 2;
        let mut out: Vec<String> = self
            .raw()
            .list_all_interfaces(ACTIVE)
            .unwrap_or_default()
            .iter()
            .filter_map(|i| {
                let xml = vmng_xml::parse(&i.get_xml_desc(0).ok()?).ok()?;
                (xml.attr("type") == Some("bridge")).then(|| xml.attr("name").unwrap_or_default().to_owned())
            })
            .filter(|n| !n.is_empty() && !n.starts_with("virbr"))
            .collect();
        out.sort();
        out
    }

    /// Every storage pool with its size and target directory.
    pub fn list_pools(&self) -> Result<Vec<(PoolSummary, Option<String>)>> {
        let mut out = Vec::new();
        for pool in self.raw().list_all_storage_pools(0)? {
            let name = pool.get_name()?;
            let info = pool.get_info()?;
            let path = vmng_xml::parse(&pool.get_xml_desc(0)?).ok().and_then(|x| x.find_text("target/path"));
            out.push((
                PoolSummary {
                    name,
                    active: pool.is_active().unwrap_or(false),
                    capacity: info.capacity,
                    allocation: info.allocation,
                    available: info.available,
                },
                path,
            ));
        }
        out.sort_by(|a, b| a.0.name.cmp(&b.0.name));
        Ok(out)
    }

    /// ISO images in active storage pools, sorted by name.
    pub fn list_install_images(&self) -> Result<Vec<InstallImage>> {
        const ACTIVE: u32 = 2;
        let mut out = Vec::new();
        for pool in self.raw().list_all_storage_pools(ACTIVE)? {
            let pool_name = pool.get_name()?;
            let _ = pool.refresh(0);
            for vol in pool.list_all_volumes(0).unwrap_or_default() {
                let Ok(name) = vol.get_name() else { continue };
                if !name.to_ascii_lowercase().ends_with(".iso") {
                    continue;
                }
                let Ok(path) = vol.get_path() else { continue };
                // Pools pointing at the same folder list the same file twice.
                if out.iter().any(|i: &InstallImage| i.path == path) {
                    continue;
                }
                let size = vol.get_info().map(|i| i.capacity).unwrap_or(0);
                out.push(InstallImage { name, path, pool: pool_name.clone(), size });
            }
        }
        out.sort_by_key(|i| i.name.to_lowercase());
        Ok(out)
    }

    /// Creates an empty disk image and returns its path.
    pub(crate) fn create_volume(&self, pool: &str, name: &str, capacity: u64, format: &str) -> Result<String> {
        let pool = StoragePool::lookup_by_name(self.raw(), pool)?;
        let xml = Element::new("volume")
            .with_child(Element::new("name").with_text(name))
            .with_child(Element::new("capacity").with_attr("unit", "bytes").with_text(capacity.to_string()))
            // Thin provisioning: qcow2 grows as the guest writes.
            .with_child(Element::new("allocation").with_attr("unit", "bytes").with_text("0"))
            .with_child(Element::new("target").with_child(Element::new("format").with_attr("type", format)));
        let vol = virt::storage_vol::StorageVol::create_xml(&pool, &xml.to_xml(), 0)?;
        Ok(vol.get_path()?)
    }

    /// Creates an empty disk image named after `base` (made unique in the
    /// pool, e.g. `fedora-2.qcow2`) and returns its path.
    pub fn create_disk(&self, pool: &str, base: &str, size: u64, format: &str) -> Result<String> {
        let ext = if format == "qcow2" { ".qcow2" } else { ".img" };
        let name = crate::manage::unique_file_name(base, ext, |n| !self.volume_name_free(pool, n));
        self.create_volume(pool, &name, size, format)
    }

    /// Deletes the storage volume at `path` (used to roll back).
    pub fn delete_volume_at(&self, path: &str) -> Result<()> {
        virt::storage_vol::StorageVol::lookup_by_path(self.raw(), path)?.delete(0)?;
        Ok(())
    }

    /// Whether a volume name is free in a pool.
    pub(crate) fn volume_name_free(&self, pool: &str, name: &str) -> bool {
        StoragePool::lookup_by_name(self.raw(), pool)
            .map(|p| virt::storage_vol::StorageVol::lookup_by_name(&p, name).is_err())
            .unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_capabilities() {
        let xml = vmng_xml::parse(
            "<capabilities><host><cpu><arch>x86_64</arch></cpu></host>\
             <guest><os_type>hvm</os_type><arch name='x86_64'><emulator>/usr/bin/qemu-system-x86_64</emulator>\
             <machine>pc-q35-10.2</machine><machine canonical='pc-q35-10.2'>q35</machine>\
             <machine deprecated='yes'>pc-q35-5.2</machine><machine>xenfv</machine>\
             <domain type='qemu'/><domain type='kvm'/></arch></guest>\
             <guest><os_type>hvm</os_type><arch name='aarch64'><machine>virt</machine><domain type='qemu'/></arch></guest>\
             </capabilities>",
        )
        .unwrap();
        let caps = HostCaps::from_xml(&xml);
        assert_eq!(caps.host_arch, "x86_64");
        let x86 = caps.guest("x86_64").unwrap();
        assert!(x86.kvm);
        assert_eq!(x86.machines, ["pc-q35-10.2", "q35"]);
        assert!(!caps.guest("aarch64").unwrap().kvm);
    }

    #[test]
    fn parses_domain_capabilities() {
        let xml = vmng_xml::parse(
            "<domainCapabilities><machine>pc-q35-10.2</machine>\
             <os supported='yes'><enum name='firmware'><value>efi</value></enum>\
             <loader supported='yes'><enum name='secure'><value>yes</value><value>no</value></enum></loader></os>\
             <devices><video supported='yes'><enum name='modelType'><value>vga</value><value>virtio</value></enum></video>\
             <graphics supported='yes'><enum name='type'><value>vnc</value><value>spice</value></enum></graphics>\
             <tpm supported='yes'><enum name='backendModel'><value>emulator</value></enum></tpm></devices>\
             </domainCapabilities>",
        )
        .unwrap();
        let c = DomainCaps::from_xml(&xml);
        assert!(c.uefi && c.secure_boot && c.tpm);
        assert_eq!(c.machine, "pc-q35-10.2");
        assert!(c.video_models.contains(&"virtio".to_owned()));
        assert!(c.graphics_types.contains(&"spice".to_owned()));
    }
}
