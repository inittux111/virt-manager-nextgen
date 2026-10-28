//! Guest operating system information from libosinfo / osinfo-db.
//!
//! libosinfo is a GObject C library with no Rust bindings, so this crate
//! declares the handful of functions it needs and wraps them in a safe API
//! that hands out plain Rust values. The database is loaded once (it takes a
//! noticeable fraction of a second) and is meant to be owned by a single
//! background thread; see [`OsDb`].

mod ffi;

use std::ffi::{CStr, CString};
use std::fmt;
use std::ptr::{self, NonNull};

/// A guest operating system known to osinfo-db.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OsInfo {
    /// The osinfo id, e.g. `http://fedoraproject.org/fedora/41`.
    pub id: String,
    /// Short id, e.g. `fedora41`.
    pub short_id: String,
    /// Human readable name, e.g. `Fedora Linux 41`.
    pub name: String,
    /// Version string, e.g. `41`.
    pub version: Option<String>,
    /// Vendor, e.g. `Fedora Project`.
    pub vendor: Option<String>,
    /// Family, e.g. `linux` or `winnt`.
    pub family: Option<String>,
    /// Distribution, e.g. `fedora`, `ubuntu`, `win`.
    pub distro: Option<String>,
    /// Release date (`YYYY-MM-DD`), if known.
    pub release_date: Option<String>,
    /// End-of-life date (`YYYY-MM-DD`), if known.
    pub eol_date: Option<String>,
}

/// Errors from libosinfo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// A loaded osinfo database.
///
/// The underlying GObjects are not thread safe, so `OsDb` is `Send` but not
/// `Sync`: it can be created on and moved to a worker thread, but only one
/// thread uses it at a time.
pub struct OsDb {
    loader: NonNull<ffi::OsinfoLoader>,
    db: NonNull<ffi::OsinfoDb>,
}

// SAFETY: `OsDb` exclusively owns its loader and database; no other
// reference to them escapes this type, so moving it to another thread is
// sound as long as it is not used from two threads at once (it is !Sync).
unsafe impl Send for OsDb {}

impl fmt::Debug for OsDb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OsDb").finish_non_exhaustive()
    }
}

impl OsDb {
    /// Loads the system osinfo database from its default locations.
    pub fn load() -> Result<Self, Error> {
        // SAFETY: plain constructor; returns a new reference or NULL.
        let loader = NonNull::new(unsafe { ffi::osinfo_loader_new() })
            .ok_or_else(|| Error("could not create an osinfo loader".into()))?;
        let mut err = ptr::null_mut();
        // SAFETY: `loader` is valid and `err` is a valid out pointer.
        unsafe { ffi::osinfo_loader_process_default_path(loader.as_ptr(), &mut err) };
        if let Some(e) = take_error(err) {
            // SAFETY: we own the loader reference.
            unsafe { ffi::g_object_unref(loader.as_ptr().cast()) };
            return Err(e);
        }
        // SAFETY: `loader` is valid; the db is owned by the loader.
        let db = NonNull::new(unsafe { ffi::osinfo_loader_get_db(loader.as_ptr()) });
        match db {
            Some(db) => Ok(Self { loader, db }),
            None => {
                // SAFETY: we own the loader reference.
                unsafe { ffi::g_object_unref(loader.as_ptr().cast()) };
                Err(Error("osinfo database is empty".into()))
            }
        }
    }

    /// Looks up an OS by its osinfo id.
    pub fn os(&self, id: &str) -> Option<OsInfo> {
        let id = CString::new(id).ok()?;
        // SAFETY: valid db and NUL-terminated id; result is borrowed (transfer none).
        let os = unsafe { ffi::osinfo_db_get_os(self.db.as_ptr(), id.as_ptr()) };
        NonNull::new(os).map(|os| os_info(os.as_ptr()))
    }

    /// Returns every OS in the database.
    pub fn all(&self) -> Vec<OsInfo> {
        // SAFETY: valid db; the returned list is a new reference we must unref.
        let list = unsafe { ffi::osinfo_db_get_os_list(self.db.as_ptr()) };
        let Some(list) = NonNull::new(list) else { return Vec::new() };
        let list = list.as_ptr().cast::<ffi::OsinfoList>();
        // SAFETY: `list` is a valid OsinfoList for the duration of this block.
        let out = unsafe {
            let len = ffi::osinfo_list_get_length(list).max(0);
            (0..len)
                .filter_map(|i| NonNull::new(ffi::osinfo_list_get_nth(list, i)))
                .map(|e| os_info(e.as_ptr().cast()))
                .collect()
        };
        // SAFETY: we own the list reference.
        unsafe { ffi::g_object_unref(list.cast()) };
        out
    }

    /// Identifies the operating system on an install medium (ISO image or
    /// optical drive). Returns `Ok(None)` when the medium is readable but
    /// not recognised.
    pub fn identify_media(&self, location: &str) -> Result<Option<OsInfo>, Error> {
        let loc = CString::new(location).map_err(|_| Error("invalid path".into()))?;
        let mut err = ptr::null_mut();
        // SAFETY: valid NUL-terminated path, no cancellable, valid out pointer.
        let media = unsafe { ffi::osinfo_media_create_from_location(loc.as_ptr(), ptr::null_mut(), &mut err) };
        if let Some(e) = take_error(err) {
            return Err(e);
        }
        let Some(media) = NonNull::new(media) else { return Ok(None) };
        // SAFETY: valid db and media. `identify_media` fills in the media's OS.
        let found = unsafe { ffi::osinfo_db_identify_media(self.db.as_ptr(), media.as_ptr()) } != 0;
        let result = if found {
            // SAFETY: valid media; returned OS is borrowed (transfer none)
            // and outlives this call because the db owns it.
            NonNull::new(unsafe { ffi::osinfo_media_get_os(media.as_ptr()) }).map(|os| os_info(os.as_ptr()))
        } else {
            None
        };
        // SAFETY: we own the media reference.
        unsafe { ffi::g_object_unref(media.as_ptr().cast()) };
        Ok(result)
    }
}

impl Drop for OsDb {
    fn drop(&mut self) {
        // SAFETY: we own one reference to the loader, which owns the db.
        unsafe { ffi::g_object_unref(self.loader.as_ptr().cast()) };
    }
}

/// Copies the interesting properties of an `OsinfoOs` into Rust values.
fn os_info(os: *mut ffi::OsinfoOs) -> OsInfo {
    let product = os.cast::<ffi::OsinfoProduct>();
    let entity = os.cast::<ffi::OsinfoEntity>();
    // SAFETY: `os` is a valid OsinfoOs (a subclass of OsinfoProduct and
    // OsinfoEntity); all getters return borrowed strings or NULL.
    unsafe {
        let name = opt_str(ffi::osinfo_product_get_name(product));
        let id = opt_str(ffi::osinfo_entity_get_id(entity)).unwrap_or_default();
        OsInfo {
            short_id: opt_str(ffi::osinfo_product_get_short_id(product)).unwrap_or_default(),
            name: name.unwrap_or_else(|| id.clone()),
            id,
            version: opt_str(ffi::osinfo_product_get_version(product)),
            vendor: opt_str(ffi::osinfo_product_get_vendor(product)),
            family: opt_str(ffi::osinfo_os_get_family(os)),
            distro: opt_str(ffi::osinfo_os_get_distro(os)),
            release_date: opt_str(ffi::osinfo_product_get_release_date_string(product)),
            eol_date: opt_str(ffi::osinfo_product_get_eol_date_string(product)),
        }
    }
}

/// # Safety
/// `p` must be NULL or a valid NUL-terminated string.
unsafe fn opt_str(p: *const std::ffi::c_char) -> Option<String> {
    if p.is_null() {
        None
    } else {
        // SAFETY: guaranteed by the caller.
        Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    }
}

/// Converts and frees a `GError`, if one was set.
fn take_error(err: *mut ffi::GError) -> Option<Error> {
    if err.is_null() {
        return None;
    }
    // SAFETY: a non-NULL GError set by libosinfo, owned by us.
    unsafe {
        let msg = opt_str((*err).message).unwrap_or_else(|| "unknown libosinfo error".into());
        ffi::g_error_free(err);
        Some(Error(msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_database_and_finds_fedora() {
        let db = OsDb::load().expect("osinfo-db must be installed");
        let os = db.os("http://fedoraproject.org/fedora/40").expect("fedora 40 is in osinfo-db");
        assert_eq!(os.distro.as_deref(), Some("fedora"));
        assert!(os.name.contains("Fedora"));
        assert!(db.all().len() > 100);
        assert!(db.os("http://example.invalid/none").is_none());
    }

    #[test]
    fn unreadable_media_is_an_error() {
        let db = OsDb::load().unwrap();
        assert!(db.identify_media("/nonexistent/image.iso").is_err());
    }
}
