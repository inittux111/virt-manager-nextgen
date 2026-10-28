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
    // Screenshot runs must not hand over to (and disturb) an instance the
    // person has open, so they run as a separate process.
    #[cfg(debug_assertions)]
    if std::env::var_os("VMNG_SCREENSHOT").is_some() {
        app.set_flags(gtk::gio::ApplicationFlags::NON_UNIQUE);
    }
    let window: Rc<RefCell<Option<Rc<MainWindow>>>> = Rc::default();
    let settings = Settings::load();

    let s = settings.clone();
    app.connect_startup(move |app| {
        load_css();
        load_icons();
        setup_actions(app, &s);
    });

    // Quitting (Ctrl+Q, the tray) doesn't close the window first, so the
    // window's state is saved here too.
    let w = window.clone();
    app.connect_shutdown(move |_| {
        if let Some(w) = w.borrow().as_ref() {
            w.save_window_state();
        }
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
    let Some(display) = gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    // APPLICATION priority is below USER priority, so the person's own
    // ~/.config/gtk-4.0/gtk.css theme always wins.
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

    // The dark-mode additions, present only while the app is dark.
    let dark = gtk::CssProvider::new();
    dark.load_from_string(include_str!("style-dark.css"));
    let apply = move |manager: &adw::StyleManager| {
        if manager.is_dark() {
            gtk::style_context_add_provider_for_display(&display, &dark, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        } else {
            gtk::style_context_remove_provider_for_display(&display, &dark);
        }
    };
    let manager = adw::StyleManager::default();
    apply(&manager);
    manager.connect_dark_notify(apply);
}

fn load_icons() {
    #[cfg(debug_assertions)]
    if let Some(display) = gdk::Display::default() {
        gtk::IconTheme::for_display(&display).add_search_path(config::DEV_ICON_DIR);
    }
    gtk::Window::set_default_icon_name(config::APP_ID);
}

fn setup_actions(app: &adw::Application, settings: &Settings) {
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
            .developer_name("Maarten")
            .website(config::WEBSITE)
            .issue_url(format!("{}/issues", config::WEBSITE))
            .license_type(gtk::License::Gpl30)
            .comments("Manage QEMU/KVM virtual machines through libvirt.")
            .build();
        dialog.present(app.active_window().as_ref());
    });
    app.add_action(&about);

    let prefs = gio::SimpleAction::new("preferences", None);
    let a = app.downgrade();
    let s = settings.clone();
    prefs.connect_activate(move |_, _| {
        if let Some(window) = a.upgrade().and_then(|app| app.active_window()) {
            crate::ui::preferences::open(&window, &s);
        }
    });
    app.add_action(&prefs);

    let shortcuts = gio::SimpleAction::new("shortcuts", None);
    let a = app.downgrade();
    shortcuts.connect_activate(move |_, _| {
        if let Some(window) = a.upgrade().and_then(|app| app.active_window()) {
            crate::ui::shortcuts::dialog().present(Some(&window));
        }
    });
    app.add_action(&shortcuts);

    crate::ui::shortcuts::install(app);
}
