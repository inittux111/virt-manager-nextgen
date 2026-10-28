//! The main window.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use adw::prelude::*;
use gtk::{gio, glib};
use vmng_virt::{CloneDiskMode, ConnectionKind, DomainSummary, GuestDetails, HostInfo, PowerAction};

use super::feedback::Feedback;
use super::snapshots::{SnapshotsView, take_snapshot_dialog};
use super::title_menu::{ConnStatus, TitleMenu, View, connection_name};
use super::vm_actions::{VmAction, VmActions};
use super::vm_details::VmDetails;
use super::vm_dialogs::{clone_dialog, delete_dialog, rename_dialog};
use super::vm_list::VmList;
use super::vm_page::{VmPage, VmTab};
use crate::backend::{Conn, Monitor, Sample};
use crate::format;
use crate::settings::Settings;

/// Guest agent details are refreshed every this many polls.
const GUEST_POLL_EVERY: u64 = 5;

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
    /// A VM page to open once the VM appears (e.g. "customize before install").
    pending_page: Option<(String, VmTab)>,
    /// A VM whose console opens once it shows as running ("open the
    /// console when a machine starts").
    pending_console: Option<String>,
}

pub struct MainWindow {
    pub window: adw::ApplicationWindow,
    settings: Settings,
    title_menu: TitleMenu,
    views: gtk::Stack,
    machines: gtk::Stack,
    error_page: adw::StatusPage,
    loading_label: gtk::Label,
    content: gtk::Stack,
    vm_list: Rc<VmList>,
    details: Rc<VmDetails>,
    banner: adw::Banner,
    feedback: Feedback,
    nav: adw::NavigationView,
    vm_page: VmPage,
    console: Rc<super::console::ConsolePage>,
    actions: VmActions,
    job: JobBar,
    search_bar: gtk::SearchBar,
    search_button: gtk::ToggleButton,
    state: RefCell<State>,
}

impl std::fmt::Debug for MainWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MainWindow").finish_non_exhaustive()
    }
}

fn placeholder_page(view: View, text: &str) -> adw::StatusPage {
    adw::StatusPage::builder().icon_name(view.icon()).title(view.title()).description(text).build()
}

impl MainWindow {
    pub fn new(app: &adw::Application, settings: Settings) -> Rc<Self> {
        let prefs = settings.get();
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(crate::config::APP_NAME)
            .default_width(1280)
            .default_height(820)
            .width_request(360)
            .height_request(400)
            .build();

        // Header.
        let title_menu = TitleMenu::new();
        let header = adw::HeaderBar::builder().title_widget(&gtk::Box::new(gtk::Orientation::Horizontal, 0)).build();
        header.pack_start(&title_menu.button);

        let main_menu = gio::Menu::new();
        let section = gio::Menu::new();
        section.append(Some("Preferences"), Some("app.preferences"));
        section.append(Some("Keyboard Shortcuts"), Some("win.show-help-overlay"));
        section.append(Some("About Virtual Machine Manager"), Some("app.about"));
        main_menu.append_section(None, &section);
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&main_menu)
            .primary(true)
            .tooltip_text("Main menu")
            .build();
        header.pack_end(&menu_button);

        let new_vm = super::widgets::labeled_button("list-add-symbolic", "New VM");
        new_vm.add_css_class("suggested-action");
        new_vm.set_action_name(Some("win.new-vm"));
        new_vm.set_tooltip_text(Some("Create a virtual machine"));
        header.pack_end(&new_vm);

        let search_button =
            gtk::ToggleButton::builder().icon_name("system-search-symbolic").tooltip_text("Search machines").build();
        header.pack_end(&search_button);

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
        let split = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        split.append(&vm_list.root);
        split.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        split.append(&content);

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

        let empty_actions = gtk::Box::builder().spacing(12).halign(gtk::Align::Center).build();
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

        let machines = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
        machines.add_named(&loading, Some("loading"));
        machines.add_named(&error_page, Some("error"));
        machines.add_named(&empty, Some("empty"));
        machines.add_named(&split, Some("content"));

        let views = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).vexpand(true).build();
        views.add_named(&machines, Some(View::Machines.id()));
        views.add_named(
            &placeholder_page(View::Host, "Detailed host usage arrives in a later update."),
            Some(View::Host.id()),
        );
        views.add_named(
            &placeholder_page(View::Networks, "Creating and managing virtual networks arrives in a later update."),
            Some(View::Networks.id()),
        );
        views.add_named(
            &placeholder_page(View::Storage, "Browsing and managing storage pools arrives in a later update."),
            Some(View::Storage.id()),
        );

        let banner = adw::Banner::builder().button_label("Retry Now").build();
        let job = JobBar::new();
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.append(&banner);
        body.append(&search_bar);
        body.append(&views);
        body.append(&job.revealer);

        let toolbar = adw::ToolbarView::builder().content(&body).build();
        toolbar.add_top_bar(&header);
        let main_page =
            adw::NavigationPage::builder().tag("main").title(crate::config::APP_NAME).child(&toolbar).build();

        let toasts = adw::ToastOverlay::new();
        let feedback = Feedback::new(&toasts, &window);
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
        let console = super::console::ConsolePage::new(settings.clone(), feedback.clone());
        nav.add(&console.page);
        let this = Rc::new(Self {
            window,
            settings,
            title_menu,
            views,
            machines,
            error_page,
            loading_label,
            content,
            vm_list,
            details,
            banner,
            feedback,
            nav,
            vm_page,
            console,
            actions,
            job,
            search_bar,
            search_button,
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
                pending_select: None,
                pending_page: None,
                pending_console: None,
            }),
        });

        me.replace(Rc::downgrade(&this));
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
            }
        });
        this
    }

    fn setup_actions(self: &Rc<Self>) {
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
        self.title_menu.connect_view_selected(move |view| {
            if let Some(this) = weak.upgrade() {
                this.set_view(view);
            }
        });
        let weak = Rc::downgrade(self);
        self.title_menu.connect_connection_selected(move |kind| {
            if let Some(this) = weak.upgrade() {
                this.switch_connection(kind);
            }
        });

        let weak = Rc::downgrade(self);
        self.vm_list.connect_selected(move |uuid| {
            if let Some(this) = weak.upgrade() {
                this.show_selected(uuid);
                if let Some(uuid) = uuid {
                    this.fetch_guest(uuid.to_owned());
                }
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
                if !this.console_open() {
                    this.console.leave();
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

    fn set_view(&self, view: View) {
        self.state.borrow_mut().view = view;
        self.views.set_visible_child_name(view.id());
        self.search_button.set_visible(view == View::Machines);
        if view != View::Machines {
            self.search_bar.set_search_mode(false);
        }
        self.update_chrome();
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

    /// Updates the title, menu and Machines page to the connection state.
    fn update_chrome(&self) {
        let (view, current, failure) = {
            let st = self.state.borrow();
            let failure = match st.conns.get(&st.current) {
                Some(Slot::Failed(msg)) => Some(msg.clone()),
                _ => None,
            };
            (st.view, st.current.clone(), failure)
        };
        let status = self.status_of(&current);
        self.title_menu.set_current(view, &current, status);
        for kind in super::title_menu::CONNECTIONS {
            self.title_menu.set_connection_status(&kind, self.status_of(&kind));
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
                    this.state.borrow_mut().conns.insert(kind, Slot::Failed(e.message().to_owned()));
                }
            }
            this.update_chrome();
        });
    }

    async fn on_connected(self: &Rc<Self>, conn: Conn) {
        if let Ok(info) = conn.call(|c| c.host_info()).await {
            self.state.borrow_mut().host_info = Some(info);
        }
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
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn
                .call(|c| {
                    Ok(Sample {
                        at: Instant::now(),
                        domains: c.list_domains()?,
                        stats: c.domain_stats()?,
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
        self.vm_list.update(&domains);
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
            let console = self.state.borrow_mut().pending_console.take();
            if let Some(uuid) = console {
                match self.vm_by_uuid(&uuid) {
                    Some(vm) if vm.state.is_active() => {
                        self.vm_list.select(&uuid);
                        self.open_console(&vm);
                    }
                    Some(_) => self.state.borrow_mut().pending_console = Some(uuid),
                    None => {}
                }
            }
            let page = self.state.borrow_mut().pending_page.take();
            if let Some((uuid, tab)) = page
                && let Some(vm) = self.vm_by_uuid(&uuid)
            {
                self.open_vm_page(&vm, tab);
            }
        }
        #[cfg(debug_assertions)]
        if tick == 1 {
            self.dev_hooks();
        }
        let selected = self.vm_list.selected();
        self.show_selected(selected.as_deref());
        if tick % GUEST_POLL_EVERY == 0
            && let Some(uuid) = selected
        {
            self.fetch_guest(uuid);
        }
        self.update_chrome();
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
                }
                if self.console_open() && self.console.is_open_for(&d.uuid) {
                    self.console.update(d);
                }
            }
            None => {
                self.content.set_visible_child_name("none");
                if self.vm_page_open() || self.console_open() {
                    // The VM went away (deleted elsewhere).
                    self.nav.pop_to_tag("main");
                }
            }
        }
    }

    fn console_open(&self) -> bool {
        self.nav.visible_page().and_then(|p| p.tag()).is_some_and(|t| t == super::console::ConsolePage::TAG)
    }

    fn open_console(self: &Rc<Self>, vm: &DomainSummary) {
        let Some(conn) = self.current_conn() else { return };
        self.console.open(conn, vm);
        if !self.console_open() {
            self.nav.push_by_tag(super::console::ConsolePage::TAG);
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

    fn power(self: &Rc<Self>, uuid: String, action: PowerAction) {
        let Some(conn) = self.current_conn() else { return };
        let name = self.domain_name(&uuid);
        let uuid_for_console = uuid.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn.call(move |c| c.power(&uuid, action)).await;
            let Some(this) = weak.upgrade() else { return };
            match result {
                Err(e) => {
                    this.feedback.error(&format!("Couldn't {} {name}", action.label().to_lowercase()), e.message())
                }
                Ok(()) if action == PowerAction::Start && this.settings.get().console_autoconnect => {
                    this.state.borrow_mut().pending_console = Some(uuid_for_console);
                }
                Ok(()) => {}
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
    fn dev_hooks(&self) {
        if let Ok(name) = std::env::var("VMNG_DEV_SELECT") {
            let uuid = self.state.borrow().domains.iter().find(|d| d.config.name == name).map(|d| d.uuid.clone());
            if let Some(uuid) = uuid {
                self.vm_list.select(&uuid);
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
            (st.current == ConnectionKind::Session, st.host_info.clone(), st.domains.clone())
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
        super::new_vm::open(&self.window, ctx, move |uuid, customize| {
            if let Some(this) = weak.upgrade() {
                {
                    let mut st = this.state.borrow_mut();
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

    fn vm_by_uuid(&self, uuid: &str) -> Option<DomainSummary> {
        self.state.borrow().domains.iter().find(|d| d.uuid == uuid).cloned()
    }

    /// Handles a `vm.*` action for the VM `uuid`.
    fn vm_action(self: &Rc<Self>, uuid: String, action: VmAction) {
        let Some(vm) = self.vm_by_uuid(&uuid) else { return };
        let Some(conn) = self.current_conn() else { return };
        match action {
            VmAction::Power(p) => self.request_power(uuid, p),
            VmAction::Console => self.open_console(&vm),
            VmAction::Hardware => self.open_vm_page(&vm, VmTab::Hardware),
            VmAction::Snapshots => self.open_vm_page(&vm, VmTab::Snapshots),
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
