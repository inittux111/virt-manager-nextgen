//! Cloning, renaming and deleting domains.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use virt::domain::Domain;
use virt::storage_pool::StoragePool;
use virt::storage_vol::StorageVol;
use vmng_xml::Element;

use crate::connection::Connection;
use crate::error::{Error, Result};

// virDomainXMLFlags
const XML_SECURE: u32 = 1;
const XML_INACTIVE: u32 = 2;
// virDomainUndefineFlagsValues
const UNDEFINE_MANAGED_SAVE: u32 = 1;
const UNDEFINE_SNAPSHOTS_METADATA: u32 = 2;
const UNDEFINE_NVRAM: u32 = 4;
const UNDEFINE_CHECKPOINTS_METADATA: u32 = 16;
// virStorageVolCreateFlags
const VOL_CREATE_REFLINK: u32 = 2;

/// What to do with one disk when cloning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloneDiskMode {
    /// Make a full copy for the new VM.
    Copy,
    /// Let both VMs use the same disk (only safe for read-only media).
    Share,
}

/// Progress of a running clone, in bytes.
pub type ProgressFn<'a> = &'a (dyn Fn(u64, u64) + Sync);

/// Checks a VM name chosen by a person.
pub fn validate_vm_name(name: &str) -> std::result::Result<(), &'static str> {
    if name.is_empty() {
        Err("Enter a name.")
    } else if name.len() > 64 {
        Err("The name is too long (64 characters at most).")
    } else if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+')) {
        Err("Use only letters, digits, dots, dashes, underscores and plus signs.")
    } else if name.starts_with(['.', '-']) {
        Err("The name can't start with a dot or a dash.")
    } else {
        Ok(())
    }
}

/// Picks `base`, `base-2`, `base-3`, … whichever `taken` does not contain.
pub fn unique_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_owned();
    }
    (2..).map(|i| format!("{base}-{i}")).find(|n| !taken(n)).unwrap_or_else(|| base.to_owned())
}

/// Picks `stem.ext`, `stem-2.ext`, `stem-3.ext`, … whichever `taken` does
/// not contain. `ext` includes the dot or is empty.
pub fn unique_file_name(stem: &str, ext: &str, taken: impl Fn(&str) -> bool) -> String {
    (1..)
        .map(|i| if i == 1 { format!("{stem}{ext}") } else { format!("{stem}-{i}{ext}") })
        .find(|n| !taken(n))
        .unwrap_or_else(|| format!("{stem}{ext}"))
}

/// Prepares the new domain's XML: new name, fresh UUID and MAC addresses.
pub(crate) fn clone_domain_xml(source: &Element, new_name: &str) -> Element {
    let mut xml = source.clone();
    xml.child_or_insert("name").set_text(new_name);
    xml.remove_children(|e| e.name == "uuid" || e.name == "title");
    if let Some(devices) = xml.child_mut("devices") {
        for iface in devices.elements_mut().filter(|e| e.name == "interface") {
            // libvirt generates a new MAC when none is given.
            iface.remove_children(|e| e.name == "mac" || e.name == "target");
        }
        for g in devices.elements_mut().filter(|e| e.name == "graphics") {
            if g.attr("autoport") == Some("yes") {
                g.remove_attr("port");
            }
        }
    }
    xml
}

/// The extension of a volume file name, including the dot (or empty).
fn extension(path: &str) -> &str {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.rfind('.').filter(|&i| i > 0).map_or("", |i| &file[i..])
}

impl Connection {
    /// Finds the storage volume backing `path`, refreshing pools once if
    /// libvirt does not know the file yet.
    fn volume_by_path(&self, path: &str) -> Result<StorageVol> {
        if let Ok(v) = StorageVol::lookup_by_path(self.raw(), path) {
            return Ok(v);
        }
        for pool in self.raw().list_all_storage_pools(2 /* ACTIVE */).unwrap_or_default() {
            let _ = pool.refresh(0);
        }
        StorageVol::lookup_by_path(self.raw(), path).map_err(|_| {
            Error::Invalid(format!(
                "{path} is not in a storage pool, so it can't be copied or deleted here. Add its folder as a storage pool first."
            ))
        })
    }

    /// Copies a volume into the same pool under a new name.
    fn copy_volume(&self, source: &StorageVol, new_name: &str, progress: &(dyn Fn(u64) + Sync)) -> Result<StorageVol> {
        let pool = StoragePool::lookup_by_volume(source)?;
        let src_xml = vmng_xml::parse(&source.get_xml_desc(0)?)?;
        let mut vol = Element::new("volume").with_child(Element::new("name").with_text(new_name));
        if let Some(cap) = src_xml.child("capacity") {
            vol.push(cap.clone());
        }
        let mut target = Element::new("target");
        if let Some(format) = src_xml.find("target/format") {
            target.push(format.clone());
        }
        vol.push(target);
        let xml = vol.to_xml();

        let done = AtomicBool::new(false);
        let result = std::thread::scope(|scope| {
            // Report progress by watching the new volume grow.
            scope.spawn(|| {
                while !done.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(700));
                    if let Ok(v) = StorageVol::lookup_by_name(&pool, new_name)
                        && let Ok(info) = v.get_info()
                    {
                        progress(info.allocation);
                    }
                }
            });
            // A reflink copy is instant on btrfs/XFS; fall back to a full copy.
            let r = StorageVol::create_xml_from(&pool, &xml, source, VOL_CREATE_REFLINK)
                .or_else(|_| StorageVol::create_xml_from(&pool, &xml, source, 0));
            done.store(true, Ordering::Relaxed);
            r
        });
        Ok(result?)
    }

    /// Clones a shut-off domain. `disks` lists, by target (e.g. `vda`), how
    /// each writable disk is handled; disks not listed are shared.
    /// Returns the new domain's UUID.
    pub fn clone_domain(
        &self,
        uuid: &str,
        new_name: &str,
        disks: &[(String, CloneDiskMode)],
        progress: ProgressFn<'_>,
    ) -> Result<String> {
        validate_vm_name(new_name).map_err(|m| Error::Invalid(m.to_owned()))?;
        if Domain::lookup_by_name(self.raw(), new_name).is_ok() {
            return Err(Error::Invalid(format!("A machine called {new_name} already exists.")));
        }
        let dom = self.domain(uuid)?;
        if dom.is_active()? {
            return Err(Error::Invalid("Shut the machine down before cloning it.".into()));
        }
        let source = vmng_xml::parse(&dom.get_xml_desc(XML_SECURE | XML_INACTIVE)?)?;
        let mut xml = clone_domain_xml(&source, new_name);

        // Work out what to copy and how much data that is.
        let mut copies: Vec<(usize, StorageVol, String)> = Vec::new();
        let device_disks: Vec<usize> = xml
            .child("devices")
            .map(|d| {
                d.children
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| matches!(n, vmng_xml::Node::Element(e) if e.name == "disk"))
                    .map(|(i, _)| i)
                    .collect()
            })
            .unwrap_or_default();
        let devices = xml.child("devices").cloned().unwrap_or_default();
        for &i in &device_disks {
            let vmng_xml::Node::Element(disk) = &devices.children[i] else { continue };
            let target = disk.find_attr("target", "dev").unwrap_or_default();
            let mode = disks.iter().find(|(t, _)| t == target).map_or(CloneDiskMode::Share, |(_, m)| *m);
            if mode == CloneDiskMode::Share {
                continue;
            }
            let path = match disk.attr("type") {
                Some("volume") => {
                    let pool = disk.find_attr("source", "pool").unwrap_or_default();
                    let vol = disk.find_attr("source", "volume").unwrap_or_default();
                    StorageVol::lookup_by_name(&StoragePool::lookup_by_name(self.raw(), pool)?, vol)?.get_path()?
                }
                _ => match disk.find_attr("source", "file").or_else(|| disk.find_attr("source", "dev")) {
                    Some(p) => p.to_owned(),
                    None => continue,
                },
            };
            copies.push((i, self.volume_by_path(&path)?, path));
        }
        let nvram = xml.find_text("os/nvram").filter(|p| !p.is_empty());
        let nvram_vol = nvram.as_deref().and_then(|p| self.volume_by_path(p).ok());

        let total: u64 = copies.iter().filter_map(|(_, v, _)| v.get_info().ok()).map(|i| i.allocation).sum();
        let base_done = std::sync::atomic::AtomicU64::new(0);
        progress(0, total);

        let mut created: Vec<StorageVol> = Vec::new();
        let result = (|| -> Result<String> {
            let single = copies.len() == 1;
            for (i, vol, path) in &copies {
                let pool = StoragePool::lookup_by_volume(vol)?;
                let taken = |n: &str| StorageVol::lookup_by_name(&pool, n).is_ok();
                let vmng_xml::Node::Element(disk) = &devices.children[*i] else { continue };
                let target = disk.find_attr("target", "dev").unwrap_or("disk");
                let stem = if single { new_name.to_owned() } else { format!("{new_name}-{target}") };
                let name = unique_file_name(&stem, extension(path), taken);
                let before = base_done.load(Ordering::Relaxed);
                let report = |bytes: u64| progress((before + bytes).min(total), total);
                let new_vol = self.copy_volume(vol, &name, &report)?;
                base_done.fetch_add(vol.get_info().map(|i| i.allocation).unwrap_or(0), Ordering::Relaxed);
                progress(base_done.load(Ordering::Relaxed).min(total), total);
                let new_path = new_vol.get_path()?;
                created.push(new_vol);

                let Some(vmng_xml::Node::Element(d)) = xml.child_mut("devices").and_then(|d| d.children.get_mut(*i))
                else {
                    continue;
                };
                if d.attr("type") == Some("volume") {
                    d.child_or_insert("source").set_attr("volume", &name);
                } else {
                    let src = d.child_or_insert("source");
                    if src.attr("dev").is_some() {
                        src.set_attr("dev", &new_path);
                    } else {
                        src.set_attr("file", &new_path);
                    }
                }
            }

            // Give the clone its own copy of the UEFI variables, or let
            // libvirt create a fresh store from the template.
            if let Some(path) = &nvram {
                let new_path = match &nvram_vol {
                    Some(vol) => {
                        let pool = StoragePool::lookup_by_volume(vol)?;
                        let taken = |n: &str| StorageVol::lookup_by_name(&pool, n).is_ok();
                        let name = unique_file_name(&format!("{new_name}_VARS"), extension(path), taken);
                        let v = self.copy_volume(vol, &name, &|_| {})?;
                        let p = v.get_path()?;
                        created.push(v);
                        Some(p)
                    }
                    None => None,
                };
                if let Some(nv) = xml.find_mut("os/nvram") {
                    match new_path {
                        Some(p) => nv.set_text(p),
                        None => nv.children.clear(),
                    }
                }
            }

            let new = Domain::define_xml(self.raw(), &xml.to_xml())?;
            Ok(new.get_uuid_string()?)
        })();

        if result.is_err() {
            // Roll back: never leave orphaned copies behind.
            for v in &created {
                let _ = v.delete(0);
            }
        }
        result
    }

    /// Renames a shut-off domain.
    pub fn rename_domain(&self, uuid: &str, new_name: &str) -> Result<()> {
        validate_vm_name(new_name).map_err(|m| Error::Invalid(m.to_owned()))?;
        self.domain(uuid)?.rename(new_name, 0)?;
        Ok(())
    }

    /// Discards a saved (managed save) state, so the next start boots fresh.
    pub fn discard_saved_state(&self, uuid: &str) -> Result<()> {
        self.domain(uuid)?.managed_save_remove(0)?;
        Ok(())
    }

    /// Deletes a domain: stops it if needed, removes its definition with
    /// saved state, snapshot metadata and UEFI variables, then deletes the
    /// listed disk images. Returns the images that could not be deleted.
    pub fn delete_domain(&self, uuid: &str, delete_paths: &[String]) -> Result<Vec<(String, Error)>> {
        let dom = self.domain(uuid)?;
        if dom.is_active()? {
            dom.destroy()?;
        }
        if dom.is_persistent().unwrap_or(true) {
            let flags =
                UNDEFINE_MANAGED_SAVE | UNDEFINE_SNAPSHOTS_METADATA | UNDEFINE_NVRAM | UNDEFINE_CHECKPOINTS_METADATA;
            dom.undefine_flags(flags)?;
        }
        let mut failed = Vec::new();
        for path in delete_paths {
            if let Err(e) = self.volume_by_path(path).and_then(|v| v.delete(0).map_err(Error::from)) {
                failed.push((path.clone(), e));
            }
        }
        Ok(failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_xml_resets_identity() {
        let src = vmng_xml::parse(
            "<domain><name>a</name><uuid>u</uuid><title>A</title><devices>\
             <interface type='network'><mac address='52:54:00:00:00:01'/><source network='default'/></interface>\
             <graphics type='spice' autoport='yes' port='5900'/></devices></domain>",
        )
        .unwrap();
        let c = clone_domain_xml(&src, "b");
        assert_eq!(c.find_text("name").as_deref(), Some("b"));
        assert!(c.child("uuid").is_none() && c.child("title").is_none());
        assert!(c.find("devices/interface/mac").is_none());
        assert_eq!(c.find_attr("devices/interface/source", "network"), Some("default"));
        assert_eq!(c.find_attr("devices/graphics", "port"), None);
    }

    #[test]
    fn names() {
        assert!(validate_vm_name("debian-build_2.0").is_ok());
        for bad in ["", "a b", "-x", ".x", "a/b", &"x".repeat(65)] {
            assert!(validate_vm_name(bad).is_err(), "{bad:?}");
        }
        assert_eq!(unique_name("a", |n| n == "a" || n == "a-2"), "a-3");
        assert_eq!(unique_name("a", |_| false), "a");
        assert_eq!(unique_file_name("vm", ".qcow2", |n| n == "vm.qcow2"), "vm-2.qcow2");
    }

    #[test]
    fn extensions() {
        assert_eq!(extension("/x/fedora.qcow2"), ".qcow2");
        assert_eq!(extension("/x/disk"), "");
        assert_eq!(extension("/x.d/.hidden"), "");
    }
}
