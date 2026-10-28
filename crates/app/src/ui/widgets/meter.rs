//! A labelled usage bar: "Memory · 18.4 of 64 GiB" over a level bar, with
//! an optional caption underneath.

use gtk::prelude::*;

/// Usage above this fraction is shown in the warning colour.
const WARN_AT: f64 = 0.9;

#[derive(Clone, Debug)]
pub struct Meter {
    pub root: gtk::Box,
    title: gtk::Label,
    value: gtk::Label,
    bar: gtk::LevelBar,
    caption: gtk::Label,
}

impl Meter {
    pub fn new(title: &str) -> Self {
        let title_label = gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["meter-title"])
            .build();
        let value = gtk::Label::builder().xalign(1.0).css_classes(["numeric", "meter-value"]).build();
        let head = gtk::Box::builder().spacing(12).build();
        head.append(&title_label);
        head.append(&value);

        let bar = gtk::LevelBar::builder().min_value(0.0).max_value(1.0).build();
        // Drop the default low/high/full offsets: the fill is the accent
        // colour, and only the explicit warning state changes it.
        for name in [gtk::LEVEL_BAR_OFFSET_LOW, gtk::LEVEL_BAR_OFFSET_HIGH, gtk::LEVEL_BAR_OFFSET_FULL] {
            bar.remove_offset_value(Some(name));
        }
        bar.update_property(&[gtk::accessible::Property::Label(title)]);

        let caption =
            gtk::Label::builder().xalign(0.0).visible(false).css_classes(["caption", "dim-label"]).wrap(true).build();

        let root =
            gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).css_classes(["meter"]).build();
        root.append(&head);
        root.append(&bar);
        root.append(&caption);
        Self { root, title: title_label, value, bar, caption }
    }

    pub fn set_title(&self, title: &str) {
        self.title.set_label(title);
        self.bar.update_property(&[gtk::accessible::Property::Label(title)]);
    }

    /// Updates the bar (0.0–1.0) and the value text.
    pub fn set(&self, fraction: f64, value: &str) {
        let f = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 0.0 };
        self.bar.set_value(f);
        if f >= WARN_AT {
            self.root.add_css_class("warning");
        } else {
            self.root.remove_css_class("warning");
        }
        self.value.set_label(value);
    }

    pub fn set_caption(&self, caption: Option<&str>) {
        self.caption.set_visible(caption.is_some());
        self.caption.set_label(caption.unwrap_or_default());
    }

    /// Never warn, even when nearly full (e.g. fully allocated raw images).
    pub fn set_warning_allowed(&self, allowed: bool) {
        if allowed {
            self.root.remove_css_class("no-warning");
        } else {
            self.root.add_css_class("no-warning");
        }
    }
}
