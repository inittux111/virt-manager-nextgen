//! Connections to other computers over SSH (`qemu+ssh://`).
//!
//! libvirt runs the system `ssh` client, so logins use the person's SSH
//! keys and agent (no passwords are typed into the app). A machine's
//! screen can't be handed over as a local socket across SSH, so it is
//! reached through a tunnel: `ssh -W` to the display's port on the other
//! computer, connected to one end of a local socket pair.

use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};

use vmng_xml::Element;

use crate::error::{Error, Result};

/// Where an SSH connection goes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SshTarget {
    /// The login name (`None` = SSH's default, usually the same name).
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
    /// The other computer's user session instead of its system VMs.
    pub session: bool,
    /// For tests: a key file, and skipping the host key check.
    pub keyfile: Option<String>,
    pub no_verify: bool,
}

fn valid_name(s: &str, extra: &[char]) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.len() <= 253
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') || extra.contains(&c))
}

impl SshTarget {
    /// Checks names typed by a person. Nothing that SSH could read as an
    /// option or that would change the address gets through.
    pub fn validate(&self) -> std::result::Result<(), &'static str> {
        let host = self.host.trim_start_matches('[').trim_end_matches(']');
        if !valid_name(host, &[':']) {
            return Err("Enter the computer's name or address, e.g. server.local or 192.168.1.20.");
        }
        if let Some(u) = &self.user
            && !valid_name(u, &[])
        {
            return Err("The user name can only contain letters, digits, dots, dashes and underscores.");
        }
        if self.port == Some(0) {
            return Err("The port must be between 1 and 65535.");
        }
        if let Some(k) = &self.keyfile
            && (k.is_empty() || k.contains(['&', '?', '#']))
        {
            return Err("Invalid key file.");
        }
        Ok(())
    }

    fn host_part(&self) -> String {
        // An IPv6 address goes in brackets.
        if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        }
    }

    /// The libvirt address, e.g. `qemu+ssh://me@server/system`.
    pub fn uri(&self) -> String {
        let mut uri = String::from("qemu+ssh://");
        if let Some(u) = &self.user {
            uri.push_str(u);
            uri.push('@');
        }
        uri.push_str(&self.host_part());
        if let Some(p) = self.port {
            uri.push_str(&format!(":{p}"));
        }
        uri.push_str(if self.session { "/session" } else { "/system" });
        let mut query = Vec::new();
        if let Some(k) = &self.keyfile {
            query.push(format!("keyfile={k}"));
        }
        if self.no_verify {
            query.push("no_verify=1".to_owned());
        }
        if !query.is_empty() {
            uri.push('?');
            uri.push_str(&query.join("&"));
        }
        uri
    }

    /// Reads a `qemu+ssh://` address.
    pub fn parse(uri: &str) -> Option<Self> {
        let rest = uri.strip_prefix("qemu+ssh://")?;
        let (main, query) = rest.split_once('?').unwrap_or((rest, ""));
        let (authority, path) = main.split_once('/').unwrap_or((main, "system"));
        let (user, hostport) = match authority.rsplit_once('@') {
            Some((u, h)) => (Some(u.to_owned()), h),
            None => (None, authority),
        };
        let (host, port) = if let Some(rest) = hostport.strip_prefix('[') {
            let (h, after) = rest.split_once(']')?;
            (h.to_owned(), after.strip_prefix(':').and_then(|p| p.parse().ok()))
        } else {
            match hostport.rsplit_once(':') {
                Some((h, p)) if !h.contains(':') => (h.to_owned(), Some(p.parse().ok()?)),
                _ => (hostport.to_owned(), None),
            }
        };
        let mut t =
            SshTarget { user, host, port, session: path.trim_end_matches('/') == "session", ..Default::default() };
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            match pair.split_once('=') {
                Some(("keyfile", k)) => t.keyfile = Some(k.to_owned()),
                Some(("no_verify", v)) => t.no_verify = v == "1",
                _ => {}
            }
        }
        t.validate().ok()?;
        Some(t)
    }

    /// `me@server`, or `me@server (user session)`.
    pub fn label(&self) -> String {
        let mut s = match &self.user {
            Some(u) => format!("{u}@{}", self.host),
            None => self.host.clone(),
        };
        if let Some(p) = self.port {
            s.push_str(&format!(":{p}"));
        }
        if self.session {
            s.push_str(" (user session)");
        }
        s
    }

    /// The `ssh` arguments that reach this computer (before the command).
    fn ssh_args(&self) -> Vec<String> {
        // BatchMode: never wait for a password prompt nobody can see.
        let mut a: Vec<String> = ["-o", "BatchMode=yes", "-o", "ConnectTimeout=15"].map(String::from).to_vec();
        if let Some(p) = self.port {
            a.extend(["-p".into(), p.to_string()]);
        }
        if let Some(u) = &self.user {
            a.extend(["-l".into(), u.clone()]);
        }
        if let Some(k) = &self.keyfile {
            a.extend(["-i".into(), k.clone()]);
        }
        if self.no_verify {
            a.extend(
                ["-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null", "-o", "LogLevel=ERROR"]
                    .map(String::from),
            );
        }
        a
    }

    /// Opens a tunnel to `dest` on the other computer (`host:port`, or a
    /// socket path) and returns the local end.
    pub fn tunnel(&self, dest: &str) -> Result<OwnedFd> {
        self.validate().map_err(|m| Error::Invalid(m.into()))?;
        let (local, remote) = UnixStream::pair().map_err(|e| Error::Invalid(e.to_string()))?;
        let remote: OwnedFd = remote.into();
        let out = remote.try_clone().map_err(|e| Error::Invalid(e.to_string()))?;
        let mut child = Command::new("ssh")
            .args(self.ssh_args())
            .arg("-W")
            .arg(dest)
            .arg("--")
            .arg(&self.host)
            .stdin(Stdio::from(remote))
            .stdout(Stdio::from(out))
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| Error::Invalid(format!("Couldn't start ssh: {e}")))?;
        // ssh ends when the display closes its end; collect it then.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(local.into())
    }
}

/// Where the display `index` (position among all `<graphics>`) of a running
/// machine listens, as seen from that computer: `host:port` or a socket.
pub fn display_address(live: &Element, index: usize) -> Option<String> {
    let g = live.find("devices")?.children_named("graphics").nth(index)?;
    let listen = g.children_named("listen").next();
    if let Some(sock) = listen.and_then(|l| l.attr("socket")).or_else(|| g.attr("socket")) {
        return Some(sock.to_owned());
    }
    let port: u32 = g.attr("port")?.parse().ok().filter(|p| *p > 0)?;
    let address = listen.and_then(|l| l.attr("address")).or_else(|| g.attr("listen")).unwrap_or("127.0.0.1");
    // Listening everywhere includes the local address.
    let address = match address {
        "" | "0.0.0.0" | "::" => "127.0.0.1",
        a => a,
    };
    Some(if address.contains(':') { format!("[{address}]:{port}") } else { format!("{address}:{port}") })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_round_trip() {
        for uri in [
            "qemu+ssh://me@server.local/system",
            "qemu+ssh://server/session",
            "qemu+ssh://root@192.168.1.20:2222/system?keyfile=/tmp/k&no_verify=1",
            "qemu+ssh://me@[fd00::1]:22/system",
        ] {
            let t = SshTarget::parse(uri).unwrap();
            assert_eq!(t.uri(), uri);
        }
        let t = SshTarget::parse("qemu+ssh://me@server.local/system").unwrap();
        assert_eq!(t.label(), "me@server.local");
        assert!(SshTarget::parse("qemu:///system").is_none());
    }

    #[test]
    fn refuses_option_injection() {
        for (user, host) in [(None, "-oProxyCommand=x"), (Some("-l"), "server"), (None, "a b"), (None, "srv/x")] {
            let t = SshTarget { user: user.map(String::from), host: host.into(), ..Default::default() };
            assert!(t.validate().is_err(), "{user:?} {host}");
        }
        assert!(SshTarget::parse("qemu+ssh://-oProxyCommand=evil/system").is_none());
    }

    #[test]
    fn display_addresses() {
        let x = |s: &str| vmng_xml::parse(s).unwrap();
        let spice = x("<domain><devices><graphics type='spice' port='5901' autoport='yes' listen='127.0.0.1'>\
                       <listen type='address' address='127.0.0.1'/></graphics></devices></domain>");
        assert_eq!(display_address(&spice, 0).as_deref(), Some("127.0.0.1:5901"));
        let any = x("<domain><devices><graphics type='vnc' port='5900' listen='0.0.0.0'/></devices></domain>");
        assert_eq!(display_address(&any, 0).as_deref(), Some("127.0.0.1:5900"));
        let sock = x("<domain><devices><graphics type='vnc'><listen type='socket' socket='/run/v.sock'/></graphics>\
                      </devices></domain>");
        assert_eq!(display_address(&sock, 0).as_deref(), Some("/run/v.sock"));
        let off = x("<domain><devices><graphics type='vnc' port='-1'/></devices></domain>");
        assert_eq!(display_address(&off, 0), None);
    }
}
