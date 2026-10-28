//! Raw declarations for the parts of libosinfo-1.0 we use.

#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_void};

macro_rules! opaque {
    ($($name:ident),*) => {$(
        #[repr(C)]
        pub struct $name {
            _private: [u8; 0],
        }
    )*};
}

opaque!(
    OsinfoLoader,
    OsinfoDb,
    OsinfoOs,
    OsinfoOsList,
    OsinfoList,
    OsinfoProduct,
    OsinfoEntity,
    OsinfoMedia,
    GCancellable
);

#[repr(C)]
pub struct GError {
    pub domain: u32,
    pub code: c_int,
    pub message: *const c_char,
}

pub type gboolean = c_int;

#[link(name = "osinfo-1.0")]
unsafe extern "C" {
    pub fn osinfo_loader_new() -> *mut OsinfoLoader;
    pub fn osinfo_loader_process_default_path(loader: *mut OsinfoLoader, err: *mut *mut GError);
    pub fn osinfo_loader_get_db(loader: *mut OsinfoLoader) -> *mut OsinfoDb;

    pub fn osinfo_db_get_os(db: *mut OsinfoDb, id: *const c_char) -> *mut OsinfoOs;
    pub fn osinfo_db_get_os_list(db: *mut OsinfoDb) -> *mut OsinfoOsList;
    pub fn osinfo_db_identify_media(db: *mut OsinfoDb, media: *mut OsinfoMedia) -> gboolean;

    pub fn osinfo_list_get_length(list: *mut OsinfoList) -> c_int;
    pub fn osinfo_list_get_nth(list: *mut OsinfoList, idx: c_int) -> *mut OsinfoEntity;

    pub fn osinfo_entity_get_id(entity: *mut OsinfoEntity) -> *const c_char;

    pub fn osinfo_product_get_name(product: *mut OsinfoProduct) -> *const c_char;
    pub fn osinfo_product_get_short_id(product: *mut OsinfoProduct) -> *const c_char;
    pub fn osinfo_product_get_version(product: *mut OsinfoProduct) -> *const c_char;
    pub fn osinfo_product_get_vendor(product: *mut OsinfoProduct) -> *const c_char;
    pub fn osinfo_product_get_release_date_string(product: *mut OsinfoProduct) -> *const c_char;
    pub fn osinfo_product_get_eol_date_string(product: *mut OsinfoProduct) -> *const c_char;

    pub fn osinfo_os_get_family(os: *mut OsinfoOs) -> *const c_char;
    pub fn osinfo_os_get_distro(os: *mut OsinfoOs) -> *const c_char;

    pub fn osinfo_media_create_from_location(
        location: *const c_char,
        cancellable: *mut GCancellable,
        err: *mut *mut GError,
    ) -> *mut OsinfoMedia;
    pub fn osinfo_media_get_os(media: *mut OsinfoMedia) -> *mut OsinfoOs;
}

#[link(name = "gobject-2.0")]
unsafe extern "C" {
    pub fn g_object_unref(object: *mut c_void);
}

#[link(name = "glib-2.0")]
unsafe extern "C" {
    pub fn g_error_free(err: *mut GError);
}
