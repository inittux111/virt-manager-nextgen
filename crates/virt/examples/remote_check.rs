//! Checks a connection to another computer over SSH: lists machines,
//! creates `vmng-test-remote` there, turns autostart on and off, starts it,
//! reaches its VNC screen through an SSH tunnel and its text console, and
//! deletes it again. Also checks autostart on this computer.
//!
//! Usage: `remote_check <qemu+ssh://… address>` (see tests/remote-host.sh)

use std::io::Read;
use std::os::unix::net::UnixStream;

use vmng_virt::{
    Connection, ConnectionKind, FirmwareChoice, GuestFamily, InstallSource, NetworkChoice, NewVm, PowerAction,
};

type Res<T> = Result<T, Box<dyn std::error::Error>>;

fn check(ok: bool, what: &str) -> Res<()> {
    if ok {
        println!("ok  {what}");
        Ok(())
    } else {
        Err(format!("FAILED: {what}").into())
    }
}

fn vm(name: &str) -> NewVm {
    NewVm {
        name: name.into(),
        os_id: None,
        family: GuestFamily::Linux,
        arch: "x86_64".into(),
        machine: Some("q35".into()),
        virt_type: "kvm".into(),
        memory: 256 << 20,
        vcpus: 1,
        firmware: FirmwareChoice::Bios,
        tpm: false,
        cpu_model: "host-passthrough".into(),
        install: InstallSource::Manual,
        disk: None,
        existing_disk: None,
        network: NetworkChoice::None,
        graphics: "vnc".into(),
        virtio: true,
        video: "virtio".into(),
    }
}

fn autostart(conn: &Connection, uuid: &str) -> Res<bool> {
    let d = conn.list_domains()?.into_iter().find(|d| d.uuid == uuid).ok_or("machine missing")?;
    Ok(d.autostart)
}

fn main() -> Res<()> {
    vmng_virt::init();
    let uri = std::env::args().nth(1).ok_or("usage: remote_check <qemu+ssh://…>")?;
    let kind = ConnectionKind::from_uri(&uri);
    check(kind.is_remote() && kind.ssh_target().is_some(), "address read as an SSH connection")?;
    let conn = Connection::open(kind)?;
    let host = conn.host_info()?;
    println!("other computer: {} ({} CPUs, libvirt {})", host.hostname, host.cpus, host.libvirt_version);
    let result = remote(&conn);
    if let Some(d) = conn.list_domains()?.into_iter().find(|d| d.config.name == "vmng-test-remote") {
        let _ = conn.power(&d.uuid, PowerAction::ForceOff);
        println!("cleanup: {:?}", conn.delete_domain(&d.uuid, &[]));
    }
    result?;

    // Autostart on this computer (VMNG_TEST_URI, by default the user session).
    let local_uri = std::env::var("VMNG_TEST_URI").unwrap_or_else(|_| "qemu:///session".into());
    let local = Connection::open(ConnectionKind::from_uri(&local_uri))?;
    let uuid = local.create_vm(&vm("vmng-test-autostart"), false)?.uuid;
    let r = (|| -> Res<()> {
        local.set_domain_autostart(&uuid, true)?;
        check(autostart(&local, &uuid)?, "autostart on (this computer)")?;
        local.set_domain_autostart(&uuid, false)?;
        check(!autostart(&local, &uuid)?, "autostart off (this computer)")
    })();
    println!("cleanup: {:?}", local.delete_domain(&uuid, &[]));
    r?;
    println!("all checks passed");
    Ok(())
}

fn remote(conn: &Connection) -> Res<()> {
    let before = conn.list_domains()?.len();
    let uuid = conn.create_vm(&vm("vmng-test-remote"), false)?.uuid;
    check(conn.list_domains()?.len() == before + 1, "machine created on the other computer")?;
    conn.set_domain_autostart(&uuid, true)?;
    check(autostart(conn, &uuid)?, "autostart on")?;
    conn.set_domain_autostart(&uuid, false)?;
    check(!autostart(conn, &uuid)?, "autostart off")?;

    conn.power(&uuid, PowerAction::Start)?;
    std::thread::sleep(std::time::Duration::from_secs(2));
    let fd = conn.open_graphics_fd(&uuid, 0)?;
    let mut s = UnixStream::from(fd);
    s.set_read_timeout(Some(std::time::Duration::from_secs(20)))?;
    let mut hello = [0u8; 12];
    s.read_exact(&mut hello)?;
    let hello = String::from_utf8_lossy(&hello);
    check(hello.starts_with("RFB "), &format!("VNC screen through the SSH tunnel ({})", hello.trim()))?;
    drop(s);
    let console = conn.open_console(&uuid, true);
    check(console.is_ok(), "text console over SSH")?;
    let stats = conn.domain_stats(vmng_virt::StatsWanted { cpu: true, memory: true, network: true })?;
    check(stats.iter().any(|s| s.uuid == uuid), "statistics over SSH")?;
    conn.power(&uuid, PowerAction::ForceOff)?;
    Ok(())
}
