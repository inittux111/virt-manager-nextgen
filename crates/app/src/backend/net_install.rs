//! Network installs: where an install tree keeps its installer, and
//! fetching it.
//!
//! Trees with a `.treeinfo` (Fedora, RHEL and its rebuilds, openSUSE) are
//! read by libosinfo. Debian trees have none, so their netboot installer is
//! found by its fixed layout, as virt-install does.
//!
//! Downloads use GIO's sockets (TLS through glib-networking, with the
//! system's certificates), so no HTTP library is needed. Only `http` and
//! `https` are allowed, sizes and redirects are limited, and a plain
//! HTTP/1.0 request keeps the response simple (no chunked encoding).

use std::io::Read;

use gtk::prelude::*;
use gtk::{gio, glib};
use vmng_osinfo::InstallTree;

/// The largest installer file accepted (initrds are a few hundred MiB).
pub const MAX_FILE: usize = 1 << 30;
const MAX_REDIRECTS: usize = 8;
const MAX_HEADER: usize = 64 << 10;
const TIMEOUT_SECS: u32 = 30;

/// The installer of a network install tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeInstaller {
    pub os_id: Option<String>,
    pub kernel_url: String,
    pub initrd_url: String,
    /// Kernel arguments that point the installer at the tree.
    pub args: String,
}

fn with_slash(url: &str) -> String {
    if url.ends_with('/') { url.to_owned() } else { format!("{url}/") }
}

/// The installer from a tree libosinfo read.
pub fn from_treeinfo(url: &str, tree: &InstallTree) -> Option<TreeInstaller> {
    let base = with_slash(url);
    Some(TreeInstaller {
        os_id: tree.os.as_ref().map(|o| o.id.clone()),
        kernel_url: format!("{base}{}", tree.kernel.as_deref()?.trim_start_matches('/')),
        initrd_url: format!("{base}{}", tree.initrd.as_deref()?.trim_start_matches('/')),
        args: tree.url_argument.as_ref().map(|a| format!("{a}={url}")).unwrap_or_default(),
    })
}

/// Debian's netboot installer, for a mirror address such as
/// `https://deb.debian.org/debian/dists/trixie/` or
/// `…/dists/trixie/main/installer-amd64/`.
pub fn debian(url: &str) -> Option<TreeInstaller> {
    let base = with_slash(url);
    let (_, after) = base.split_once("/dists/")?;
    let codename = after.split('/').next().filter(|c| !c.is_empty())?;
    let installer = if base.contains("/installer-amd64/") {
        format!("{}installer-amd64/", &base[..base.find("installer-amd64/")?])
    } else {
        format!("{base}main/installer-amd64/")
    };
    let images = format!("{installer}current/images/netboot/debian-installer/amd64/");
    let version = match codename {
        "bookworm" => "12",
        "trixie" | "stable" => "13",
        "forky" => "14",
        _ => "testing",
    };
    Some(TreeInstaller {
        os_id: Some(format!("http://debian.org/debian/{version}")),
        kernel_url: format!("{images}linux"),
        initrd_url: format!("{images}initrd.gz"),
        // The Debian installer asks for a mirror itself.
        args: String::new(),
    })
}

/// Checks an address typed by a person.
pub fn validate_url(url: &str) -> Result<(), &'static str> {
    let uri = glib::Uri::parse(url, glib::UriFlags::NONE).map_err(|_| "Enter a full address, e.g. https://….")?;
    match uri.scheme().as_str() {
        "http" | "https" if uri.host().is_some_and(|h| !h.is_empty()) => Ok(()),
        "http" | "https" => Err("The address has no server name."),
        _ => Err("Use an http:// or https:// address."),
    }
}

/// Downloads `url`, following redirects. `progress` gets the bytes read so
/// far and the total, when the server says it. Blocking: run it on a
/// worker thread.
pub fn fetch(url: &str, max: usize, progress: &dyn Fn(u64, Option<u64>)) -> Result<Vec<u8>, String> {
    let mut current = url.to_owned();
    for _ in 0..=MAX_REDIRECTS {
        match get(&current, max, progress)? {
            Response::Body(b) => return Ok(b),
            Response::Redirect(to) => {
                current = glib::Uri::resolve_relative(Some(&current), &to, glib::UriFlags::NONE)
                    .map_err(|e| format!("The server sent an invalid redirect: {e}"))?
                    .to_string();
                validate_url(&current).map_err(|m| format!("The server redirected to an unusable address. {m}"))?;
            }
        }
    }
    Err("The server redirected too many times.".into())
}

enum Response {
    Body(Vec<u8>),
    Redirect(String),
}

fn get(url: &str, max: usize, progress: &dyn Fn(u64, Option<u64>)) -> Result<Response, String> {
    let uri = glib::Uri::parse(url, glib::UriFlags::NONE).map_err(|e| e.to_string())?;
    let https = uri.scheme() == "https";
    let host = uri.host().ok_or("The address has no server name.")?.to_string();
    let port = if uri.port() > 0 {
        uri.port() as u16
    } else if https {
        443
    } else {
        80
    };
    let mut target = uri.path().to_string();
    if target.is_empty() {
        target.push('/');
    }
    if let Some(q) = uri.query() {
        target.push('?');
        target.push_str(&q);
    }
    if target.contains(['\r', '\n']) || host.contains(['\r', '\n', '/']) {
        return Err("The address contains invalid characters.".into());
    }

    let client = gio::SocketClient::new();
    client.set_tls(https);
    client.set_timeout(TIMEOUT_SECS);
    let conn = client
        .connect_to_host(&format!("{host}:{port}"), port, gio::Cancellable::NONE)
        .map_err(|e| format!("Couldn't connect to {host}: {e}"))?;
    let request = format!(
        "GET {target} HTTP/1.0\r\nHost: {host}\r\nUser-Agent: virt-manager-nextgen\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    conn.output_stream()
        .write_all(request.as_bytes(), gio::Cancellable::NONE)
        .map_err(|e| format!("Couldn't send the request to {host}: {e}"))?;
    let mut input = conn.input_stream().into_read();

    // Status line and headers.
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > MAX_HEADER {
            return Err(format!("{host} sent an oversized answer."));
        }
        match input.read(&mut byte) {
            Ok(0) => return Err(format!("{host} closed the connection.")),
            Ok(_) => head.push(byte[0]),
            Err(e) => return Err(format!("Couldn't read from {host}: {e}")),
        }
    }
    let head = String::from_utf8_lossy(&head);
    let mut lines = head.split("\r\n");
    let status: u32 = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| format!("{host} sent an answer that isn't HTTP."))?;
    let header = |name: &str| {
        head.split("\r\n").skip(1).find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
        })
    };
    if matches!(status, 301 | 302 | 303 | 307 | 308) {
        return header("location").map(Response::Redirect).ok_or_else(|| format!("{host} redirected nowhere."));
    }
    if status != 200 {
        return Err(match status {
            404 => format!("Not found on {host}: {target}"),
            s => format!("{host} answered with error {s} for {target}"),
        });
    }
    let total = header("content-length").and_then(|v| v.parse::<u64>().ok());
    if total.is_some_and(|t| t > max as u64) {
        return Err(format!("The file on {host} is too large."));
    }

    let mut body = Vec::with_capacity(total.map_or(0, |t| t as usize));
    let mut buf = vec![0u8; 256 << 10];
    let mut last = 0u64;
    loop {
        let n = input.read(&mut buf).map_err(|e| format!("The download from {host} failed: {e}"))?;
        if n == 0 {
            break;
        }
        if body.len() + n > max {
            return Err(format!("The file on {host} is too large."));
        }
        body.extend_from_slice(&buf[..n]);
        if body.len() as u64 - last >= 4 << 20 {
            last = body.len() as u64;
            progress(last, total);
        }
    }
    if total.is_some_and(|t| t != body.len() as u64) {
        return Err(format!("The download from {host} was cut off."));
    }
    progress(body.len() as u64, total);
    Ok(Response::Body(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_debian_netboot() {
        for url in [
            "https://deb.debian.org/debian/dists/trixie",
            "https://deb.debian.org/debian/dists/trixie/main/installer-amd64/",
        ] {
            let d = debian(url).unwrap();
            assert_eq!(
                d.kernel_url,
                "https://deb.debian.org/debian/dists/trixie/main/installer-amd64/current/images/netboot/debian-installer/amd64/linux"
            );
            assert!(d.initrd_url.ends_with("/amd64/initrd.gz"));
            assert_eq!(d.os_id.as_deref(), Some("http://debian.org/debian/13"));
        }
        assert!(debian("https://example.org/os/").is_none());
    }

    #[test]
    fn treeinfo_paths_join_the_address() {
        let tree = InstallTree {
            os: None,
            kernel: Some("images/pxeboot/vmlinuz".into()),
            initrd: Some("images/pxeboot/initrd.img".into()),
            url_argument: Some("inst.repo".into()),
        };
        let t = from_treeinfo("https://mirror.example/fedora/os", &tree).unwrap();
        assert_eq!(t.kernel_url, "https://mirror.example/fedora/os/images/pxeboot/vmlinuz");
        assert_eq!(t.args, "inst.repo=https://mirror.example/fedora/os");
    }

    #[test]
    fn only_web_addresses() {
        assert!(validate_url("https://deb.debian.org/debian/").is_ok());
        assert!(validate_url("ftp://example.org/").is_err());
        assert!(validate_url("file:///etc/passwd").is_err());
        assert!(validate_url("not a url").is_err());
    }
}
