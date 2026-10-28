//! A text console (serial port) shown in a VTE terminal widget.

use gtk::glib::translate::{FromGlibPtrNone, ToGlibPtr};
use gtk::prelude::*;

use crate::ffi;

/// A terminal that shows what the guest writes and reports what the
/// person types.
#[derive(Clone, Debug)]
pub struct Terminal {
    widget: gtk::Widget,
}

impl Default for Terminal {
    fn default() -> Self {
        Self::new()
    }
}

impl Terminal {
    pub fn new() -> Self {
        // SAFETY: vte_terminal_new returns a floating GtkWidget reference,
        // which from_glib_none sinks and owns.
        let widget: gtk::Widget =
            unsafe { gtk::Widget::from_glib_none(ffi::vte_terminal_new().cast::<gtk::ffi::GtkWidget>()) };
        widget.set_hexpand(true);
        widget.set_vexpand(true);
        widget.add_css_class("guest-terminal");
        let t = Self { widget };
        // SAFETY: valid terminal.
        unsafe { ffi::vte_terminal_set_scrollback_lines(t.ptr(), 10_000) };
        t
    }

    fn ptr(&self) -> *mut ffi::GObject {
        let p: *mut gtk::ffi::GtkWidget = self.widget.to_glib_none().0;
        p.cast()
    }

    pub fn widget(&self) -> &gtk::Widget {
        &self.widget
    }

    /// Shows output from the guest.
    pub fn feed(&self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        // SAFETY: valid terminal; data/length describe a valid buffer.
        unsafe { ffi::vte_terminal_feed(self.ptr(), data.as_ptr().cast(), data.len() as isize) };
    }

    /// Calls `f` with the bytes the person types or pastes.
    pub fn connect_input(&self, f: impl Fn(&[u8]) + 'static) {
        self.widget.connect_local("commit", false, move |args| {
            if let Ok(text) = args[1].get::<String>() {
                f(text.as_bytes());
            }
            None
        });
    }

    pub fn copy(&self) {
        // SAFETY: valid terminal; 0 = VTE_FORMAT_TEXT.
        unsafe { ffi::vte_terminal_copy_clipboard_format(self.ptr(), 0) };
    }

    pub fn paste(&self) {
        // SAFETY: valid terminal.
        unsafe { ffi::vte_terminal_paste_clipboard(self.ptr()) };
    }
}
