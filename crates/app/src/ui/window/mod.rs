//! The main window.

mod connections;
mod poll;
mod tray_glue;
mod vm_ops;

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
use connections::{connection_id, connection_of};
use vm_ops::JobBar;

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
}
