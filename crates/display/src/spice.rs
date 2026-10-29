//! SPICE backend on top of spice-client-glib.
//!
//! The session starts on a socket from libvirt; every further channel
//! (display, input, cursor, audio) asks for its own socket through the
//! `open-fd` signal, which the caller's [`FdProvider`] supplies.

use std::cell::{Cell, RefCell};
use std::os::fd::{AsRawFd, BorrowedFd, IntoRawFd, OwnedFd};
use std::rc::{Rc, Weak};

use gtk::glib;
use gtk::glib::translate::{FromGlibPtrFull, ToGlibPtr};
use gtk::prelude::*;

use crate::ffi;
use crate::keymap;
use crate::screen::{Backend, Button, GuestScreen, Source, cursor_from_rgba, pixel_bytes};
use crate::{FdProvider, Status};

// SpiceChannelEvent
const EVENT_OPENED: i32 = 10;
const EVENT_CLOSED: i32 = 12;
const EVENT_ERROR_FIRST: i32 = 20;
// SpiceMouseMode
const MOUSE_MODE_CLIENT: i32 = 2;

#[link(name = "spice-client-glib-2.0")]
unsafe extern "C" {
    fn spice_audio_get(session: *mut ffi::GObject, context: *mut std::ffi::c_void) -> *mut ffi::GObject;
    fn spice_display_channel_get_gl_scanout(channel: *mut ffi::GObject) -> *const GlScanout;
    fn spice_display_channel_gl_draw_done(channel: *mut ffi::GObject);
}

/// `SpiceGlScanout`: the guest image as a GPU buffer (3D acceleration).
#[repr(C)]
struct GlScanout {
    fd: i32,
    width: u32,
    height: u32,
    stride: u32,
    /// A DRM fourcc.
    format: u32,
    y0top: glib::ffi::gboolean,
}

/// SPICE doesn't pass the buffer's memory layout (modifier), so the
/// driver's implicit one is used, as SPICE's own GTK widget does.
const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

/// Turns the channel's current GPU buffer into a texture. The texture
/// gets its own copy of the descriptor, since SPICE closes its own when
/// the next buffer arrives.
fn gl_texture(display: &glib::Object) -> Option<(gtk::gdk::Texture, bool)> {
    // SAFETY: a valid display channel; the scanout (owned by the channel)
    // is valid until the next one, and we only read it here.
    let scanout = unsafe { spice_display_channel_get_gl_scanout(ptr(display)).as_ref() }?;
    if scanout.fd < 0 || scanout.width == 0 || scanout.height == 0 {
        return None;
    }
    // SAFETY: the descriptor is open while the scanout is current.
    let fd = unsafe { BorrowedFd::borrow_raw(scanout.fd) }.try_clone_to_owned().ok()?;
    let raw = fd.as_raw_fd();
    let builder = gtk::gdk::DmabufTextureBuilder::new()
        .set_display(&gtk::gdk::Display::default()?)
        .set_width(scanout.width)
        .set_height(scanout.height)
        .set_fourcc(scanout.format)
        .set_modifier(DRM_FORMAT_MOD_INVALID)
        .set_n_planes(1)
        .set_stride(0, scanout.stride)
        .set_offset(0, 0);
    // SAFETY: `raw` stays open until the release function drops `fd`,
    // which GTK calls once it no longer needs the buffer.
    let texture = unsafe { builder.set_fd(0, raw).build_with_release_func(move || drop(fd)) };
    match texture {
        Ok(t) => Some((t, scanout.y0top == 0)),
        Err(e) => {
            glib::g_warning!("vmng-display", "can't show the 3D-accelerated display: {e}");
            None
        }
    }
}

fn ptr(o: &glib::Object) -> *mut ffi::GObject {
    let p: *mut glib::gobject_ffi::GObject = o.to_glib_none().0;
    p.cast()
}

pub struct SpiceBackend {
    session: glib::Object,
    main: RefCell<Option<glib::Object>>,
    inputs: RefCell<Option<glib::Object>>,
    screen: glib::WeakRef<GuestScreen>,
    fds: FdProvider,
    status: Box<dyn Fn(Status)>,
    client_mouse: Cell<bool>,
    agent: Cell<bool>,
    last: Cell<(i32, i32)>,
    connected: Cell<bool>,
    me: RefCell<Weak<Self>>,
}

impl std::fmt::Debug for SpiceBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpiceBackend").finish_non_exhaustive()
    }
}

impl SpiceBackend {
    /// Starts a session on `fd` and shows it in `screen`.
    pub fn connect(screen: &GuestScreen, fd: OwnedFd, fds: FdProvider, status: impl Fn(Status) + 'static) -> Rc<Self> {
        // SAFETY: spice_session_new returns a new reference we take over.
        let session: glib::Object =
            unsafe { glib::Object::from_glib_full(ffi::spice_session_new().cast::<glib::gobject_ffi::GObject>()) };
        session.set_property("enable-usbredir", false);
        session.set_property("enable-audio", true);
        let this = Rc::new(Self {
            session,
            main: RefCell::default(),
            inputs: RefCell::default(),
            screen: screen.downgrade(),
            fds,
            status: Box::new(status),
            client_mouse: Cell::new(true),
            agent: Cell::new(false),
            last: Cell::new((0, 0)),
            connected: Cell::new(false),
            me: RefCell::default(),
        });
        this.me.replace(Rc::downgrade(&this));
        (this.status)(Status::Connecting);

        let weak = Rc::downgrade(&this);
        this.session.connect_local("channel-new", false, move |args| {
            if let (Some(this), Ok(channel)) = (weak.upgrade(), args[1].get::<glib::Object>()) {
                this.channel_new(&channel);
            }
            None
        });
        let weak = Rc::downgrade(&this);
        this.session.connect_local("disconnected", false, move |_| {
            if let Some(this) = weak.upgrade() {
                this.closed(None);
            }
            None
        });
        // SAFETY: valid session; audio is optional (playback through
        // GStreamer), the returned object is owned by the session.
        unsafe {
            spice_audio_get(ptr(&this.session), std::ptr::null_mut());
        }
        // SAFETY: valid session; it takes ownership of the descriptor.
        let ok = unsafe { ffi::spice_session_open_fd(ptr(&this.session), fd.into_raw_fd()) } != 0;
        if !ok {
            (this.status)(Status::Disconnected(Some("Couldn't start the SPICE session.".into())));
        }
        this
    }

    fn closed(&self, error: Option<String>) {
        if self.connected.replace(false) || error.is_some() {
            if let Some(s) = self.screen.upgrade() {
                s.set_source(None);
            }
            (self.status)(Status::Disconnected(error));
        }
    }

    fn channel_new(&self, channel: &glib::Object) {
        let kind = channel.type_().name().to_owned();
        // Every channel but the first (main) one needs its own socket.
        let fds = self.fds.clone();
        channel.connect_local("open-fd", false, move |args| {
            let Ok(ch) = args[0].get::<glib::Object>() else { return None };
            fds(Box::new(move |result| match result {
                // SAFETY: valid channel; it takes ownership of the descriptor.
                Ok(fd) => unsafe {
                    ffi::spice_channel_open_fd(ptr(&ch), fd.into_raw_fd());
                },
                Err(e) => glib::g_warning!("vmng-display", "no socket for a SPICE channel: {e}"),
            }));
            None
        });
        let id: i32 = channel.property("channel-id");
        match kind.as_str() {
            "SpiceMainChannel" => self.setup_main(channel),
            "SpiceDisplayChannel" if id == 0 => {
                self.setup_display(channel);
                Self::start(channel);
            }
            "SpiceInputsChannel" => {
                self.inputs.replace(Some(channel.clone()));
                Self::start(channel);
            }
            "SpiceCursorChannel" => {
                self.setup_cursor(channel);
                Self::start(channel);
            }
            // Audio channels are started by spice_audio; USB redirection,
            // smartcards and shared folders are not used.
            _ => {}
        }
    }

    fn start(channel: &glib::Object) {
        // SAFETY: valid channel object.
        unsafe {
            ffi::spice_channel_connect(ptr(channel));
        }
    }

    fn setup_main(&self, main: &glib::Object) {
        self.main.replace(Some(main.clone()));
        let weak = self.me.borrow().clone();
        main.connect_local("channel-event", false, move |args| {
            let this = weak.upgrade()?;
            let event = args[1].transform::<i32>().ok().and_then(|v| v.get::<i32>().ok()).unwrap_or(0);
            match event {
                EVENT_OPENED => {
                    this.connected.set(true);
                    (this.status)(Status::Connected);
                }
                EVENT_CLOSED => this.closed(None),
                e if e >= EVENT_ERROR_FIRST => this.closed(Some("The connection to the display failed.".into())),
                _ => {}
            }
            None
        });
        let weak = self.me.borrow().clone();
        main.connect_local("main-mouse-update", false, move |args| {
            let this = weak.upgrade()?;
            let ch = args[0].get::<glib::Object>().ok()?;
            this.client_mouse.set(ch.property::<i32>("mouse-mode") == MOUSE_MODE_CLIENT);
            None
        });
        let weak = self.me.borrow().clone();
        main.connect_local("main-agent-update", false, move |args| {
            let this = weak.upgrade()?;
            let ch = args[0].get::<glib::Object>().ok()?;
            this.agent.set(ch.property::<bool>("agent-connected"));
            None
        });
    }

    fn setup_display(&self, display: &glib::Object) {
        // 3D acceleration: the guest image arrives as GPU buffers.
        let screen = self.screen.clone();
        display.connect_local("gl-draw", false, move |args| {
            let ch = args[0].get::<glib::Object>().ok()?;
            if let Some(s) = screen.upgrade() {
                s.set_gpu_image(gl_texture(&ch));
            }
            // SAFETY: a valid display channel.
            unsafe { spice_display_channel_gl_draw_done(ptr(&ch)) };
            None
        });
        let screen = self.screen.clone();
        display.connect_notify_local(Some("gl-scanout"), move |ch, _| {
            if let Some(s) = screen.upgrade() {
                s.set_gpu_image(gl_texture(ch));
            }
        });
        let screen = self.screen.clone();
        display.connect_local("display-primary-create", false, move |args| {
            let s = screen.upgrade()?;
            let width = args[2].get::<i32>().ok()?;
            let height = args[3].get::<i32>().ok()?;
            let stride = args[4].get::<i32>().ok()?;
            let data = args[6].get::<glib::Pointer>().ok()?;
            // Both 32-bit SPICE formats are B, G, R, X/A in memory.
            s.set_source(Some(Source {
                data: data as *const u8,
                width,
                height,
                stride,
                format: gtk::gdk::MemoryFormat::B8g8r8x8,
            }));
            None
        });
        let screen = self.screen.clone();
        display.connect_local("display-primary-destroy", false, move |_| {
            if let Some(s) = screen.upgrade() {
                s.set_source(None);
            }
            None
        });
        let screen = self.screen.clone();
        display.connect_local("display-invalidate", false, move |args| {
            let s = screen.upgrade()?;
            let v: Vec<i32> = (1..=4).filter_map(|i| args[i].get::<i32>().ok()).collect();
            if let [x, y, w, h] = v[..] {
                s.invalidate(x, y, w, h);
            }
            None
        });
    }

    fn setup_cursor(&self, cursor: &glib::Object) {
        let screen = self.screen.clone();
        cursor.connect_local("cursor-set", false, move |args| {
            let s = screen.upgrade()?;
            let v: Vec<i32> = (1..=4).filter_map(|i| args[i].get::<i32>().ok()).collect();
            let rgba = args[5].get::<glib::Pointer>().ok()?;
            if let (&[w, h, hx, hy], false) = (&v[..], rgba.is_null()) {
                let len = pixel_bytes(w, h)?;
                // SAFETY: spice passes `w * h` RGBA pixels valid during the signal.
                let data = unsafe { std::slice::from_raw_parts(rgba as *const u8, len) };
                s.set_guest_cursor(cursor_from_rgba(data, w, h, hx, hy).as_ref());
            }
            None
        });
        let screen = self.screen.clone();
        cursor.connect_local("cursor-hide", false, move |_| {
            if let Some(s) = screen.upgrade() {
                s.set_guest_cursor(None);
            }
            None
        });
        let screen = self.screen.clone();
        cursor.connect_local("cursor-reset", false, move |_| {
            if let Some(s) = screen.upgrade() {
                s.set_cursor(None);
            }
            None
        });
    }

    fn inputs(&self) -> Option<*mut ffi::GObject> {
        self.inputs.borrow().as_ref().map(ptr)
    }
}

fn spice_button(b: Button) -> i32 {
    match b {
        Button::Left => 1,
        Button::Middle => 2,
        Button::Right => 3,
        Button::WheelUp => 4,
        Button::WheelDown => 5,
    }
}

impl Backend for SpiceBackend {
    fn key(&self, down: bool, evdev: u32, _keyval: u32) {
        let (Some(inputs), Some(code)) = (self.inputs(), keymap::evdev_to_xt(evdev)) else { return };
        // SAFETY: valid inputs channel; the scan code uses SPICE's
        // "0x100 means E0 prefix" convention.
        unsafe {
            if down {
                ffi::spice_inputs_channel_key_press(inputs, code);
            } else {
                ffi::spice_inputs_channel_key_release(inputs, code);
            }
        }
    }

    fn motion(&self, x: i32, y: i32, buttons: u32) {
        let Some(inputs) = self.inputs() else { return };
        let (lx, ly) = self.last.replace((x, y));
        // SAFETY: valid inputs channel.
        unsafe {
            if self.client_mouse.get() {
                ffi::spice_inputs_channel_position(inputs, x, y, 0, buttons as i32);
            } else {
                ffi::spice_inputs_channel_motion(inputs, x - lx, y - ly, buttons as i32);
            }
        }
    }

    fn button(&self, button: Button, down: bool, buttons: u32, x: i32, y: i32) {
        let Some(inputs) = self.inputs() else { return };
        if self.client_mouse.get() {
            // SAFETY: valid inputs channel.
            unsafe { ffi::spice_inputs_channel_position(inputs, x, y, 0, buttons as i32) };
        }
        // SAFETY: valid inputs channel.
        unsafe {
            if down {
                ffi::spice_inputs_channel_button_press(inputs, spice_button(button), buttons as i32);
            } else {
                ffi::spice_inputs_channel_button_release(inputs, spice_button(button), buttons as i32);
            }
        }
    }

    fn resize_guest(&self, width: u32, height: u32) {
        if !self.agent.get() || width < 320 || height < 200 {
            return;
        }
        let Some(main) = self.main.borrow().as_ref().map(ptr) else { return };
        // SAFETY: valid main channel. With `update` set, spice-glib sends
        // the new monitor layout after a short delay (debounced).
        unsafe {
            ffi::spice_main_channel_update_display_enabled(main, 0, 1, 0);
            ffi::spice_main_channel_update_display(main, 0, 0, 0, width as i32 & !7, height as i32, 1);
        }
    }

    fn can_resize_guest(&self) -> bool {
        self.agent.get()
    }

    fn disconnect(&self) {
        // SAFETY: valid session.
        unsafe { ffi::spice_session_disconnect(ptr(&self.session)) };
    }
}
