//! VNC backend on top of gvnc (the GTK-independent part of gtk-vnc).
//!
//! We own the framebuffer memory and hand it to gvnc, which decodes the
//! server's updates straight into it.

use std::cell::{Cell, RefCell};
use std::os::fd::{IntoRawFd, OwnedFd};
use std::rc::{Rc, Weak};

use gtk::glib;
use gtk::glib::translate::{FromGlibPtrFull, ToGlibPtr};
use gtk::prelude::*;

use crate::Status;
use crate::ffi::{self, VncPixelFormat};
use crate::keymap;
use crate::screen::{Backend, Button, GuestScreen, Source, cursor_from_rgba};

const AUTH_NONE: u32 = 1;
const AUTH_VNC: u32 = 2;

/// Our pixel layout: 32 bits, little endian, B G R X in memory.
const LOCAL_FORMAT: VncPixelFormat = VncPixelFormat {
    bits_per_pixel: 32,
    depth: 24,
    byte_order: 1234,
    true_color_flag: 1,
    red_max: 255,
    green_max: 255,
    blue_max: 255,
    red_shift: 16,
    green_shift: 8,
    blue_shift: 0,
};

/// Preferred encodings first, then pseudo-encodings for features.
const ENCODINGS: [i32; 11] = [16, 5, 1, 0, -223, -308, -239, -240, -257, -258, -261];

fn ptr(o: &glib::Object) -> *mut ffi::GObject {
    let p: *mut glib::gobject_ffi::GObject = o.to_glib_none().0;
    p.cast()
}

struct Framebuffer {
    /// Pixel memory; its heap address stays fixed while gvnc uses it.
    pixels: Box<[u8]>,
    /// The gvnc framebuffer object pointing into `pixels`.
    _object: glib::Object,
}

pub struct VncBackend {
    conn: glib::Object,
    framebuffer: RefCell<Option<Framebuffer>>,
    screen: glib::WeakRef<GuestScreen>,
    status: Box<dyn Fn(Status)>,
    update_pending: Cell<bool>,
    connected: Cell<bool>,
    me: RefCell<Weak<Self>>,
}

impl std::fmt::Debug for VncBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VncBackend").finish_non_exhaustive()
    }
}

impl VncBackend {
    pub fn connect(screen: &GuestScreen, fd: OwnedFd, status: impl Fn(Status) + 'static) -> Rc<Self> {
        // SAFETY: vnc_connection_new returns a new reference we take over.
        let conn: glib::Object =
            unsafe { glib::Object::from_glib_full(ffi::vnc_connection_new().cast::<glib::gobject_ffi::GObject>()) };
        let this = Rc::new(Self {
            conn,
            framebuffer: RefCell::default(),
            screen: screen.downgrade(),
            status: Box::new(status),
            update_pending: Cell::new(false),
            connected: Cell::new(false),
            me: RefCell::default(),
        });
        this.me.replace(Rc::downgrade(&this));
        (this.status)(Status::Connecting);
        this.wire();
        // SAFETY: valid connection; it takes ownership of the descriptor.
        let ok = unsafe { ffi::vnc_connection_open_fd(ptr(&this.conn), fd.into_raw_fd()) } != 0;
        if !ok {
            (this.status)(Status::Disconnected(Some("Couldn't start the VNC connection.".into())));
        }
        this
    }

    fn on<F: Fn(&Rc<Self>, &[glib::Value]) + 'static>(&self, signal: &str, f: F) {
        let weak = self.me.borrow().clone();
        self.conn.connect_local(signal, false, move |args| {
            if let Some(this) = weak.upgrade() {
                f(&this, args);
            }
            None
        });
    }

    fn wire(&self) {
        self.on("vnc-auth-choose-type", |this, args| {
            // libvirt already authorised us; prefer "none".
            let types = args[1].get::<glib::ValueArray>().ok();
            let offered: Vec<u32> = types
                .iter()
                .flat_map(|a| {
                    a.iter()
                        .filter_map(|v| v.get::<u32>().ok().or_else(|| v.transform::<u32>().ok()?.get::<u32>().ok()))
                })
                .collect();
            let choice = if offered.contains(&AUTH_NONE) || offered.is_empty() { AUTH_NONE } else { AUTH_VNC };
            // SAFETY: valid connection.
            unsafe { ffi::vnc_connection_set_auth_type(ptr(&this.conn), choice) };
        });
        self.on("vnc-auth-credential", |this, _| {
            // A password would be needed; we have none to give.
            // SAFETY: valid connection.
            unsafe { ffi::vnc_connection_shutdown(ptr(&this.conn)) };
            this.fail("The display asks for a password.");
        });
        self.on("vnc-auth-failure", |this, _| this.fail("The display refused the connection."));
        self.on("vnc-error", |this, args| {
            let msg = args[1].get::<String>().unwrap_or_else(|_| "The display connection failed.".into());
            this.fail(&msg);
        });
        self.on("vnc-initialized", |this, _| {
            let c = ptr(&this.conn);
            // SAFETY: valid connection; the format and encodings are
            // copied by gvnc.
            unsafe {
                ffi::vnc_connection_set_pixel_format(c, &LOCAL_FORMAT);
                ffi::vnc_connection_set_encodings(c, ENCODINGS.len() as i32, ENCODINGS.as_ptr());
            }
            this.connected.set(true);
            this.resize_framebuffer();
            (this.status)(Status::Connected);
        });
        self.on("vnc-desktop-resize", |this, _| this.resize_framebuffer());
        self.on("vnc-framebuffer-update", |this, args| {
            let v: Vec<i32> = (1..=4).filter_map(|i| args[i].get::<i32>().ok()).collect();
            if let (Some(s), [x, y, w, h]) = (this.screen.upgrade(), &v[..]) {
                s.invalidate(*x, *y, *w, *h);
            }
            this.request_update();
        });
        self.on("vnc-cursor-changed", |this, args| {
            let Some(s) = this.screen.upgrade() else { return };
            match args[1].get::<glib::Object>() {
                Ok(cursor) => {
                    let c = ptr(&cursor);
                    // SAFETY: valid cursor object; data holds w*h RGBA pixels.
                    let made = unsafe {
                        let (w, h) =
                            (i32::from(ffi::vnc_cursor_get_width(c)), i32::from(ffi::vnc_cursor_get_height(c)));
                        let data = ffi::vnc_cursor_get_data(c);
                        (!data.is_null() && w > 0 && h > 0).then(|| {
                            let pixels = std::slice::from_raw_parts(data, (w * h * 4) as usize);
                            cursor_from_rgba(
                                pixels,
                                w,
                                h,
                                i32::from(ffi::vnc_cursor_get_hotx(c)),
                                i32::from(ffi::vnc_cursor_get_hoty(c)),
                            )
                        })
                    };
                    s.set_guest_cursor(made.flatten().as_ref());
                }
                Err(_) => s.set_cursor(None),
            }
        });
        self.on("vnc-disconnected", |this, _| {
            if this.connected.replace(false) {
                if let Some(s) = this.screen.upgrade() {
                    s.set_source(None);
                }
                (this.status)(Status::Disconnected(None));
            }
        });
    }

    fn fail(&self, message: &str) {
        self.connected.set(false);
        if let Some(s) = self.screen.upgrade() {
            s.set_source(None);
        }
        (self.status)(Status::Disconnected(Some(message.to_owned())));
    }

    /// (Re)creates the framebuffer at the server's current size.
    fn resize_framebuffer(&self) {
        let c = ptr(&self.conn);
        // SAFETY: valid connection.
        let (w, h) = unsafe { (ffi::vnc_connection_get_width(c), ffi::vnc_connection_get_height(c)) };
        let (Ok(w16), Ok(h16)) = (u16::try_from(w), u16::try_from(h)) else { return };
        if w16 == 0 || h16 == 0 {
            return;
        }
        // Servers announce the size again during the first update; keep the
        // picture already drawn if nothing changed.
        let same = self.framebuffer.borrow().as_ref().is_some_and(|f| f.pixels.len() == (w * 4 * h) as usize)
            && self.screen.upgrade().and_then(|s| s.guest_size()) == Some((w, h));
        if same {
            return;
        }
        let stride = w * 4;
        let mut pixels = vec![0u8; (stride * h) as usize].into_boxed_slice();
        // SAFETY: `pixels` outlives the framebuffer object (both live in
        // `self.framebuffer`, replaced together); formats are copied.
        let object = unsafe {
            let remote = ffi::vnc_connection_get_pixel_format(c);
            let fb = ffi::vnc_base_framebuffer_new(
                pixels.as_mut_ptr(),
                w16,
                h16,
                stride,
                &LOCAL_FORMAT,
                if remote.is_null() { &LOCAL_FORMAT } else { remote },
            );
            glib::Object::from_glib_full(fb.cast::<glib::gobject_ffi::GObject>())
        };
        // SAFETY: valid connection and framebuffer.
        unsafe { ffi::vnc_connection_set_framebuffer(c, ptr(&object)) };
        let source =
            Source { data: pixels.as_ptr(), width: w, height: h, stride, format: gtk::gdk::MemoryFormat::B8g8r8x8 };
        let old = self.framebuffer.replace(Some(Framebuffer { pixels, _object: object }));
        if let Some(s) = self.screen.upgrade() {
            s.set_source(Some(source));
        }
        drop(old);
        // SAFETY: valid connection.
        unsafe { ffi::vnc_connection_framebuffer_update_request(c, 0, 0, 0, w16, h16) };
    }

    /// Asks for the next incremental update once the current batch of
    /// update signals has been handled.
    fn request_update(&self) {
        if self.update_pending.replace(true) {
            return;
        }
        let weak = self.me.borrow().clone();
        glib::idle_add_local_once(move || {
            let Some(this) = weak.upgrade() else { return };
            this.update_pending.set(false);
            let size = this.framebuffer.borrow().as_ref().map(|f| f.pixels.len());
            if size.is_none() || !this.connected.get() {
                return;
            }
            let c = ptr(&this.conn);
            // SAFETY: valid connection.
            unsafe {
                let (w, h) = (ffi::vnc_connection_get_width(c), ffi::vnc_connection_get_height(c));
                ffi::vnc_connection_framebuffer_update_request(c, 1, 0, 0, w as u16, h as u16);
            }
        });
    }
}

impl Backend for VncBackend {
    fn key(&self, down: bool, evdev: u32, keyval: u32) {
        let c = ptr(&self.conn);
        let scancode = keymap::evdev_to_xt(evdev).map_or(0, keymap::xt_to_qnum) as u16;
        let keysym = if keyval != 0 { keyval } else { keymap::evdev_to_keysym(evdev) };
        // SAFETY: valid connection. QEMU uses the scan code (extended key
        // event); other servers use the key symbol.
        unsafe { ffi::vnc_connection_key_event(c, i32::from(down), keysym, scancode) };
    }

    fn motion(&self, x: i32, y: i32, buttons: u32) {
        // SAFETY: valid connection.
        unsafe { ffi::vnc_connection_pointer_event(ptr(&self.conn), buttons as u8, x.max(0) as u16, y.max(0) as u16) };
    }

    fn button(&self, _button: Button, _down: bool, buttons: u32, x: i32, y: i32) {
        self.motion(x, y, buttons);
    }

    fn resize_guest(&self, width: u32, height: u32) {
        if width >= 320 && height >= 200 {
            // SAFETY: valid connection; servers without the extended
            // desktop size extension ignore this.
            unsafe { ffi::vnc_connection_set_size(ptr(&self.conn), width & !7, height) };
        }
    }

    fn can_resize_guest(&self) -> bool {
        true
    }

    fn disconnect(&self) {
        // SAFETY: valid connection.
        unsafe { ffi::vnc_connection_shutdown(ptr(&self.conn)) };
    }
}
