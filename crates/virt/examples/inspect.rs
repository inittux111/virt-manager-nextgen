//! Prints what the access layer sees on a connection.
//!
//! Usage: `inspect [URI]` (defaults to `qemu:///system`).

use vmng_virt::{Connection, ConnectionKind};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    vmng_virt::init();
    let uri = std::env::args().nth(1).unwrap_or_else(|| "qemu:///system".into());
    let conn = Connection::open(ConnectionKind::from_uri(&uri))?;
    println!("{:#?}", conn.host_info()?);
    println!("{:#?}", conn.host_stats()?);
    println!("{:#?}", conn.pool_summary("default")?);
    for d in conn.list_domains()? {
        println!(
            "{} [{}] os={:?} vcpus={} mem={} fw={:?} disks={:?}",
            d.display_name(),
            d.state.label(),
            d.config.os_id,
            d.config.vcpus,
            d.config.memory,
            d.config.firmware,
            d.config.disks.iter().map(|x| (&x.target, x.source_path.as_deref())).collect::<Vec<_>>()
        );
        for disk in d.config.disks.iter().filter(|x| !x.is_cdrom()) {
            println!("   {} usage {:?}", disk.target, conn.disk_usage(&d.uuid, &disk.target));
        }
    }
    println!("{:#?}", conn.domain_stats(Default::default())?);
    Ok(())
}
