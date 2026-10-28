//! Access to the osinfo database from the UI.
//!
//! The database is loaded once on a dedicated thread at startup. That
//! thread keeps owning it (libosinfo objects are not thread safe) and
//! answers install-media identification requests. A compact copy of the OS
//! list is published for instant lookups from the UI thread.

// Media identification and the full OS list are used by the New VM wizard.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::mpsc;

use gtk::glib;
use vmng_osinfo::{OsDb, OsInfo};

static CATALOG: OnceLock<HashMap<String, OsInfo>> = OnceLock::new();
static WORKER: OnceLock<mpsc::Sender<Request>> = OnceLock::new();

enum Request {
    IdentifyMedia(String, futures_oneshot::Sender<Result<Option<OsInfo>, String>>),
}

/// Starts loading the database in the background. Safe to call more than once.
pub fn preload() {
    if WORKER.get().is_some() {
        return;
    }
    let (tx, rx) = mpsc::channel::<Request>();
    if WORKER.set(tx).is_err() {
        return;
    }
    let spawned = std::thread::Builder::new().name("osinfo".into()).spawn(move || {
        let db = match OsDb::load() {
            Ok(db) => db,
            Err(e) => {
                glib::g_warning!("osinfo", "could not load the osinfo database: {e}");
                let _ = CATALOG.set(HashMap::new());
                for req in rx {
                    match req {
                        Request::IdentifyMedia(_, reply) => reply.send(Err(e.to_string())),
                    }
                }
                return;
            }
        };
        let _ = CATALOG.set(db.all().into_iter().map(|os| (os.id.clone(), os)).collect());
        for req in rx {
            match req {
                Request::IdentifyMedia(path, reply) => reply.send(db.identify_media(&path).map_err(|e| e.0)),
            }
        }
    });
    if let Err(e) = spawned {
        glib::g_warning!("osinfo", "could not start the osinfo thread: {e}");
    }
}

/// Looks up an OS by osinfo id. Returns `None` if unknown or not loaded yet.
pub fn lookup(id: &str) -> Option<&'static OsInfo> {
    CATALOG.get()?.get(id)
}

/// Every OS in the database (empty until loaded).
pub fn all() -> impl Iterator<Item = &'static OsInfo> {
    CATALOG.get().into_iter().flat_map(HashMap::values)
}

/// Identifies the OS on an ISO image or optical drive.
pub async fn identify_media(path: String) -> Result<Option<OsInfo>, String> {
    let (tx, rx) = futures_oneshot::channel();
    let sent = WORKER.get().map(|w| w.send(Request::IdentifyMedia(path, tx)).is_ok()).unwrap_or(false);
    if !sent {
        return Err("The operating system database is not available.".into());
    }
    rx.await.unwrap_or_else(|| Err("The operating system database stopped.".into()))
}

/// A display name for an osinfo id, e.g. "Fedora Linux 40".
///
/// Falls back to a readable form of the id while the database loads or
/// when the id is unknown.
pub fn display_name(id: &str) -> String {
    if let Some(os) = lookup(id) {
        return os.name.clone();
    }
    // http://fedoraproject.org/fedora/unknown -> "Fedora"
    let mut parts = id.trim_end_matches('/').rsplit('/');
    let version = parts.next().unwrap_or_default();
    let distro = parts.next().unwrap_or(version);
    let mut name = capitalize(distro);
    if version != distro && version != "unknown" && version != "rolling" {
        name.push(' ');
        name.push_str(version);
    }
    name
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Picks the logo for an osinfo id (see `ui::widgets::logo_data`).
pub fn logo_key(id: &str) -> Option<&'static str> {
    let os = lookup(id);
    let distro = os.and_then(|o| o.distro.as_deref()).unwrap_or_default().to_ascii_lowercase();
    let family = os.and_then(|o| o.family.as_deref()).unwrap_or_default();
    let by_distro = match distro.as_str() {
        "fedora" => Some("fedora"),
        "ubuntu" => Some(if id.contains("kubuntu") { "kubuntu" } else { "ubuntu" }),
        "debian" => Some("debian"),
        "win" | "winnt" => Some("windows"),
        "archlinux" | "arch" => Some("archlinux"),
        "opensuse" | "sles" | "sled" | "sle" | "slem" => Some("opensuse"),
        "rhel" | "rhl" => Some("rhel"),
        "centos" => Some("centos"),
        "almalinux" => Some("almalinux"),
        "alpinelinux" => Some("alpinelinux"),
        "eos" => Some("elementary"),
        "freebsd" => Some("freebsd"),
        "netbsd" => Some("netbsd"),
        "nixos" => Some("nixos"),
        "popos" => Some("popos"),
        "manjaro" => Some("manjaro"),
        _ => None,
    };
    by_distro.or_else(|| {
        // Not in the database (yet): recognise the id URL itself.
        const BY_ID: [(&str, &str); 17] = [
            ("fedoraproject.org", "fedora"),
            ("ubuntu.com", "ubuntu"),
            ("debian.org", "debian"),
            ("microsoft.com/win", "windows"),
            ("archlinux.org", "archlinux"),
            ("opensuse.org", "opensuse"),
            ("redhat.com/rhel", "rhel"),
            ("centos.org", "centos"),
            ("almalinux.org", "almalinux"),
            ("alpinelinux.org", "alpinelinux"),
            ("elementary.io", "elementary"),
            ("freebsd.org", "freebsd"),
            ("netbsd.org", "netbsd"),
            ("nixos.org", "nixos"),
            ("system76.com", "popos"),
            ("manjaro.org", "manjaro"),
            ("linuxmint.com", "linuxmint"),
        ];
        BY_ID
            .iter()
            .find(|(needle, _)| id.contains(needle))
            .map(|(_, key)| *key)
            .or_else(|| (family == "linux" || id.contains("linux")).then_some("linux"))
    })
}

/// A minimal single-use channel for handing a worker-thread result back to
/// an `async` caller on the GTK main loop.
mod futures_oneshot {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Waker};

    struct State<T> {
        value: Option<T>,
        closed: bool,
        waker: Option<Waker>,
    }

    pub struct Sender<T>(Arc<Mutex<State<T>>>);
    pub struct Receiver<T>(Arc<Mutex<State<T>>>);

    pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
        let s = Arc::new(Mutex::new(State { value: None, closed: false, waker: None }));
        (Sender(s.clone()), Receiver(s))
    }

    impl<T> Sender<T> {
        pub fn send(self, value: T) {
            let mut st = self.0.lock().unwrap_or_else(|p| p.into_inner());
            st.value = Some(value);
            if let Some(w) = st.waker.take() {
                w.wake();
            }
        }
    }

    impl<T> Drop for Sender<T> {
        fn drop(&mut self) {
            let mut st = self.0.lock().unwrap_or_else(|p| p.into_inner());
            st.closed = true;
            if let Some(w) = st.waker.take() {
                w.wake();
            }
        }
    }

    impl<T> Future for Receiver<T> {
        /// `None` if the sender went away without sending.
        type Output = Option<T>;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
            let mut st = self.0.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(v) = st.value.take() {
                Poll::Ready(Some(v))
            } else if st.closed {
                Poll::Ready(None)
            } else {
                st.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_fallback_names() {
        assert_eq!(display_name("http://fedoraproject.org/fedora/unknown"), "Fedora");
        assert_eq!(display_name("http://debian.org/debian/13"), "Debian 13");
    }

    #[test]
    fn logo_from_id_without_database() {
        assert_eq!(logo_key("http://fedoraproject.org/fedora/unknown"), Some("fedora"));
        assert_eq!(logo_key("http://microsoft.com/win/11"), Some("windows"));
        assert_eq!(logo_key("http://example.com/other"), None);
    }
}
