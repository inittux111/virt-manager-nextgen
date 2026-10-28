//! The main window.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::Instant;

use adw::prelude::*;
use gtk::{gio, glib};
use vmng_virt::{ConnectionKind, DomainSummary, GuestDetails, HostInfo, PowerAction};

use super::title_menu::{ConnStatus, TitleMenu, View, connection_name};
use super::vm_details::VmDetails;
use super::vm_list::VmList;
use crate::backend::{Conn, Monitor, Sample};
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
    toasts: adw::ToastOverlay,
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
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.append(&banner);
        body.append(&search_bar);
        body.append(&views);
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&body));

        let toolbar = adw::ToolbarView::builder().content(&toasts).build();
        toolbar.add_top_bar(&header);
        window.set_content(Some(&toolbar));

        let current = ConnectionKind::from_uri(&prefs.last_connection);
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
            toasts,
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
            }),
        });

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
        // Features that arrive in later milestones stay visible but disabled.
        for name in ["new-vm", "import-disk"] {
            let a = gio::SimpleAction::new(name, None);
            a.set_enabled(false);
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
        self.vm_list.connect_action(on_action.clone());
        self.details.connect_action(on_action);
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
        let st = self.state.borrow();
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
            self.vm_list.ensure_selection();
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
        match uuid.and_then(|u| st.domains.iter().find(|d| d.uuid == u)) {
            Some(d) => {
                self.details.show(d, st.monitor.vm(&d.uuid), st.guest.get(&d.uuid));
                self.content.set_visible_child_name("details");
            }
            None => self.content.set_visible_child_name("none"),
        }
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
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn.call(move |c| c.power(&uuid, action)).await;
            let Some(this) = weak.upgrade() else { return };
            if let Err(e) = result {
                this.show_error(&format!("Couldn't {} {name}", action.label().to_lowercase()), e.message());
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
                this.show_error("Couldn't change the start-at-boot setting", e.message());
            }
            this.refresh();
        });
    }

    /// Reports a failed operation: a toast with a button for the details.
    fn show_error(&self, title: &str, details: &str) {
        let toast =
            adw::Toast::builder().title(glib::markup_escape_text(title)).button_label("Details").timeout(8).build();
        let window = self.window.clone();
        let (title, details) = (title.to_owned(), details.to_owned());
        toast.connect_button_clicked(move |_| {
            let dialog = adw::AlertDialog::builder().heading(&title).body(&details).build();
            dialog.add_response("close", "Close");
            dialog.present(Some(&window));
        });
        self.toasts.add_toast(toast);
    }
}
