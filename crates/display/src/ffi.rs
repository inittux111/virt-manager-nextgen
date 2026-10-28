//! Raw declarations for spice-client-glib-2.0, gvnc-1.0 and vte-2.91-gtk4.
//!
//! Only plain functions are declared; GObject signals and properties are
//! used through glib-rs, which handles their marshalling safely.

#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_uint, c_void};

pub type gboolean = c_int;
pub type GObject = c_void;

#[link(name = "spice-client-glib-2.0")]
unsafe extern "C" {
    pub fn spice_session_new() -> *mut GObject;
    pub fn spice_session_open_fd(session: *mut GObject, fd: c_int) -> gboolean;
    pub fn spice_session_disconnect(session: *mut GObject);

    pub fn spice_channel_open_fd(channel: *mut GObject, fd: c_int) -> gboolean;
    pub fn spice_channel_connect(channel: *mut GObject) -> gboolean;

    pub fn spice_inputs_channel_position(
        channel: *mut GObject,
        x: c_int,
        y: c_int,
        display: c_int,
        button_state: c_int,
    );
    pub fn spice_inputs_channel_motion(channel: *mut GObject, dx: c_int, dy: c_int, button_state: c_int);
    pub fn spice_inputs_channel_button_press(channel: *mut GObject, button: c_int, button_state: c_int);
    pub fn spice_inputs_channel_button_release(channel: *mut GObject, button: c_int, button_state: c_int);
    pub fn spice_inputs_channel_key_press(channel: *mut GObject, scancode: c_uint);
    pub fn spice_inputs_channel_key_release(channel: *mut GObject, scancode: c_uint);

    pub fn spice_main_channel_update_display_enabled(
        channel: *mut GObject,
        id: c_int,
        enabled: gboolean,
        update: gboolean,
    );
    pub fn spice_main_channel_update_display(
        channel: *mut GObject,
        id: c_int,
        x: c_int,
        y: c_int,
        width: c_int,
        height: c_int,
        update: gboolean,
    );
}

/// `VncPixelFormat` from gvnc; the layout must match the C struct.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VncPixelFormat {
    pub bits_per_pixel: u8,
    pub depth: u8,
    pub byte_order: u16,
    pub true_color_flag: u8,
    pub red_max: u16,
    pub green_max: u16,
    pub blue_max: u16,
    pub red_shift: u8,
    pub green_shift: u8,
    pub blue_shift: u8,
}

#[link(name = "gvnc-1.0")]
unsafe extern "C" {
    pub fn vnc_connection_new() -> *mut GObject;
    pub fn vnc_connection_open_fd(conn: *mut GObject, fd: c_int) -> gboolean;
    pub fn vnc_connection_shutdown(conn: *mut GObject);
    pub fn vnc_connection_set_auth_type(conn: *mut GObject, kind: c_uint) -> gboolean;
    pub fn vnc_connection_set_encodings(conn: *mut GObject, n: c_int, encodings: *const i32) -> gboolean;
    pub fn vnc_connection_set_pixel_format(conn: *mut GObject, fmt: *const VncPixelFormat) -> gboolean;
    pub fn vnc_connection_get_pixel_format(conn: *mut GObject) -> *const VncPixelFormat;
    pub fn vnc_connection_set_framebuffer(conn: *mut GObject, fb: *mut GObject) -> gboolean;
    pub fn vnc_connection_framebuffer_update_request(
        conn: *mut GObject,
        incremental: gboolean,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
    ) -> gboolean;
    pub fn vnc_connection_get_width(conn: *mut GObject) -> c_int;
    pub fn vnc_connection_get_height(conn: *mut GObject) -> c_int;
    pub fn vnc_connection_key_event(conn: *mut GObject, down: gboolean, key: u32, scancode: u16) -> gboolean;
    pub fn vnc_connection_pointer_event(conn: *mut GObject, mask: u8, x: u16, y: u16) -> gboolean;
    pub fn vnc_connection_set_size(conn: *mut GObject, width: c_uint, height: c_uint) -> c_int;

    pub fn vnc_base_framebuffer_new(
        buffer: *mut u8,
        width: u16,
        height: u16,
        rowstride: c_int,
        local: *const VncPixelFormat,
        remote: *const VncPixelFormat,
    ) -> *mut GObject;

    pub fn vnc_cursor_get_data(cursor: *mut GObject) -> *const u8;
    pub fn vnc_cursor_get_hotx(cursor: *mut GObject) -> u16;
    pub fn vnc_cursor_get_hoty(cursor: *mut GObject) -> u16;
    pub fn vnc_cursor_get_width(cursor: *mut GObject) -> u16;
    pub fn vnc_cursor_get_height(cursor: *mut GObject) -> u16;
}

#[link(name = "vte-2.91-gtk4")]
unsafe extern "C" {
    pub fn vte_terminal_new() -> *mut GObject;
    pub fn vte_terminal_feed(terminal: *mut GObject, data: *const c_char, length: isize);
    pub fn vte_terminal_set_scrollback_lines(terminal: *mut GObject, lines: std::ffi::c_long);
    pub fn vte_terminal_copy_clipboard_format(terminal: *mut GObject, format: c_int);
    pub fn vte_terminal_paste_clipboard(terminal: *mut GObject);
}
