//! A coloured status dot paired with a text label.
//!
//! Status colours always mean the same thing (green = running, yellow =
//! paused/warning, red = error, gray = off) and are never shown without a
//! word next to them.

use gtk::prelude::*;
use vmng_virt::DomainState;

/// The visual tone of a status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Running,
    Paused,
    Error,
    Off,
}

impl Tone {
    pub fn of(state: DomainState) -> Self {
        match state {
            DomainState::Running | DomainState::ShuttingDown => Self::Running,
            DomainState::Paused | DomainState::Suspended => Self::Paused,
            DomainState::Crashed => Self::Error,
            DomainState::ShutOff | DomainState::Unknown => Self::Off,
        }
    }

    fn class(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Error => "error",
            Self::Off => "off",
        }
    }
}

/// A dot plus label, e.g. "● Running · Fedora Linux 40".
#[derive(Clone, Debug)]
pub struct StatusLabel {
    pub root: gtk::Box,
    dot: gtk::Box,
    label: gtk::Label,
}

impl StatusLabel {
    pub fn new() -> Self {
        let dot = gtk::Box::builder().valign(gtk::Align::Center).css_classes(["status-dot"]).build();
        let label = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).hexpand(true).build();
        let root = gtk::Box::builder().spacing(6).css_classes(["status-label"]).build();
        root.append(&dot);
        root.append(&label);
        Self { root, dot, label }
    }

    pub fn set(&self, tone: Tone, text: &str) {
        for t in [Tone::Running, Tone::Paused, Tone::Error, Tone::Off] {
            if t == tone {
                self.dot.add_css_class(t.class());
            } else {
                self.dot.remove_css_class(t.class());
            }
        }
        if self.label.label() != text {
            self.label.set_label(text);
        }
    }
}

impl Default for StatusLabel {
    fn default() -> Self {
        Self::new()
    }
}
