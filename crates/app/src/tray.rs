//! A system tray icon via the StatusNotifierItem D-Bus protocol.
//!
//! GNOME shows these through the AppIndicator extension; KDE and others
//! natively. Speaking the protocol directly over GIO's D-Bus avoids the
//! AppIndicator libraries (which are GTK3-only and not installed here).
//! The icon's menu is exported with the `com.canonical.dbusmenu` protocol.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib::{Variant, VariantDict, VariantTy};
use gtk::prelude::*;
use gtk::{gio, glib};
use vmng_virt::{DomainState, PowerAction};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";

const ITEM_XML: &str = r#"<node><interface name="org.kde.StatusNotifierItem">
<property name="Category" type="s" access="read"/>
<property name="Id" type="s" access="read"/>
<property name="Title" type="s" access="read"/>
<property name="Status" type="s" access="read"/>
<property name="WindowId" type="i" access="read"/>
<property name="IconName" type="s" access="read"/>
<property name="IconThemePath" type="s" access="read"/>
<property name="OverlayIconName" type="s" access="read"/>
<property name="AttentionIconName" type="s" access="read"/>
<property name="IconPixmap" type="a(iiay)" access="read"/>
<property name="ToolTip" type="(sa(iiay)ss)" access="read"/>
<property name="ItemIsMenu" type="b" access="read"/>
<property name="Menu" type="o" access="read"/>
<method name="ContextMenu"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
<method name="Activate"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
<method name="SecondaryActivate"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
<method name="Scroll"><arg name="delta" type="i" direction="in"/><arg name="orientation" type="s" direction="in"/></method>
<signal name="NewTitle"/><signal name="NewIcon"/><signal name="NewToolTip"/><signal name="NewStatus"><arg type="s"/></signal>
</interface></node>"#;

const MENU_XML: &str = r#"<node><interface name="com.canonical.dbusmenu">
<property name="Version" type="u" access="read"/>
<property name="TextDirection" type="s" access="read"/>
<property name="Status" type="s" access="read"/>
<property name="IconThemePath" type="as" access="read"/>
<method name="GetLayout"><arg type="i" direction="in"/><arg type="i" direction="in"/><arg type="as" direction="in"/><arg type="u" direction="out"/><arg type="(ia{sv}av)" direction="out"/></method>
<method name="GetGroupProperties"><arg type="ai" direction="in"/><arg type="as" direction="in"/><arg type="a(ia{sv})" direction="out"/></method>
<method name="GetProperty"><arg type="i" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="out"/></method>
<method name="Event"><arg type="i" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="in"/><arg type="u" direction="in"/></method>
<method name="EventGroup"><arg type="a(isvu)" direction="in"/><arg type="ai" direction="out"/></method>
<method name="AboutToShow"><arg type="i" direction="in"/><arg type="b" direction="out"/></method>
<method name="AboutToShowGroup"><arg type="ai" direction="in"/><arg type="ai" direction="out"/><arg type="ai" direction="out"/></method>
<signal name="ItemsPropertiesUpdated"><arg type="a(ia{sv})"/><arg type="a(ias)"/></signal>
<signal name="LayoutUpdated"><arg type="u"/><arg type="i"/></signal>
<signal name="ItemActivationRequested"><arg type="i"/><arg type="u"/></signal>
</interface></node>"#;

/// What a tray menu entry does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayCommand {
    ShowWindow,
    Quit,
    Power(String, PowerAction),
    Console(String),
}

/// A machine as listed in the tray menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayVm {
    pub uuid: String,
    pub name: String,
    pub state: DomainState,
}

#[derive(Clone, Debug)]
struct Item {
    id: i32,
    label: String,
    enabled: bool,
    separator: bool,
    command: Option<TrayCommand>,
    children: Vec<Item>,
}

impl Item {
    fn props(&self) -> Variant {
        let d = VariantDict::new(None);
        if self.separator {
            d.insert_value("type", &"separator".to_variant());
        } else {
            d.insert_value("label", &self.label.to_variant());
            d.insert_value("enabled", &self.enabled.to_variant());
            if !self.children.is_empty() {
                d.insert_value("children-display", &"submenu".to_variant());
            }
        }
        d.end()
    }

    /// `(ia{sv}av)` for GetLayout.
    fn layout(&self, depth: i32) -> Variant {
        let children = if depth == 0 {
            Vec::new()
        } else {
            self.children.iter().map(|c| c.layout(depth - 1).to_variant()).collect()
        };
        Variant::tuple_from_iter([
            self.id.to_variant(),
            self.props(),
            Variant::array_from_iter_with_type(VariantTy::VARIANT, children),
        ])
    }

    fn find(&self, id: i32) -> Option<&Item> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(id))
    }
}

/// Builds the menu: Show, the machines (each with a submenu), Quit.
fn build_menu(vms: &[TrayVm]) -> Item {
    let mut next = 1;
    let mut id = || {
        next += 1;
        next
    };
    let mut entry = |label: &str, enabled: bool, command: Option<TrayCommand>| Item {
        id: id(),
        label: label.to_owned(),
        enabled,
        separator: false,
        command,
        children: Vec::new(),
    };
    let separator = |id: i32| Item {
        id,
        label: String::new(),
        enabled: true,
        separator: true,
        command: None,
        children: Vec::new(),
    };

    let mut children = vec![entry("Show Virtual Machine Manager", true, Some(TrayCommand::ShowWindow))];
    if !vms.is_empty() {
        children.push(separator(1000));
    }
    for vm in vms {
        let off = matches!(vm.state, DomainState::ShutOff | DomainState::Crashed);
        let running = vm.state == DomainState::Running;
        let paused = matches!(vm.state, DomainState::Paused | DomainState::Suspended);
        let mut sub = Vec::new();
        if off {
            sub.push(entry("Start", true, Some(TrayCommand::Power(vm.uuid.clone(), PowerAction::Start))));
        }
        if running {
            sub.push(entry("Pause", true, Some(TrayCommand::Power(vm.uuid.clone(), PowerAction::Pause))));
        }
        if paused {
            sub.push(entry("Resume", true, Some(TrayCommand::Power(vm.uuid.clone(), PowerAction::Resume))));
        }
        if vm.state.is_active() {
            sub.push(entry("Shut Down", true, Some(TrayCommand::Power(vm.uuid.clone(), PowerAction::Shutdown))));
            sub.push(entry("Open Console", true, Some(TrayCommand::Console(vm.uuid.clone()))));
        }
        let mut parent = entry(&format!("{} — {}", vm.name, vm.state.label()), true, None);
        parent.children = sub;
        children.push(parent);
    }
    children.push(separator(1001));
    children.push(entry("Quit", true, Some(TrayCommand::Quit)));
    Item { id: 0, label: String::new(), enabled: true, separator: false, command: None, children }
}

struct Inner {
    conn: gio::DBusConnection,
    registrations: RefCell<Vec<gio::RegistrationId>>,
    /// Stops watching for the tray host (the id type isn't nameable in
    /// gio's API, so the unwatch call is kept as a closure).
    unwatch: RefCell<Option<Box<dyn FnOnce()>>>,
    menu: RefCell<Item>,
    revision: Cell<u32>,
    tooltip: RefCell<String>,
    icon: String,
    on_command: Box<dyn Fn(TrayCommand)>,
}

/// The tray icon; removed again when dropped.
pub struct Tray(Rc<Inner>);

impl std::fmt::Debug for Tray {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tray").finish_non_exhaustive()
    }
}

/// Whether a tray host (e.g. GNOME's AppIndicator extension) is running.
pub fn available(conn: &gio::DBusConnection) -> bool {
    conn.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameHasOwner",
        Some(&(WATCHER,).to_variant()),
        Some(VariantTy::new("(b)").expect("valid type")),
        gio::DBusCallFlags::NONE,
        1000,
        None::<&gio::Cancellable>,
    )
    .ok()
    .and_then(|v| v.child_value(0).get::<bool>())
    .unwrap_or(false)
}

impl Tray {
    /// Puts the icon in the tray. `icon` is an icon theme name.
    pub fn new(
        conn: &gio::DBusConnection,
        icon: &str,
        on_command: impl Fn(TrayCommand) + 'static,
    ) -> Result<Self, glib::Error> {
        let inner = Rc::new(Inner {
            conn: conn.clone(),
            registrations: RefCell::default(),
            unwatch: RefCell::default(),
            menu: RefCell::new(build_menu(&[])),
            revision: Cell::new(1),
            tooltip: RefCell::new(String::new()),
            icon: icon.to_owned(),
            on_command: Box::new(on_command),
        });

        let item_info = gio::DBusNodeInfo::for_xml(ITEM_XML)?
            .lookup_interface("org.kde.StatusNotifierItem")
            .expect("interface in XML");
        let menu_info =
            gio::DBusNodeInfo::for_xml(MENU_XML)?.lookup_interface("com.canonical.dbusmenu").expect("interface in XML");

        let weak = Rc::downgrade(&inner);
        let w2 = weak.clone();
        let item_id = conn
            .register_object(ITEM_PATH, &item_info)
            .method_call(move |_, _, _, _, method, _, invocation| {
                if let Some(inner) = weak.upgrade()
                    && matches!(method, "Activate" | "SecondaryActivate")
                {
                    (inner.on_command)(TrayCommand::ShowWindow);
                }
                invocation.return_value(None);
            })
            .property(move |_, _, _, _, name| {
                let inner = w2.upgrade();
                let icon = inner.as_ref().map_or_else(String::new, |i| i.icon.clone());
                let tip = inner.as_ref().map_or_else(String::new, |i| i.tooltip.borrow().clone());
                match name {
                    "Category" => "ApplicationStatus".to_variant(),
                    "Id" => crate::config::APP_ID.to_variant(),
                    "Title" => crate::config::APP_NAME.to_variant(),
                    "Status" => "Active".to_variant(),
                    "WindowId" => 0i32.to_variant(),
                    "IconName" => icon.to_variant(),
                    // Development builds aren't installed, so point the host at our icons.
                    #[cfg(debug_assertions)]
                    "IconThemePath" => crate::config::DEV_TRAY_ICON_DIR.to_variant(),
                    "ItemIsMenu" => false.to_variant(),
                    "Menu" => {
                        glib::variant::ObjectPath::try_from(MENU_PATH.to_owned()).expect("valid path").to_variant()
                    }
                    "IconPixmap" => Variant::array_from_iter_with_type(
                        VariantTy::new("(iiay)").expect("valid type"),
                        Vec::<Variant>::new(),
                    ),
                    "ToolTip" => Variant::tuple_from_iter([
                        "".to_variant(),
                        Variant::array_from_iter_with_type(
                            VariantTy::new("(iiay)").expect("valid type"),
                            Vec::<Variant>::new(),
                        ),
                        crate::config::APP_NAME.to_variant(),
                        tip.to_variant(),
                    ]),
                    _ => "".to_variant(),
                }
            })
            .build()?;

        let weak = Rc::downgrade(&inner);
        let menu_id = conn
            .register_object(MENU_PATH, &menu_info)
            .method_call(move |_, _, _, _, method, params, invocation| {
                let Some(inner) = weak.upgrade() else { return invocation.return_value(None) };
                inner.menu_call(method, &params, invocation);
            })
            .property(|_, _, _, _, name| match name {
                "Version" => 3u32.to_variant(),
                "TextDirection" => "ltr".to_variant(),
                "Status" => "normal".to_variant(),
                _ => Vec::<String>::new().to_variant(),
            })
            .build()?;
        inner.registrations.replace(vec![item_id, menu_id]);

        // Register now, and again whenever the tray host (re)starts.
        let weak = Rc::downgrade(&inner);
        let watch = gio::bus_watch_name_on_connection(
            conn,
            WATCHER,
            gio::BusNameWatcherFlags::NONE,
            move |conn, _, _| {
                if weak.upgrade().is_some() {
                    register_with_watcher(&conn);
                }
            },
            |_, _| {},
        );
        inner.unwatch.replace(Some(Box::new(move || gio::bus_unwatch_name(watch))));
        Ok(Self(inner))
    }

    /// Updates the machines in the menu and the tooltip.
    pub fn set_vms(&self, vms: &[TrayVm]) {
        let menu = build_menu(vms);
        let running = vms.iter().filter(|v| v.state.is_active()).count();
        let tip = match running {
            0 => "No machines running".to_owned(),
            1 => "1 machine running".to_owned(),
            n => format!("{n} machines running"),
        };
        let inner = &self.0;
        let changed_tip = *inner.tooltip.borrow() != tip;
        inner.tooltip.replace(tip);
        if format!("{:?}", *inner.menu.borrow()) != format!("{menu:?}") {
            inner.menu.replace(menu);
            inner.revision.set(inner.revision.get() + 1);
            let _ = inner.conn.emit_signal(
                None,
                MENU_PATH,
                "com.canonical.dbusmenu",
                "LayoutUpdated",
                Some(&(inner.revision.get(), 0i32).to_variant()),
            );
        }
        if changed_tip {
            let _ = inner.conn.emit_signal(None, ITEM_PATH, "org.kde.StatusNotifierItem", "NewToolTip", None);
        }
    }
}

fn register_with_watcher(conn: &gio::DBusConnection) {
    let Some(name) = conn.unique_name() else { return };
    conn.call(
        Some(WATCHER),
        "/StatusNotifierWatcher",
        WATCHER,
        "RegisterStatusNotifierItem",
        Some(&(name.as_str(),).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        -1,
        None::<&gio::Cancellable>,
        |r| {
            if let Err(e) = r {
                glib::g_warning!("tray", "couldn't register the tray icon: {e}");
            }
        },
    );
}

impl Inner {
    fn menu_call(&self, method: &str, params: &Variant, invocation: gio::DBusMethodInvocation) {
        let menu = self.menu.borrow().clone();
        match method {
            "GetLayout" => {
                let parent = params.child_value(0).get::<i32>().unwrap_or(0);
                let depth = params.child_value(1).get::<i32>().unwrap_or(-1);
                let node = menu.find(parent).unwrap_or(&menu).layout(depth);
                invocation.return_value(Some(&Variant::tuple_from_iter([self.revision.get().to_variant(), node])));
            }
            "GetGroupProperties" => {
                let ids: Vec<i32> = params.child_value(0).get().unwrap_or_default();
                let items = ids
                    .iter()
                    .filter_map(|id| menu.find(*id))
                    .map(|i| Variant::tuple_from_iter([i.id.to_variant(), i.props()]));
                let array = Variant::array_from_iter_with_type(VariantTy::new("(ia{sv})").expect("valid type"), items);
                invocation.return_value(Some(&Variant::tuple_from_iter([array])));
            }
            "GetProperty" => {
                let id = params.child_value(0).get::<i32>().unwrap_or(0);
                let name = params.child_value(1).get::<String>().unwrap_or_default();
                let value = menu
                    .find(id)
                    .and_then(|i| VariantDict::new(Some(&i.props())).lookup_value(&name, None))
                    .unwrap_or_else(|| "".to_variant());
                invocation.return_value(Some(&Variant::tuple_from_iter([value.to_variant()])));
            }
            "Event" => {
                let id = params.child_value(0).get::<i32>().unwrap_or(-1);
                let event = params.child_value(1).get::<String>().unwrap_or_default();
                invocation.return_value(None);
                if event == "clicked"
                    && let Some(cmd) = menu.find(id).and_then(|i| i.command.clone())
                {
                    (self.on_command)(cmd);
                }
            }
            "EventGroup" => {
                let events = params.child_value(0);
                for e in events.iter() {
                    let id = e.child_value(0).get::<i32>().unwrap_or(-1);
                    let event = e.child_value(1).get::<String>().unwrap_or_default();
                    if event == "clicked"
                        && let Some(cmd) = menu.find(id).and_then(|i| i.command.clone())
                    {
                        (self.on_command)(cmd);
                    }
                }
                invocation.return_value(Some(&(Vec::<i32>::new(),).to_variant()));
            }
            "AboutToShow" => invocation.return_value(Some(&(false,).to_variant())),
            "AboutToShowGroup" => invocation.return_value(Some(&(Vec::<i32>::new(), Vec::<i32>::new()).to_variant())),
            _ => invocation.return_value(None),
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let inner = &self.0;
        if let Some(unwatch) = inner.unwatch.take() {
            unwatch();
        }
        for id in inner.registrations.take() {
            let _ = inner.conn.unregister_object(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_layout_has_the_right_type() {
        let vms = [
            TrayVm { uuid: "a".into(), name: "fedora".into(), state: DomainState::Running },
            TrayVm { uuid: "b".into(), name: "debian".into(), state: DomainState::ShutOff },
        ];
        let menu = build_menu(&vms);
        let layout = menu.layout(-1);
        assert_eq!(layout.type_().as_str(), "(ia{sv}av)");
        // Show, separator, two machines, separator, Quit.
        assert_eq!(menu.children.len(), 6);
        let fedora = &menu.children[2];
        assert!(fedora.children.iter().any(|c| c.command == Some(TrayCommand::Power("a".into(), PowerAction::Pause))));
        let ids: Vec<i32> = menu.children.iter().map(|c| c.id).collect();
        let mut dedup = ids.clone();
        dedup.dedup();
        assert_eq!(ids, dedup);
    }
}
