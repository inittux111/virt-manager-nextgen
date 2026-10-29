//! Virtual networks: reading, creating, editing, starting and deleting.

use std::net::Ipv4Addr;

use virt::network::Network;
use vmng_xml::Element;

use crate::connection::Connection;
use crate::error::{Error, Result};

/// How a virtual network connects to the outside world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkMode {
    /// Share the host's connection (masquerading).
    Nat,
    /// Route to the host's network without address translation.
    Routed,
    /// Machines only see each other and the host.
    Isolated,
    /// Use an existing host bridge (no addresses handed out by libvirt).
    Bridge,
    /// Anything else (macvtap, hostdev, open, …): shown, not editable.
    Other,
}

impl NetworkMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Nat => "NAT",
            Self::Routed => "Routed",
            Self::Isolated => "Isolated",
            Self::Bridge => "Bridge",
            Self::Other => "Other",
        }
    }

    fn from_xml(net: &Element) -> Self {
        match net.find_attr("forward", "mode") {
            None if net.child("forward").is_none() => Self::Isolated,
            None | Some("nat") => Self::Nat,
            Some("route") => Self::Routed,
            Some("bridge") => Self::Bridge,
            Some(_) => Self::Other,
        }
    }
}

/// An IPv4 subnet with optional DHCP range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ipv4Config {
    /// The host's address on the network, e.g. 192.168.122.1.
    pub address: Ipv4Addr,
    pub prefix: u8,
    pub dhcp: Option<(Ipv4Addr, Ipv4Addr)>,
}

impl Ipv4Config {
    /// Parses `192.168.100.0/24` into a network whose host address is the
    /// first usable address and whose DHCP range covers the rest.
    pub fn from_cidr(cidr: &str) -> std::result::Result<Self, &'static str> {
        let (addr, prefix) =
            cidr.trim().split_once('/').ok_or("Write the network as address/size, e.g. 192.168.100.0/24.")?;
        let addr: Ipv4Addr = addr.trim().parse().map_err(|_| "That isn't a valid IPv4 address.")?;
        let prefix: u8 = prefix.trim().parse().map_err(|_| "The size after / must be a number.")?;
        if !(8..=30).contains(&prefix) {
            return Err("The size after / must be between 8 and 30.");
        }
        let mask = u32::MAX << (32 - prefix);
        let network = u32::from(addr) & mask;
        let broadcast = network | !mask;
        Ok(Self {
            address: Ipv4Addr::from(network + 1),
            prefix,
            dhcp: Some((Ipv4Addr::from(network + 2), Ipv4Addr::from(broadcast - 1))),
        })
    }

    /// `192.168.122.0/24`.
    pub fn cidr(&self) -> String {
        let mask = u32::MAX << (32 - self.prefix);
        format!("{}/{}", Ipv4Addr::from(u32::from(self.address) & mask), self.prefix)
    }

    fn netmask(&self) -> Ipv4Addr {
        Ipv4Addr::from(u32::MAX << (32 - self.prefix))
    }

    fn from_xml(ip: &Element) -> Option<Self> {
        if ip.attr("family").is_some_and(|f| f != "ipv4") {
            return None;
        }
        let address: Ipv4Addr = ip.attr("address")?.parse().ok()?;
        let prefix = match (ip.attr("prefix"), ip.attr("netmask")) {
            (Some(p), _) => p.parse().ok()?,
            (None, Some(m)) => u32::from(m.parse::<Ipv4Addr>().ok()?).count_ones() as u8,
            _ => 24,
        };
        let dhcp =
            ip.find("dhcp/range").and_then(|r| Some((r.attr("start")?.parse().ok()?, r.attr("end")?.parse().ok()?)));
        Some(Self { address, prefix, dhcp })
    }
}

/// Everything shown about a network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkDetails {
    pub name: String,
    pub active: bool,
    pub autostart: bool,
    pub persistent: bool,
    pub mode: NetworkMode,
    /// For NAT/routed: the host device to forward to (`None` = any).
    pub forward_dev: Option<String>,
    /// The bridge device (libvirt's `virbrN`, or the host bridge).
    pub bridge: Option<String>,
    pub ipv4: Option<Ipv4Config>,
    /// IPv6 address/prefix, e.g. `fd00:1::1/64`.
    pub ipv6: Option<String>,
    pub dns_domain: Option<String>,
    /// The full XML, for editing.
    pub xml: Element,
    /// The saved configuration (what a restart uses), for XML editing.
    pub config: Element,
}

impl NetworkDetails {
    pub fn from_xml(xml: Element, active: bool, autostart: bool, persistent: bool) -> Self {
        let ipv4 = xml.children_named("ip").find_map(Ipv4Config::from_xml);
        let ipv6 = xml
            .children_named("ip")
            .find(|ip| ip.attr("family") == Some("ipv6"))
            .map(|ip| format!("{}/{}", ip.attr("address").unwrap_or_default(), ip.attr("prefix").unwrap_or("64")));
        Self {
            name: xml.find_text("name").unwrap_or_default(),
            active,
            autostart,
            persistent,
            mode: NetworkMode::from_xml(&xml),
            forward_dev: xml.find_attr("forward", "dev").map(str::to_owned),
            bridge: xml.find_attr("bridge", "name").map(str::to_owned),
            ipv4,
            ipv6,
            dns_domain: xml.find_attr("domain", "name").map(str::to_owned),
            config: xml.clone(),
            xml,
        }
    }

    /// "NAT · 192.168.122.0/24", "Bridge to br0", …
    pub fn describe(&self) -> String {
        match (self.mode, &self.ipv4) {
            (NetworkMode::Bridge, _) => format!("Bridge to {}", self.bridge.as_deref().unwrap_or("?")),
            (mode, Some(ip)) => format!("{} · {}", mode.label(), ip.cidr()),
            (mode, None) => mode.label().to_owned(),
        }
    }
}

/// The settings a person can change; applied onto the existing XML so
/// anything else in it is kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkSettings {
    pub name: String,
    pub mode: NetworkMode,
    pub forward_dev: Option<String>,
    /// Host bridge for [`NetworkMode::Bridge`].
    pub bridge: Option<String>,
    pub ipv4: Option<Ipv4Config>,
    pub ipv6: Option<String>,
    pub dns_domain: Option<String>,
}

/// Writes `settings` into network XML (new or existing).
pub fn apply_network_settings(xml: &mut Element, s: &NetworkSettings) -> std::result::Result<(), &'static str> {
    if s.name.trim().is_empty() || s.name.contains(['/', ' ']) {
        return Err("The name can't be empty or contain spaces or slashes.");
    }
    xml.name = "network".into();
    xml.child_or_insert("name").set_text(s.name.trim());
    let was_host_bridge = xml.find_attr("forward", "mode") == Some("bridge");
    xml.remove_children(|e| e.name == "forward" || e.name == "ip" || e.name == "domain");
    if s.mode == NetworkMode::Bridge {
        let br = s.bridge.as_deref().filter(|b| !b.is_empty()).ok_or("Choose the host bridge to use.")?;
        xml.push(Element::new("forward").with_attr("mode", "bridge"));
        xml.remove_children(|e| e.name == "bridge" || e.name == "mac");
        xml.push(Element::new("bridge").with_attr("name", br));
        return Ok(());
    }
    // A bridge libvirt manages itself (virbrN).
    if was_host_bridge || xml.child("bridge").is_none() {
        xml.remove_children(|e| e.name == "bridge");
        xml.push(Element::new("bridge").with_attr("stp", "on").with_attr("delay", "0"));
    }
    match s.mode {
        NetworkMode::Nat | NetworkMode::Routed => {
            let mut f =
                Element::new("forward").with_attr("mode", if s.mode == NetworkMode::Nat { "nat" } else { "route" });
            if let Some(dev) = s.forward_dev.as_deref().filter(|d| !d.is_empty()) {
                f.set_attr("dev", dev);
            }
            xml.push(f);
        }
        _ => {}
    }
    if let Some(domain) = s.dns_domain.as_deref().filter(|d| !d.is_empty()) {
        xml.push(Element::new("domain").with_attr("name", domain).with_attr("localOnly", "yes"));
    }
    if let Some(ip) = &s.ipv4 {
        let mut el = Element::new("ip")
            .with_attr("address", ip.address.to_string())
            .with_attr("netmask", ip.netmask().to_string());
        if let Some((start, end)) = ip.dhcp {
            el.push(Element::new("dhcp").with_child(
                Element::new("range").with_attr("start", start.to_string()).with_attr("end", end.to_string()),
            ));
        }
        xml.push(el);
    }
    if let Some(v6) = s.ipv6.as_deref().filter(|v| !v.is_empty()) {
        let (addr, prefix) = v6.split_once('/').unwrap_or((v6, "64"));
        xml.push(Element::new("ip").with_attr("family", "ipv6").with_attr("address", addr).with_attr("prefix", prefix));
    }
    Ok(())
}

/// Checks edited network XML against the saved one: it must still be a
/// network with the same name and UUID.
pub fn check_network_xml(current: &Element, edited: &Element) -> std::result::Result<(), &'static str> {
    if edited.name != "network" {
        return Err("The XML must describe a network (<network>…</network>).");
    }
    if edited.find_text("name") != current.find_text("name") {
        return Err("Networks can't be renamed.");
    }
    if let Some(uuid) = edited.find_text("uuid")
        && current.find_text("uuid").as_deref() != Some(uuid.as_str())
    {
        return Err("The network's UUID can't be changed.");
    }
    Ok(())
}

/// Picks a free private /24 (192.168.100.0/24, 101, …).
pub fn suggest_subnet(existing: &[NetworkDetails]) -> String {
    let used: Vec<u8> = existing
        .iter()
        .filter_map(|n| n.ipv4.as_ref())
        .filter_map(|ip| {
            let o = ip.address.octets();
            (o[0] == 192 && o[1] == 168).then_some(o[2])
        })
        .collect();
    let third = (100..=254).find(|t| !used.contains(t)).unwrap_or(100);
    format!("192.168.{third}.0/24")
}

impl Connection {
    fn network(&self, name: &str) -> Result<Network> {
        Network::lookup_by_name(self.raw(), name).map_err(Error::from)
    }

    pub fn network_details(&self) -> Result<Vec<NetworkDetails>> {
        let mut out = Vec::new();
        for net in self.raw().list_all_networks(0)? {
            let xml = vmng_xml::parse(&net.get_xml_desc(0)?)?;
            let persistent = net.is_persistent().unwrap_or(true);
            let mut d = NetworkDetails::from_xml(
                xml,
                net.is_active().unwrap_or(false),
                net.get_autostart().unwrap_or(false),
                persistent,
            );
            if persistent {
                d.config = vmng_xml::parse(&net.get_xml_desc(1 /* VIR_NETWORK_XML_INACTIVE */)?)?;
            }
            out.push(d);
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Creates, starts and (optionally) autostarts a network.
    pub fn create_network(&self, settings: &NetworkSettings, autostart: bool) -> Result<()> {
        let mut xml = Element::new("network");
        apply_network_settings(&mut xml, settings).map_err(|m| Error::Invalid(m.into()))?;
        let net = Network::define_xml(self.raw(), &xml.to_xml())?;
        if let Err(e) = net.create() {
            let _ = net.undefine();
            return Err(e.into());
        }
        if autostart {
            net.set_autostart(true)?;
        }
        Ok(())
    }

    /// Saves changed settings. A running network keeps its old settings
    /// until it is restarted.
    pub fn update_network(&self, name: &str, settings: &NetworkSettings) -> Result<()> {
        let net = self.network(name)?;
        let mut xml = vmng_xml::parse(&net.get_xml_desc(1 /* VIR_NETWORK_XML_INACTIVE */)?)?;
        if settings.name != name {
            return Err(Error::Invalid("Networks can't be renamed.".into()));
        }
        apply_network_settings(&mut xml, settings).map_err(|m| Error::Invalid(m.into()))?;
        Network::define_xml(self.raw(), &xml.to_xml())?;
        Ok(())
    }

    /// Saves a network's configuration from edited XML. The name and UUID
    /// must stay the same. A running network uses it after a restart.
    pub fn define_network_xml(&self, name: &str, xml: &str) -> Result<()> {
        let net = self.network(name)?;
        let current = vmng_xml::parse(&net.get_xml_desc(1 /* VIR_NETWORK_XML_INACTIVE */)?)?;
        let edited = vmng_xml::parse(xml)?;
        check_network_xml(&current, &edited).map_err(|m| Error::Invalid(m.into()))?;
        Network::define_xml(self.raw(), &edited.to_xml())?;
        Ok(())
    }

    pub fn set_network_active(&self, name: &str, on: bool) -> Result<()> {
        let net = self.network(name)?;
        if on { net.create().map(drop) } else { net.destroy() }.map_err(Error::from)
    }

    /// Stops and starts a network so changed settings take effect.
    pub fn restart_network(&self, name: &str) -> Result<()> {
        let net = self.network(name)?;
        if net.is_active().unwrap_or(false) {
            net.destroy()?;
        }
        net.create()?;
        Ok(())
    }

    pub fn set_network_autostart(&self, name: &str, on: bool) -> Result<()> {
        self.network(name)?.set_autostart(on)?;
        Ok(())
    }

    /// Stops and removes a network.
    pub fn delete_network(&self, name: &str) -> Result<()> {
        let net = self.network(name)?;
        if net.is_active().unwrap_or(false) {
            net.destroy()?;
        }
        if net.is_persistent().unwrap_or(true) {
            net.undefine()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: &str = "<network><name>default</name><uuid>u</uuid><forward mode='nat'><nat><port start='1024' end='65535'/></nat></forward>\
        <bridge name='virbr0' stp='on' delay='0'/><mac address='52:54:00:85:94:0d'/>\
        <ip address='192.168.122.1' netmask='255.255.255.0'><dhcp><range start='192.168.122.2' end='192.168.122.254'/></dhcp></ip></network>";

    #[test]
    fn reads_default_network() {
        let d = NetworkDetails::from_xml(vmng_xml::parse(DEFAULT).unwrap(), true, true, true);
        assert_eq!(d.mode, NetworkMode::Nat);
        assert_eq!(d.bridge.as_deref(), Some("virbr0"));
        let ip = d.ipv4.clone().unwrap();
        assert_eq!(ip.cidr(), "192.168.122.0/24");
        assert_eq!(ip.dhcp.unwrap().1, Ipv4Addr::new(192, 168, 122, 254));
        assert_eq!(d.describe(), "NAT · 192.168.122.0/24");
    }

    #[test]
    fn cidr_parsing() {
        let ip = Ipv4Config::from_cidr("10.10.0.77/16").unwrap();
        assert_eq!(ip.address, Ipv4Addr::new(10, 10, 0, 1));
        assert_eq!(ip.dhcp.unwrap(), (Ipv4Addr::new(10, 10, 0, 2), Ipv4Addr::new(10, 10, 255, 254)));
        for bad in ["10.0.0.0", "10.0.0/24", "10.0.0.0/31", "x/24"] {
            assert!(Ipv4Config::from_cidr(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn edits_keep_unknown_parts() {
        let mut xml = vmng_xml::parse(DEFAULT).unwrap();
        let s = NetworkSettings {
            name: "default".into(),
            mode: NetworkMode::Isolated,
            forward_dev: None,
            bridge: None,
            ipv4: Some(Ipv4Config::from_cidr("192.168.50.0/24").unwrap()),
            ipv6: Some("fd00:1::1/64".into()),
            dns_domain: Some("vm.local".into()),
        };
        apply_network_settings(&mut xml, &s).unwrap();
        assert!(xml.child("forward").is_none());
        assert_eq!(xml.find_attr("bridge", "name"), Some("virbr0"));
        assert!(xml.child("mac").is_some() && xml.child("uuid").is_some());
        let d = NetworkDetails::from_xml(xml, false, false, true);
        assert_eq!(d.mode, NetworkMode::Isolated);
        assert_eq!(d.ipv4.unwrap().cidr(), "192.168.50.0/24");
        assert_eq!(d.ipv6.as_deref(), Some("fd00:1::1/64"));
        assert_eq!(d.dns_domain.as_deref(), Some("vm.local"));
    }

    #[test]
    fn bridge_mode_and_subnets() {
        let mut xml = Element::new("network");
        let s = NetworkSettings {
            name: "lan".into(),
            mode: NetworkMode::Bridge,
            forward_dev: None,
            bridge: Some("br0".into()),
            ipv4: None,
            ipv6: None,
            dns_domain: None,
        };
        apply_network_settings(&mut xml, &s).unwrap();
        assert_eq!(NetworkDetails::from_xml(xml, true, true, true).describe(), "Bridge to br0");
        let d = NetworkDetails::from_xml(vmng_xml::parse(DEFAULT).unwrap(), true, true, true);
        assert_eq!(suggest_subnet(&[d]), "192.168.100.0/24");
    }

    #[test]
    fn edited_xml_keeps_name_and_uuid() {
        let cur = vmng_xml::parse("<network><name>n</name><uuid>u1</uuid></network>").unwrap();
        let ok = vmng_xml::parse("<network><name>n</name><uuid>u1</uuid><mtu size='9000'/></network>").unwrap();
        assert!(check_network_xml(&cur, &ok).is_ok());
        let no_uuid = vmng_xml::parse("<network><name>n</name></network>").unwrap();
        assert!(check_network_xml(&cur, &no_uuid).is_ok());
        for bad in [
            "<network><name>m</name></network>",
            "<network><name>n</name><uuid>u2</uuid></network>",
            "<pool><name>n</name></pool>",
        ] {
            assert!(check_network_xml(&cur, &vmng_xml::parse(bad).unwrap()).is_err(), "{bad}");
        }
    }
}
