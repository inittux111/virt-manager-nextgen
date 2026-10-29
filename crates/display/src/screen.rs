//! `GuestScreen`: shows a guest's framebuffer and sends input back.
//!
//! The protocol backends (SPICE, VNC) own the pixel buffer; the widget
//! copies it into a GPU texture once per frame when something changed,
//! telling GTK which region is new so only that part is uploaded. With 3D
//! acceleration (SPICE OpenGL) the guest image is a GPU buffer instead,
//! shown as it is ([`GuestScreen::set_gpu_image`]).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib::translate::IntoGlib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{cairo, gdk, glib, graphene, gsk};

use crate::keymap;

/// Mouse buttons in the order of their mask bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

impl Button {
    pub fn mask(self) -> u32 {
        match self {
            Self::Left => 1,
            Self::Middle => 2,
            Self::Right => 4,
            Self::WheelUp => 8,
            Self::WheelDown => 16,
        }
    }
}

/// A connection to a guest display.
pub trait Backend {
    /// A key went down or up. `evdev` is the Linux key code, `keyval` the
    /// GDK key value (for VNC servers that want key symbols).
    fn key(&self, down: bool, evdev: u32, keyval: u32);
    /// The pointer moved to guest coordinates, with `buttons` held.
    fn motion(&self, x: i32, y: i32, buttons: u32);
    /// A button changed state at guest coordinates.
    fn button(&self, button: Button, down: bool, buttons: u32, x: i32, y: i32);
    /// Asks the guest to use this resolution (needs guest support).
    fn resize_guest(&self, width: u32, height: u32);
    /// Whether the guest can follow the window size right now.
    fn can_resize_guest(&self) -> bool {
        false
    }
    /// Closes the connection.
    fn disconnect(&self);
}

/// How the guest image is fitted into the widget.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Scaling {
    /// Scale to fit, keeping the aspect ratio.
    #[default]
    Fit,
    /// Show pixels 1:1 (the widget asks for the guest's size).
    Native,
}

/// Where the protocol keeps the current image.
#[derive(Clone, Copy, Debug)]
pub struct Source {
    pub(crate) data: *const u8,
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) stride: i32,
    pub(crate) format: gdk::MemoryFormat,
}

type GrabFn = Box<dyn Fn(bool)>;
type SizeFn = Box<dyn Fn(i32, i32)>;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct GuestScreen {
        pub source: Cell<Option<Source>>,
        pub dirty: RefCell<Option<cairo::Region>>,
        pub texture: RefCell<Option<gdk::Texture>>,
        /// A GPU buffer shown instead of the source, and whether it is
        /// stored bottom row first.
        pub gpu: RefCell<Option<(gdk::Texture, bool)>>,
        pub backend: RefCell<Option<Rc<dyn Backend>>>,
        pub scaling: Cell<Scaling>,
        /// Guest pixels per widget pixel and the image origin, for input.
        pub transform: Cell<(f64, f64, f64)>,
        pub buttons: Cell<u32>,
        pub last_pos: Cell<(i32, i32)>,
        pub pressed: RefCell<Vec<u32>>,
        pub release_combo: RefCell<Vec<u32>>,
        pub grabbed: Cell<bool>,
        pub on_grab: RefCell<Option<GrabFn>>,
        pub on_size: RefCell<Option<SizeFn>>,
    }

    impl std::fmt::Debug for GuestScreen {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("GuestScreen").finish_non_exhaustive()
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GuestScreen {
        const NAME: &'static str = "VmngGuestScreen";
        type Type = super::GuestScreen;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("guest-screen");
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for GuestScreen {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_focusable(true);
            obj.set_hexpand(true);
            obj.set_vexpand(true);
            obj.set_overflow(gtk::Overflow::Hidden);
            obj.setup_input();
        }
    }

    impl WidgetImpl for GuestScreen {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let size =
                self.image_size().map_or(0, |(w, h)| if orientation == gtk::Orientation::Horizontal { w } else { h });
            match self.scaling.get() {
                Scaling::Fit => (1, size.max(1), -1, -1),
                Scaling::Native => (size, size, -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            if let Some(f) = self.on_size.borrow().as_ref() {
                f(width, height);
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let (w, h) = (obj.width() as f32, obj.height() as f32);
            let gpu = self.gpu.borrow().clone();
            let (texture, flipped) = match gpu {
                Some(g) => g,
                None => {
                    self.update_texture();
                    let Some(texture) = self.texture.borrow().clone() else { return };
                    (texture, false)
                }
            };
            let (tw, th) = (texture.width() as f32, texture.height() as f32);
            let scale = match self.scaling.get() {
                Scaling::Fit => (w / tw).min(h / th),
                Scaling::Native => 1.0,
            };
            let (dw, dh) = (tw * scale, th * scale);
            let (x, y) = (((w - dw) / 2.0).max(0.0), ((h - dh) / 2.0).max(0.0));
            self.transform.set((f64::from(scale), f64::from(x), f64::from(y)));
            let filter = if (scale - 1.0).abs() < f32::EPSILON {
                gsk::ScalingFilter::Nearest
            } else {
                gsk::ScalingFilter::Linear
            };
            if flipped {
                snapshot.save();
                snapshot.translate(&graphene::Point::new(x, y + dh));
                snapshot.scale(1.0, -1.0);
                snapshot.append_scaled_texture(&texture, filter, &graphene::Rect::new(0.0, 0.0, dw, dh));
                snapshot.restore();
            } else {
                snapshot.append_scaled_texture(&texture, filter, &graphene::Rect::new(x, y, dw, dh));
            }
        }
    }

    impl GuestScreen {
        /// The size of the image shown.
        pub fn image_size(&self) -> Option<(i32, i32)> {
            if let Some((t, _)) = self.gpu.borrow().as_ref() {
                return Some((t.width(), t.height()));
            }
            self.source.get().map(|s| (s.width, s.height))
        }

        /// Copies the guest image into a texture if it changed.
        fn update_texture(&self) {
            let Some(region) = self.dirty.borrow_mut().take() else { return };
            let Some(src) = self.source.get() else { return };
            if src.data.is_null() || src.width <= 0 || src.height <= 0 {
                return;
            }
            let len = src.stride as usize * src.height as usize;
            // SAFETY: the backend guarantees `data` points at `stride *
            // height` readable bytes until it clears the source, and it
            // only changes them on the main thread (where we run).
            let bytes = glib::Bytes::from_owned(unsafe { std::slice::from_raw_parts(src.data, len) }.to_vec());
            let previous = self.texture.borrow().clone();
            let mut builder = gdk::MemoryTextureBuilder::new()
                .set_bytes(Some(&bytes))
                .set_width(src.width)
                .set_height(src.height)
                .set_stride(src.stride as usize)
                .set_format(src.format);
            if let Some(prev) = previous.filter(|p| p.width() == src.width && p.height() == src.height) {
                builder = builder.set_update_texture(Some(&prev)).set_update_region(Some(&region));
            }
            self.texture.replace(Some(builder.build()));
        }
    }
}

glib::wrapper! {
    pub struct GuestScreen(ObjectSubclass<imp::GuestScreen>) @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for GuestScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl GuestScreen {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn set_backend(&self, backend: Option<Rc<dyn Backend>>) {
        if let Some(old) = self.imp().backend.replace(backend) {
            old.disconnect();
        }
    }

    pub fn backend(&self) -> Option<Rc<dyn Backend>> {
        self.imp().backend.borrow().clone()
    }

    pub fn set_scaling(&self, scaling: Scaling) {
        self.imp().scaling.set(scaling);
        self.queue_resize();
    }

    /// The key combination (GDK key values) that releases the keyboard.
    pub fn set_release_keys(&self, keys: Vec<u32>) {
        self.imp().release_combo.replace(keys);
    }

    /// Called with `true` when the screen takes the keyboard and `false`
    /// when it lets go.
    pub fn connect_grab_changed(&self, f: impl Fn(bool) + 'static) {
        self.imp().on_grab.replace(Some(Box::new(f)));
    }

    /// Called with the widget size whenever it changes.
    pub fn connect_size_changed(&self, f: impl Fn(i32, i32) + 'static) {
        self.imp().on_size.replace(Some(Box::new(f)));
    }

    /// The guest resolution, if an image is shown.
    pub fn guest_size(&self) -> Option<(i32, i32)> {
        self.imp().image_size()
    }

    /// The current image, e.g. for a screenshot.
    pub fn texture(&self) -> Option<gdk::Texture> {
        let imp = self.imp();
        if let Some((t, _)) = imp.gpu.borrow().as_ref() {
            return Some(t.clone());
        }
        if imp.dirty.borrow().is_some() {
            self.queue_draw();
        }
        imp.texture.borrow().clone()
    }

    /// Shows a GPU buffer (3D acceleration) instead of the source;
    /// `flipped` when its first row is the bottom one. `None` goes back
    /// to the source.
    pub(crate) fn set_gpu_image(&self, image: Option<(gdk::Texture, bool)>) {
        let imp = self.imp();
        let old = imp.image_size();
        imp.gpu.replace(image);
        if imp.image_size() != old {
            self.queue_resize();
        }
        self.queue_draw();
    }

    /// Sets where the image lives. The pointer must stay valid until
    /// `set_source(None)` or the next `set_source`.
    pub(crate) fn set_source(&self, source: Option<Source>) {
        let imp = self.imp();
        let resized = imp.source.get().map(|s| (s.width, s.height)) != source.map(|s| (s.width, s.height));
        imp.source.set(source);
        match source {
            Some(s) => self.invalidate(0, 0, s.width, s.height),
            None => {
                imp.texture.replace(None);
                imp.dirty.replace(None);
                self.queue_draw();
            }
        }
        if resized {
            self.queue_resize();
        }
    }

    /// Marks a rectangle of the guest image as changed.
    pub(crate) fn invalidate(&self, x: i32, y: i32, width: i32, height: i32) {
        let rect = cairo::RectangleInt::new(x, y, width, height);
        let mut dirty = self.imp().dirty.borrow_mut();
        match dirty.as_mut() {
            Some(r) => {
                let _ = r.union_rectangle(&rect);
            }
            None => *dirty = Some(cairo::Region::create_rectangle(&rect)),
        }
        drop(dirty);
        self.queue_draw();
    }

    /// Sets the pointer shown over the screen; `None` hides it.
    pub(crate) fn set_guest_cursor(&self, cursor: Option<&gdk::Cursor>) {
        match cursor {
            Some(c) => self.set_cursor(Some(c)),
            None => self.set_cursor_from_name(Some("none")),
        }
    }

    /// Sends a key combination, e.g. Ctrl+Alt+Del, as evdev codes.
    pub fn send_keys(&self, keys: &[u32]) {
        let Some(b) = self.backend() else { return };
        for &k in keys {
            b.key(true, k, 0);
        }
        for &k in keys.iter().rev() {
            b.key(false, k, 0);
        }
    }

    /// Takes the keyboard: keys (including system shortcuts where the
    /// compositor allows) go to the guest until the release keys.
    pub fn grab(&self) {
        let imp = self.imp();
        self.grab_focus();
        if imp.grabbed.replace(true) {
            return;
        }
        if let Some(t) = self.native().and_then(|n| n.surface()).and_downcast::<gdk::Toplevel>() {
            t.inhibit_system_shortcuts(None::<&gdk::Event>);
        }
        if let Some(f) = imp.on_grab.borrow().as_ref() {
            f(true);
        }
    }

    /// Gives the keyboard back to the desktop.
    pub fn ungrab(&self) {
        let imp = self.imp();
        if !imp.grabbed.replace(false) {
            return;
        }
        // Release everything still held in the guest.
        if let Some(b) = self.backend() {
            for k in imp.pressed.take() {
                b.key(false, k, 0);
            }
        }
        if let Some(t) = self.native().and_then(|n| n.surface()).and_downcast::<gdk::Toplevel>() {
            t.restore_system_shortcuts();
        }
        if let Some(root) = self.root() {
            root.set_focus(None::<&gtk::Widget>);
        }
        if let Some(f) = imp.on_grab.borrow().as_ref() {
            f(false);
        }
    }

    pub fn is_grabbed(&self) -> bool {
        self.imp().grabbed.get()
    }

    fn to_guest(&self, x: f64, y: f64) -> (i32, i32) {
        let (scale, ox, oy) = self.imp().transform.get();
        let (gw, gh) = self.guest_size().unwrap_or((1, 1));
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let gx = ((x - ox) / scale).round().clamp(0.0, f64::from(gw - 1));
        let gy = ((y - oy) / scale).round().clamp(0.0, f64::from(gh - 1));
        (gx as i32, gy as i32)
    }

    fn setup_input(&self) {
        // Keyboard.
        let keys = gtk::EventControllerKey::new();
        let w = self.downgrade();
        keys.connect_key_pressed(move |_, keyval, keycode, _| {
            let Some(s) = w.upgrade() else { return glib::Propagation::Proceed };
            s.on_key(true, keyval, keycode)
        });
        let w = self.downgrade();
        keys.connect_key_released(move |_, keyval, keycode, _| {
            if let Some(s) = w.upgrade() {
                s.on_key(false, keyval, keycode);
            }
        });
        self.add_controller(keys);
        // Losing focus (e.g. switching windows) gives the keyboard back.
        let focus = gtk::EventControllerFocus::new();
        let w = self.downgrade();
        focus.connect_leave(move |_| {
            if let Some(s) = w.upgrade() {
                s.ungrab();
            }
        });
        self.add_controller(focus);

        // Pointer.
        let motion = gtk::EventControllerMotion::new();
        let w = self.downgrade();
        motion.connect_motion(move |_, x, y| {
            let Some(s) = w.upgrade() else { return };
            let (gx, gy) = s.to_guest(x, y);
            s.imp().last_pos.set((gx, gy));
            if let Some(b) = s.backend() {
                b.motion(gx, gy, s.imp().buttons.get());
            }
        });
        self.add_controller(motion);

        let click = gtk::GestureClick::builder().button(0).build();
        let w = self.downgrade();
        click.connect_pressed(move |g, _, x, y| {
            let Some(s) = w.upgrade() else { return };
            s.grab();
            s.on_button(g.current_button(), true, x, y);
        });
        let w = self.downgrade();
        click.connect_released(move |g, _, x, y| {
            if let Some(s) = w.upgrade() {
                s.on_button(g.current_button(), false, x, y);
            }
        });
        self.add_controller(click);

        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
        );
        let w = self.downgrade();
        scroll.connect_scroll(move |_, _, dy| {
            let Some(s) = w.upgrade() else { return glib::Propagation::Proceed };
            let Some(b) = s.backend() else { return glib::Propagation::Proceed };
            let button = if dy < 0.0 { Button::WheelUp } else { Button::WheelDown };
            let (x, y) = s.imp().last_pos.get();
            let held = s.imp().buttons.get();
            b.button(button, true, held | button.mask(), x, y);
            b.button(button, false, held, x, y);
            glib::Propagation::Stop
        });
        self.add_controller(scroll);
    }

    fn on_button(&self, gdk_button: u32, down: bool, x: f64, y: f64) {
        let button = match gdk_button {
            1 => Button::Left,
            2 => Button::Middle,
            3 => Button::Right,
            _ => return,
        };
        let imp = self.imp();
        let held = if down { imp.buttons.get() | button.mask() } else { imp.buttons.get() & !button.mask() };
        imp.buttons.set(held);
        let (gx, gy) = self.to_guest(x, y);
        if let Some(b) = self.backend() {
            b.button(button, down, held, gx, gy);
        }
    }

    fn on_key(&self, down: bool, keyval: gdk::Key, keycode: u32) -> glib::Propagation {
        let imp = self.imp();
        if !imp.grabbed.get() {
            return glib::Propagation::Proceed;
        }
        let evdev = keycode.saturating_sub(8);
        let mut pressed = imp.pressed.borrow_mut();
        if down {
            if !pressed.contains(&evdev) {
                pressed.push(evdev);
            }
        } else {
            pressed.retain(|&k| k != evdev);
        }
        drop(pressed);
        // The release combination (e.g. Ctrl+Alt) gives the keyboard back.
        if down {
            let combo = imp.release_combo.borrow().clone();
            let held_vals: Vec<u32> = imp.pressed.borrow().iter().map(|&k| keymap::evdev_to_keysym(k)).collect();
            if !combo.is_empty() && combo.iter().all(|k| held_vals.contains(k) || *k == keyval.into_glib()) {
                if let Some(b) = self.backend() {
                    b.key(true, evdev, keyval.into_glib());
                }
                self.ungrab();
                return glib::Propagation::Stop;
            }
        }
        if let Some(b) = self.backend() {
            b.key(down, evdev, keyval.into_glib());
        }
        glib::Propagation::Stop
    }
}

/// Parses a key combination like `Control_L+Alt_L` into GDK key values.
pub fn parse_keys(spec: &str) -> Vec<u32> {
    spec.split('+').filter_map(|name| gdk::Key::from_name(name.trim())).map(|k| k.into_glib()).collect()
}

/// A cursor from straight (non-premultiplied) RGBA pixels.
pub(crate) fn cursor_from_rgba(data: &[u8], width: i32, height: i32, hot_x: i32, hot_y: i32) -> Option<gdk::Cursor> {
    if width <= 0 || height <= 0 || data.len() < (width * height * 4) as usize {
        return None;
    }
    let bytes = glib::Bytes::from(&data[..(width * height * 4) as usize]);
    let texture = gdk::MemoryTexture::new(width, height, gdk::MemoryFormat::R8g8b8a8, &bytes, (width * 4) as usize);
    Some(gdk::Cursor::from_texture(&texture, hot_x, hot_y, None))
}
