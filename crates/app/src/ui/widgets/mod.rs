//! Reusable building blocks. Every screen composes these so that the same
//! idea always looks the same everywhere.

mod chart;
mod form_dialog;
mod logo_data;
mod meter;
mod os_logo;
mod sparkline;
mod status;

pub use chart::Chart;
pub use form_dialog::FormDialog;
pub use meter::Meter;
pub use os_logo::OsLogo;
pub use sparkline::Sparkline;
pub use status::{StatusLabel, Tone};

use adw::prelude::*;

/// A section heading above a group of cards or rows, e.g. "Storage".
pub fn section_heading(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).css_classes(["heading", "section-heading"]).build()
}

/// A rounded, neutral card container.
pub fn card(orientation: gtk::Orientation, spacing: i32) -> gtk::Box {
    gtk::Box::builder().orientation(orientation).spacing(spacing).css_classes(["card"]).build()
}

/// A `boxed-list` styled list for rows of facts or settings.
pub fn boxed_list() -> gtk::ListBox {
    gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build()
}

/// A key/value row for read-only facts ("IP address — 192.168.122.24").
pub fn fact_row(title: &str) -> (adw::ActionRow, gtk::Label) {
    let value = gtk::Label::builder()
        .xalign(1.0)
        .selectable(true)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .css_classes(["dim-label", "fact-value"])
        .build();
    let row = adw::ActionRow::builder().title(title).build();
    row.add_suffix(&value);
    (row, value)
}

/// An icon-only button with a tooltip and accessible label.
pub fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::builder().icon_name(icon).tooltip_text(tooltip).valign(gtk::Align::Center).build();
    b.update_property(&[gtk::accessible::Property::Label(tooltip)]);
    b
}

/// A button with an icon and a text label.
pub fn labeled_button(icon: &str, label: &str) -> gtk::Button {
    let content = adw::ButtonContent::builder().icon_name(icon).label(label).build();
    gtk::Button::builder().child(&content).valign(gtk::Align::Center).build()
}
