//! A rounded tile showing the guest OS logo.
//!
//! Known operating systems are drawn from vector path data in the brand's
//! colours; unknown ones get a neutral, theme-coloured computer icon.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, graphene, gsk};

use super::logo_data::{LOGOS, LogoArt};

/// Fraction of the tile occupied by the glyph.
const GLYPH_SCALE: f32 = 0.62;

thread_local! {
    static PATHS: RefCell<HashMap<&'static str, Option<gsk::Path>>> = RefCell::default();
}

fn parsed_path(art: &'static LogoArt) -> Option<gsk::Path> {
    PATHS.with(|cache| cache.borrow_mut().entry(art.key).or_insert_with(|| gsk::Path::parse(art.path).ok()).clone())
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct OsLogo {
        pub art: Cell<Option<&'static LogoArt>>,
        pub size: Cell<i32>,
        pub generic: RefCell<Option<gtk::Image>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for OsLogo {
        const NAME: &'static str = "VmngOsLogo";
        type Type = super::OsLogo;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("os-logo");
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for OsLogo {
        fn constructed(&self) {
            self.parent_constructed();
            let image = gtk::Image::from_icon_name("computer-symbolic");
            image.set_parent(&*self.obj());
            self.generic.replace(Some(image));
        }

        fn dispose(&self) {
            if let Some(image) = self.generic.take() {
                image.unparent();
            }
        }
    }

    impl WidgetImpl for OsLogo {
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let s = self.size.get();
            (s, s, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(image) = self.generic.borrow().as_ref() {
                image.allocate(width, height, baseline, None);
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let Some(art) = self.art.get() else {
                // Generic tile: CSS draws the background, the child the icon.
                self.parent_snapshot(snapshot);
                return;
            };
            let w = widget.width() as f32;
            let h = widget.height() as f32;
            let side = w.min(h);
            let bounds = graphene::Rect::new((w - side) / 2.0, (h - side) / 2.0, side, side);
            let radius = (side * 0.24).round();
            let rounded = gsk::RoundedRect::from_rect(bounds, radius);
            let bg = gdk::RGBA::parse(art.background).unwrap_or(gdk::RGBA::BLACK);
            snapshot.push_rounded_clip(&rounded);
            snapshot.append_color(&bg, &bounds);
            if let Some(path) = parsed_path(art) {
                let fg = gdk::RGBA::parse(art.glyph).unwrap_or(gdk::RGBA::WHITE);
                let glyph = side * GLYPH_SCALE;
                snapshot.save();
                snapshot.translate(&graphene::Point::new(
                    bounds.x() + (side - glyph) / 2.0,
                    bounds.y() + (side - glyph) / 2.0,
                ));
                snapshot.scale(glyph / 24.0, glyph / 24.0);
                snapshot.append_fill(&path, gsk::FillRule::Winding, &fg);
                snapshot.restore();
            }
            snapshot.pop();
        }
    }
}

glib::wrapper! {
    pub struct OsLogo(ObjectSubclass<imp::OsLogo>) @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl OsLogo {
    /// Creates a logo tile of `size`×`size` pixels.
    pub fn new(size: i32) -> Self {
        let logo: Self = glib::Object::new();
        logo.imp().size.set(size);
        if let Some(img) = logo.imp().generic.borrow().as_ref() {
            img.set_pixel_size((size as f32 * 0.55).round() as i32);
        }
        logo.set_halign(gtk::Align::Center);
        logo.set_valign(gtk::Align::Center);
        logo.set_logo(None);
        logo
    }

    /// Shows the logo for a key from `logo_data`, or the generic tile.
    pub fn set_logo(&self, key: Option<&str>) {
        let art = key.and_then(|k| LOGOS.iter().find(|a| a.key == k));
        let imp = self.imp();
        if imp.art.get().map(|a| a.key) == art.map(|a| a.key) && self.has_css_class("generic") == art.is_none() {
            return;
        }
        imp.art.set(art);
        if let Some(img) = imp.generic.borrow().as_ref() {
            img.set_visible(art.is_none());
        }
        if art.is_some() {
            self.remove_css_class("generic");
        } else {
            self.add_css_class("generic");
        }
        self.queue_draw();
    }
}
