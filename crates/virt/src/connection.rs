//! A connection to a libvirt hypervisor.

use virt::connect::Connect;
use virt::domain::Domain;
use virt::storage_pool::StoragePool;

use crate::config::DomainConfig;
use crate::domain::{DomainState, DomainSummary, PowerAction};
use crate::error::{Error, Result};
use crate::stats::{self, DomainStats, HostStats};

/// Which hypervisor a connection points at.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ConnectionKind {
    /// `qemu:///system`: system-wide VMs, managed by the libvirt daemon.
    System,
    /// `qemu:///session`: VMs owned by and running as the current user.
    Session,
    /// Any other URI.
    Other(String),
}

impl ConnectionKind {
    pub fn uri(&self) -> &str {
        match self {
            Self::System => "qemu:///system",
            Self::Session => "qemu:///session",
            Self::Other(uri) => uri,
        }
    }

    pub fn from_uri(uri: &str) -> Self {
        match uri {
            "qemu:///system" => Self::System,
            "qemu:///session" => Self::Session,
            other => Self::Other(other.to_owned()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::System => "QEMU/KVM",
            Self::Session => "QEMU/KVM user session",
            Self::Other(uri) => uri,
        }
    }
}

/// Static facts about the host.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostInfo {
    pub hostname: String,
    pub cpu_model: String,
    pub cpus: u32,
    pub mhz: u32,
    pub memory: u64,
    /// libvirt library version, e.g. `12.0.0`.
    pub libvirt_version: String,
    /// Hypervisor (QEMU) version, e.g. `10.1.0`.
    pub hypervisor_version: String,
}

/// Size and usage of a storage pool.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PoolSummary {
    pub name: String,
    pub active: bool,
    pub capacity: u64,
    pub allocation: u64,
    pub available: u64,
}

/// What a running guest reports about itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuestDetails {
    pub hostname: Option<String>,
    /// IP addresses (without loopback), IPv4 first.
    pub addresses: Vec<String>,
    /// Whether the qemu-guest-agent answered.
    pub agent_connected: bool,
}

/// An open libvirt connection. Cheap to share between threads behind an
/// `Arc`; every method is a blocking call.
pub struct Connection {
    conn: Connect,
    kind: ConnectionKind,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection").field("uri", &self.kind.uri()).finish_non_exhaustive()
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // Nothing useful can be done if closing fails.
        let _ = self.conn.close();
    }
}

fn version_string(v: u32) -> String {
    format!("{}.{}.{}", v / 1_000_000, (v / 1000) % 1000, v % 1000)
}

impl Connection {
    /// Opens a read-write connection. For `qemu:///system` this may show a
    /// polkit authentication prompt.
    pub fn open(kind: ConnectionKind) -> Result<Self> {
        let conn = Connect::open(Some(kind.uri()))?;
        // Detect a dead daemon quickly instead of hanging forever.
        let _ = conn.set_keep_alive(5, 3);
        Ok(Self { conn, kind })
    }

    pub fn kind(&self) -> &ConnectionKind {
        &self.kind
    }

    pub fn is_alive(&self) -> bool {
        self.conn.is_alive().unwrap_or(false)
    }

    pub fn host_info(&self) -> Result<HostInfo> {
        let node = self.conn.get_node_info()?;
        Ok(HostInfo {
            hostname: self.conn.get_hostname().unwrap_or_default(),
            cpu_model: node.model,
            cpus: node.cpus,
            mhz: node.mhz,
            memory: node.memory.saturating_mul(1024),
            libvirt_version: self.conn.get_lib_version().map(version_string).unwrap_or_default(),
            hypervisor_version: self.conn.get_hyp_version().map(version_string).unwrap_or_default(),
        })
    }

    pub(crate) fn raw(&self) -> &Connect {
        &self.conn
    }

    pub(crate) fn domain(&self, uuid: &str) -> Result<Domain> {
        Domain::lookup_by_uuid_string(&self.conn, uuid).map_err(Error::from)
    }

    fn summarize(dom: &Domain) -> Result<DomainSummary> {
        let (state, _reason) = dom.get_state()?;
        let xml = dom.get_xml_desc(0)?;
        let config = DomainConfig::from_xml(&vmng_xml::parse(&xml)?);
        Ok(DomainSummary {
            uuid: dom.get_uuid_string()?,
            state: DomainState::from_raw(state),
            persistent: dom.is_persistent().unwrap_or(true),
            autostart: dom.get_autostart().unwrap_or(false),
            has_managed_save: dom.has_managed_save(0).unwrap_or(false),
            config,
        })
    }

    /// Lists every domain (running and defined) with its configuration.
    ///
    /// Domains that disappear while being listed are skipped.
    pub fn list_domains(&self) -> Result<Vec<DomainSummary>> {
        let domains = self.conn.list_all_domains(0)?;
        let mut out = Vec::with_capacity(domains.len());
        for dom in &domains {
            match Self::summarize(dom) {
                Ok(s) => out.push(s),
                Err(e) if e.is_not_found() => {}
                Err(e) => return Err(e),
            }
        }
        out.sort_by_cached_key(|d| d.display_name().to_lowercase());
        Ok(out)
    }

    /// Counters of every domain.
    pub fn domain_stats(&self, wanted: stats::StatsWanted) -> Result<Vec<DomainStats>> {
        stats::all_domain_stats(&self.conn, wanted)
    }

    /// Host CPU and memory counters.
    pub fn host_stats(&self) -> Result<HostStats> {
        stats::host_stats(&self.conn)
    }

    /// Size and usage of a storage pool, or `None` if it does not exist.
    pub fn pool_summary(&self, name: &str) -> Result<Option<PoolSummary>> {
        let pool = match StoragePool::lookup_by_name(&self.conn, name) {
            Ok(p) => p,
            Err(e) => {
                let e = Error::from(e);
                return if e.is_not_found() { Ok(None) } else { Err(e) };
            }
        };
        let info = pool.get_info()?;
        Ok(Some(PoolSummary {
            name: name.to_owned(),
            active: pool.is_active().unwrap_or(false),
            capacity: info.capacity,
            allocation: info.allocation,
            available: info.available,
        }))
    }

    /// `(allocation, capacity)` in bytes of one of a domain's disks,
    /// identified by its target (e.g. `vda`).
    pub fn disk_usage(&self, uuid: &str, target: &str) -> Result<(u64, u64)> {
        let info = self.domain(uuid)?.get_block_info(target, 0)?;
        Ok((info.allocation, info.capacity))
    }

    /// Hostname and IP addresses as reported by the guest agent, falling
    /// back to DHCP leases of libvirt networks.
    pub fn guest_details(&self, uuid: &str) -> Result<GuestDetails> {
        const SRC_LEASE: u32 = 0;
        const SRC_AGENT: u32 = 1;
        const HOSTNAME_AGENT: u32 = 2;
        let dom = self.domain(uuid)?;
        let mut details = GuestDetails::default();

        let agent = dom.interface_addresses(SRC_AGENT, 0);
        details.agent_connected = agent.is_ok();
        let ifaces = agent.or_else(|_| dom.interface_addresses(SRC_LEASE, 0)).unwrap_or_default();
        let mut v4 = Vec::new();
        let mut v6 = Vec::new();
        for iface in ifaces.iter().filter(|i| i.name != "lo") {
            for a in &iface.addrs {
                let ip = &a.addr;
                if ip.starts_with("127.") || ip == "::1" || ip.starts_with("fe80:") {
                    continue;
                }
                if a.typed == 0 { v4.push(ip.clone()) } else { v6.push(ip.clone()) }
            }
        }
        details.addresses = v4.into_iter().chain(v6).collect();
        if details.agent_connected {
            details.hostname = dom.get_hostname(HOSTNAME_AGENT).ok().filter(|h| !h.is_empty());
        }
        Ok(details)
    }

    /// The domain XML. `inactive` returns the persistent configuration
    /// (what the VM will use on its next start) instead of the live one.
    pub fn domain_xml(&self, uuid: &str, inactive: bool) -> Result<String> {
        const XML_INACTIVE: u32 = 2;
        Ok(self.domain(uuid)?.get_xml_desc(if inactive { XML_INACTIVE } else { 0 })?)
    }

    /// Performs a lifecycle action.
    pub fn power(&self, uuid: &str, action: PowerAction) -> Result<()> {
        let dom = self.domain(uuid)?;
        match action {
            PowerAction::Start => dom.create_with_flags(0).map(drop),
            PowerAction::Shutdown => dom.shutdown_flags(0).map(drop),
            PowerAction::Reboot => dom.reboot(0),
            PowerAction::Reset => dom.reset().map(drop),
            PowerAction::ForceOff => dom.destroy_flags(0).map(drop),
            PowerAction::Pause => dom.suspend().map(drop),
            PowerAction::Resume => dom.resume().map(drop),
            PowerAction::Save => dom.managed_save(0).map(drop),
        }
        .map_err(Error::from)
    }

    /// Whether the domain should start when the host boots.
    pub fn set_autostart(&self, uuid: &str, on: bool) -> Result<()> {
        self.domain(uuid)?.set_autostart(on)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(version_string(12_000_000), "12.0.0");
        assert_eq!(version_string(10_001_002), "10.1.2");
    }

    #[test]
    fn kinds() {
        assert_eq!(ConnectionKind::from_uri("qemu:///system"), ConnectionKind::System);
        assert_eq!(ConnectionKind::Session.uri(), "qemu:///session");
    }
}
