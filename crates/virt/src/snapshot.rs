//! Domain snapshots.

use virt::domain_snapshot::DomainSnapshot;
use vmng_xml::Element;

use crate::config::DomainConfig;
use crate::connection::Connection;
use crate::error::{Error, Result};

// virDomainSnapshotCreateFlags
const CREATE_REDEFINE: u32 = 1;
const CREATE_CURRENT: u32 = 2;
const CREATE_DISK_ONLY: u32 = 16;
const CREATE_ATOMIC: u32 = 128;
// virDomainSnapshotXMLFlags
const XML_SECURE: u32 = 1;

/// Where a snapshot's data lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotLocation {
    /// Inside the qcow2 disk images.
    Internal,
    /// In separate overlay files next to the disks.
    External,
}

/// What a snapshot captured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotInfo {
    pub name: String,
    pub description: Option<String>,
    /// Unix time of creation.
    pub created: i64,
    /// The VM state when the snapshot was taken (`running`, `shutoff`, …).
    pub state: String,
    pub parent: Option<String>,
    /// Whether this is the snapshot the VM currently derives from.
    pub is_current: bool,
    pub children: u32,
    pub location: SnapshotLocation,
}

impl SnapshotInfo {
    /// Whether restoring brings back a running machine (memory was saved).
    pub fn has_memory(&self) -> bool {
        matches!(self.state.as_str(), "running" | "paused")
    }

    fn from_xml(xml: &Element, is_current: bool, children: u32) -> Self {
        let external = xml
            .child("disks")
            .is_some_and(|d| d.children_named("disk").any(|disk| disk.attr("snapshot") == Some("external")))
            || xml.find_attr("memory", "snapshot") == Some("external");
        Self {
            name: xml.find_text("name").unwrap_or_default(),
            description: xml.find_text("description").filter(|d| !d.is_empty()),
            created: xml.find_text("creationTime").and_then(|t| t.parse().ok()).unwrap_or(0),
            state: xml.find_text("state").unwrap_or_default(),
            parent: xml.find_text("parent/name"),
            is_current,
            children,
            location: if external { SnapshotLocation::External } else { SnapshotLocation::Internal },
        }
    }
}

/// Checks a snapshot name chosen by a person.
pub fn validate_snapshot_name(name: &str) -> std::result::Result<(), &'static str> {
    if name.trim().is_empty() {
        Err("Enter a name.")
    } else if name.len() > 100 {
        Err("The name is too long.")
    } else if name.contains(['/', '\\', '\0']) || name.chars().any(char::is_control) {
        Err("The name can't contain slashes or control characters.")
    } else if name.starts_with('.') || name.starts_with('-') {
        Err("The name can't start with a dot or a dash.")
    } else {
        Ok(())
    }
}

/// Checks the name for how the snapshot is taken: QEMU names its job for
/// an internal snapshot of a running machine after the snapshot, and job
/// names may only use letters, digits, `-`, `.` and `_`.
pub fn validate_snapshot_name_for(
    name: &str,
    running: bool,
    location: SnapshotLocation,
) -> std::result::Result<(), &'static str> {
    validate_snapshot_name(name)?;
    if running
        && location == SnapshotLocation::Internal
        && !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
    {
        return Err("For a running machine stored inside the disk, use only letters, digits, - . and _ (no spaces).");
    }
    Ok(())
}

/// How to take a snapshot (virt-manager's "Snapshot Mode" and memory
/// location).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotOptions {
    pub location: SnapshotLocation,
    /// For an external snapshot of a running machine: the folder for the
    /// memory file; `None` puts it next to the first disk.
    pub memory_dir: Option<String>,
}

/// What a machine allows for a new snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotModes {
    /// Whether the data can be stored inside the disk images.
    pub internal: bool,
    /// The suggested choice.
    pub default: SnapshotLocation,
}

/// Decides whether a snapshot can be stored inside the disk images.
///
/// Internal snapshots need every snapshotted disk to be qcow2 and, with
/// UEFI, a qcow2 variable store. Everything else uses external overlays.
pub(crate) fn snapshot_location(config: &DomainConfig) -> SnapshotLocation {
    let disks_ok =
        config.disks.iter().filter(|d| !d.is_cdrom() && !d.read_only).all(|d| d.format.as_deref() == Some("qcow2"));
    let nvram_ok = config.nvram_path.is_none() || config.nvram_format.as_deref() == Some("qcow2");
    if disks_ok && nvram_ok { SnapshotLocation::Internal } else { SnapshotLocation::External }
}

/// Builds the `<domainsnapshot>` request.
pub(crate) fn snapshot_xml(
    config: &DomainConfig,
    name: &str,
    description: &str,
    running: bool,
    location: SnapshotLocation,
    memory_dir: Option<&str>,
) -> String {
    let mut xml = Element::new("domainsnapshot").with_child(Element::new("name").with_text(name));
    if !description.trim().is_empty() {
        xml.push(Element::new("description").with_text(description.trim()));
    }
    if location == SnapshotLocation::External {
        if running {
            // Memory goes next to the first disk, where libvirt can write.
            let dir = memory_dir.map(|d| d.trim_end_matches('/').to_owned()).unwrap_or_else(|| {
                config
                    .disks
                    .iter()
                    .find_map(|d| d.source_path.as_deref())
                    .and_then(|p| std::path::Path::new(p).parent())
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "/var/lib/libvirt/qemu/snapshot".to_owned())
            });
            let file = format!("{dir}/{}-{}.mem", config.name, sanitize(name));
            xml.push(Element::new("memory").with_attr("snapshot", "external").with_attr("file", file));
        } else {
            xml.push(Element::new("memory").with_attr("snapshot", "no"));
        }
        let disks = xml.push(Element::new("disks"));
        for d in &config.disks {
            let mode = if d.is_cdrom() || d.read_only { "no" } else { "external" };
            disks.push(Element::new("disk").with_attr("name", &d.target).with_attr("snapshot", mode));
        }
    }
    xml.to_xml()
}

/// Keeps file names derived from snapshot names simple.
fn sanitize(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect()
}

impl Connection {
    /// All snapshots of a domain, newest first.
    pub fn list_snapshots(&self, uuid: &str) -> Result<Vec<SnapshotInfo>> {
        let dom = self.domain(uuid)?;
        let mut out = Vec::new();
        for snap in dom.list_all_snapshots(0)? {
            let xml = vmng_xml::parse(&snap.get_xml_desc(0)?)?;
            let current = snap.is_current(0).unwrap_or(false);
            let children = snap.num_children(0).unwrap_or(0);
            out.push(SnapshotInfo::from_xml(&xml, current, children));
        }
        out.sort_by_key(|s| std::cmp::Reverse(s.created));
        Ok(out)
    }

    /// Which kinds of snapshot the machine can take.
    pub fn snapshot_modes(&self, uuid: &str) -> Result<SnapshotModes> {
        let dom = self.domain(uuid)?;
        let config = DomainConfig::from_xml(&vmng_xml::parse(&dom.get_xml_desc(0)?)?);
        let location = snapshot_location(&config);
        Ok(SnapshotModes { internal: location == SnapshotLocation::Internal, default: location })
    }

    /// Takes a snapshot. A running machine is captured with its memory, so
    /// restoring it resumes exactly where it was. `options` picks where it
    /// is stored (by default as [`snapshot_modes`](Self::snapshot_modes)
    /// suggests).
    pub fn create_snapshot(
        &self,
        uuid: &str,
        name: &str,
        description: &str,
        options: Option<&SnapshotOptions>,
    ) -> Result<()> {
        validate_snapshot_name(name).map_err(|m| Error::Invalid(m.to_owned()))?;
        let dom = self.domain(uuid)?;
        let config = DomainConfig::from_xml(&vmng_xml::parse(&dom.get_xml_desc(0)?)?);
        let running = dom.is_active()?;
        let location = options.map_or_else(|| snapshot_location(&config), |o| o.location);
        if location == SnapshotLocation::Internal && snapshot_location(&config) != SnapshotLocation::Internal {
            return Err(Error::Invalid(
                "This machine's disks can't hold snapshots (they must all be qcow2). Use separate files.".into(),
            ));
        }
        validate_snapshot_name_for(name, running, location).map_err(|m| Error::Invalid(m.to_owned()))?;
        let memory_dir = options.and_then(|o| o.memory_dir.as_deref());
        let xml = snapshot_xml(&config, name, description, running, location, memory_dir);
        let mut flags = CREATE_ATOMIC;
        if location == SnapshotLocation::External && !running {
            flags |= CREATE_DISK_ONLY;
        }
        DomainSnapshot::create_xml(&dom, &xml, flags)?;
        Ok(())
    }

    /// Returns the machine to a snapshot.
    pub fn revert_snapshot(&self, uuid: &str, name: &str) -> Result<()> {
        let dom = self.domain(uuid)?;
        DomainSnapshot::lookup_by_name(&dom, name, 0)?.revert(0)?;
        Ok(())
    }

    /// Deletes a snapshot. Its children are kept and re-parented.
    pub fn delete_snapshot(&self, uuid: &str, name: &str) -> Result<()> {
        let dom = self.domain(uuid)?;
        DomainSnapshot::lookup_by_name(&dom, name, 0)?.delete(0)?;
        Ok(())
    }

    /// Changes the description of a snapshot (libvirt cannot rename them).
    pub fn set_snapshot_description(&self, uuid: &str, name: &str, description: &str) -> Result<()> {
        let dom = self.domain(uuid)?;
        let snap = DomainSnapshot::lookup_by_name(&dom, name, 0)?;
        let current = snap.is_current(0).unwrap_or(false);
        let mut xml = vmng_xml::parse(&snap.get_xml_desc(XML_SECURE)?)?;
        xml.remove_children(|e| e.name == "description");
        if !description.trim().is_empty() {
            xml.children.insert(1, vmng_xml::Node::Element(Element::new("description").with_text(description.trim())));
        }
        let mut flags = CREATE_REDEFINE;
        if current {
            flags |= CREATE_CURRENT;
        }
        DomainSnapshot::create_xml(&dom, &xml.to_xml(), flags)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DiskConfig;

    fn disk(target: &str, format: &str, device: &str) -> DiskConfig {
        DiskConfig {
            device: device.into(),
            target: target.into(),
            bus: None,
            format: Some(format.into()),
            source_path: Some(format!("/images/{target}.img")),
            source_volume: None,
            read_only: device == "cdrom",
        }
    }

    #[test]
    fn chooses_location() {
        let mut c = DomainConfig {
            disks: vec![disk("vda", "qcow2", "disk"), disk("sda", "raw", "cdrom")],
            ..Default::default()
        };
        assert_eq!(snapshot_location(&c), SnapshotLocation::Internal);
        c.nvram_path = Some("/nvram/x_VARS.fd".into());
        c.nvram_format = Some("raw".into());
        assert_eq!(snapshot_location(&c), SnapshotLocation::External);
        c.nvram_format = Some("qcow2".into());
        assert_eq!(snapshot_location(&c), SnapshotLocation::Internal);
        c.disks.push(disk("vdb", "raw", "disk"));
        assert_eq!(snapshot_location(&c), SnapshotLocation::External);
    }

    #[test]
    fn external_xml_lists_disks_and_memory() {
        let c = DomainConfig {
            name: "vm".into(),
            disks: vec![disk("vda", "raw", "disk"), disk("sda", "raw", "cdrom")],
            ..Default::default()
        };
        let xml = vmng_xml::parse(&snapshot_xml(&c, "Before update", "note", true, SnapshotLocation::External, None))
            .unwrap();
        assert_eq!(xml.find_text("name").as_deref(), Some("Before update"));
        assert_eq!(xml.find_attr("memory", "file"), Some("/images/vm-Before_update.mem"));
        let modes: Vec<_> = xml.child("disks").unwrap().elements().map(|d| d.attr("snapshot").unwrap()).collect();
        assert_eq!(modes, ["external", "no"]);
        let chosen = snapshot_xml(&c, "s", "", true, SnapshotLocation::External, Some("/big/disk/"));
        assert_eq!(vmng_xml::parse(&chosen).unwrap().find_attr("memory", "file"), Some("/big/disk/vm-s.mem"));
    }

    #[test]
    fn internal_xml_is_minimal() {
        let xml =
            vmng_xml::parse(&snapshot_xml(&DomainConfig::default(), "s", "", false, SnapshotLocation::Internal, None))
                .unwrap();
        assert!(xml.child("disks").is_none() && xml.child("description").is_none());
    }

    #[test]
    fn parses_snapshot_xml() {
        let xml = vmng_xml::parse(
            "<domainsnapshot><name>Clean</name><state>shutoff</state><parent><name>base</name></parent>\
             <creationTime>1791488012</creationTime><memory snapshot='no'/>\
             <disks><disk name='vda' snapshot='internal'/></disks></domainsnapshot>",
        )
        .unwrap();
        let s = SnapshotInfo::from_xml(&xml, true, 2);
        assert_eq!(s.name, "Clean");
        assert_eq!(s.parent.as_deref(), Some("base"));
        assert_eq!(s.created, 1_791_488_012);
        assert_eq!(s.location, SnapshotLocation::Internal);
        assert!(!s.has_memory());
    }

    #[test]
    fn validates_names() {
        assert!(validate_snapshot_name("Before update").is_ok());
        for bad in ["", "  ", "a/b", ".hidden", "-x", "tab\there"] {
            assert!(validate_snapshot_name(bad).is_err(), "{bad:?}");
        }
        let live = |n| validate_snapshot_name_for(n, true, SnapshotLocation::Internal);
        assert!(live("Snapshot-2026-10-29-05.44").is_ok());
        assert!(live("Before update").is_err());
        assert!(validate_snapshot_name_for("Before update", true, SnapshotLocation::External).is_ok());
        assert!(validate_snapshot_name_for("Before update", false, SnapshotLocation::Internal).is_ok());
    }
}
