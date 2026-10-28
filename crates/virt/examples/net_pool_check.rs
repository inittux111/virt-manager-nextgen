//! Creates, edits and removes a throwaway network and storage pool
//! (`vmng-test-net`, `vmng-test-pool`).
//!
//! Usage: `net_pool_check <uri> <empty-dir-for-pool>`

use vmng_virt::{
    Connection, ConnectionKind, Ipv4Config, NetworkMode, NetworkSettings, NewPool, PoolKind, suggest_subnet,
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
        println!("network ok: {} ({})", d.describe(), d.bridge.unwrap_or_default());
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
        conn.create_named_volume("vmng-test-pool", "a.qcow2", 1 << 30, "qcow2")?;
        let vols = conn.list_volumes("vmng-test-pool")?;
        assert_eq!(vols.len(), 1);
        assert_eq!(vols[0].format.as_deref(), Some("qcow2"));
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
