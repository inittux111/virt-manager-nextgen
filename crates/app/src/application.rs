//! Application setup: styling, app-wide actions and the main window.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::config;
use crate::settings::Settings;
use crate::ui::MainWindow;

/// Runs the application and returns its exit code.
pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(config::APP_ID).build();
    let window: Rc<RefCell<Option<Rc<MainWindow>>>> = Rc::default();
    let settings = Settings::load();

    app.connect_startup(|app| {
        load_css();
        setup_actions(app);
    });

    app.connect_activate(move |app| {
        if let Some(w) = window.borrow().as_ref() {
            w.window.present();
            return;
        }
        let w = MainWindow::new(app, settings.clone());
        w.window.present();
        #[cfg(debug_assertions)]
        crate::dev::maybe_screenshot(&w.window);
        window.replace(Some(w));
    });

    // GTK parses its own arguments; we have none yet.
    app.run_with_args::<&str>(&[])
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    if let Some(display) = gdk::Display::default() {
        // APPLICATION priority is below USER priority, so the person's own
        // ~/.config/gtk-4.0/gtk.css theme always wins.
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
}

fn setup_actions(app: &adw::Application) {
    let quit = gio::SimpleAction::new("quit", None);
    let a = app.downgrade();
    quit.connect_activate(move |_, _| {
        if let Some(app) = a.upgrade() {
            app.quit();
        }
    });
    app.add_action(&quit);

    let about = gio::SimpleAction::new("about", None);
    let a = app.downgrade();
    about.connect_activate(move |_, _| {
        let Some(app) = a.upgrade() else { return };
        let dialog = adw::AboutDialog::builder()
            .application_name(config::APP_NAME)
            .application_icon(config::APP_ID)
            .version(config::VERSION)
            .developer_name("Maarten van der Rijst")
            .website(config::WEBSITE)
            .issue_url(format!("{}/issues", config::WEBSITE))
            .license_type(gtk::License::Gpl30)
            .comments("Manage QEMU/KVM virtual machines through libvirt.")
            .build();
        dialog.present(app.active_window().as_ref());
    });
    app.add_action(&about);

    // Arrives with the Preferences milestone.
    let prefs = gio::SimpleAction::new("preferences", None);
    prefs.set_enabled(false);
    app.add_action(&prefs);

    app.set_accels_for_action("app.quit", &["<Control>q"]);
    app.set_accels_for_action("app.preferences", &["<Control>comma"]);
    app.set_accels_for_action("win.search", &["<Control>f"]);
    app.set_accels_for_action("win.new-vm", &["<Control>n"]);
    app.set_accels_for_action("window.close", &["<Control>w"]);
}
