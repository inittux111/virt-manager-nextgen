//! Storage pools and volumes.

use virt::storage_pool::StoragePool;
use virt::storage_vol::StorageVol;
use vmng_xml::Element;

use crate::connection::Connection;
use crate::error::{Error, Result};

/// The kinds of pool the app can create.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolKind {
    /// A folder on this host (`dir`).
    Folder,
    /// An existing LVM volume group (`logical`).
    Lvm,
    /// An NFS export mounted by libvirt (`netfs`).
    Nfs,
    /// An iSCSI target (`iscsi`).
    Iscsi,
    Other,
}

impl PoolKind {
    pub fn from_type(t: &str) -> Self {
        match t {
            "dir" => Self::Folder,
            "logical" => Self::Lvm,
            "netfs" => Self::Nfs,
            "iscsi" | "iscsi-direct" => Self::Iscsi,
            _ => Self::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Folder => "Folder",
            Self::Lvm => "LVM volume group",
            Self::Nfs => "Network share (NFS)",
            Self::Iscsi => "iSCSI",
            Self::Other => "Other",
        }
    }
}

/// Everything shown about a pool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolDetails {
    pub name: String,
    pub kind: PoolKind,
    pub type_name: String,
    pub active: bool,
    pub autostart: bool,
    pub persistent: bool,
    /// Where the volumes appear on the host.
    pub path: Option<String>,
    /// Remote host (NFS/iSCSI), export or volume group.
    pub source: Option<String>,
    pub capacity: u64,
    pub allocation: u64,
    pub available: u64,
}

/// One volume (disk image, ISO, …) in a pool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolumeInfo {
    pub name: String,
    pub path: String,
    pub capacity: u64,
    pub allocation: u64,
    pub format: Option<String>,
}

/// What a person fills in to create a pool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewPool {
    pub name: String,
    pub kind: PoolKind,
    /// Folder: the directory. NFS: where to mount it.
    pub path: String,
    /// NFS/iSCSI: the server.
    pub host: String,
    /// NFS: the exported directory. LVM: volume group. iSCSI: target IQN.
    pub source: String,
}

/// Builds the pool XML for a new pool.
pub fn pool_xml(p: &NewPool) -> std::result::Result<Element, &'static str> {
    let name = p.name.trim();
    if name.is_empty() || name.contains(['/', ' ']) {
        return Err("The name can't be empty or contain spaces or slashes.");
    }
    let need = |v: &str, msg: &'static str| if v.trim().is_empty() { Err(msg) } else { Ok(v.trim().to_owned()) };
    let mut pool = Element::new("pool").with_child(Element::new("name").with_text(name));
    let mut source = Element::new("source");
    let target_path: Option<String> = match p.kind {
        PoolKind::Folder => Some(need(&p.path, "Choose the folder.")?),
        PoolKind::Lvm => {
            let vg = need(&p.source, "Enter the volume group name.")?;
            source.push(Element::new("name").with_text(&vg));
            source.push(Element::new("format").with_attr("type", "lvm2"));
            Some(format!("/dev/{vg}"))
        }
        PoolKind::Nfs => {
            source.push(Element::new("host").with_attr("name", need(&p.host, "Enter the server.")?));
            source.push(
                Element::new("dir").with_attr("path", need(&p.source, "Enter the shared folder on the server.")?),
            );
            source.push(Element::new("format").with_attr("type", "nfs"));
            Some(need(&p.path, "Choose where to mount the share.")?)
        }
        PoolKind::Iscsi => {
            source.push(Element::new("host").with_attr("name", need(&p.host, "Enter the server.")?));
            source.push(Element::new("device").with_attr("path", need(&p.source, "Enter the target (IQN).")?));
            Some("/dev/disk/by-path".to_owned())
        }
        PoolKind::Other => return Err("This kind of pool can't be created here."),
    };
    let kind = match p.kind {
        PoolKind::Folder => "dir",
        PoolKind::Lvm => "logical",
        PoolKind::Nfs => "netfs",
        PoolKind::Iscsi => "iscsi",
        PoolKind::Other => unreachable!(),
    };
    pool.set_attr("type", kind);
    if !source.children.is_empty() {
        pool.push(source);
    }
    if let Some(path) = target_path {
        pool.push(Element::new("target").with_child(Element::new("path").with_text(path)));
    }
    Ok(pool)
}

fn details(pool: &StoragePool) -> Result<PoolDetails> {
    let xml = vmng_xml::parse(&pool.get_xml_desc(0)?)?;
    let info = pool.get_info()?;
    let type_name = xml.attr("type").unwrap_or_default().to_owned();
    let src = xml.child("source");
    let source = src.and_then(|s| {
        let host = s.find_attr("host", "name");
        let what = s
            .find_attr("dir", "path")
            .or_else(|| s.find_attr("device", "path"))
            .map(str::to_owned)
            .or_else(|| s.find_text("name"));
        match (host, what) {
            (Some(h), Some(w)) => Some(format!("{h}:{w}")),
            (Some(h), None) => Some(h.to_owned()),
            (None, w) => w.filter(|w| !w.is_empty()),
        }
    });
    Ok(PoolDetails {
        name: pool.get_name()?,
        kind: PoolKind::from_type(&type_name),
        type_name,
        active: pool.is_active().unwrap_or(false),
        autostart: pool.get_autostart().unwrap_or(false),
        persistent: pool.is_persistent().unwrap_or(true),
        path: xml.find_text("target/path"),
        source,
        capacity: info.capacity,
        allocation: info.allocation,
        available: info.available,
    })
}

impl Connection {
    fn pool(&self, name: &str) -> Result<StoragePool> {
        StoragePool::lookup_by_name(self.raw(), name).map_err(Error::from)
    }

    pub fn pool_details(&self) -> Result<Vec<PoolDetails>> {
        let mut out: Vec<PoolDetails> =
            self.raw().list_all_storage_pools(0)?.iter().filter_map(|p| details(p).ok()).collect();
        out.sort_by(|a, b| (a.name != "default", &a.name).cmp(&(b.name != "default", &b.name)));
        Ok(out)
    }

    /// The volumes of a pool, refreshed first so new files show up.
    pub fn list_volumes(&self, pool: &str) -> Result<Vec<VolumeInfo>> {
        let pool = self.pool(pool)?;
        if !pool.is_active().unwrap_or(false) {
            return Ok(Vec::new());
        }
        let _ = pool.refresh(0);
        let mut out = Vec::new();
        for vol in pool.list_all_volumes(0)? {
            let Ok(info) = vol.get_info() else { continue };
            let format = vol
                .get_xml_desc(0)
                .ok()
                .and_then(|x| vmng_xml::parse(&x).ok())
                .and_then(|x| x.find_attr("target/format", "type").map(str::to_owned));
            out.push(VolumeInfo {
                name: vol.get_name()?,
                path: vol.get_path()?,
                capacity: info.capacity,
                allocation: info.allocation,
                format,
            });
        }
        out.sort_by_key(|v| v.name.to_lowercase());
        Ok(out)
    }

    /// Creates a pool: defines it, prepares its storage (e.g. creates the
    /// folder), starts it and turns on autostart.
    pub fn create_pool(&self, spec: &NewPool) -> Result<()> {
        let xml = pool_xml(spec).map_err(|m| Error::Invalid(m.into()))?;
        let pool = StoragePool::define_xml(self.raw(), &xml.to_xml(), 0)?;
        // Building only prepares what's missing (e.g. mkdir); LVM groups
        // and remote shares must already exist.
        if matches!(spec.kind, PoolKind::Folder | PoolKind::Nfs) {
            let _ = pool.build(0);
        }
        if let Err(e) = pool.create(0) {
            let _ = pool.undefine();
            return Err(e.into());
        }
        pool.set_autostart(true)?;
        Ok(())
    }

    pub fn set_pool_active(&self, name: &str, on: bool) -> Result<()> {
        let pool = self.pool(name)?;
        if on { pool.create(0).map(drop) } else { pool.destroy() }.map_err(Error::from)
    }

    pub fn set_pool_autostart(&self, name: &str, on: bool) -> Result<()> {
        self.pool(name)?.set_autostart(on)?;
        Ok(())
    }

    pub fn refresh_pool(&self, name: &str) -> Result<()> {
        self.pool(name)?.refresh(0)?;
        Ok(())
    }

    /// Stops and forgets a pool. Its files are left alone.
    pub fn remove_pool(&self, name: &str) -> Result<()> {
        let pool = self.pool(name)?;
        if pool.is_active().unwrap_or(false) {
            pool.destroy()?;
        }
        pool.undefine()?;
        Ok(())
    }

    /// Creates an empty volume with exactly this name.
    pub fn create_named_volume(&self, pool: &str, name: &str, size: u64, format: &str) -> Result<String> {
        if name.trim().is_empty() || name.contains('/') {
            return Err(Error::Invalid("The name can't be empty or contain slashes.".into()));
        }
        if !self.volume_name_free(pool, name) {
            return Err(Error::Invalid(format!("{name} already exists in this pool.")));
        }
        self.create_volume(pool, name, size, format)
    }

    /// Deletes a volume (its file or logical volume is removed).
    pub fn delete_volume(&self, pool: &str, name: &str) -> Result<()> {
        StorageVol::lookup_by_name(&self.pool(pool)?, name)?.delete(0)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: PoolKind) -> NewPool {
        NewPool { name: "p".into(), kind, path: "/srv/vms".into(), host: "nas".into(), source: "/export/vms".into() }
    }

    #[test]
    fn builds_pool_xml() {
        let dir = pool_xml(&spec(PoolKind::Folder)).unwrap();
        assert_eq!(dir.attr("type"), Some("dir"));
        assert_eq!(dir.find_text("target/path").as_deref(), Some("/srv/vms"));
        assert!(dir.child("source").is_none());

        let nfs = pool_xml(&spec(PoolKind::Nfs)).unwrap();
        assert_eq!(nfs.find_attr("source/host", "name"), Some("nas"));
        assert_eq!(nfs.find_attr("source/dir", "path"), Some("/export/vms"));

        let mut lvm = spec(PoolKind::Lvm);
        lvm.source = "vg0".into();
        assert_eq!(pool_xml(&lvm).unwrap().find_text("target/path").as_deref(), Some("/dev/vg0"));

        let mut bad = spec(PoolKind::Folder);
        bad.path.clear();
        assert!(pool_xml(&bad).is_err());
        bad.name = "a b".into();
        assert!(pool_xml(&bad).is_err());
    }
}
