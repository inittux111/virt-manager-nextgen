//! A machine's processor: which CPU the guest sees (as virt-manager's
//! "Configuration" section offers it), the CPU security mitigations, and
//! the CPU layout (sockets, cores, threads).

use vmng_xml::{Element, Node};

/// The CPU features virt-manager counts as security mitigations. Only the
/// ones the host's CPU has are used (see
/// [`DomainCaps::cpu_security_features`](crate::DomainCaps::cpu_security_features)).
pub const SECURITY_FEATURES: [&str; 5] = ["spec-ctrl", "ssbd", "ibpb", "virt-ssbd", "md-clear"];

/// The CPU the guest sees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CpuMode {
    /// The host's CPU as it is (virt-manager's "Copy host CPU
    /// configuration"): fastest.
    HostPassthrough,
    /// A named model close to the host's CPU, so the machine can move to a
    /// similar host.
    HostModel,
    /// A named CPU model, e.g. `Skylake-Client`.
    Model(String),
    /// QEMU's own default.
    Default,
}

impl CpuMode {
    pub fn of(domain: &Element) -> Self {
        let Some(cpu) = domain.child("cpu") else { return Self::Default };
        match cpu.attr("mode") {
            Some("host-passthrough") => Self::HostPassthrough,
            Some("host-model") => Self::HostModel,
            _ => match cpu.child("model").map(|m| m.text().trim().to_owned()) {
                Some(model) if !model.is_empty() => Self::Model(model),
                _ => Self::Default,
            },
        }
    }
}

/// Sets which CPU the guest sees. The layout (topology) is kept;
/// mitigations only apply to a named model, so they are dropped otherwise.
pub fn set_cpu_mode(domain: &mut Element, mode: &CpuMode) {
    let cpu = domain.child_or_insert("cpu");
    for attr in ["mode", "match", "check", "migratable"] {
        cpu.remove_attr(attr);
    }
    cpu.remove_children(|c| c.name == "model");
    match mode {
        CpuMode::HostPassthrough => {
            cpu.set_attr("mode", "host-passthrough");
            cpu.set_attr("check", "none");
            cpu.set_attr("migratable", "on");
        }
        CpuMode::HostModel => cpu.set_attr("mode", "host-model"),
        CpuMode::Model(name) => {
            cpu.set_attr("mode", "custom");
            cpu.set_attr("match", "exact");
            cpu.set_attr("check", "partial");
            let model = Element::new("model").with_attr("fallback", "allow").with_text(name.as_str());
            cpu.children.insert(0, Node::Element(model));
        }
        CpuMode::Default => {}
    }
    if !matches!(mode, CpuMode::Model(_)) {
        remove_security(cpu);
    }
    if cpu.attrs.is_empty() && cpu.elements().next().is_none() {
        domain.remove_children(|c| c.name == "cpu");
    }
}

fn remove_security(cpu: &mut Element) {
    cpu.remove_children(|c| {
        c.name == "feature"
            && c.attr("policy") == Some("require")
            && c.attr("name").is_some_and(|n| SECURITY_FEATURES.contains(&n))
    });
}

/// Whether the machine has all of the host's security mitigations
/// (`available`), as virt-manager decides it.
pub fn mitigations_on(domain: &Element, available: &[String]) -> bool {
    if available.is_empty() {
        return false;
    }
    let Some(cpu) = domain.child("cpu") else { return false };
    let mut have: Vec<&str> = cpu
        .children_named("feature")
        .filter(|f| f.attr("policy") == Some("require"))
        .filter_map(|f| f.attr("name"))
        .collect();
    // Models like "Skylake-Client-IBRS" include them by name.
    if let CpuMode::Model(m) = CpuMode::of(domain) {
        if m.ends_with("IBRS") {
            have.push("spec-ctrl");
        }
        if m.ends_with("IBPB") {
            have.push("ibpb");
        }
    }
    available.iter().all(|f| have.contains(&f.as_str()))
}

/// Adds or removes the security mitigations. Only a named model takes
/// them; the host's own CPU (passthrough, host model) already has them.
pub fn set_mitigations(domain: &mut Element, available: &[String], on: bool) {
    if !matches!(CpuMode::of(domain), CpuMode::Model(_)) {
        return;
    }
    let cpu = domain.child_or_insert("cpu");
    remove_security(cpu);
    if on {
        for name in available {
            cpu.push(Element::new("feature").with_attr("policy", "require").with_attr("name", name.as_str()));
        }
    }
}

/// How the virtual CPUs are arranged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Topology {
    pub sockets: u32,
    pub dies: u32,
    pub cores: u32,
    pub threads: u32,
}

impl Topology {
    pub fn total(&self) -> u32 {
        self.sockets.max(1) * self.dies.max(1) * self.cores.max(1) * self.threads.max(1)
    }

    /// The layout virt-manager suggests for a CPU count: one socket with
    /// that many cores (some operating systems, such as Windows desktop
    /// editions, only use one or two sockets).
    pub fn for_vcpus(vcpus: u32) -> Self {
        Self { sockets: 1, dies: 1, cores: vcpus.max(1), threads: 1 }
    }

    /// The machine's layout, when one is set.
    pub fn of(domain: &Element) -> Option<Self> {
        let t = domain.find("cpu/topology")?;
        let n = |a| t.attr(a).and_then(|v| v.parse().ok()).unwrap_or(1);
        Some(Self { sockets: n("sockets"), dies: n("dies"), cores: n("cores"), threads: n("threads") })
    }
}

/// Sets the CPU layout and the CPU count that follows from it, or (with
/// `None`) leaves the layout to QEMU.
pub fn set_topology(domain: &mut Element, topology: Option<Topology>) {
    match topology {
        Some(t) => {
            let vcpu = domain.child_or_insert("vcpu");
            vcpu.remove_attr("current");
            vcpu.set_text(t.total().to_string());
            let cpu = domain.child_or_insert("cpu");
            cpu.remove_children(|c| c.name == "topology");
            let mut el = Element::new("topology").with_attr("sockets", t.sockets.max(1).to_string());
            if t.dies > 1 {
                el.set_attr("dies", t.dies.to_string());
            }
            el.set_attr("cores", t.cores.max(1).to_string());
            el.set_attr("threads", t.threads.max(1).to_string());
            // libvirt expects the topology after the model.
            let at = cpu.children.iter().position(|n| matches!(n, Node::Element(e) if e.name == "model"));
            cpu.children.insert(at.map_or(0, |i| i + 1), Node::Element(el));
        }
        None => {
            if let Some(cpu) = domain.child_mut("cpu") {
                cpu.remove_children(|c| c.name == "topology");
                if cpu.attrs.is_empty() && cpu.elements().next().is_none() {
                    domain.remove_children(|c| c.name == "cpu");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dom(inner: &str) -> Element {
        vmng_xml::parse(&format!("<domain><vcpu placement='static'>4</vcpu>{inner}</domain>")).unwrap()
    }

    #[test]
    fn reads_modes() {
        assert_eq!(CpuMode::of(&dom("")), CpuMode::Default);
        assert_eq!(CpuMode::of(&dom("<cpu mode='host-passthrough'/>")), CpuMode::HostPassthrough);
        assert_eq!(CpuMode::of(&dom("<cpu mode='host-model'/>")), CpuMode::HostModel);
        let named = dom("<cpu mode='custom' match='exact'><model fallback='allow'>Skylake-Client</model></cpu>");
        assert_eq!(CpuMode::of(&named), CpuMode::Model("Skylake-Client".into()));
        // libvirt writes a topology-only CPU as custom without a model.
        assert_eq!(CpuMode::of(&dom("<cpu mode='custom'><topology sockets='1'/></cpu>")), CpuMode::Default);
    }

    #[test]
    fn topology_survives_mode_changes() {
        let mut d = dom("");
        set_topology(&mut d, Some(Topology { sockets: 1, dies: 1, cores: 3, threads: 2 }));
        assert_eq!(d.find_text("vcpu").as_deref(), Some("6"));
        set_cpu_mode(&mut d, &CpuMode::Model("EPYC".into()));
        set_cpu_mode(&mut d, &CpuMode::Default);
        assert_eq!(Topology::of(&d).map(|t| t.total()), Some(6));
        set_cpu_mode(&mut d, &CpuMode::Model("EPYC".into()));
        let cpu = d.child("cpu").unwrap();
        let order: Vec<&str> = cpu.elements().map(|e| e.name.as_str()).collect();
        assert_eq!(order, ["model", "topology"]);
        set_topology(&mut d, None);
        set_cpu_mode(&mut d, &CpuMode::Default);
        assert!(d.child("cpu").is_none());
    }

    #[test]
    fn mitigations_follow_the_model() {
        let available: Vec<String> = ["spec-ctrl", "ssbd"].map(String::from).to_vec();
        let mut d = dom("");
        set_cpu_mode(&mut d, &CpuMode::Model("EPYC".into()));
        assert!(!mitigations_on(&d, &available));
        set_mitigations(&mut d, &available, true);
        assert!(mitigations_on(&d, &available));
        // Switching to the host's own CPU drops them.
        set_cpu_mode(&mut d, &CpuMode::HostPassthrough);
        assert!(d.find("cpu/feature").is_none());
        set_mitigations(&mut d, &available, true);
        assert!(d.find("cpu/feature").is_none());
        // Nothing to turn on on a host without them.
        assert!(!mitigations_on(&d, &[]));
    }
}
