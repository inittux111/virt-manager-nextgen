//! Keyboard shortcuts: one table that both installs the accelerators and
//! fills the Keyboard Shortcuts dialog, so the two never disagree.

use adw::prelude::*;

struct Shortcut {
    title: &'static str,
    /// The action it triggers, or `None` for keys handled elsewhere (shown
    /// in the dialog only).
    action: Option<&'static str>,
    accels: &'static [&'static str],
}

const fn act(title: &'static str, action: &'static str, accels: &'static [&'static str]) -> Shortcut {
    Shortcut { title, action: Some(action), accels }
}

const fn info(title: &'static str, accels: &'static [&'static str]) -> Shortcut {
    Shortcut { title, action: None, accels }
}

const SECTIONS: &[(&str, &[Shortcut])] = &[
    (
        "General",
        &[
            act("New virtual machine", "win.new-vm", &["<Control>n"]),
            act("Search machines", "win.search", &["<Control>f"]),
            act("Preferences", "app.preferences", &["<Control>comma"]),
            act("Keyboard shortcuts", "app.shortcuts", &["<Control>question"]),
            act("Close window", "window.close", &["<Control>w"]),
            act("Quit", "app.quit", &["<Control>q"]),
        ],
    ),
    (
        "Views",
        &[
            act("Virtual machines", "win.view::machines", &["<Control>1"]),
            act("Host overview", "win.view::host", &["<Control>2"]),
            act("Virtual networks", "win.view::networks", &["<Control>3"]),
            act("Storage pools", "win.view::storage", &["<Control>4"]),
            info("Go back", &["<Alt>Left", "Escape"]),
        ],
    ),
    (
        "Selected machine",
        &[
            act("Open the console", "vm.console", &["<Control>o"]),
            act("Hardware", "vm.hardware", &["<Control>i"]),
            act("Take a snapshot", "vm.take-snapshot", &["<Control>t"]),
            act("Rename", "vm.rename", &["F2"]),
        ],
    ),
    (
        "Console",
        &[
            info("Fullscreen", &["F11"]),
            info("Release the keyboard and mouse (changeable in Preferences)", &["<Control>Alt_L"]),
        ],
    ),
];

/// Registers every shortcut's accelerators with the application.
pub fn install(app: &impl IsA<gtk::Application>) {
    set_enabled(app, true);
}

/// Turns the app's shortcuts off while a guest or terminal has the keyboard:
/// GTK handles them before the focused widget, so Ctrl+W would otherwise
/// close the window instead of reaching the guest.
pub fn set_enabled(app: &impl IsA<gtk::Application>, enabled: bool) {
    for (_, items) in SECTIONS {
        for s in *items {
            if let Some(action) = s.action {
                app.set_accels_for_action(action, if enabled { s.accels } else { &[] });
            }
        }
    }
}

/// [`set_enabled`] on the running application, plus GTK's own F10 (which
/// opens the main menu).
pub fn set_app_enabled(enabled: bool) {
    if let Some(app) = gtk::gio::Application::default().and_downcast::<gtk::Application>() {
        set_enabled(&app, enabled);
        for window in app.windows() {
            window.set_handle_menubar_accel(enabled);
        }
    }
}

/// The Keyboard Shortcuts dialog (libadwaita's own, from 1.8).
#[cfg(feature = "gnome49")]
pub fn dialog() -> adw::Dialog {
    let dialog = adw::ShortcutsDialog::new();
    for (title, items) in SECTIONS {
        let section = adw::ShortcutsSection::new(Some(title));
        for s in *items {
            section.add(adw::ShortcutsItem::new(s.title, &s.accels.join(" ")));
        }
        dialog.add(section);
    }
    dialog.upcast()
}

/// The Keyboard Shortcuts dialog for libadwaita 1.7 (Debian 13): the same
/// sections and keys, as a preferences-style page.
#[cfg(not(feature = "gnome49"))]
pub fn dialog() -> adw::Dialog {
    let page = adw::PreferencesPage::new();
    for (title, items) in SECTIONS {
        let group = adw::PreferencesGroup::builder().title(*title).build();
        for s in *items {
            let keys: Vec<String> = s
                .accels
                .iter()
                .filter_map(|a| gtk::accelerator_parse(*a).map(|(key, mods)| gtk::accelerator_get_label(key, mods)))
                .map(|l| l.to_string())
                .collect();
            let row = adw::ActionRow::builder().title(s.title).title_lines(2).build();
            row.add_suffix(&gtk::Label::builder().label(keys.join("  or  ")).css_classes(["dim-label"]).build());
            group.add(&row);
        }
        page.add(&group);
    }
    let toolbar = adw::ToolbarView::builder().content(&page).build();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    adw::Dialog::builder().title("Keyboard Shortcuts").content_width(560).content_height(640).child(&toolbar).build()
}

#[cfg(test)]
mod tests {
    use super::SECTIONS;

    #[test]
    fn accelerators_are_unique() {
        let mut seen = Vec::new();
        for (_, items) in SECTIONS {
            for s in *items {
                for a in s.accels {
                    // Escape also closes dialogs; only actions must be unique.
                    if s.action.is_some() {
                        assert!(!seen.contains(a), "{a} used twice");
                        seen.push(*a);
                    }
                }
            }
        }
    }
}
