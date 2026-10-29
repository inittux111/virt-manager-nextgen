//! The main window.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use vmng_virt::{CloneDiskMode, ConnectionKind, DomainSummary, GuestDetails, HostInfo, PowerAction};

use super::console::ConsoleWindow;
use super::feedback::Feedback;
use super::snapshots::{SnapshotsView, take_snapshot_dialog};
use super::views::{ConnStatus, View, connection_name};
use super::vm_actions::{VmAction, VmActions};
use super::vm_details::VmDetails;
use super::vm_dialogs::{clone_dialog, delete_dialog, rename_dialog};
use super::vm_list::VmList;
use super::vm_page::{VmPage, VmTab};
use super::widgets::{FormDialog, ListDetail};
use crate::backend::{Conn, Monitor, Sample};
use crate::format;
use crate::settings::Settings;

/// Guest agent details are refreshed every this many polls.
const GUEST_POLL_EVERY: u64 = 5;
/// The screen preview is refreshed every this many polls (a few seconds:
/// each picture is a full screen image, which adds up on remote hosts).
const PREVIEW_POLL_EVERY: u64 = 2;

/// Turns a captured screen into something GTK can draw.
pub(crate) fn screenshot_texture(shot: vmng_virt::Screenshot) -> Option<gdk::Texture> {
    match shot {
        vmng_virt::Screenshot::Rgb { width, height, pixels } => {
            // Four bytes per pixel: renderers handle 3-byte RGB poorly.
            let mut rgba = Vec::with_capacity(pixels.len() / 3 * 4);
            for [r, g, b] in pixels.as_chunks::<3>().0 {
                rgba.extend_from_slice(&[*r, *g, *b, 255]);
            }
            Some(
                gdk::MemoryTexture::new(
                    i32::try_from(width).ok()?,
                    i32::try_from(height).ok()?,
                    gdk::MemoryFormat::R8g8b8a8,
                    &glib::Bytes::from_owned(rgba),
                    width as usize * 4,
                )
                .upcast(),
            )
        }
        vmng_virt::Screenshot::Png(data) => gdk::Texture::from_bytes(&glib::Bytes::from_owned(data)).ok(),
    }
}

#[derive(Debug)]
enum Slot {
    Connecting,
    Connected(Conn),
    Failed(String),
}

#[derive(Debug)]
struct State {
    view: View,
    current: ConnectionKind,
    conns: HashMap<ConnectionKind, Slot>,
    monitor: Monitor,
    domains: Vec<DomainSummary>,
    host_info: Option<HostInfo>,
    guest: HashMap<String, GuestDetails>,
    polling: bool,
    tick: u64,
    timer: Option<glib::SourceId>,
    /// A VM to select once it appears in the list (e.g. a new clone).
    pending_select: Option<String>,
    /// The view to show once the first data has arrived (the last one used).
    pending_view: Option<View>,
    /// A VM page to open once the VM appears (e.g. "customize before install").
    pending_page: Option<(String, VmTab)>,
    /// Whether hardware virtualization is available on the host.
    kvm: Option<bool>,
    /// A screen preview is being captured.
    preview_busy: bool,
    /// Machines running a network installer: it stops the machine at its
    /// end, and they are started again to boot the installed system.
    installing: std::collections::HashSet<String>,
}

pub struct MainWindow {
    pub window: adw::ApplicationWindow,
    settings: Settings,
    views: adw::ViewStack,
    machines: gtk::Stack,
    error_page: adw::StatusPage,
    loading_label: gtk::Label,
    content: gtk::Stack,
    split: ListDetail,
    vm_list: Rc<VmList>,
    details: Rc<VmDetails>,
    banner: adw::Banner,
    feedback: Feedback,
    nav: adw::NavigationView,
    vm_page: VmPage,
    /// Open console windows, by VM UUID.
    consoles: RefCell<HashMap<String, Rc<ConsoleWindow>>>,
    host_view: Rc<super::host_view::HostView>,
    networks_view: Rc<super::networks_view::NetworksView>,
    storage_view: Rc<super::storage_view::StorageView>,
    new_button: gtk::Button,
    new_wrap: gtk::Box,
    tray: RefCell<Option<crate::tray::Tray>>,
    actions: VmActions,
    job: JobBar,
    search_bar: gtk::SearchBar,
    search_button: gtk::ToggleButton,
    /// The Connection section of the main menu.
    conn_menu: gio::Menu,
    state: RefCell<State>,
}

/// Added to a failed SSH connection's message.
const REMOTE_HINT: &str = "\n\nThe app logs in with your SSH key. Set it up with “ssh-copy-id user@computer”, and \
connect once with “ssh user@computer” in a terminal to accept the computer's key. The other computer needs \
libvirt running.";

/// The `win.connection` target of a connection.
fn connection_id(kind: &ConnectionKind) -> String {
    match kind {
        ConnectionKind::System => "system".into(),
        ConnectionKind::Session => "session".into(),
        ConnectionKind::Other(uri) => uri.clone(),
    }
}

fn connection_of(id: &str) -> ConnectionKind {
    match id {
        "system" => ConnectionKind::System,
        "session" => ConnectionKind::Session,
        uri => ConnectionKind::Other(uri.to_owned()),
    }
}

impl std::fmt::Debug for MainWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MainWindow").finish_non_exhaustive()
    }
}

/// Whether the window's size, view and VM are restored and saved. Not in
/// debug screenshot runs, which mustn't disturb the person's own settings.
fn remember_window() -> bool {
    // `VMNG_DEV_REMEMBER=1` turns it on anyway (with a test XDG_CONFIG_HOME).
    #[cfg(debug_assertions)]
    if std::env::var_os("VMNG_SCREENSHOT").is_some() {
        return std::env::var_os("VMNG_DEV_REMEMBER").is_some();
    }
    true
}

impl MainWindow {
    /// Saves the window's size, maximized state, view and selected VM, so
    /// the next start opens the same way.
    pub fn save_window_state(&self) {
        if !remember_window() {
            return;
        }
        let maximized = self.window.is_maximized();
        // The size before maximizing, so un-maximizing later is sensible.
        let (width, height) = self.window.default_size();
        let view = self.state.borrow().view.id().to_owned();
        let vm = self.vm_list.selected().unwrap_or_default();
        self.settings.update(|p| {
            if width > 0 && height > 0 {
                p.window_width = width;
                p.window_height = height;
            }
            p.window_maximized = maximized;
            p.last_view = view;
            p.last_vm = vm;
        });
    }

    pub fn new(app: &adw::Application, settings: Settings) -> Rc<Self> {
        let prefs = settings.get();
        // Opens as it was last closed.
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(crate::config::APP_NAME)
            .default_width(prefs.window_width)
            .default_height(prefs.window_height)
            .maximized(prefs.window_maximized && remember_window())
            .width_request(360)
            .height_request(400)
            .build();
        // Debug builds: `VMNG_DEV_SIZE=<w>x<h>` to review small windows.
        #[cfg(debug_assertions)]
        if let Some((w, h)) = std::env::var("VMNG_DEV_SIZE").ok().and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        }) {
            window.set_default_size(w, h);
        }

        // Header.
        // Views are header tabs; the connection is chosen in the main menu.
        let views = adw::ViewStack::builder().hhomogeneous(false).vexpand(true).enable_transitions(true).build();
        let switcher = adw::ViewSwitcher::builder().stack(&views).policy(adw::ViewSwitcherPolicy::Wide).build();
        let header = adw::HeaderBar::builder().title_widget(&switcher).build();
        let switcher_bar = adw::ViewSwitcherBar::builder().stack(&views).build();

        let main_menu = gio::Menu::new();
        let section = gio::Menu::new();
        let conns = gio::Menu::new();
        main_menu.append_section(Some("Connection"), &conns);
        section.append(Some("Preferences"), Some("app.preferences"));
        section.append(Some("Keyboard Shortcuts"), Some("app.shortcuts"));
        section.append(Some("About Virtual Machines"), Some("app.about"));
        main_menu.append_section(None, &section);
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&main_menu)
            .primary(true)
            .tooltip_text("Main menu")
            .build();
        header.pack_end(&menu_button);
        // Debug builds: VMNG_DEV_MAIN_MENU=1 opens the main menu (screenshots).
        #[cfg(debug_assertions)]
        if std::env::var_os("VMNG_DEV_MAIN_MENU").is_some() {
            let b = menu_button.clone();
            glib::timeout_add_local_once(std::time::Duration::from_secs(3), move || b.popup());
        }

        // The view's main action (New VM, New Network): a plain "+" at the
        // start, as in GNOME's own apps.
        let new_button = gtk::Button::builder().icon_name("list-add-symbolic").valign(gtk::Align::Center).build();
        new_button.set_action_name(Some("win.new-vm"));
        new_button.set_tooltip_text(Some("Create a virtual machine"));
        let new_wrap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        new_wrap.append(&new_button);
        header.pack_start(&new_wrap);
        // Narrow windows: the view tabs move to a bar at the bottom.
        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            760.0,
            adw::LengthUnit::Px,
        ));
        narrow.add_setter(&switcher, "visible", Some(&false.to_value()));
        narrow.add_setter(&switcher_bar, "reveal", Some(&true.to_value()));
        // Very narrow: also drop the search button. (Only one breakpoint
        // applies at a time, so it repeats the narrow setters.)
        let compact = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            560.0,
            adw::LengthUnit::Px,
        ));
        compact.add_setter(&switcher, "visible", Some(&false.to_value()));
        compact.add_setter(&switcher_bar, "reveal", Some(&true.to_value()));

        let search_button =
            gtk::ToggleButton::builder().icon_name("system-search-symbolic").tooltip_text("Search machines").build();
        // Narrow windows drop the button (typing or Ctrl+F still searches).
        let search_wrap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        search_wrap.append(&search_button);
        header.pack_end(&search_wrap);
        compact.add_setter(&search_wrap, "visible", Some(&false.to_value()));
        window.add_breakpoint(narrow);
        window.add_breakpoint(compact);

        let search_entry = gtk::SearchEntry::builder().placeholder_text("Search machines").build();
        let search_bar = gtk::SearchBar::builder()
            .child(&adw::Clamp::builder().maximum_size(480).child(&search_entry).build())
            .build();
        search_bar.connect_entry(&search_entry);
        search_bar.set_key_capture_widget(Some(&window));
        search_button.bind_property("active", &search_bar, "search-mode-enabled").bidirectional().build();

        // Machines view.
        let vm_list = VmList::new();
        let details = VmDetails::new();
        let content = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
        content.add_named(&details.root, Some("details"));
        content.add_named(
            &adw::StatusPage::builder()
                .icon_name("computer-symbolic")
                .title("No machine selected")
                .description("Pick a virtual machine from the list to see its details.")
                .build(),
            Some("none"),
        );
        let split = ListDetail::new(&vm_list.root, &content, 0, true, "Machines", 720.0, |b| b.clone().upcast());
        let list = vm_list.clone();
        split.connect_layout(move |narrow| list.set_full_width(narrow));

        let loading_label = gtk::Label::builder().css_classes(["dim-label"]).build();
        let loading = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Center)
            .build();
        loading.append(&adw::Spinner::builder().width_request(32).height_request(32).build());
        loading.append(&loading_label);

        let retry = gtk::Button::builder()
            .label("Try Again")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        let error_page = adw::StatusPage::builder().icon_name("network-offline-symbolic").child(&retry).build();

        let empty_actions =
            adw::WrapBox::builder().child_spacing(12).line_spacing(12).halign(gtk::Align::Center).build();
        let empty_new = gtk::Button::builder()
            .label("New VM")
            .action_name("win.new-vm")
            .css_classes(["pill", "suggested-action"])
            .build();
        let empty_import =
            gtk::Button::builder().label("Import Disk").action_name("win.import-disk").css_classes(["pill"]).build();
        empty_actions.append(&empty_new);
        empty_actions.append(&empty_import);
        let empty = adw::StatusPage::builder()
            .icon_name("computer-symbolic")
            .title("No virtual machines")
            .description("Create one from an installer image, or import a disk you already have.")
            .child(&empty_actions)
            .build();

        let machines =
            gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).hhomogeneous(false).build();
        machines.add_named(&loading, Some("loading"));
        machines.add_named(&error_page, Some("error"));
        machines.add_named(&empty, Some("empty"));
        machines.add_named(split.root(), Some("content"));

        let tab = |view: View, child: &gtk::Widget| {
            views.add_titled_with_icon(child, Some(view.id()), view.tab_title(), view.icon());
        };
        tab(View::Machines, machines.upcast_ref());
        let host_view = super::host_view::HostView::new();
        tab(View::Host, host_view.root.upcast_ref());

        let banner = adw::Banner::builder().button_label("Retry Now").build();
        let job = JobBar::new();
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.append(&banner);
        body.append(&search_bar);
        body.append(&views);
        body.append(&job.revealer);

        let toolbar = adw::ToolbarView::builder().content(&body).build();
        toolbar.add_top_bar(&header);
        toolbar.add_bottom_bar(&switcher_bar);
        let main_page =
            adw::NavigationPage::builder().tag("main").title(crate::config::APP_NAME).child(&toolbar).build();

        let toasts = adw::ToastOverlay::new();
        let feedback = Feedback::new(&toasts, &window);
        let networks_view = super::networks_view::NetworksView::new(feedback.clone(), settings.clone());
        tab(View::Networks, networks_view.root.upcast_ref());
        let storage_view = super::storage_view::StorageView::new(feedback.clone());
        tab(View::Storage, storage_view.root.upcast_ref());
        let nav = adw::NavigationView::new();
        nav.add(&main_page);
        toasts.set_child(Some(&nav));
        window.set_content(Some(&toasts));

        let actions = VmActions::new();
        window.insert_action_group("vm", Some(&actions.group));

        let current = ConnectionKind::from_uri(&prefs.last_connection);
        // The snapshots view reports changes back through a weak reference
        // that is filled in once the window exists.
        let me: Rc<RefCell<Weak<Self>>> = Rc::default();
        let m = me.clone();
        let snapshots = SnapshotsView::new(feedback.clone(), move || {
            if let Some(this) = m.borrow().upgrade() {
                this.refresh();
            }
        });
        let m = me.clone();
        let hardware = super::hardware::HardwareView::new(feedback.clone(), settings.clone(), move || {
            if let Some(this) = m.borrow().upgrade() {
                this.refresh();
            }
        });
        let vm_page = VmPage::new(snapshots, hardware);
        nav.add(&vm_page.page);
        let this = Rc::new(Self {
            window,
            settings,
            views,
            machines,
            error_page,
            loading_label,
            content,
            split,
            vm_list,
            details,
            banner,
            feedback,
            nav,
            vm_page,
            consoles: RefCell::default(),
            host_view,
            networks_view,
            storage_view,
            new_button,
            new_wrap,
            tray: RefCell::default(),
            actions,
            job,
            search_bar,
            search_button,
            conn_menu: conns,
            state: RefCell::new(State {
                view: View::Machines,
                current,
                conns: HashMap::new(),
                monitor: Monitor::default(),
                domains: Vec::new(),
                host_info: None,
                guest: HashMap::new(),
                polling: false,
                tick: 0,
                timer: None,
                pending_select: Some(prefs.last_vm.clone()).filter(|u| !u.is_empty() && remember_window()),
                pending_view: View::ALL
                    .into_iter()
                    .find(|v| v.id() == prefs.last_view && *v != View::Machines)
                    .filter(|_| remember_window()),
                pending_page: None,
                kvm: None,
                preview_busy: false,
                installing: Default::default(),
            }),
        });

        me.replace(Rc::downgrade(&this));
        this.fill_connection_menu();
        this.setup_actions();
        this.wire(&search_entry, &retry);
        this.update_chrome();

        // Connect at startup: the configured connections plus the one shown.
        let mut uris = prefs.autoconnect.clone();
        if !uris.contains(&prefs.last_connection) {
            uris.push(prefs.last_connection.clone());
        }
        for uri in uris {
            this.connect_to(ConnectionKind::from_uri(&uri));
        }
        this.restart_timer();
        let weak = Rc::downgrade(&this);
        this.settings.connect_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                this.restart_timer();
                this.update_tray();
            }
        });
        this.update_tray();
        // With a tray icon, closing the window keeps the app running there.
        let weak = Rc::downgrade(&this);
        this.window.connect_close_request(move |w| {
            let Some(this) = weak.upgrade() else { return glib::Propagation::Proceed };
            this.save_window_state();
            if this.tray.borrow().is_some() {
                w.set_visible(false);
                return glib::Propagation::Stop;
            }
            // Closing the main window quits, consoles included.
            let consoles: Vec<_> = this.consoles.borrow().values().cloned().collect();
            for c in consoles {
                c.window.close();
            }
            glib::Propagation::Proceed
        });
        this
    }

    fn setup_actions(self: &Rc<Self>) {
        // The header tabs: follow the person's choice.
        let weak = Rc::downgrade(self);
        self.views.connect_visible_child_name_notify(move |stack| {
            let Some(this) = weak.upgrade() else { return };
            let picked = stack.visible_child_name().and_then(|n| View::ALL.into_iter().find(|v| v.id() == n));
            if let Some(view) = picked
                && this.state.borrow().view != view
            {
                this.set_view(view);
            }
        });
        // `win.connection::system|session|<address>` (main menu) switches connection.
        let current = connection_id(&self.state.borrow().current);
        let conn = gio::SimpleAction::new_stateful("connection", Some(glib::VariantTy::STRING), &current.to_variant());
        let weak = Rc::downgrade(self);
        conn.connect_activate(move |a, p| {
            let (Some(this), Some(id)) = (weak.upgrade(), p.and_then(|v| v.get::<String>())) else { return };
            a.set_state(&id.to_variant());
            this.switch_connection(connection_of(&id));
        });
        self.window.add_action(&conn);
        let add = gio::SimpleAction::new("add-connection", None);
        let weak = Rc::downgrade(self);
        add.connect_activate(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.add_connection_dialog();
            }
        });
        self.window.add_action(&add);
        let remove = gio::SimpleAction::new("remove-connection", None);
        let weak = Rc::downgrade(self);
        remove.connect_activate(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.ask_remove_connection();
            }
        });
        remove.set_enabled(self.state.borrow().current.is_remote());
        self.window.add_action(&remove);
        // `win.view::<id>` switches views (Ctrl+1…4).
        let view = gio::SimpleAction::new("view", Some(glib::VariantTy::STRING));
        let weak = Rc::downgrade(self);
        view.connect_activate(move |_, p| {
            let (Some(this), Some(id)) = (weak.upgrade(), p.and_then(|v| v.get::<String>())) else { return };
            if let Some(v) = View::ALL.into_iter().find(|v| v.id() == id) {
                this.set_view(v);
            }
        });
        self.window.add_action(&view);
        let new_network = gio::SimpleAction::new("new-network", None);
        let weak = Rc::downgrade(self);
        new_network.connect_activate(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.networks_view.new_network(&this.window);
            }
        });
        self.window.add_action(&new_network);
        let weak = Rc::downgrade(self);
        self.host_view.connect_open_vm(move |uuid| {
            if let Some(this) = weak.upgrade() {
                this.set_view(View::Machines);
                this.vm_list.select(uuid);
            }
        });
        for (name, import) in [("new-vm", false), ("import-disk", true)] {
            let a = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            a.connect_activate(move |_, _| {
                if let Some(this) = weak.upgrade() {
                    this.open_new_vm(import);
                }
            });
            self.window.add_action(&a);
        }
        let weak = Rc::downgrade(self);
        let search = gio::SimpleAction::new("search", None);
        search.connect_activate(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.set_view(View::Machines);
                this.search_button.set_active(true);
            }
        });
        self.window.add_action(&search);
    }

    fn wire(self: &Rc<Self>, search_entry: &gtk::SearchEntry, retry: &gtk::Button) {
        let weak = Rc::downgrade(self);
        self.vm_list.connect_selected(move |uuid| {
            if let Some(this) = weak.upgrade() {
                this.show_selected(uuid);
                if let Some(uuid) = uuid {
                    this.fetch_guest(uuid.to_owned());
                    this.fetch_preview(uuid.to_owned());
                }
            }
        });
        let weak = Rc::downgrade(self);
        let weak2 = Rc::downgrade(self);
        self.vm_list.connect_double_clicked(move |uuid| {
            if let Some(this) = weak2.upgrade()
                && let Some(vm) = this.vm_by_uuid(uuid)
            {
                this.open_console(&vm);
            }
        });
        self.vm_list.connect_activated(move |_| {
            if let Some(this) = weak.upgrade() {
                this.split.show_detail();
            }
        });
        let weak = Rc::downgrade(self);
        self.vm_list.connect_host_clicked(move || {
            if let Some(this) = weak.upgrade() {
                this.set_view(View::Host);
            }
        });
        let weak: Weak<Self> = Rc::downgrade(self);
        let on_action = move |uuid: &str, action: PowerAction| {
            if let Some(this) = weak.upgrade() {
                this.request_power(uuid.to_owned(), action);
            }
        };
        self.vm_list.connect_action(on_action);
        let weak = Rc::downgrade(self);
        self.actions.connect(move |uuid, action| {
            if let Some(this) = weak.upgrade() {
                this.vm_action(uuid.to_owned(), action);
            }
        });
        let weak = Rc::downgrade(self);
        self.nav.connect_visible_page_notify(move |_| {
            if let Some(this) = weak.upgrade() {
                if !this.vm_page_open() {
                    this.vm_page.hardware.leave();
                }
                let selected = this.vm_list.selected();
                this.show_selected(selected.as_deref());
            }
        });
        let weak = Rc::downgrade(self);
        self.details.connect_autostart(move |uuid, on| {
            if let Some(this) = weak.upgrade() {
                this.set_autostart(uuid.to_owned(), on);
            }
        });

        let weak = Rc::downgrade(self);
        search_entry.connect_search_changed(move |e| {
            if let Some(this) = weak.upgrade() {
                this.vm_list.set_filter(&e.text());
            }
        });
        let weak = Rc::downgrade(self);
        self.search_bar.connect_search_mode_enabled_notify(move |bar| {
            if let Some(this) = weak.upgrade()
                && !bar.is_search_mode()
            {
                this.vm_list.set_filter("");
            }
        });

        let weak = Rc::downgrade(self);
        retry.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                let kind = this.state.borrow().current.clone();
                this.connect_to(kind);
            }
        });
        let weak = Rc::downgrade(self);
        self.banner.connect_button_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                let kind = this.state.borrow().current.clone();
                this.connect_to(kind);
            }
        });
    }

    fn set_view(self: &Rc<Self>, view: View) {
        self.state.borrow_mut().view = view;
        self.views.set_visible_child_name(view.id());
        // The header's main button follows the view.
        let (label, action, tooltip) = match view {
            View::Machines => (Some("New VM"), "win.new-vm", "Create a virtual machine"),
            View::Networks => (Some("New Network"), "win.new-network", "Create a virtual network"),
            _ => (None, "win.new-vm", ""),
        };
        self.new_wrap.set_visible(label.is_some());
        if label.is_some() {
            self.new_button.set_action_name(Some(action));
            self.new_button.set_tooltip_text(Some(tooltip));
        }
        self.load_view(view);
        self.search_button.set_visible(view == View::Machines);
        if view != View::Machines {
            self.search_bar.set_search_mode(false);
        }
        self.update_chrome();
    }

    /// Loads the data a view shows when it becomes visible.
    fn load_view(self: &Rc<Self>, view: View) {
        let Some(conn) = self.current_conn() else { return };
        let domains = self.state.borrow().domains.clone();
        match view {
            View::Networks => self.networks_view.load(conn, &domains),
            View::Storage => self.storage_view.load(conn, &domains),
            View::Host => self.update_host_view(),
            View::Machines => {}
        }
    }

    fn update_host_view(&self) {
        let st = self.state.borrow();
        self.host_view.update(st.host_info.as_ref(), st.kvm, st.current.uri(), &st.monitor, &st.domains);
    }

    fn status_of(&self, kind: &ConnectionKind) -> ConnStatus {
        match self.state.borrow().conns.get(kind) {
            Some(Slot::Connecting) => ConnStatus::Connecting,
            Some(Slot::Connected(_)) => ConnStatus::Connected,
            Some(Slot::Failed(_)) => ConnStatus::Failed,
            None => ConnStatus::Disconnected,
        }
    }

    fn current_conn(&self) -> Option<Conn> {
        self.current_conn_in(&self.state.borrow())
    }

    fn current_conn_in(&self, st: &State) -> Option<Conn> {
        match st.conns.get(&st.current) {
            Some(Slot::Connected(c)) => Some(c.clone()),
            _ => None,
        }
    }

    /// Updates the list header, connection menu and Machines page to the
    /// connection state.
    fn update_chrome(&self) {
        let (current, failure) = {
            let st = self.state.borrow();
            let failure = match st.conns.get(&st.current) {
                Some(Slot::Failed(msg)) => Some(msg.clone()),
                _ => None,
            };
            (st.current.clone(), failure)
        };
        let status = self.status_of(&current);
        self.vm_list.set_connection(&format!("QEMU/KVM · {}", connection_name(&current)));
        if let Some(a) = self.window.lookup_action("connection").and_downcast::<gio::SimpleAction>() {
            a.set_state(&connection_id(&current).to_variant());
        }
        if let Some(a) = self.window.lookup_action("remove-connection").and_downcast::<gio::SimpleAction>() {
            a.set_enabled(current.is_remote());
        }
        let name = connection_name(&current);
        let has_data = !self.state.borrow().domains.is_empty() || self.vm_list_loaded();
        match status {
            ConnStatus::Connecting | ConnStatus::Disconnected if !has_data => {
                self.loading_label.set_label(&format!("Connecting to QEMU/KVM · {name}…"));
                self.machines.set_visible_child_name("loading");
            }
            ConnStatus::Failed if !has_data => {
                self.error_page.set_title(&format!("Couldn't connect to {name}"));
                self.error_page.set_description(failure.as_deref().map(glib::markup_escape_text).as_deref());
                self.machines.set_visible_child_name("error");
            }
            _ => {}
        }
        // A connection that drops while showing data keeps the (stale) list
        // visible under a banner instead of blanking the window.
        if status == ConnStatus::Failed && has_data {
            self.banner.set_title(&format!("Lost connection to {name}."));
            self.banner.set_revealed(true);
        } else {
            self.banner.set_revealed(false);
        }
    }

    fn vm_list_loaded(&self) -> bool {
        self.machines.visible_child_name().is_some_and(|n| n == "content" || n == "empty")
    }

    fn connect_to(self: &Rc<Self>, kind: ConnectionKind) {
        if matches!(self.state.borrow().conns.get(&kind), Some(Slot::Connecting | Slot::Connected(_))) {
            return;
        }
        self.state.borrow_mut().conns.insert(kind.clone(), Slot::Connecting);
        self.update_chrome();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = Conn::open(kind.clone()).await;
            let Some(this) = weak.upgrade() else { return };
            let is_current = this.state.borrow().current == kind;
            match result {
                Ok(conn) => {
                    this.state.borrow_mut().conns.insert(kind.clone(), Slot::Connected(conn.clone()));
                    if is_current {
                        this.on_connected(conn).await;
                    }
                }
                Err(e) => {
                    let mut msg = e.message().to_owned();
                    if kind.is_remote() {
                        msg.push_str(REMOTE_HINT);
                    }
                    this.state.borrow_mut().conns.insert(kind, Slot::Failed(msg));
                }
            }
            this.update_chrome();
        });
    }

    /// The Connection section: this computer, then the other computers.
    fn fill_connection_menu(&self) {
        let menu = &self.conn_menu;
        menu.remove_all();
        menu.append(Some("QEMU/KVM · System"), Some("win.connection::system"));
        menu.append(Some("QEMU/KVM · User Session"), Some("win.connection::session"));
        for uri in self.settings.get().remote_connections {
            let label = format!("QEMU/KVM · {}", ConnectionKind::Other(uri.clone()).label());
            let item = gio::MenuItem::new(Some(&label), None);
            item.set_action_and_target_value(Some("win.connection"), Some(&uri.to_variant()));
            menu.append_item(&item);
        }
        menu.append(Some("Add Remote Computer…"), Some("win.add-connection"));
        menu.append(Some("Remove This Connection"), Some("win.remove-connection"));
    }

    /// Asks for another computer to connect to over SSH.
    fn add_connection_dialog(self: &Rc<Self>) {
        let form = FormDialog::new("Add Remote Computer", "Connect");
        let group = adw::PreferencesGroup::builder()
            .description(
                "Manage the virtual machines of another computer over SSH. The app logs in with your SSH key; \
                 set it up first with: ssh-copy-id user@computer",
            )
            .build();
        let host = adw::EntryRow::builder().title("Computer (name or address)").build();
        let user =
            adw::EntryRow::builder().title("User name").text(glib::user_name().to_string_lossy().into_owned()).build();
        let port =
            adw::EntryRow::builder().title("SSH port (empty = 22)").input_purpose(gtk::InputPurpose::Digits).build();
        let kind = adw::ComboRow::builder()
            .title("Machines")
            .model(&gtk::StringList::new(&["System", "User session"]))
            .build();
        let startup = adw::SwitchRow::builder().title("Connect at startup").build();
        for w in [
            host.upcast_ref::<gtk::Widget>(),
            user.upcast_ref(),
            port.upcast_ref(),
            kind.upcast_ref(),
            startup.upcast_ref(),
        ] {
            group.add(w);
        }
        form.add(&group);
        // Debug builds: VMNG_DEV_REMOTE=<address> fills it in (tests).
        #[cfg(debug_assertions)]
        if let Some(t) = std::env::var("VMNG_DEV_REMOTE").ok().and_then(|u| vmng_virt::SshTarget::parse(&u)) {
            host.set_text(&t.host);
            user.set_text(t.user.as_deref().unwrap_or_default());
            port.set_text(&t.port.map(|p| p.to_string()).unwrap_or_default());
        }
        let weak = Rc::downgrade(self);
        form.connect_primary(move |form| {
            let Some(this) = weak.upgrade() else { return };
            let port_text = port.text().trim().to_owned();
            let port = if port_text.is_empty() {
                None
            } else {
                match port_text.parse::<u16>() {
                    Ok(p) if p > 0 => Some(p),
                    _ => return form.set_error(Some("The port must be a number between 1 and 65535.")),
                }
            };
            let user = Some(user.text().trim().to_owned()).filter(|u| !u.is_empty());
            #[allow(unused_mut)]
            let mut target = vmng_virt::SshTarget {
                user,
                host: host.text().trim().to_owned(),
                port,
                session: kind.selected() == 1,
                ..Default::default()
            };
            // Tests reach the test computer with their own key.
            #[cfg(debug_assertions)]
            if let Some(t) = std::env::var("VMNG_DEV_REMOTE").ok().and_then(|u| vmng_virt::SshTarget::parse(&u)) {
                target.keyfile = t.keyfile;
                target.no_verify = t.no_verify;
            }
            if let Err(m) = target.validate() {
                return form.set_error(Some(m));
            }
            let uri = target.uri();
            let at_startup = startup.is_active();
            this.settings.update(|p| {
                if !p.remote_connections.contains(&uri) {
                    p.remote_connections.push(uri.clone());
                }
                if at_startup && !p.autoconnect.contains(&uri) {
                    p.autoconnect.push(uri.clone());
                }
            });
            form.close();
            this.fill_connection_menu();
            this.switch_connection(ConnectionKind::Other(uri));
        });
        form.present(&self.window);
    }

    /// Forgets the current other computer and goes back to this one.
    fn ask_remove_connection(self: &Rc<Self>) {
        let kind = self.state.borrow().current.clone();
        if !kind.is_remote() {
            return;
        }
        let weak = Rc::downgrade(self);
        let label = kind.label();
        self.feedback.confirm(
            &format!("Remove {label}?"),
            "The app stops showing this computer. Its machines keep running there.",
            "Remove",
            true,
            move || {
                let Some(this) = weak.upgrade() else { return };
                let uri = kind.uri().to_owned();
                this.settings.update(|p| {
                    p.remote_connections.retain(|u| *u != uri);
                    p.autoconnect.retain(|u| *u != uri);
                });
                this.switch_connection(ConnectionKind::System);
                this.state.borrow_mut().conns.remove(&kind);
                this.fill_connection_menu();
            },
        );
    }

    async fn on_connected(self: &Rc<Self>, conn: Conn) {
        if let Ok(info) = conn.call(|c| c.host_info()).await {
            self.state.borrow_mut().host_info = Some(info);
        }
        if let Ok(caps) = conn.call(|c| c.host_caps()).await {
            let kvm = caps.guest(&caps.host_arch).is_some_and(|g| g.kvm);
            self.state.borrow_mut().kvm = Some(kvm);
        }
        let view = self.state.borrow().view;
        self.load_view(view);
        self.refresh();
    }

    fn switch_connection(self: &Rc<Self>, kind: ConnectionKind) {
        if self.state.borrow().current == kind {
            return;
        }
        {
            let mut st = self.state.borrow_mut();
            st.current = kind.clone();
            st.monitor.reset();
            st.domains.clear();
            st.guest.clear();
            st.host_info = None;
        }
        self.settings.update(|p| p.last_connection = kind.uri().to_owned());
        self.nav.pop_to_tag("main");
        self.machines.set_visible_child_name("loading");
        self.vm_list.update(&[]);
        self.update_chrome();
        match self.current_conn() {
            Some(conn) => {
                let this = self.clone();
                glib::spawn_future_local(async move { this.on_connected(conn).await });
            }
            None => self.connect_to(kind),
        }
    }

    fn restart_timer(self: &Rc<Self>) {
        let secs = self.settings.get().stats_interval_secs.max(1);
        if let Some(id) = self.state.borrow_mut().timer.take() {
            id.remove();
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_seconds_local(secs, move || {
            let Some(this) = weak.upgrade() else { return glib::ControlFlow::Break };
            this.refresh();
            glib::ControlFlow::Continue
        });
        self.state.borrow_mut().timer = Some(id);
    }

    /// Polls the current connection once (skipped if a poll is running).
    fn refresh(self: &Rc<Self>) {
        let Some(conn) = self.current_conn() else { return };
        {
            let mut st = self.state.borrow_mut();
            if st.polling {
                return;
            }
            st.polling = true;
            st.tick += 1;
        }
        let kind = conn.kind().clone();
        let prefs = self.settings.get();
        let wanted =
            vmng_virt::StatsWanted { cpu: prefs.stats_cpu, memory: prefs.stats_memory, network: prefs.stats_network };
        self.details.set_collect([prefs.stats_cpu, prefs.stats_memory, prefs.stats_disk, prefs.stats_network]);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn
                .call(move |c| {
                    Ok(Sample {
                        at: Instant::now(),
                        domains: c.list_domains()?,
                        stats: c.domain_stats(wanted)?,
                        host: c.host_stats()?,
                        pool: c.pool_summary("default").ok().flatten(),
                    })
                })
                .await;
            let alive = if result.is_err() { conn.call(|c| Ok(c.is_alive())).await.unwrap_or(false) } else { true };
            let Some(this) = weak.upgrade() else { return };
            this.state.borrow_mut().polling = false;
            if this.state.borrow().current != kind {
                return;
            }
            match result {
                Ok(sample) => this.apply_sample(sample),
                Err(e) if !alive => {
                    this.state.borrow_mut().conns.insert(kind, Slot::Failed(e.message().to_owned()));
                    this.update_chrome();
                }
                Err(e) => glib::g_warning!("vmng", "refresh failed: {e}"),
            }
        });
    }

    fn apply_sample(self: &Rc<Self>, sample: Sample) {
        let tick = {
            let mut st = self.state.borrow_mut();
            st.monitor.ingest(&sample);
            st.domains = sample.domains;
            let alive: Vec<&str> = st.domains.iter().map(|d| d.uuid.as_str()).collect();
            let stale: Vec<String> = st.guest.keys().filter(|k| !alive.contains(&k.as_str())).cloned().collect();
            for k in stale {
                st.guest.remove(&k);
            }
            st.tick
        };
        let domains = self.state.borrow().domains.clone();
        self.restart_finished_installs(&domains);
        self.vm_list.update(&domains);
        if self.state.borrow().view == View::Host {
            self.update_host_view();
        }
        self.update_tray_menu();
        {
            let st = self.state.borrow();
            self.vm_list.update_host(st.host_info.as_ref(), st.monitor.host());
        }
        if domains.is_empty() {
            self.machines.set_visible_child_name("empty");
        } else {
            self.machines.set_visible_child_name("content");
            let pending = self.state.borrow_mut().pending_select.take();
            if let Some(uuid) = pending {
                self.vm_list.select(&uuid);
            }
            self.vm_list.ensure_selection();
            let page = self.state.borrow_mut().pending_page.take();
            if let Some((uuid, tab)) = page
                && let Some(vm) = self.vm_by_uuid(&uuid)
            {
                self.open_vm_page(&vm, tab);
            }
        }
        // (Taken first: the state can't stay borrowed while switching.)
        let pending_view = if tick == 1 { self.state.borrow_mut().pending_view.take() } else { None };
        if let Some(view) = pending_view {
            self.set_view(view);
        }
        #[cfg(debug_assertions)]
        if tick == 1 {
            self.dev_hooks();
        }
        let selected = self.vm_list.selected();
        self.show_selected(selected.as_deref());
        self.update_consoles();
        if let Some(uuid) = &selected
            && tick % PREVIEW_POLL_EVERY == 0
        {
            self.fetch_preview(uuid.clone());
        }
        if tick % GUEST_POLL_EVERY == 0
            && let Some(uuid) = selected
        {
            self.fetch_guest(uuid);
        }
        self.update_chrome();
    }

    /// Captures the selected machine's screen for the details preview, when
    /// that preview is actually on screen.
    fn fetch_preview(self: &Rc<Self>, uuid: String) {
        let showing = self.window.is_visible()
            && self.state.borrow().view == View::Machines
            && self.nav.visible_page().and_then(|p| p.tag()).is_some_and(|t| t == "main")
            && !(self.split.is_narrow() && !self.split.showing_detail());
        let running = self
            .state
            .borrow()
            .domains
            .iter()
            .any(|d| d.uuid == uuid && d.state == vmng_virt::DomainState::Running && !d.config.graphics.is_empty());
        if !showing || !running || self.state.borrow().preview_busy {
            return;
        }
        let Some(conn) = self.current_conn() else { return };
        self.state.borrow_mut().preview_busy = true;
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let id = uuid.clone();
            let result = conn.call(move |c| c.screenshot(&id)).await;
            let Some(this) = weak.upgrade() else { return };
            this.state.borrow_mut().preview_busy = false;
            // No picture (e.g. no display yet) keeps the "Running" placeholder.
            if let Some(texture) = result.ok().and_then(screenshot_texture) {
                this.details.set_screen(&uuid, &texture);
            }
        });
    }

    fn show_selected(&self, uuid: Option<&str>) {
        let st = self.state.borrow();
        let vm = uuid.and_then(|u| st.domains.iter().find(|d| d.uuid == u));
        self.actions.set_target(vm);
        match vm {
            Some(d) => {
                self.details.show(d, st.monitor.vm(&d.uuid), st.guest.get(&d.uuid));
                self.content.set_visible_child_name("details");
                if self.vm_page_open()
                    && let Some(conn) = self.current_conn_in(&st)
                {
                    self.vm_page.show(conn, d, st.host_info.clone());
                    self.vm_page.performance.update(d, st.monitor.vm(&d.uuid), self.settings.get().stats_interval_secs);
                }
            }
            None => {
                self.content.set_visible_child_name("none");
                if self.vm_page_open() {
                    // The VM went away (deleted elsewhere).
                    self.nav.pop_to_tag("main");
                }
            }
        }
    }

    /// Opens `vm`'s console window, or brings it forward if it is open.
    fn open_console(self: &Rc<Self>, vm: &DomainSummary) {
        let Some(conn) = self.current_conn() else { return };
        let Some(app) = self.window.application().and_downcast::<adw::Application>() else { return };
        let existing = self.consoles.borrow().get(&vm.uuid).cloned();
        let console = existing.unwrap_or_else(|| {
            let weak = Rc::downgrade(self);
            let console = ConsoleWindow::new(&app, self.settings.clone(), move |uuid, action| {
                if let Some(this) = weak.upgrade() {
                    this.vm_action(uuid.to_owned(), action);
                }
            });
            let (weak, uuid) = (Rc::downgrade(self), vm.uuid.clone());
            console.window.connect_close_request(move |_| {
                if let Some(this) = weak.upgrade()
                    && let Some(c) = this.consoles.borrow_mut().remove(&uuid)
                {
                    c.leave();
                }
                glib::Propagation::Proceed
            });
            self.consoles.borrow_mut().insert(vm.uuid.clone(), console.clone());
            #[cfg(debug_assertions)]
            crate::dev::maybe_screenshot(&console.window);
            console
        });
        console.open(conn, vm);
        console.window.present();
    }

    /// Keeps the open console windows in step with their machines; closes
    /// those whose machine is gone.
    fn update_consoles(&self) {
        let open: Vec<(String, Rc<ConsoleWindow>)> =
            self.consoles.borrow().iter().map(|(u, c)| (u.clone(), c.clone())).collect();
        for (uuid, console) in open {
            match self.vm_by_uuid(&uuid) {
                Some(vm) => console.update(&vm),
                None if console.is_on(&self.state.borrow().current) => console.window.close(),
                None => {}
            }
        }
    }

    fn vm_page_open(&self) -> bool {
        self.nav.visible_page().and_then(|p| p.tag()).is_some_and(|t| t == VmPage::TAG)
    }

    /// Asks the guest (agent) for its hostname and addresses.
    fn fetch_guest(self: &Rc<Self>, uuid: String) {
        let active = self.state.borrow().domains.iter().any(|d| d.uuid == uuid && d.state.is_active());
        let Some(conn) = self.current_conn().filter(|_| active) else { return };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let id = uuid.clone();
            let result = conn.call(move |c| c.guest_details(&id)).await;
            let Some(this) = weak.upgrade() else { return };
            if let Ok(details) = result {
                this.state.borrow_mut().guest.insert(uuid.clone(), details);
                if this.vm_list.selected().as_deref() == Some(uuid.as_str()) {
                    this.show_selected(Some(&uuid));
                }
            }
        });
    }

    fn domain_name(&self, uuid: &str) -> String {
        self.state
            .borrow()
            .domains
            .iter()
            .find(|d| d.uuid == uuid)
            .map_or_else(|| "the machine".to_owned(), |d| d.display_name().to_owned())
    }

    /// Runs a power action, asking for confirmation first where configured.
    fn request_power(self: &Rc<Self>, uuid: String, action: PowerAction) {
        let prefs = self.settings.get();
        let name = self.domain_name(&uuid);
        let confirm = match action {
            PowerAction::ForceOff | PowerAction::Reset => prefs.confirm_force_off,
            PowerAction::Shutdown | PowerAction::Reboot => prefs.confirm_shutdown,
            PowerAction::Pause => prefs.confirm_pause,
            _ => false,
        };
        if !confirm {
            self.power(uuid, action);
            return;
        }
        let (heading, body, verb, destructive) = match action {
            PowerAction::ForceOff => (
                format!("Force off {name}?"),
                "This is like pulling the power plug. Unsaved work in the machine is lost.",
                "Force Off",
                true,
            ),
            PowerAction::Reset => (
                format!("Reset {name}?"),
                "This is like pressing the reset button. Unsaved work in the machine is lost.",
                "Reset",
                true,
            ),
            PowerAction::Shutdown => {
                (format!("Shut down {name}?"), "The guest is asked to shut down cleanly.", "Shut Down", false)
            }
            PowerAction::Reboot => (format!("Reboot {name}?"), "The guest is asked to restart.", "Reboot", false),
            _ => (format!("Pause {name}?"), "The machine stops running until it is resumed.", "Pause", false),
        };
        let dialog = adw::AlertDialog::builder().heading(heading).body(body).build();
        dialog.add_responses(&[("cancel", "Cancel"), ("confirm", verb)]);
        dialog.set_response_appearance(
            "confirm",
            if destructive { adw::ResponseAppearance::Destructive } else { adw::ResponseAppearance::Suggested },
        );
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        dialog.connect_response(Some("confirm"), move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.power(uuid.clone(), action);
            }
        });
        dialog.present(Some(&self.window));
    }

    /// A network installer stops its machine when it's done (the first
    /// boot ran the installer's kernel); start it again from its disk.
    fn restart_finished_installs(self: &Rc<Self>, domains: &[DomainSummary]) {
        let finished: Vec<String> = {
            let mut st = self.state.borrow_mut();
            // A machine not listed yet (a poll from before it was made)
            // is kept waiting.
            let done: Vec<String> = st
                .installing
                .iter()
                .filter(|u| domains.iter().any(|d| &d.uuid == *u && !d.state.is_active()))
                .cloned()
                .collect();
            for u in &done {
                st.installing.remove(u);
            }
            done
        };
        for uuid in finished {
            let name = self.domain_name(&uuid);
            self.feedback.toast(&format!("The installer of {name} finished; starting it from its disk"));
            self.power(uuid, PowerAction::Start);
        }
    }

    fn power(self: &Rc<Self>, uuid: String, action: PowerAction) {
        let Some(conn) = self.current_conn() else { return };
        let name = self.domain_name(&uuid);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn.call(move |c| c.power(&uuid, action)).await;
            let Some(this) = weak.upgrade() else { return };
            if let Err(e) = result {
                let mut details = e.message().to_owned();
                if let Some(hint) = start_hint(&details) {
                    details.push_str("\n\n");
                    details.push_str(hint);
                }
                this.feedback.error(&format!("Couldn't {} {name}", action.label().to_lowercase()), &details);
            }
            this.refresh();
        });
    }

    fn set_autostart(self: &Rc<Self>, uuid: String, on: bool) {
        let Some(conn) = self.current_conn() else { return };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn.call(move |c| c.set_autostart(&uuid, on)).await;
            let Some(this) = weak.upgrade() else { return };
            if let Err(e) = result {
                this.feedback.error("Couldn't change the start-at-boot setting", e.message());
            }
            this.refresh();
        });
    }

    /// Debug builds: `VMNG_DEV_SELECT=<vm name>` selects a VM and
    /// `VMNG_DEV_ACTION=<action>` (e.g. `vm.snapshots`) activates an action,
    /// so screenshots of deeper screens can be taken unattended.
    #[cfg(debug_assertions)]
    fn dev_hooks(self: &Rc<Self>) {
        if let Ok(name) = std::env::var("VMNG_DEV_SELECT") {
            let uuid = self.state.borrow().domains.iter().find(|d| d.config.name == name).map(|d| d.uuid.clone());
            if let Some(uuid) = uuid {
                self.vm_list.select(&uuid);
                self.split.show_detail();
            }
        }
        if let Ok(view) = std::env::var("VMNG_DEV_VIEW") {
            let view = View::ALL.into_iter().find(|v| v.id() == view);
            if let Some(view) = view {
                self.set_view_later(view);
            }
        }
        // VMNG_DEV_CONSOLES=<name>,<name>: open those consoles (several windows).
        if let Ok(names) = std::env::var("VMNG_DEV_CONSOLES") {
            let vms: Vec<DomainSummary> = names
                .split(',')
                .filter_map(|n| self.state.borrow().domains.iter().find(|d| d.config.name == n.trim()).cloned())
                .collect();
            for vm in vms {
                self.open_console(&vm);
            }
        }
        if let Ok(action) = std::env::var("VMNG_DEV_ACTION") {
            let window = self.window.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || {
                let _ = WidgetExt::activate_action(&window, &action, None);
            });
        }
    }

    fn open_new_vm(self: &Rc<Self>, import: bool) {
        let Some(conn) = self.current_conn() else {
            self.feedback.toast("Connect to a hypervisor first.");
            return;
        };
        let (session, host, domains) = {
            let st = self.state.borrow();
            (st.current.is_session(), st.host_info.clone(), st.domains.clone())
        };
        let ctx = super::new_vm::WizardContext {
            conn,
            session,
            prefs: self.settings.get(),
            host,
            domains,
            feedback: self.feedback.clone(),
            import,
        };
        let weak = Rc::downgrade(self);
        super::new_vm::open(&self.window, ctx, move |created, customize| {
            if let Some(this) = weak.upgrade() {
                {
                    let mut st = this.state.borrow_mut();
                    let uuid = created.uuid;
                    if created.restart_after_install {
                        st.installing.insert(uuid.clone());
                    }
                    st.pending_select = Some(uuid.clone());
                    if customize {
                        st.pending_page = Some((uuid, VmTab::Hardware));
                    }
                }
                this.set_view(View::Machines);
                this.refresh();
            }
        });
    }

    #[cfg(debug_assertions)]
    fn set_view_later(&self, view: View) {
        // Called from a &self context; the window keeps an Rc via the actions.
        let _ = WidgetExt::activate_action(&self.window, "win.view", Some(&view.id().to_variant()));
    }

    /// Shows or removes the tray icon to match Preferences.
    fn update_tray(self: &Rc<Self>) {
        let prefs = self.settings.get();
        let wanted = prefs.tray_icon;
        let icon = if prefs.tray_icon_color {
            crate::tray::TrayIcon::Image(crate::config::APP_ICON_SVG)
        } else {
            crate::tray::TrayIcon::Named(crate::config::TRAY_ICON)
        };
        let conn = self.window.application().and_then(|a| a.dbus_connection());
        let can = conn.as_ref().is_some_and(crate::tray::available);
        if !(wanted && can) {
            self.tray.replace(None);
            return;
        }
        if let Some(tray) = self.tray.borrow().as_ref() {
            tray.set_icon(icon);
            return;
        }
        let Some(conn) = conn else { return };
        let weak = Rc::downgrade(self);
        match crate::tray::Tray::new(&conn, icon, move |cmd| {
            if let Some(this) = weak.upgrade() {
                this.tray_command(cmd);
            }
        }) {
            Ok(tray) => {
                self.tray.replace(Some(tray));
                self.update_tray_menu();
            }
            Err(e) => glib::g_warning!("tray", "couldn't create the tray icon: {e}"),
        }
    }

    /// Shows the window in front. GNOME (Wayland) only lets a window raise
    /// itself in response to input inside the app, and a tray menu click
    /// isn't that; a window that appears anew is put in front, though. So a
    /// window that is open but behind others (or minimised) is hidden and
    /// shown again.
    fn bring_to_front(&self) {
        if self.window.is_visible() && !self.window.is_active() {
            self.window.set_visible(false);
        }
        self.window.present();
    }

    fn update_tray_menu(&self) {
        let tray = self.tray.borrow();
        let Some(tray) = tray.as_ref() else { return };
        let vms: Vec<crate::tray::TrayVm> = self
            .state
            .borrow()
            .domains
            .iter()
            .map(|d| crate::tray::TrayVm { uuid: d.uuid.clone(), name: d.display_name().to_owned(), state: d.state })
            .collect();
        tray.set_vms(&vms);
    }

    fn tray_command(self: &Rc<Self>, cmd: crate::tray::TrayCommand) {
        use crate::tray::TrayCommand;
        match cmd {
            TrayCommand::ShowWindow => self.bring_to_front(),
            TrayCommand::Quit => {
                if let Some(app) = self.window.application() {
                    app.quit();
                }
            }
            TrayCommand::Power(uuid, action) => self.request_power(uuid, action),
            TrayCommand::Console(uuid) => {
                if let Some(vm) = self.vm_by_uuid(&uuid) {
                    self.open_console(&vm);
                }
            }
        }
    }

    fn vm_by_uuid(&self, uuid: &str) -> Option<DomainSummary> {
        self.state.borrow().domains.iter().find(|d| d.uuid == uuid).cloned()
    }

    /// Handles a `vm.*` action for the VM `uuid`.
    fn vm_action(self: &Rc<Self>, uuid: String, action: VmAction) {
        let Some(vm) = self.vm_by_uuid(&uuid) else { return };
        let Some(conn) = self.current_conn() else { return };
        // Pages and dialogs live in the main window: bring it forward when
        // they are asked for from a console window.
        if !matches!(action, VmAction::Power(_) | VmAction::Console | VmAction::SyncTime) {
            self.bring_to_front();
        }
        match action {
            VmAction::Power(p) => self.request_power(uuid, p),
            VmAction::Console => self.open_console(&vm),
            VmAction::SyncTime => {
                let name = vm.display_name().to_owned();
                let id = uuid.clone();
                self.run_vm_op(
                    move |c| c.sync_guest_time(&id),
                    format!("Couldn't sync the clock of {name}"),
                    format!("Synced the clock of {name} with this computer"),
                );
            }
            VmAction::Hardware => self.open_vm_page(&vm, VmTab::Hardware),
            VmAction::Snapshots => self.open_vm_page(&vm, VmTab::Snapshots),
            VmAction::Performance => self.open_vm_page(&vm, VmTab::Performance),
            VmAction::TakeSnapshot => {
                let weak = Rc::downgrade(self);
                take_snapshot_dialog(&self.feedback, conn, &vm, move || {
                    if let Some(this) = weak.upgrade() {
                        this.vm_page.snapshots.reload();
                        this.refresh();
                    }
                });
            }
            VmAction::Clone => {
                let (live, all) = {
                    let st = self.state.borrow();
                    (st.monitor.vm(&uuid).cloned(), st.domains.clone())
                };
                let weak = Rc::downgrade(self);
                clone_dialog(&self.window, &vm, live.as_ref(), &all, move |name, disks| {
                    if let Some(this) = weak.upgrade() {
                        this.clone_vm(uuid.clone(), name, disks);
                    }
                });
            }
            VmAction::Rename => {
                let all = self.state.borrow().domains.clone();
                let weak = Rc::downgrade(self);
                let old_name = vm.display_name().to_owned();
                rename_dialog(&self.window, &vm, &all, move |name| {
                    if let Some(this) = weak.upgrade() {
                        let (id, n) = (uuid.clone(), name.clone());
                        this.run_vm_op(
                            move |c| c.rename_domain(&id, &n),
                            format!("Couldn't rename {old_name}"),
                            format!("Renamed {old_name} to {name}"),
                        );
                    }
                });
            }
            VmAction::DiscardSavedState => {
                let weak = Rc::downgrade(self);
                let name = vm.display_name().to_owned();
                self.feedback.confirm(
                    &format!("Discard the saved state of {name}?"),
                    "The machine starts fresh next time instead of resuming. Unsaved work in it is lost.",
                    "Discard",
                    true,
                    move || {
                        if let Some(this) = weak.upgrade() {
                            let id = uuid.clone();
                            this.run_vm_op(
                                move |c| c.discard_saved_state(&id),
                                format!("Couldn't discard the saved state of {name}"),
                                format!("Discarded the saved state of {name}"),
                            );
                        }
                    },
                );
            }
            VmAction::Delete => {
                if self.settings.get().confirm_delete {
                    let (live, all) = {
                        let st = self.state.borrow();
                        (st.monitor.vm(&uuid).cloned(), st.domains.clone())
                    };
                    let weak = Rc::downgrade(self);
                    delete_dialog(&self.window, &vm, live.as_ref(), &all, move |paths| {
                        if let Some(this) = weak.upgrade() {
                            this.delete_vm(uuid.clone(), paths);
                        }
                    });
                } else {
                    self.delete_vm(uuid, Vec::new());
                }
            }
        }
    }

    fn open_vm_page(self: &Rc<Self>, vm: &DomainSummary, tab: VmTab) {
        let Some(conn) = self.current_conn() else { return };
        self.vm_page.select(tab);
        let host = self.state.borrow().host_info.clone();
        self.vm_page.show(conn, vm, host);
        if !self.vm_page_open() {
            self.nav.push_by_tag(VmPage::TAG);
        }
    }

    /// Runs a quick operation on the current connection, then refreshes.
    fn run_vm_op(
        self: &Rc<Self>,
        op: impl FnOnce(&vmng_virt::Connection) -> vmng_virt::Result<()> + Send + 'static,
        failure: String,
        success: String,
    ) {
        let Some(conn) = self.current_conn() else { return };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn.call(op).await;
            let Some(this) = weak.upgrade() else { return };
            match result {
                Ok(()) => this.feedback.toast(&success),
                Err(e) => this.feedback.error(&failure, e.message()),
            }
            this.refresh();
        });
    }

    fn delete_vm(self: &Rc<Self>, uuid: String, paths: Vec<String>) {
        let Some(conn) = self.current_conn() else { return };
        let name = self.domain_name(&uuid);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let id = uuid.clone();
            let result = conn.call(move |c| c.delete_domain(&id, &paths)).await;
            let Some(this) = weak.upgrade() else { return };
            match result {
                Ok(failed) if failed.is_empty() => this.feedback.toast(&format!("Deleted {name}")),
                Ok(failed) => {
                    let details: Vec<String> = failed.iter().map(|(p, e)| format!("{p}: {}", e.message())).collect();
                    this.feedback.error(&format!("Deleted {name}, but some disks were kept"), &details.join("\n"));
                }
                Err(e) => this.feedback.error(&format!("Couldn't delete {name}"), e.message()),
            }
            if this.vm_page_open() {
                this.nav.pop_to_tag("main");
            }
            this.refresh();
        });
    }

    fn clone_vm(self: &Rc<Self>, uuid: String, new_name: String, disks: Vec<(String, CloneDiskMode)>) {
        let Some(conn) = self.current_conn() else { return };
        let source = self.domain_name(&uuid);
        let progress = Arc::new((AtomicU64::new(0), AtomicU64::new(0)));
        self.job.start(&format!("Cloning {source} as {new_name}"));

        // The copy runs on a worker thread; the bar follows its progress.
        let p = progress.clone();
        let job = self.job.clone();
        let label = format!("Cloning {source}");
        let ticker = glib::timeout_add_local(std::time::Duration::from_millis(400), move || {
            let (done, total) = (p.0.load(Ordering::Relaxed), p.1.load(Ordering::Relaxed));
            if total > 0 {
                job.update(&format!("{label} — {}", format::used_of(done, total)), done as f64 / total as f64);
            } else {
                job.pulse();
            }
            glib::ControlFlow::Continue
        });

        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let n = new_name.clone();
            let result = conn
                .call(move |c| {
                    c.clone_domain(&uuid, &n, &disks, &|done, total| {
                        progress.0.store(done, Ordering::Relaxed);
                        progress.1.store(total, Ordering::Relaxed);
                    })
                })
                .await;
            ticker.remove();
            let Some(this) = weak.upgrade() else { return };
            this.job.finish();
            match result {
                Ok(new_uuid) => {
                    this.feedback.toast(&format!("Cloned {source} as {new_name}"));
                    this.state.borrow_mut().pending_select = Some(new_uuid);
                }
                Err(e) => this.feedback.error(&format!("Couldn't clone {source}"), e.message()),
            }
            this.refresh();
        });
    }
}

/// A slim progress bar at the bottom of the window for long operations.
#[derive(Clone, Debug)]
struct JobBar {
    revealer: gtk::Revealer,
    label: gtk::Label,
    bar: gtk::ProgressBar,
}

impl JobBar {
    fn new() -> Self {
        let label = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
        let bar = gtk::ProgressBar::new();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .css_classes(["card", "job-bar"])
            .build();
        content.append(&label);
        content.append(&bar);
        content.update_property(&[gtk::accessible::Property::Label("Operation in progress")]);
        let clamp = adw::Clamp::builder()
            .maximum_size(560)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .child(&content)
            .build();
        let revealer =
            gtk::Revealer::builder().transition_type(gtk::RevealerTransitionType::SlideUp).child(&clamp).build();
        Self { revealer, label, bar }
    }

    fn start(&self, text: &str) {
        self.label.set_label(text);
        self.bar.set_fraction(0.0);
        self.revealer.set_reveal_child(true);
    }

    fn update(&self, text: &str, fraction: f64) {
        self.label.set_label(text);
        self.bar.set_fraction(fraction.clamp(0.0, 1.0));
    }

    fn pulse(&self) {
        self.bar.pulse();
    }

    fn finish(&self) {
        self.revealer.set_reveal_child(false);
    }
}

/// Plain advice for start failures whose libvirt message doesn't say what
/// to do.
fn start_hint(details: &str) -> Option<&'static str> {
    // 3D acceleration: QEMU couldn't use the graphics card. On the System
    // connection QEMU runs confined (SELinux), which NVIDIA's own driver
    // isn't allowed to work in.
    (details.contains("egl:") || details.contains("render node")).then_some(
        "The machine's 3D acceleration couldn't use the graphics card. Turn off 3D acceleration in Hardware → \
         Video, or run the machine on the User session connection. On the System connection, some graphics \
         drivers (such as NVIDIA's own) aren't allowed to run inside the virtual machine's process.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_ids_round_trip() {
        for kind in [
            ConnectionKind::System,
            ConnectionKind::Session,
            ConnectionKind::Other("qemu+ssh://me@server/system".into()),
        ] {
            assert_eq!(connection_of(&connection_id(&kind)), kind);
        }
    }

    #[test]
    fn start_failures_get_advice_only_when_known() {
        assert!(start_hint("qemu: egl: no drm render node available").is_some());
        assert!(start_hint("Requested operation is not valid: network 'default' is not active").is_none());
    }
}
