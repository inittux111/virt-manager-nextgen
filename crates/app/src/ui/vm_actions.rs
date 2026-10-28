//! The `vm.*` actions for the selected machine.
//!
//! One action group, installed on the window, serves every place that acts
//! on "the current VM" (details panel, VM page, menus), so buttons are
//! enabled and disabled consistently everywhere.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;
use vmng_virt::{DomainState, DomainSummary, PowerAction};

/// Everything a person can do to one VM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VmAction {
    Power(PowerAction),
    Console,
    Clone,
    Rename,
    Delete,
    DiscardSavedState,
    Hardware,
    Snapshots,
    TakeSnapshot,
    SyncTime,
    Performance,
}

const ACTIONS: [(&str, VmAction); 18] = [
    ("start", VmAction::Power(PowerAction::Start)),
    ("shutdown", VmAction::Power(PowerAction::Shutdown)),
    ("reboot", VmAction::Power(PowerAction::Reboot)),
    ("reset", VmAction::Power(PowerAction::Reset)),
    ("force-off", VmAction::Power(PowerAction::ForceOff)),
    ("pause", VmAction::Power(PowerAction::Pause)),
    ("resume", VmAction::Power(PowerAction::Resume)),
    ("save", VmAction::Power(PowerAction::Save)),
    ("console", VmAction::Console),
    ("clone", VmAction::Clone),
    ("rename", VmAction::Rename),
    ("delete", VmAction::Delete),
    ("discard-saved", VmAction::DiscardSavedState),
    ("hardware", VmAction::Hardware),
    ("snapshots", VmAction::Snapshots),
    ("take-snapshot", VmAction::TakeSnapshot),
    ("sync-time", VmAction::SyncTime),
    ("performance", VmAction::Performance),
];

type Handler = Rc<dyn Fn(&str, VmAction)>;

#[derive(Clone)]
pub struct VmActions {
    pub group: gio::SimpleActionGroup,
    uuid: Rc<RefCell<Option<String>>>,
    handler: Rc<RefCell<Option<Handler>>>,
}

impl std::fmt::Debug for VmActions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VmActions").field("uuid", &self.uuid).finish_non_exhaustive()
    }
}

impl VmActions {
    pub fn new() -> Self {
        let this = Self { group: gio::SimpleActionGroup::new(), uuid: Rc::default(), handler: Rc::default() };
        for (name, action) in ACTIONS {
            let a = gio::SimpleAction::new(name, None);
            let (uuid, handler) = (this.uuid.clone(), this.handler.clone());
            a.connect_activate(move |_, _| {
                let id = uuid.borrow().clone();
                let h = handler.borrow().clone();
                if let (Some(id), Some(h)) = (id, h) {
                    h(&id, action);
                }
            });
            a.set_enabled(false);
            this.group.add_action(&a);
        }
        this
    }

    pub fn connect(&self, f: impl Fn(&str, VmAction) + 'static) {
        self.handler.replace(Some(Rc::new(f)));
    }

    fn enable(&self, name: &str, on: bool) {
        if let Some(a) = self.group.lookup_action(name).and_downcast::<gio::SimpleAction>() {
            a.set_enabled(on);
        }
    }

    /// Points the actions at `vm` (or nothing) and updates what is allowed.
    pub fn set_target(&self, vm: Option<&DomainSummary>) {
        self.uuid.replace(vm.map(|d| d.uuid.clone()));
        let Some(d) = vm else {
            for (name, _) in ACTIONS {
                self.enable(name, false);
            }
            return;
        };
        let s = d.state;
        let off = matches!(s, DomainState::ShutOff | DomainState::Crashed);
        self.enable("start", off);
        self.enable("pause", s == DomainState::Running);
        self.enable("resume", matches!(s, DomainState::Paused | DomainState::Suspended));
        for name in ["shutdown", "reboot", "reset", "force-off"] {
            self.enable(name, s.is_active());
        }
        self.enable("save", d.persistent && matches!(s, DomainState::Running | DomainState::Paused));
        // The console page shows the screen, or the text console if the
        // machine has no graphical display.
        self.enable("console", s.is_active());
        self.enable("clone", off && d.persistent);
        self.enable("rename", off && d.persistent);
        self.enable("delete", true);
        self.enable("discard-saved", off && d.has_managed_save);
        self.enable("hardware", true);
        self.enable("performance", true);
        self.enable("snapshots", d.persistent);
        self.enable("take-snapshot", d.persistent && s != DomainState::ShuttingDown);
        self.enable("sync-time", s == DomainState::Running && d.config.has_guest_agent_channel);
    }
}

fn item(label: &str, action: &str) -> gio::MenuItem {
    gio::MenuItem::new(Some(label), Some(action))
}

/// The power options menu.
pub fn power_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let soft = gio::Menu::new();
    soft.append_item(&item("Shut Down", "vm.shutdown"));
    soft.append_item(&item("Reboot", "vm.reboot"));
    soft.append_item(&item("Save and Stop", "vm.save"));
    menu.append_section(None, &soft);
    let hard = gio::Menu::new();
    hard.append_item(&item("Force Reset", "vm.reset"));
    hard.append_item(&item("Force Off", "vm.force-off"));
    menu.append_section(None, &hard);
    menu
}

/// A menu item that disappears when its action isn't possible right now
/// (e.g. "Pause" on a machine that is shut off).
fn state_item(label: &str, action: &str) -> gio::MenuItem {
    let i = item(label, action);
    i.set_attribute_value("hidden-when", Some(&"action-disabled".to_variant()));
    i
}

/// The right-click menu of a VM in the list.
pub fn context_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let power = gio::Menu::new();
    for (label, action) in [
        ("Start", "vm.start"),
        ("Resume", "vm.resume"),
        ("Pause", "vm.pause"),
        ("Shut Down", "vm.shutdown"),
        ("Reboot", "vm.reboot"),
        ("Force Off", "vm.force-off"),
    ] {
        power.append_item(&state_item(label, action));
    }
    menu.append_section(None, &power);
    let open = gio::Menu::new();
    open.append_item(&item("Open Console", "vm.console"));
    open.append_item(&item("Settings…", "vm.hardware"));
    open.append_item(&item("Snapshots", "vm.snapshots"));
    open.append_item(&item("Performance", "vm.performance"));
    open.append_item(&item("Take Snapshot…", "vm.take-snapshot"));
    menu.append_section(None, &open);
    let manage = gio::Menu::new();
    manage.append_item(&item("Clone…", "vm.clone"));
    manage.append_item(&item("Rename…", "vm.rename"));
    manage.append_item(&state_item("Discard Saved State", "vm.discard-saved"));
    menu.append_section(None, &manage);
    let danger = gio::Menu::new();
    danger.append_item(&item("Delete…", "vm.delete"));
    menu.append_section(None, &danger);
    menu
}

/// The "more actions" menu.
pub fn more_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let manage = gio::Menu::new();
    manage.append_item(&item("Take Snapshot…", "vm.take-snapshot"));
    manage.append_item(&item("Clone…", "vm.clone"));
    manage.append_item(&item("Rename…", "vm.rename"));
    manage.append_item(&item("Discard Saved State", "vm.discard-saved"));
    manage.append_item(&state_item("Sync Guest Clock", "vm.sync-time"));
    menu.append_section(None, &manage);
    let danger = gio::Menu::new();
    danger.append_item(&item("Delete…", "vm.delete"));
    menu.append_section(None, &danger);
    menu
}

/// The action buttons shown next to a VM's name: the primary action
/// (Start or Open Console), pause/resume, power options and more.
#[derive(Clone, Debug)]
pub struct VmButtons {
    pub root: gtk::Box,
    start: gtk::Button,
    console: gtk::Button,
    pause: gtk::Button,
    resume: gtk::Button,
    /// Whether "Open Console" is offered (not on the console page itself).
    show_console: std::cell::Cell<bool>,
}

impl VmButtons {
    pub fn new() -> Self {
        use super::widgets::{icon_button, labeled_button};
        let start = labeled_button("media-playback-start-symbolic", "Start");
        start.set_action_name(Some("vm.start"));
        start.add_css_class("suggested-action");
        let console = labeled_button("video-display-symbolic", "Open Console");
        console.set_action_name(Some("vm.console"));
        console.add_css_class("suggested-action");
        let pause = icon_button("media-playback-pause-symbolic", "Pause");
        pause.set_action_name(Some("vm.pause"));
        let resume = icon_button("media-playback-start-symbolic", "Resume");
        resume.set_action_name(Some("vm.resume"));
        let power = menu_button("system-shutdown-symbolic", "Power options", &power_menu());
        let more = menu_button("view-more-symbolic", "More actions", &more_menu());
        let root = gtk::Box::builder().spacing(8).valign(gtk::Align::Center).build();
        for w in [start.upcast_ref::<gtk::Widget>(), console.upcast_ref(), pause.upcast_ref(), resume.upcast_ref()] {
            root.append(w);
        }
        root.append(&power);
        root.append(&more);
        Self { root, start, console, pause, resume, show_console: std::cell::Cell::new(true) }
    }

    /// Hides "Open Console" (for the console page itself).
    pub fn without_console(self) -> Self {
        self.show_console.set(false);
        self.console.set_visible(false);
        self
    }

    /// Shows the buttons that fit the VM's state.
    pub fn update(&self, state: DomainState) {
        let off = matches!(state, DomainState::ShutOff | DomainState::Crashed);
        self.start.set_visible(off);
        self.console.set_visible(!off && self.show_console.get());
        self.pause.set_visible(state == DomainState::Running);
        self.resume.set_visible(matches!(state, DomainState::Paused | DomainState::Suspended));
    }
}

fn menu_button(icon: &str, tooltip: &str, menu: &gio::Menu) -> gtk::MenuButton {
    let b = gtk::MenuButton::builder()
        .icon_name(icon)
        .menu_model(menu)
        .tooltip_text(tooltip)
        .valign(gtk::Align::Center)
        .build();
    b.update_property(&[gtk::accessible::Property::Label(tooltip)]);
    b
}
