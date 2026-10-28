//! Consistent ways to tell the person what happened: toasts for results,
//! an error toast with details, and confirmation dialogs.

use adw::prelude::*;
use gtk::glib;

#[derive(Clone, Debug)]
pub struct Feedback {
    toasts: adw::ToastOverlay,
    parent: gtk::Widget,
}

impl Feedback {
    pub fn new(toasts: &adw::ToastOverlay, parent: &impl IsA<gtk::Widget>) -> Self {
        Self { toasts: toasts.clone(), parent: parent.clone().upcast() }
    }

    pub fn parent(&self) -> &gtk::Widget {
        &self.parent
    }

    /// A short message that disappears on its own.
    pub fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::builder().title(glib::markup_escape_text(text)).timeout(4).build());
    }

    /// A failed operation: a toast with a button that shows the details.
    pub fn error(&self, title: &str, details: &str) {
        let toast =
            adw::Toast::builder().title(glib::markup_escape_text(title)).button_label("Details").timeout(8).build();
        let parent = self.parent.clone();
        let (title, details) = (title.to_owned(), details.to_owned());
        toast.connect_button_clicked(move |_| {
            let dialog = adw::AlertDialog::builder().heading(&title).body(&details).build();
            dialog.add_response("close", "Close");
            dialog.present(Some(&parent));
        });
        self.toasts.add_toast(toast);
    }

    /// Asks before doing something. `destructive` marks data loss.
    pub fn confirm(&self, heading: &str, body: &str, verb: &str, destructive: bool, on_confirm: impl Fn() + 'static) {
        let dialog = confirm_dialog(heading, body, verb, destructive);
        dialog.connect_response(Some("confirm"), move |_, _| on_confirm());
        dialog.present(Some(&self.parent));
    }
}

/// The standard two-button confirmation dialog.
pub fn confirm_dialog(heading: &str, body: &str, verb: &str, destructive: bool) -> adw::AlertDialog {
    let dialog = adw::AlertDialog::builder().heading(heading).body(body).build();
    dialog.add_responses(&[("cancel", "Cancel"), ("confirm", verb)]);
    dialog.set_response_appearance(
        "confirm",
        if destructive { adw::ResponseAppearance::Destructive } else { adw::ResponseAppearance::Suggested },
    );
    dialog.set_default_response(Some(if destructive { "cancel" } else { "confirm" }));
    dialog.set_close_response("cancel");
    dialog
}
