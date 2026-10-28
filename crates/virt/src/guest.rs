//! What the qemu-guest-agent reports about the system inside a machine.

use std::collections::BTreeMap;
use std::ffi::{CStr, c_int};
use std::ptr;

use virt::domain::Domain;
use virt::sys;

use crate::connection::Connection;
use crate::error::Result;
use crate::stats::{Value, read_value, string_of};

/// A mounted filesystem inside the guest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuestFilesystem {
    pub mountpoint: String,
    pub fstype: String,
    pub used: Option<u64>,
    pub total: Option<u64>,
}

/// Details from the guest agent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuestInfo {
    /// e.g. "Fedora Linux 44 (Workstation Edition)".
    pub os_name: Option<String>,
    pub kernel: Option<String>,
    pub timezone: Option<String>,
    /// Logged-in users.
    pub users: Vec<String>,
    pub filesystems: Vec<GuestFilesystem>,
}

impl GuestInfo {
    /// Builds the info from libvirt's flat `key = value` list.
    fn from_params(params: &BTreeMap<String, ParamValue>) -> Self {
        let s = |k: &str| match params.get(k) {
            Some(ParamValue::Str(v)) if !v.is_empty() => Some(v.clone()),
            _ => None,
        };
        let n = |k: &str| match params.get(k) {
            Some(ParamValue::Num(v)) => Some(*v),
            _ => None,
        };
        let mut users: Vec<String> =
            (0..n("user.count").unwrap_or(0)).filter_map(|i| s(&format!("user.{i}.name"))).collect();
        users.sort();
        users.dedup();
        let filesystems = (0..n("fs.count").unwrap_or(0))
            .filter_map(|i| {
                let mountpoint = s(&format!("fs.{i}.mountpoint"))?;
                Some(GuestFilesystem {
                    mountpoint,
                    fstype: s(&format!("fs.{i}.fstype")).unwrap_or_default(),
                    used: n(&format!("fs.{i}.used-bytes")),
                    total: n(&format!("fs.{i}.total-bytes")),
                })
            })
            // Pseudo filesystems (no size) are noise.
            .filter(|f| f.total.unwrap_or(0) > 0)
            .collect();
        Self {
            os_name: s("os.pretty-name").or_else(|| match (s("os.name"), s("os.version")) {
                (Some(a), Some(b)) => Some(format!("{a} {b}")),
                (a, _) => a,
            }),
            kernel: s("os.kernel-release"),
            timezone: s("timezone.name"),
            users,
            filesystems,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ParamValue {
    Num(u64),
    Str(String),
}

// virDomainGuestInfoTypes
const INFO_USERS: u32 = 1;
const INFO_OS: u32 = 2;
const INFO_TIMEZONE: u32 = 4;
const INFO_FILESYSTEM: u32 = 16;

pub(crate) fn guest_info(dom: &Domain) -> Result<GuestInfo> {
    let mut params: sys::virTypedParameterPtr = ptr::null_mut();
    let mut n: c_int = 0;
    let types = INFO_USERS | INFO_OS | INFO_TIMEZONE | INFO_FILESYSTEM;
    // SAFETY: valid domain and out pointers; on success libvirt allocates
    // `n` parameters that we free with virTypedParamsFree.
    let rc = unsafe { sys::virDomainGetGuestInfo(dom.as_ptr(), types, &mut params, &mut n, 0) };
    if rc < 0 {
        return Err(virt::error::Error::last_error().into());
    }
    let mut map = BTreeMap::new();
    // SAFETY: `params` holds `n` initialised entries.
    unsafe {
        for p in std::slice::from_raw_parts(params, usize::try_from(n).unwrap_or(0)) {
            let key = CStr::from_ptr(p.field.as_ptr()).to_string_lossy().into_owned();
            let value = match read_value(p) {
                Value::Str => string_of(p).map(ParamValue::Str),
                v => v.as_u64().map(ParamValue::Num),
            };
            if let Some(v) = value {
                map.insert(key, v);
            }
        }
        sys::virTypedParamsFree(params, n);
    }
    Ok(GuestInfo::from_params(&map))
}

impl Connection {
    /// Sets the guest's clock to the host's time (needs the guest agent).
    pub fn sync_guest_time(&self, uuid: &str) -> Result<()> {
        const TIME_SYNC: u32 = 1;
        self.domain(uuid)?.set_time(0, 0, TIME_SYNC)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_info_from_params() {
        let mut m = BTreeMap::new();
        let s = |v: &str| ParamValue::Str(v.into());
        m.insert("os.pretty-name".into(), s("Fedora Linux 44"));
        m.insert("user.count".into(), ParamValue::Num(2));
        m.insert("user.0.name".into(), s("maarten"));
        m.insert("user.1.name".into(), s("maarten"));
        m.insert("fs.count".into(), ParamValue::Num(2));
        m.insert("fs.0.mountpoint".into(), s("/"));
        m.insert("fs.0.fstype".into(), s("btrfs"));
        m.insert("fs.0.total-bytes".into(), ParamValue::Num(100));
        m.insert("fs.0.used-bytes".into(), ParamValue::Num(40));
        m.insert("fs.1.mountpoint".into(), s("/proc"));
        let i = GuestInfo::from_params(&m);
        assert_eq!(i.os_name.as_deref(), Some("Fedora Linux 44"));
        assert_eq!(i.users, ["maarten"]);
        assert_eq!(i.filesystems.len(), 1);
        assert_eq!(i.filesystems[0].used, Some(40));
    }
}
