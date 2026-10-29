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
            Response::Redirect(to) => current = follow_redirect(&current, &to)?,
        }
    }
    Err("The server redirected too many times.".into())
}

/// The address a redirect from `from` to `to` leads to. A secure address
/// may not lead to a plain `http` one.
fn follow_redirect(from: &str, to: &str) -> Result<String, String> {
    let next = glib::Uri::resolve_relative(Some(from), to, glib::UriFlags::NONE)
        .map_err(|e| format!("The server sent an invalid redirect: {e}"))?
        .to_string();
    validate_url(&next).map_err(|m| format!("The server redirected to an unusable address. {m}"))?;
    if from.starts_with("https:") && !next.starts_with("https:") {
        return Err("The server redirected from a secure (https) to an insecure address.".into());
    }
    Ok(next)
}

/// What the status line and headers of an answer say.
#[derive(Debug, PartialEq, Eq)]
enum Head {
    Ok { length: Option<u64> },
    Redirect(String),
}

/// Reads the status line and headers (up to the blank line) from `input`.
fn read_head(input: &mut impl Read, host: &str, target: &str, max: usize) -> Result<Head, String> {
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
    let status: u32 = head
        .split("\r\n")
        .next()
        .filter(|l| l.starts_with("HTTP/"))
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
        return header("location").map(Head::Redirect).ok_or_else(|| format!("{host} redirected nowhere."));
    }
    if status != 200 {
        return Err(match status {
            404 => format!("Not found on {host}: {target}"),
            s => format!("{host} answered with error {s} for {target}"),
        });
    }
    let length = match header("content-length") {
        Some(v) => Some(v.parse::<u64>().map_err(|_| format!("{host} sent an invalid file size."))?),
        None => None,
    };
    if length.is_some_and(|t| t > max as u64) {
        return Err(format!("The file on {host} is too large."));
    }
    Ok(Head::Ok { length })
}

/// Reads the body of an answer: at most `max` bytes, and exactly `total`
/// when the server gave the length.
fn read_body(
    input: &mut impl Read,
    host: &str,
    total: Option<u64>,
    max: usize,
    progress: &dyn Fn(u64, Option<u64>),
) -> Result<Vec<u8>, String> {
    // `total` is at most `max` (checked by `read_head`).
    let mut body = Vec::with_capacity(total.map_or(0, |t| t.min(max as u64) as usize));
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
    Ok(body)
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

    match read_head(&mut input, &host, &target, max)? {
        Head::Redirect(to) => Ok(Response::Redirect(to)),
        Head::Ok { length } => read_body(&mut input, &host, length, max, progress).map(Response::Body),
    }
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

    fn head(answer: &str) -> Result<Head, String> {
        read_head(&mut answer.as_bytes(), "h", "/f", 1000)
    }

    #[test]
    fn reads_answer_heads() {
        assert_eq!(head("HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\n"), Ok(Head::Ok { length: Some(12) }));
        assert_eq!(head("HTTP/1.0 200 OK\r\nServer: x\r\n\r\nbody"), Ok(Head::Ok { length: None }));
        assert_eq!(head("HTTP/1.1 302 Found\r\nlocation:  /next \r\n\r\n"), Ok(Head::Redirect("/next".into())));
        assert!(head("HTTP/1.1 302 Found\r\n\r\n").is_err(), "redirect without a location");
        assert!(head("HTTP/1.1 404 Not Found\r\n\r\n").unwrap_err().contains("Not found"));
        assert!(head("HTTP/1.1 500 Oops\r\n\r\n").is_err());
        assert!(head("SSH-2.0-OpenSSH\r\n\r\n").is_err(), "not HTTP");
        assert!(head("HTTP/1.1 abc\r\n\r\n").is_err());
        assert!(head("HTTP/1.1 200 OK\r\nContent-Length: 1001\r\n\r\n").unwrap_err().contains("too large"));
        assert!(head("HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n").is_err());
        assert!(head("HTTP/1.1 200 OK\r\nContent-Length: 99999999999999999999999\r\n\r\n").is_err());
        assert!(head("HTTP/1.1 200 OK\r\n").unwrap_err().contains("closed"), "cut off in the headers");
        assert!(head(&format!("HTTP/1.1 200 OK\r\nX: {}\r\n\r\n", "a".repeat(MAX_HEADER))).is_err());
        assert!(head("HTTP/1.1 200 \u{fffd}\u{0}\r\n\r\n").is_ok(), "odd bytes in the reason are fine");
    }

    #[test]
    fn reads_bodies_within_limits() {
        let body = |data: &[u8], total, max| read_body(&mut &data[..], "h", total, max, &|_, _| {});
        assert_eq!(body(b"hello", Some(5), 10), Ok(b"hello".to_vec()));
        assert_eq!(body(b"hello", None, 10), Ok(b"hello".to_vec()));
        assert!(body(b"hel", Some(5), 10).unwrap_err().contains("cut off"));
        assert!(body(b"hello world", Some(5), 100).unwrap_err().contains("cut off"), "longer than announced");
        assert!(body(b"hello world", None, 5).unwrap_err().contains("too large"));
        assert_eq!(body(b"", None, 5), Ok(Vec::new()));
    }

    #[test]
    fn redirects_stay_on_the_web_and_secure() {
        assert_eq!(follow_redirect("https://a.example/x/y", "z").unwrap(), "https://a.example/x/z");
        assert_eq!(follow_redirect("http://a.example/x", "https://b.example/").unwrap(), "https://b.example/");
        assert!(follow_redirect("https://a.example/x", "http://b.example/").is_err(), "downgrade");
        assert!(follow_redirect("https://a.example/x", "file:///etc/passwd").is_err());
        assert!(follow_redirect("https://a.example/x", "ftp://b.example/").is_err());
    }

    #[test]
    fn odd_tree_addresses_never_panic() {
        for url in
            ["", "/", "/dists/", "x/dists//", "é/dists/ü/installer-amd64/", "https://h/dists/sid/main/installer-amd64"]
        {
            let _ = debian(url);
        }
        assert!(debian("https://h/dists/").is_none());
    }

    #[test]
    fn only_web_addresses() {
        assert!(validate_url("https://deb.debian.org/debian/").is_ok());
        assert!(validate_url("ftp://example.org/").is_err());
        assert!(validate_url("file:///etc/passwd").is_err());
        assert!(validate_url("not a url").is_err());
    }
}
