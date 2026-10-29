//! Creates, edits and removes a throwaway network and storage pool
//! (`vmng-test-net`, `vmng-test-pool`): network settings and edited XML,
//! thin, fully allocated and based-on (backing file) volumes.
//!
//! Usage: `net_pool_check <uri> <empty-dir-for-pool>`

use vmng_virt::{
    Connection, ConnectionKind, Ipv4Config, NetworkMode, NetworkSettings, NewPool, PoolKind, VolumeOptions,
    suggest_subnet,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    vmng_virt::init();
    let mut a = std::env::args().skip(1);
    let (uri, dir) = match (a.next(), a.next()) {
        (Some(u), Some(d)) => (u, d),
        _ => return Err("usage: net_pool_check <uri> <empty-dir>".into()),
    };
    let conn = Connection::open(ConnectionKind::from_uri(&uri))?;

    let nets = conn.network_details()?;
    let subnet = suggest_subnet(&nets);
    let mut s = NetworkSettings {
        name: "vmng-test-net".into(),
        mode: NetworkMode::Isolated,
        forward_dev: None,
        bridge: None,
        ipv4: Some(Ipv4Config::from_cidr(&subnet)?),
        ipv6: None,
        dns_domain: None,
    };
    let net_result = (|| -> Result<(), Box<dyn std::error::Error>> {
        conn.create_network(&s, false)?;
        s.dns_domain = Some("test.local".into());
        conn.update_network("vmng-test-net", &s)?;
        conn.restart_network("vmng-test-net")?;
        conn.set_network_autostart("vmng-test-net", true)?;
        let d = conn.network_details()?.into_iter().find(|n| n.name == "vmng-test-net").ok_or("missing")?;
        assert!(d.active && d.autostart);
        assert_eq!(d.dns_domain.as_deref(), Some("test.local"));
        println!("network ok: {} ({})", d.describe(), d.bridge.clone().unwrap_or_default());
        // Edited XML: a new MTU is saved; renaming is refused.
        let mut xml = d.config.clone();
        xml.push(vmng_xml::Element::new("mtu").with_attr("size", "1400"));
        conn.define_network_xml("vmng-test-net", &xml.to_xml())?;
        let d = conn.network_details()?.into_iter().find(|n| n.name == "vmng-test-net").ok_or("missing")?;
        assert_eq!(d.config.find_attr("mtu", "size"), Some("1400"), "edited XML saved");
        let renamed = xml.to_xml().replace("<name>vmng-test-net</name>", "<name>vmng-test-other</name>");
        assert!(conn.define_network_xml("vmng-test-net", &renamed).is_err(), "rename refused");
        assert!(conn.define_network_xml("vmng-test-net", "<network><name>").is_err(), "bad XML refused");
        println!("network XML ok");
        conn.set_network_active("vmng-test-net", false)?;
        Ok(())
    })();
    let _ = conn.delete_network("vmng-test-net");

    let pool_result = (|| -> Result<(), Box<dyn std::error::Error>> {
        conn.create_pool(&NewPool {
            name: "vmng-test-pool".into(),
            kind: PoolKind::Folder,
            path: dir.clone(),
            host: String::new(),
            source: String::new(),
        })?;
        let base = conn.create_named_volume("vmng-test-pool", "a.qcow2", 1 << 30, "qcow2", &Default::default())?;
        let full = VolumeOptions { allocate: true, backing: None };
        conn.create_named_volume("vmng-test-pool", "full.img", 64 << 20, "raw", &full)?;
        conn.create_named_volume("vmng-test-pool", "full.qcow2", 64 << 20, "qcow2", &full)?;
        let child = VolumeOptions { allocate: false, backing: Some(base.clone()) };
        conn.create_named_volume("vmng-test-pool", "child.qcow2", 1 << 30, "qcow2", &child)?;
        assert!(
            conn.create_named_volume("vmng-test-pool", "child.img", 1 << 30, "raw", &child).is_err(),
            "raw can't have a base"
        );
        let vols = conn.list_volumes("vmng-test-pool")?;
        let vol = |n: &str| vols.iter().find(|v| v.name == n).ok_or(format!("missing {n}"));
        assert_eq!(vols.len(), 4);
        assert_eq!(vol("a.qcow2")?.format.as_deref(), Some("qcow2"));
        let a = vol("a.qcow2")?.allocation;
        let f = vol("full.img")?.allocation;
        let fq = vol("full.qcow2")?.allocation;
        println!("allocation: thin {a}, full raw {f}, full qcow2 {fq}");
        assert!(a < 16 << 20, "thin image is small");
        assert!(f >= 64 << 20, "raw image fully allocated");
        assert!(fq >= 64 << 20, "qcow2 image fully allocated");
        assert_eq!(vol("child.qcow2")?.backing.as_deref(), Some(base.as_str()), "child image is based on a.qcow2");
        assert_eq!(vol("a.qcow2")?.backing, None);
        println!("backing file ok");
        for n in ["child.qcow2", "full.qcow2", "full.img"] {
            conn.delete_volume("vmng-test-pool", n)?;
        }
        let p = conn.pool_details()?.into_iter().find(|p| p.name == "vmng-test-pool").ok_or("missing pool")?;
        assert!(p.active && p.autostart);
        println!("pool ok: {:?} {} volumes", p.path, vols.len());
        conn.delete_volume("vmng-test-pool", "a.qcow2")?;
        assert!(conn.list_volumes("vmng-test-pool")?.is_empty());
        conn.set_pool_active("vmng-test-pool", false)?;
        conn.set_pool_active("vmng-test-pool", true)?;
        Ok(())
    })();
    let _ = conn.remove_pool("vmng-test-pool");
    net_result?;
    pool_result?;
    println!("all checks passed");
    Ok(())
}
