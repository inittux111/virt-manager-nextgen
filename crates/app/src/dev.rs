//! Developer aids, compiled into debug builds only.

use std::time::Duration;

use gtk::prelude::*;
use gtk::{glib, graphene};

/// If `VMNG_SCREENSHOT=<file.png>` is set, renders `window` to that file
/// after a few seconds and quits. Used to review the UI in different themes.
pub fn maybe_screenshot(window: &impl IsA<gtk::Widget>) {
    let Some(path) = std::env::var_os("VMNG_SCREENSHOT") else { return };
    let delay = std::env::var("VMNG_SCREENSHOT_DELAY").ok().and_then(|d| d.parse().ok()).unwrap_or(6);
    let widget = window.clone().upcast::<gtk::Widget>();
    glib::timeout_add_local_once(Duration::from_secs(delay), move || {
        if std::env::var_os("VMNG_DEV_OVERFLOW").is_some() {
            report_overflow(&widget, 0);
        }
        let paintable = gtk::WidgetPaintable::new(Some(&widget));
        let (w, h) = (widget.width() as f64, widget.height() as f64);
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(&snapshot, w, h);
        let saved = snapshot.to_node().and_then(|node| {
            let renderer = widget.native()?.renderer()?;
            let texture = renderer.render_texture(&node, Some(&graphene::Rect::new(0.0, 0.0, w as f32, h as f32)));
            texture.save_to_png(&path).ok()
        });
        if saved.is_none() {
            eprintln!("screenshot failed");
        }
        if let Some(app) = widget.root().and_downcast::<gtk::Window>().and_then(|w| w.application()) {
            app.quit();
        }
    });
}

/// Prints the widgets that are narrower than their minimum width, deepest
/// first, to find what holds a window at its size.
fn report_overflow(widget: &gtk::Widget, depth: usize) {
    let mut child = widget.first_child();
    while let Some(c) = child {
        report_overflow(&c, depth + 1);
        child = c.next_sibling();
    }
    if !widget.is_visible() || !widget.is_mapped() {
        return;
    }
    let (min, ..) = widget.measure(gtk::Orientation::Horizontal, -1);
    if std::env::var_os("VMNG_DEV_OVERFLOW_ALL").is_some() && min >= 361 {
        eprintln!("HDR {depth:2} {} {:?} min {min}", widget.type_().name(), widget.css_classes());
    }
    if min > widget.width() {
        eprintln!(
            "OVERFLOW {depth:2} {} {:?} min {min} has {}",
            widget.type_().name(),
            widget.css_classes(),
            widget.width()
        );
    }
}
