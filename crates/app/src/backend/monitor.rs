//! Turns periodic counter snapshots into rates and short histories.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use vmng_virt::{BlockStats, DomainStats, DomainSummary, HostStats, PoolSummary};

/// Number of samples kept for graphs.
pub const HISTORY_LEN: usize = 60;

/// One poll of a connection.
#[derive(Clone, Debug)]
pub struct Sample {
    pub at: Instant,
    pub domains: Vec<DomainSummary>,
    pub stats: Vec<DomainStats>,
    pub host: HostStats,
    pub pool: Option<PoolSummary>,
}

/// A fixed-length history of values, oldest first.
#[derive(Clone, Debug, Default)]
pub struct History(VecDeque<f64>);

impl History {
    fn push(&mut self, v: f64) {
        if self.0.len() == HISTORY_LEN {
            self.0.pop_front();
        }
        self.0.push_back(v);
    }

    pub fn values(&self) -> impl Iterator<Item = f64> + '_ {
        self.0.iter().copied()
    }
}

/// Live figures of one VM.
#[derive(Clone, Debug, Default)]
pub struct VmLive {
    /// Guest CPU usage, 0–100 % of its virtual CPUs.
    pub cpu_pct: Option<f64>,
    pub mem_used: Option<u64>,
    pub disk_read: f64,
    pub disk_write: f64,
    pub net_rx: f64,
    pub net_tx: f64,
    pub cpu: History,
    pub mem: History,
    pub disk: History,
    pub net: History,
    /// Per-disk allocation and capacity.
    pub disks: Vec<BlockStats>,
}

/// Live figures of the host.
#[derive(Clone, Debug, Default)]
pub struct HostLive {
    pub cpu_pct: Option<f64>,
    pub mem_used: u64,
    pub mem_total: u64,
    /// Memory assigned to running VMs.
    pub vm_memory: u64,
    pub pool: Option<PoolSummary>,
    pub cpu: History,
    pub mem: History,
}

/// Keeps the previous sample to compute rates.
#[derive(Debug, Default)]
pub struct Monitor {
    prev_at: Option<Instant>,
    prev_stats: HashMap<String, DomainStats>,
    prev_host: Option<HostStats>,
    vms: HashMap<String, VmLive>,
    host: HostLive,
}

fn rate(now: u64, before: u64, secs: f64) -> f64 {
    if secs <= 0.0 { 0.0 } else { now.saturating_sub(before) as f64 / secs }
}

impl Monitor {
    pub fn vm(&self, uuid: &str) -> Option<&VmLive> {
        self.vms.get(uuid)
    }

    pub fn host(&self) -> &HostLive {
        &self.host
    }

    /// Forgets everything, e.g. after switching connections.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn ingest(&mut self, s: &Sample) {
        let secs = self.prev_at.map_or(0.0, |p| s.at.duration_since(p).as_secs_f64());

        // Host.
        let h = &mut self.host;
        if let Some(prev) = self.prev_host {
            let total = s.host.cpu_total_ns.saturating_sub(prev.cpu_total_ns);
            let busy = s.host.cpu_busy_ns.saturating_sub(prev.cpu_busy_ns);
            if total > 0 {
                let pct = (busy as f64 / total as f64 * 100.0).clamp(0.0, 100.0);
                h.cpu_pct = Some(pct);
                h.cpu.push(pct);
            }
        }
        h.mem_used = s.host.memory_used;
        h.mem_total = s.host.memory_total;
        h.mem.push(s.host.memory_used as f64);
        h.pool = s.pool.clone();
        h.vm_memory = s.domains.iter().filter(|d| d.state.is_active()).map(|d| d.config.memory).sum();
        self.prev_host = Some(s.host);

        // Domains.
        let active: HashMap<&str, &DomainSummary> =
            s.domains.iter().filter(|d| d.state.is_active()).map(|d| (d.uuid.as_str(), d)).collect();
        let mut seen = Vec::with_capacity(s.stats.len());
        for st in &s.stats {
            let live = self.vms.entry(st.uuid.clone()).or_default();
            live.disks = st.blocks.clone();
            seen.push(st.uuid.clone());
            let Some(dom) = active.get(st.uuid.as_str()) else {
                // Shut off: no activity, keep the graphs flat.
                live.cpu_pct = None;
                live.mem_used = None;
                live.disk_read = 0.0;
                live.disk_write = 0.0;
                live.net_rx = 0.0;
                live.net_tx = 0.0;
                live.cpu = History::default();
                live.mem = History::default();
                live.disk = History::default();
                live.net = History::default();
                continue;
            };
            live.mem_used = st.memory_used().or(Some(dom.config.memory));
            live.mem.push(live.mem_used.unwrap_or(0) as f64);
            match self.prev_stats.get(&st.uuid) {
                Some(prev) if secs > 0.0 => {
                    let vcpus = f64::from(st.vcpus.max(1));
                    let cpu = rate(st.cpu_time_ns, prev.cpu_time_ns, secs) / 1e9 / vcpus * 100.0;
                    let cpu = cpu.clamp(0.0, 100.0);
                    live.cpu_pct = Some(cpu);
                    let (rd, wr) = st.disk_bytes();
                    let (prd, pwr) = prev.disk_bytes();
                    live.disk_read = rate(rd, prd, secs);
                    live.disk_write = rate(wr, pwr, secs);
                    let (rx, tx) = st.net_bytes();
                    let (prx, ptx) = prev.net_bytes();
                    live.net_rx = rate(rx, prx, secs);
                    live.net_tx = rate(tx, ptx, secs);
                    live.cpu.push(cpu);
                    live.disk.push(live.disk_read + live.disk_write);
                    live.net.push(live.net_rx + live.net_tx);
                }
                _ => {}
            }
        }
        self.vms.retain(|uuid, _| seen.contains(uuid));
        self.prev_stats = s.stats.iter().map(|st| (st.uuid.clone(), st.clone())).collect();
        self.prev_at = Some(s.at);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use vmng_virt::{DomainConfig, DomainState};

    use super::*;

    fn sample(at: Instant, cpu_ns: u64, rd: u64, host_busy: u64, host_total: u64) -> Sample {
        Sample {
            at,
            domains: vec![DomainSummary {
                uuid: "u".into(),
                state: DomainState::Running,
                persistent: true,
                autostart: false,
                has_managed_save: false,
                config: DomainConfig { memory: 4 << 30, ..Default::default() },
            }],
            stats: vec![DomainStats {
                uuid: "u".into(),
                cpu_time_ns: cpu_ns,
                vcpus: 2,
                blocks: vec![BlockStats { read_bytes: rd, ..Default::default() }],
                ..Default::default()
            }],
            host: HostStats { cpu_busy_ns: host_busy, cpu_total_ns: host_total, memory_total: 100, memory_used: 40 },
            pool: None,
        }
    }

    #[test]
    fn computes_rates_between_samples() {
        let t0 = Instant::now();
        let mut m = Monitor::default();
        m.ingest(&sample(t0, 0, 0, 0, 0));
        assert_eq!(m.vm("u").unwrap().cpu_pct, None);
        // 1 s later: 1 s of CPU time over 2 vCPUs = 50 %, 4 MB read.
        m.ingest(&sample(t0 + Duration::from_secs(1), 1_000_000_000, 4_000_000, 25, 100));
        let vm = m.vm("u").unwrap();
        assert!((vm.cpu_pct.unwrap() - 50.0).abs() < 1e-6);
        assert!((vm.disk_read - 4_000_000.0).abs() < 1e-3);
        assert_eq!(vm.mem_used, Some(4 << 30));
        assert!((m.host().cpu_pct.unwrap() - 25.0).abs() < 1e-6);
        assert_eq!(m.host().vm_memory, 4 << 30);
    }

    #[test]
    fn history_is_bounded() {
        let mut h = History::default();
        for i in 0..(HISTORY_LEN + 10) {
            h.push(i as f64);
        }
        assert_eq!(h.values().count(), HISTORY_LEN);
        assert_eq!(h.values().next(), Some(10.0));
    }
}
