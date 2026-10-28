//! The Preferences dialog: General, New VMs, Console and Confirmations.
//! Every change is saved immediately.

use std::rc::Rc;

use adw::prelude::*;

use crate::settings::{ConsoleScaling, Prefs, Settings};

/// A switch row bound to one boolean preference.
fn switch(
    settings: &Settings,
    title: &str,
    subtitle: &str,
    get: fn(&Prefs) -> bool,
    set: fn(&mut Prefs, bool),
) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder().title(title).active(get(&settings.get())).build();
    if !subtitle.is_empty() {
        row.set_subtitle(subtitle);
    }
    let s = settings.clone();
    row.connect_active_notify(move |r| {
        let on = r.is_active();
        s.update(|p| set(p, on));
    });
    row
}

/// A drop-down bound to one string-valued preference.
fn choice(
    settings: &Settings,
    title: &str,
    options: &'static [(&'static str, &'static str)],
    get: fn(&Prefs) -> String,
    set: fn(&mut Prefs, &str),
) -> adw::ComboRow {
    let labels: Vec<&str> = options.iter().map(|(_, l)| *l).collect();
    let row = adw::ComboRow::builder().title(title).model(&gtk::StringList::new(&labels)).build();
    let current = get(&settings.get());
    row.set_selected(options.iter().position(|(v, _)| *v == current).unwrap_or(0) as u32);
    let s = settings.clone();
    row.connect_selected_notify(move |r| {
        if let Some((v, _)) = options.get(r.selected() as usize) {
            s.update(|p| set(p, v));
        }
    });
    row
}

fn group(title: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder().title(title).build()
}

pub fn open(parent: &impl IsA<gtk::Widget>, settings: &Settings) {
    // Wide enough that the page switcher sits at the top.
    let dialog = adw::PreferencesDialog::builder().search_enabled(true).content_width(820).content_height(640).build();
    dialog.add(&general_page(settings));
    dialog.add(&new_vm_page(settings));
    dialog.add(&console_page(settings));
    dialog.add(&confirm_page(settings));
    dialog.present(Some(parent));
}

fn general_page(s: &Settings) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title("General").icon_name("preferences-system-symbolic").build();

    let features = group("Features");
    let tray = switch(
        s,
        "System tray icon",
        "Closing the window keeps the app running in the tray",
        |p| p.tray_icon,
        |p, v| p.tray_icon = v,
    );
    let host = gtk::gio::Application::default().and_then(|a| a.dbus_connection());
    if !host.as_ref().is_some_and(crate::tray::available) {
        tray.set_subtitle("Not available: no tray is running (in GNOME, turn on the AppIndicator extension)");
        tray.set_sensitive(false);
    }
    features.add(&tray);
    let color = switch(
        s,
        "Colored tray icon",
        "Shows the app icon in color instead of matching the top bar",
        |p| p.tray_icon_color,
        |p, v| p.tray_icon_color = v,
    );
    tray.bind_property("active", &color, "sensitive").sync_create().build();
    if !tray.is_sensitive() {
        color.set_sensitive(false);
    }
    features.add(&color);
    features.add(&switch(
        s,
        "Allow editing the raw XML",
        "For advanced users; unchecked changes can break a machine",
        |p| p.xml_editing,
        |p, v| p.xml_editing = v,
    ));
    page.add(&features);

    let stats = group("Statistics");
    let interval = adw::ComboRow::builder()
        .title("Update every")
        .model(&gtk::StringList::new(&["1 second", "2 seconds", "3 seconds", "5 seconds", "10 seconds"]))
        .build();
    const SECS: [u32; 5] = [1, 2, 3, 5, 10];
    interval.set_selected(SECS.iter().position(|&v| v == s.get().stats_interval_secs).unwrap_or(1) as u32);
    let st = s.clone();
    interval.connect_selected_notify(move |r| {
        let v = SECS[(r.selected() as usize).min(SECS.len() - 1)];
        st.update(|p| p.stats_interval_secs = v);
    });
    stats.add(&interval);
    stats.add(&switch(s, "Collect CPU usage", "", |p| p.stats_cpu, |p, v| p.stats_cpu = v));
    stats.add(&switch(s, "Collect memory usage", "", |p| p.stats_memory, |p, v| p.stats_memory = v));
    stats.add(&switch(s, "Collect disk I/O", "", |p| p.stats_disk, |p, v| p.stats_disk = v));
    stats.add(&switch(s, "Collect network I/O", "", |p| p.stats_network, |p, v| p.stats_network = v));
    page.add(&stats);

    let startup = group("Startup");
    let auto = |uri: &'static str| move |p: &Prefs| p.autoconnect.iter().any(|u| u == uri);
    let set_auto = |uri: &'static str| {
        move |p: &mut Prefs, on: bool| {
            p.autoconnect.retain(|u| u != uri);
            if on {
                p.autoconnect.push(uri.to_owned());
            }
        }
    };
    for (title, uri) in [
        ("Connect to QEMU/KVM · System at startup", "qemu:///system"),
        ("Connect to QEMU/KVM · User session at startup", "qemu:///session"),
    ] {
        let row = adw::SwitchRow::builder().title(title).active(auto(uri)(&s.get())).build();
        let st = s.clone();
        let set = Rc::new(set_auto(uri));
        row.connect_active_notify(move |r| {
            let on = r.is_active();
            let set = set.clone();
            st.update(move |p| set(p, on));
        });
        startup.add(&row);
    }
    page.add(&startup);
    page
}

fn new_vm_page(s: &Settings) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title("New VMs").icon_name("list-add-symbolic").build();
    let g = group("Defaults for new machines");
    g.set_description(Some("The New VM wizard starts with these; you can still change them per machine."));
    g.add(&choice(
        s,
        "Display",
        &[("spice", "SPICE (recommended)"), ("vnc", "VNC")],
        |p| p.new_vm_graphics.clone(),
        |p, v| p.new_vm_graphics = v.into(),
    ));
    g.add(&choice(
        s,
        "Disk format",
        &[("qcow2", "qcow2 (snapshots, grows as needed)"), ("raw", "raw (fastest)")],
        |p| p.new_vm_disk_format.clone(),
        |p, v| p.new_vm_disk_format = v.into(),
    ));
    g.add(&choice(
        s,
        "CPU model",
        &[
            ("host-passthrough", "Same as the host (fastest)"),
            ("host-model", "Similar to the host (portable)"),
            ("default", "Hypervisor default"),
        ],
        |p| p.new_vm_cpu_model.clone(),
        |p, v| p.new_vm_cpu_model = v.into(),
    ));
    g.add(&choice(
        s,
        "Firmware",
        &[("uefi", "UEFI (recommended)"), ("bios", "BIOS")],
        |p| p.new_vm_firmware.clone(),
        |p, v| p.new_vm_firmware = v.into(),
    ));
    page.add(&g);
    page
}

fn console_page(s: &Settings) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title("Console").icon_name("video-display-symbolic").build();
    let g = group("Display");
    g.add(&switch(
        s,
        "Connect automatically",
        "Show the screen as soon as a console window opens or its machine starts",
        |p| p.console_autoconnect,
        |p, v| p.console_autoconnect = v,
    ));
    let scaling = adw::ComboRow::builder()
        .title("Scale the screen to the window")
        .model(&gtk::StringList::new(&["Always", "Never", "Only in fullscreen"]))
        .build();
    const MODES: [ConsoleScaling; 3] = [ConsoleScaling::Always, ConsoleScaling::Never, ConsoleScaling::FullscreenOnly];
    scaling.set_selected(MODES.iter().position(|m| *m == s.get().console_scaling).unwrap_or(0) as u32);
    let st = s.clone();
    scaling.connect_selected_notify(move |r| {
        let m = MODES[(r.selected() as usize).min(2)];
        st.update(|p| p.console_scaling = m);
    });
    g.add(&scaling);
    g.add(&switch(
        s,
        "Resize the guest to the window",
        "Needs the SPICE agent (spice-vdagent) in the guest",
        |p| p.console_resize_guest,
        |p, v| p.console_resize_guest = v,
    ));
    page.add(&g);

    let keys = group("Keyboard");
    keys.add(&choice(
        s,
        "Release the keyboard with",
        &[("Control_L+Alt_L", "Ctrl+Alt"), ("Control_L+Alt_L+Shift_L", "Ctrl+Alt+Shift"), ("Control_R", "Right Ctrl")],
        |p| p.console_grab_keys.clone(),
        |p, v| p.console_grab_keys = v.into(),
    ));
    page.add(&keys);
    page
}

fn confirm_page(s: &Settings) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title("Confirmations").icon_name("dialog-question-symbolic").build();
    let g = group("Ask before…");
    g.set_description(Some("Turn off a question to do the action right away."));
    g.add(&switch(
        s,
        "Forcing a machine off or resetting it",
        "",
        |p| p.confirm_force_off,
        |p, v| p.confirm_force_off = v,
    ));
    g.add(&switch(s, "Shutting down or rebooting", "", |p| p.confirm_shutdown, |p, v| p.confirm_shutdown = v));
    g.add(&switch(s, "Pausing a machine", "", |p| p.confirm_pause, |p, v| p.confirm_pause = v));
    g.add(&switch(s, "Deleting a machine", "", |p| p.confirm_delete, |p, v| p.confirm_delete = v));
    g.add(&switch(s, "Removing a device", "", |p| p.confirm_remove_device, |p, v| p.confirm_remove_device = v));
    g.add(&switch(
        s,
        "Leaving hardware changes unapplied",
        "",
        |p| p.confirm_unapplied_changes,
        |p, v| p.confirm_unapplied_changes = v,
    ));
    page.add(&g);
    page
}
