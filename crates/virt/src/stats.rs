//! Bulk statistics for domains and the host.
//!
//! The `virt` crate does not wrap these APIs, so they are called through
//! `virt::sys` here. One `virConnectGetAllDomainStats` call returns the
//! counters of every domain at once, which keeps polling cheap.

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_int};
use std::ptr;

use virt::connect::Connect;
use virt::sys;

use crate::domain::DomainState;
use crate::error::{Error, Result};

// virDomainStatsTypes
const STATS_STATE: u32 = 1;
const STATS_CPU_TOTAL: u32 = 2;
const STATS_BALLOON: u32 = 4;
const STATS_VCPU: u32 = 8;
const STATS_INTERFACE: u32 = 16;
const STATS_BLOCK: u32 = 32;

// virTypedParameterType
const PARAM_INT: c_int = 1;
const PARAM_UINT: c_int = 2;
const PARAM_LLONG: c_int = 3;
const PARAM_ULLONG: c_int = 4;
const PARAM_DOUBLE: c_int = 5;
const PARAM_BOOLEAN: c_int = 6;
const PARAM_STRING: c_int = 7;

/// Raw counters of one block device.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockStats {
    /// Guest target name, e.g. `vda`.
    pub name: String,
    pub path: Option<String>,
    pub read_bytes: u64,
    pub write_bytes: u64,
    /// For a stopped machine: bytes the image uses on the host. For a
    /// running one libvirt reports the highest written offset here instead;
    /// use [`BlockStats::used`].
    pub allocation: Option<u64>,
    /// Bytes the image uses on the host (reported for running machines).
    pub physical: Option<u64>,
    /// Virtual size seen by the guest.
    pub capacity: Option<u64>,
}

impl BlockStats {
    /// Space the disk image takes on the host.
    pub fn used(&self, running: bool) -> Option<u64> {
        if running { self.physical.or(self.allocation) } else { self.allocation }
    }
}

/// Raw counters of one network interface.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetStats {
    pub name: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// A snapshot of one domain's counters. Rates are computed by the caller
/// from two consecutive snapshots.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DomainStats {
    pub uuid: String,
    pub state: Option<DomainState>,
    /// Total CPU time used, in nanoseconds.
    pub cpu_time_ns: u64,
    pub vcpus: u32,
    /// Memory currently given to the guest (balloon), in bytes.
    pub balloon_current: Option<u64>,
    /// Memory the guest reports as available, in bytes (needs the balloon
    /// driver in the guest).
    pub balloon_available: Option<u64>,
    /// Memory the guest reports as unused, in bytes.
    pub balloon_unused: Option<u64>,
    /// Resident memory of the QEMU process, in bytes.
    pub rss: Option<u64>,
    pub blocks: Vec<BlockStats>,
    pub nets: Vec<NetStats>,
}

impl DomainStats {
    /// Best estimate of the memory the guest is actually using, in bytes.
    pub fn memory_used(&self) -> Option<u64> {
        match (self.balloon_available, self.balloon_unused) {
            (Some(avail), Some(unused)) if avail >= unused => Some(avail - unused),
            _ => self.rss.or(self.balloon_current),
        }
    }

    pub fn disk_bytes(&self) -> (u64, u64) {
        self.blocks.iter().fold((0, 0), |(r, w), b| (r + b.read_bytes, w + b.write_bytes))
    }

    pub fn net_bytes(&self) -> (u64, u64) {
        self.nets.iter().fold((0, 0), |(r, t), n| (r + n.rx_bytes, t + n.tx_bytes))
    }
}

/// A snapshot of the host's counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostStats {
    /// Busy CPU time summed over all CPUs, in nanoseconds.
    pub cpu_busy_ns: u64,
    /// Total CPU time (busy + idle) summed over all CPUs, in nanoseconds.
    pub cpu_total_ns: u64,
    pub memory_total: u64,
    /// Memory in use, excluding buffers and page cache.
    pub memory_used: u64,
}

#[derive(Clone, Copy)]
pub(crate) enum Value {
    Int(i64),
    UInt(u64),
    Double,
    Bool,
    Str,
}

impl Value {
    pub(crate) fn as_u64(self) -> Option<u64> {
        match self {
            Self::UInt(v) => Some(v),
            Self::Int(v) => u64::try_from(v).ok(),
            _ => None,
        }
    }
}

/// Fetches counters for every domain on the connection.
/// Which groups of counters to collect. Block devices (needed for disk
/// usage) and the run state are always collected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatsWanted {
    pub cpu: bool,
    pub memory: bool,
    pub network: bool,
}

impl Default for StatsWanted {
    fn default() -> Self {
        Self { cpu: true, memory: true, network: true }
    }
}

pub(crate) fn all_domain_stats(conn: &Connect, wanted: StatsWanted) -> Result<Vec<DomainStats>> {
    let mut records: *mut sys::virDomainStatsRecordPtr = ptr::null_mut();
    let mut what = STATS_STATE | STATS_BLOCK;
    if wanted.cpu {
        what |= STATS_CPU_TOTAL | STATS_VCPU;
    }
    if wanted.memory {
        what |= STATS_BALLOON;
    }
    if wanted.network {
        what |= STATS_INTERFACE;
    }
    // SAFETY: valid connection and out pointer; on success libvirt hands us
    // a NULL-terminated array we release with virDomainStatsRecordListFree.
    let n = unsafe { sys::virConnectGetAllDomainStats(conn.as_ptr(), what, &mut records, 0) };
    if n < 0 {
        return Err(virt::error::Error::last_error().into());
    }
    let mut out = Vec::with_capacity(usize::try_from(n).unwrap_or(0));
    // SAFETY: `records` holds `n` valid record pointers.
    unsafe {
        for i in 0..usize::try_from(n).unwrap_or(0) {
            let rec = &**records.add(i);
            let mut stats = DomainStats { uuid: domain_uuid(rec.dom).unwrap_or_default(), ..Default::default() };
            let params = std::slice::from_raw_parts(rec.params, usize::try_from(rec.nparams).unwrap_or(0));
            let mut blocks: HashMap<usize, BlockStats> = HashMap::new();
            let mut nets: HashMap<usize, NetStats> = HashMap::new();
            for p in params {
                let field = CStr::from_ptr(p.field.as_ptr()).to_str().unwrap_or_default();
                let value = read_value(p);
                apply(&mut stats, &mut blocks, &mut nets, field, value, p);
            }
            stats.blocks = sorted(blocks);
            stats.nets = sorted(nets);
            out.push(stats);
        }
        sys::virDomainStatsRecordListFree(records);
    }
    Ok(out)
}

fn sorted<T>(map: HashMap<usize, T>) -> Vec<T> {
    let mut v: Vec<_> = map.into_iter().collect();
    v.sort_by_key(|(k, _)| *k);
    v.into_iter().map(|(_, s)| s).collect()
}

/// # Safety
/// `p` must be a valid typed parameter.
pub(crate) unsafe fn read_value(p: &sys::virTypedParameter) -> Value {
    // SAFETY: the union member read matches the tag in `type_`.
    unsafe {
        match p.type_ {
            PARAM_INT => Value::Int(i64::from(p.value.i)),
            PARAM_UINT => Value::UInt(u64::from(p.value.ui)),
            PARAM_LLONG => Value::Int(p.value.l),
            PARAM_ULLONG => Value::UInt(p.value.ul),
            PARAM_DOUBLE => Value::Double,
            PARAM_BOOLEAN => Value::Bool,
            PARAM_STRING => Value::Str,
            _ => Value::Bool,
        }
    }
}

/// # Safety
/// `p` must be a valid typed parameter (strings are read from it).
pub(crate) unsafe fn string_of(p: &sys::virTypedParameter) -> Option<String> {
    if p.type_ != PARAM_STRING {
        return None;
    }
    // SAFETY: tag says the union holds a NUL-terminated string (or NULL).
    unsafe {
        let s = p.value.s;
        (!s.is_null()).then(|| CStr::from_ptr(s).to_string_lossy().into_owned())
    }
}

/// Splits `block.3.rd.bytes` into (3, "rd.bytes").
fn indexed<'a>(field: &'a str, prefix: &str) -> Option<(usize, &'a str)> {
    let rest = field.strip_prefix(prefix)?.strip_prefix('.')?;
    let (idx, key) = rest.split_once('.')?;
    Some((idx.parse().ok()?, key))
}

/// # Safety
/// `p` must be the valid typed parameter `value` was read from.
unsafe fn apply(
    s: &mut DomainStats,
    blocks: &mut HashMap<usize, BlockStats>,
    nets: &mut HashMap<usize, NetStats>,
    field: &str,
    value: Value,
    p: &sys::virTypedParameter,
) {
    let kib = |v: Value| v.as_u64().map(|k| k.saturating_mul(1024));
    match field {
        "state.state" => s.state = value.as_u64().map(|v| DomainState::from_raw(v as u32)),
        "cpu.time" => s.cpu_time_ns = value.as_u64().unwrap_or(0),
        "vcpu.current" => s.vcpus = value.as_u64().unwrap_or(0) as u32,
        "balloon.current" => s.balloon_current = kib(value),
        "balloon.available" => s.balloon_available = kib(value),
        "balloon.unused" => s.balloon_unused = kib(value),
        "balloon.rss" => s.rss = kib(value),
        _ => {
            if let Some((i, key)) = indexed(field, "block") {
                let b = blocks.entry(i).or_default();
                match key {
                    // SAFETY: forwarded from the caller.
                    "name" => b.name = unsafe { string_of(p) }.unwrap_or_default(),
                    // SAFETY: forwarded from the caller.
                    "path" => b.path = unsafe { string_of(p) },
                    "rd.bytes" => b.read_bytes = value.as_u64().unwrap_or(0),
                    "wr.bytes" => b.write_bytes = value.as_u64().unwrap_or(0),
                    "allocation" => b.allocation = value.as_u64(),
                    "physical" => b.physical = value.as_u64(),
                    "capacity" => b.capacity = value.as_u64(),
                    _ => {}
                }
            } else if let Some((i, key)) = indexed(field, "net") {
                let n = nets.entry(i).or_default();
                match key {
                    // SAFETY: forwarded from the caller.
                    "name" => n.name = unsafe { string_of(p) }.unwrap_or_default(),
                    "rx.bytes" => n.rx_bytes = value.as_u64().unwrap_or(0),
                    "tx.bytes" => n.tx_bytes = value.as_u64().unwrap_or(0),
                    _ => {}
                }
            }
        }
    }
}

/// # Safety
/// `dom` must be a valid domain pointer.
unsafe fn domain_uuid(dom: sys::virDomainPtr) -> Option<String> {
    let mut buf = [0 as c_char; 37]; // VIR_UUID_STRING_BUFLEN
    // SAFETY: buffer is VIR_UUID_STRING_BUFLEN bytes as required.
    let rc = unsafe { sys::virDomainGetUUIDString(dom, buf.as_mut_ptr()) };
    // SAFETY: on success libvirt wrote a NUL-terminated string.
    (rc == 0).then(|| unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned())
}

/// Reads `(field, value)` pairs from one of the node stats APIs, which all
/// share the "call once for the count, then again for the values" shape.
fn node_params<T>(
    count: impl Fn(*mut T, *mut c_int) -> c_int,
    field: impl Fn(&T) -> (&[c_char; 80], u64),
) -> Result<Vec<(String, u64)>>
where
    T: Copy,
{
    let mut n: c_int = 0;
    if count(ptr::null_mut(), &mut n) < 0 {
        return Err(virt::error::Error::last_error().into());
    }
    if n <= 0 {
        return Ok(Vec::new());
    }
    let len = usize::try_from(n).map_err(|_| Error::Invalid("bad stats count".into()))?;
    let mut buf: Vec<std::mem::MaybeUninit<T>> = Vec::with_capacity(len);
    // SAFETY: capacity reserved above; libvirt initialises `n` entries.
    unsafe { buf.set_len(len) };
    if count(buf.as_mut_ptr().cast(), &mut n) < 0 {
        return Err(virt::error::Error::last_error().into());
    }
    let filled = usize::try_from(n).unwrap_or(0).min(len);
    Ok(buf[..filled]
        .iter()
        .map(|e| {
            // SAFETY: the first `filled` entries were written by libvirt.
            let e = unsafe { e.assume_init_ref() };
            let (name, value) = field(e);
            // SAFETY: libvirt NUL-terminates the fixed-size field.
            let name = unsafe { CStr::from_ptr(name.as_ptr()) }.to_string_lossy().into_owned();
            (name, value)
        })
        .collect())
}

/// Fetches host CPU and memory counters.
pub(crate) fn host_stats(conn: &Connect) -> Result<HostStats> {
    let c = conn.as_ptr();
    let cpu = node_params::<sys::virNodeCPUStats>(
        // SAFETY: valid connection; VIR_NODE_CPU_STATS_ALL_CPUS = -1.
        |p, n| unsafe { sys::virNodeGetCPUStats(c, -1, p, n, 0) },
        |e| (&e.field, e.value),
    )?;
    let mem = node_params::<sys::virNodeMemoryStats>(
        // SAFETY: valid connection; VIR_NODE_MEMORY_STATS_ALL_CELLS = -1.
        |p, n| unsafe { sys::virNodeGetMemoryStats(c, -1, p, n, 0) },
        |e| (&e.field, e.value),
    )?;

    let get = |list: &[(String, u64)], key: &str| list.iter().find(|(k, _)| k == key).map_or(0, |(_, v)| *v);
    let busy = get(&cpu, "kernel") + get(&cpu, "user");
    let idle = get(&cpu, "idle") + get(&cpu, "iowait");
    // Values are in KiB.
    let total = get(&mem, "total") * 1024;
    let free = (get(&mem, "free") + get(&mem, "buffers") + get(&mem, "cached")) * 1024;
    Ok(HostStats {
        cpu_busy_ns: busy,
        cpu_total_ns: busy + idle,
        memory_total: total,
        memory_used: total.saturating_sub(free),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_indexed_fields() {
        assert_eq!(indexed("block.12.rd.bytes", "block"), Some((12, "rd.bytes")));
        assert_eq!(indexed("net.0.name", "net"), Some((0, "name")));
        assert_eq!(indexed("block.count", "block"), None);
        assert_eq!(indexed("blockx.1.a", "block"), None);
    }

    #[test]
    fn memory_used_prefers_guest_view() {
        let s = DomainStats {
            balloon_available: Some(8 << 30),
            balloon_unused: Some(6 << 30),
            rss: Some(7 << 30),
            ..Default::default()
        };
        assert_eq!(s.memory_used(), Some(2 << 30));
        let s = DomainStats { rss: Some(7 << 30), ..Default::default() };
        assert_eq!(s.memory_used(), Some(7 << 30));
    }
}
