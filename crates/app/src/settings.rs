//! Persistent user preferences.
//!
//! Stored as an INI-style key file in
//! `$XDG_CONFIG_HOME/virt-manager-nextgen/settings.ini` through GLib's
//! `KeyFile`. This avoids GSettings schemas, which would have to be compiled
//! and installed before the app could even start from a build directory.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::glib;

use crate::config;

/// How the guest display is scaled in the console.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleScaling {
    Always,
    Never,
    FullscreenOnly,
}

/// The preferences as plain values.
#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    /// Connections opened at startup, by URI.
    pub autoconnect: Vec<String>,
    /// The connection shown when the app starts.
    pub last_connection: String,

    pub stats_interval_secs: u32,
    pub stats_cpu: bool,
    pub stats_memory: bool,
    pub stats_disk: bool,
    pub stats_network: bool,

    pub tray_icon: bool,
    /// Full-colour tray icon instead of one matching the top bar.
    pub tray_icon_color: bool,
    pub libguestfs_inspection: bool,
    pub xml_editing: bool,

    pub console_autoconnect: bool,
    pub console_scaling: ConsoleScaling,
    pub console_resize_guest: bool,
    /// Key combination that releases the keyboard/mouse grab, e.g. `Control_L+Alt_L`.
    pub console_grab_keys: String,

    pub new_vm_graphics: String,
    pub new_vm_disk_format: String,
    pub new_vm_cpu_model: String,
    pub new_vm_firmware: String,

    pub confirm_force_off: bool,
    pub confirm_shutdown: bool,
    pub confirm_pause: bool,
    pub confirm_delete: bool,
    pub confirm_remove_device: bool,
    pub confirm_unapplied_changes: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            autoconnect: vec!["qemu:///system".into()],
            last_connection: "qemu:///system".into(),
            stats_interval_secs: 2,
            stats_cpu: true,
            stats_memory: true,
            stats_disk: true,
            stats_network: true,
            tray_icon: false,
            tray_icon_color: false,
            libguestfs_inspection: false,
            xml_editing: false,
            console_autoconnect: true,
            console_scaling: ConsoleScaling::Always,
            console_resize_guest: true,
            console_grab_keys: "Control_L+Alt_L".into(),
            new_vm_graphics: "spice".into(),
            new_vm_disk_format: "qcow2".into(),
            new_vm_cpu_model: "host-passthrough".into(),
            new_vm_firmware: "uefi".into(),
            confirm_force_off: true,
            confirm_shutdown: false,
            confirm_pause: false,
            confirm_delete: true,
            confirm_remove_device: true,
            confirm_unapplied_changes: true,
        }
    }
}

/// Shared, auto-saving preferences handle.
#[derive(Clone, Debug)]
pub struct Settings(Rc<Inner>);

type Listener = Rc<dyn Fn(&Prefs)>;

struct Inner {
    prefs: RefCell<Prefs>,
    path: PathBuf,
    listeners: RefCell<Vec<Listener>>,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Settings").field("path", &self.path).field("prefs", &self.prefs).finish_non_exhaustive()
    }
}

impl Settings {
    /// Loads the settings file, falling back to defaults for anything
    /// missing or unreadable.
    pub fn load() -> Self {
        let path = glib::user_config_dir().join(config::CONFIG_DIR).join("settings.ini");
        let kf = glib::KeyFile::new();
        let prefs = match kf.load_from_file(&path, glib::KeyFileFlags::NONE) {
            Ok(()) => read(&kf),
            Err(_) => Prefs::default(),
        };
        Self(Rc::new(Inner { prefs: RefCell::new(prefs), path, listeners: RefCell::default() }))
    }

    /// A copy of the current preferences.
    pub fn get(&self) -> Prefs {
        self.0.prefs.borrow().clone()
    }

    /// Changes preferences, saves them and notifies listeners.
    pub fn update(&self, f: impl FnOnce(&mut Prefs)) {
        let snapshot = {
            let mut p = self.0.prefs.borrow_mut();
            let before = p.clone();
            f(&mut p);
            if *p == before {
                return;
            }
            p.clone()
        };
        if let Err(e) = self.save(&snapshot) {
            glib::g_warning!("settings", "could not save settings to {}: {e}", self.0.path.display());
        }
        let listeners: Vec<_> = self.0.listeners.borrow().clone();
        for l in listeners {
            l(&snapshot);
        }
    }

    /// Calls `f` after every change.
    pub fn connect_changed(&self, f: impl Fn(&Prefs) + 'static) {
        self.0.listeners.borrow_mut().push(Rc::new(f));
    }

    fn save(&self, p: &Prefs) -> Result<(), glib::Error> {
        if let Some(dir) = self.0.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| glib::Error::new(glib::FileError::Failed, &e.to_string()))?;
        }
        write(p).save_to_file(&self.0.path)
    }
}

const SCALING: [(ConsoleScaling, &str); 3] = [
    (ConsoleScaling::Always, "always"),
    (ConsoleScaling::Never, "never"),
    (ConsoleScaling::FullscreenOnly, "fullscreen"),
];

fn read(kf: &glib::KeyFile) -> Prefs {
    let d = Prefs::default();
    let b = |g: &str, k: &str, def: bool| kf.boolean(g, k).unwrap_or(def);
    let s = |g: &str, k: &str, def: &str| kf.string(g, k).map_or_else(|_| def.to_owned(), |v| v.to_string());
    let scaling = s("console", "scaling", "always");
    Prefs {
        autoconnect: kf
            .string_list("connections", "autoconnect")
            .map_or(d.autoconnect.clone(), |l| l.iter().map(|g| g.to_string()).collect()),
        last_connection: s("connections", "last", &d.last_connection),
        stats_interval_secs: kf.integer("stats", "interval").map_or(d.stats_interval_secs, |v| v.clamp(1, 60) as u32),
        stats_cpu: b("stats", "cpu", d.stats_cpu),
        stats_memory: b("stats", "memory", d.stats_memory),
        stats_disk: b("stats", "disk", d.stats_disk),
        stats_network: b("stats", "network", d.stats_network),
        tray_icon: b("general", "tray-icon", d.tray_icon),
        tray_icon_color: b("general", "tray-icon-color", d.tray_icon_color),
        libguestfs_inspection: b("general", "libguestfs", d.libguestfs_inspection),
        xml_editing: b("general", "xml-editing", d.xml_editing),
        console_autoconnect: b("console", "autoconnect", d.console_autoconnect),
        console_scaling: SCALING.iter().find(|(_, n)| *n == scaling).map_or(d.console_scaling, |(v, _)| *v),
        console_resize_guest: b("console", "resize-guest", d.console_resize_guest),
        console_grab_keys: s("console", "grab-keys", &d.console_grab_keys),
        new_vm_graphics: s("new-vm", "graphics", &d.new_vm_graphics),
        new_vm_disk_format: s("new-vm", "disk-format", &d.new_vm_disk_format),
        new_vm_cpu_model: s("new-vm", "cpu-model", &d.new_vm_cpu_model),
        new_vm_firmware: s("new-vm", "firmware", &d.new_vm_firmware),
        confirm_force_off: b("confirm", "force-off", d.confirm_force_off),
        confirm_shutdown: b("confirm", "shutdown", d.confirm_shutdown),
        confirm_pause: b("confirm", "pause", d.confirm_pause),
        confirm_delete: b("confirm", "delete", d.confirm_delete),
        confirm_remove_device: b("confirm", "remove-device", d.confirm_remove_device),
        confirm_unapplied_changes: b("confirm", "unapplied-changes", d.confirm_unapplied_changes),
    }
}

fn write(p: &Prefs) -> glib::KeyFile {
    let kf = glib::KeyFile::new();
    // KeyFile lists are ';'-separated; escape separators inside values.
    let list: Vec<String> = p.autoconnect.iter().map(|u| u.replace('\\', "\\\\").replace(';', "\\;")).collect();
    kf.set_value("connections", "autoconnect", &list.join(";"));
    kf.set_string("connections", "last", &p.last_connection);
    kf.set_integer("stats", "interval", p.stats_interval_secs as i32);
    kf.set_boolean("stats", "cpu", p.stats_cpu);
    kf.set_boolean("stats", "memory", p.stats_memory);
    kf.set_boolean("stats", "disk", p.stats_disk);
    kf.set_boolean("stats", "network", p.stats_network);
    kf.set_boolean("general", "tray-icon", p.tray_icon);
    kf.set_boolean("general", "tray-icon-color", p.tray_icon_color);
    kf.set_boolean("general", "libguestfs", p.libguestfs_inspection);
    kf.set_boolean("general", "xml-editing", p.xml_editing);
    kf.set_boolean("console", "autoconnect", p.console_autoconnect);
    let scaling = SCALING.iter().find(|(v, _)| *v == p.console_scaling).map_or("always", |(_, n)| n);
    kf.set_string("console", "scaling", scaling);
    kf.set_boolean("console", "resize-guest", p.console_resize_guest);
    kf.set_string("console", "grab-keys", &p.console_grab_keys);
    kf.set_string("new-vm", "graphics", &p.new_vm_graphics);
    kf.set_string("new-vm", "disk-format", &p.new_vm_disk_format);
    kf.set_string("new-vm", "cpu-model", &p.new_vm_cpu_model);
    kf.set_string("new-vm", "firmware", &p.new_vm_firmware);
    kf.set_boolean("confirm", "force-off", p.confirm_force_off);
    kf.set_boolean("confirm", "shutdown", p.confirm_shutdown);
    kf.set_boolean("confirm", "pause", p.confirm_pause);
    kf.set_boolean("confirm", "delete", p.confirm_delete);
    kf.set_boolean("confirm", "remove-device", p.confirm_remove_device);
    kf.set_boolean("confirm", "unapplied-changes", p.confirm_unapplied_changes);
    kf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_key_file() {
        let p = Prefs {
            autoconnect: vec!["qemu:///system".into(), "qemu:///session".into()],
            stats_interval_secs: 5,
            console_scaling: ConsoleScaling::FullscreenOnly,
            xml_editing: true,
            tray_icon_color: true,
            ..Default::default()
        };
        let kf = write(&p);
        let again = glib::KeyFile::new();
        again.load_from_data(&kf.to_data(), glib::KeyFileFlags::NONE).unwrap();
        assert_eq!(read(&again), p);
    }

    #[test]
    fn empty_file_gives_defaults() {
        assert_eq!(read(&glib::KeyFile::new()), Prefs::default());
    }
}
