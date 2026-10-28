//! The standard dialog for entering a few values: "Cancel" on the left,
//! the primary action on the right, content in preference groups, and an
//! inline error line that also disables the primary button.

use std::rc::Rc;

use adw::prelude::*;

#[derive(Clone, Debug)]
pub struct FormDialog {
    pub dialog: adw::Dialog,
    content: gtk::Box,
    primary: gtk::Button,
    primary_label: gtk::Label,
    spinner: adw::Spinner,
    error: gtk::Label,
}

impl FormDialog {
    pub fn new(title: &str, primary_label: &str) -> Self {
        let cancel = gtk::Button::builder().label("Cancel").build();
        let label = gtk::Label::new(Some(primary_label));
        let spinner = adw::Spinner::builder().visible(false).build();
        let inner = gtk::Box::builder().spacing(8).halign(gtk::Align::Center).build();
        inner.append(&spinner);
        inner.append(&label);
        let primary = gtk::Button::builder().child(&inner).css_classes(["suggested-action"]).build();

        let header = adw::HeaderBar::builder().show_start_title_buttons(false).show_end_title_buttons(false).build();
        header.pack_start(&cancel);
        header.pack_end(&primary);

        let error =
            gtk::Label::builder().xalign(0.0).wrap(true).visible(false).css_classes(["error", "caption"]).build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .margin_top(12)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .build();
        let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
        body.append(&content);
        let error_box = gtk::Box::builder().margin_start(24).margin_end(24).margin_bottom(18).build();
        error_box.append(&error);
        body.append(&error_box);

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .child(&body)
            .build();
        let toolbar = adw::ToolbarView::builder().content(&scroller).build();
        toolbar.add_top_bar(&header);
        let dialog = adw::Dialog::builder().title(title).content_width(460).child(&toolbar).build();
        dialog.set_default_widget(Some(&primary));

        let d = dialog.clone();
        cancel.connect_clicked(move |_| {
            d.close();
        });
        Self { dialog, content, primary, primary_label: label, spinner, error }
    }

    pub fn add(&self, widget: &impl IsA<gtk::Widget>) {
        self.content.append(widget);
    }

    /// Shows a problem with the input; `None` clears it and re-enables the
    /// primary button.
    pub fn set_error(&self, message: Option<&str>) {
        self.error.set_visible(message.is_some());
        self.error.set_label(message.unwrap_or_default());
        self.primary.set_sensitive(message.is_none());
    }

    /// Disables the dialog while the operation runs.
    pub fn set_busy(&self, busy: bool) {
        self.spinner.set_visible(busy);
        self.primary.set_sensitive(!busy);
        self.content.set_sensitive(!busy);
        self.dialog.set_can_close(!busy);
    }

    pub fn set_primary_label(&self, label: &str) {
        self.primary_label.set_label(label);
    }

    pub fn connect_primary(&self, f: impl Fn(&Self) + 'static) {
        let this = self.clone();
        let f = Rc::new(f);
        self.primary.connect_clicked(move |_| f(&this));
    }

    pub fn present(&self, parent: &impl IsA<gtk::Widget>) {
        self.dialog.present(Some(parent));
        // Debug builds: VMNG_DEV_FORM_SUBMIT=1 presses the primary button.
        #[cfg(debug_assertions)]
        if std::env::var_os("VMNG_DEV_FORM_SUBMIT").is_some() {
            let primary = self.primary.clone();
            gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(800), move || primary.emit_clicked());
        }
    }

    pub fn close(&self) {
        self.dialog.set_can_close(true);
        self.dialog.close();
    }
}
