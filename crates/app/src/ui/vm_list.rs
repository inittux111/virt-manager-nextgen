//! The VM list with its state summary and the compact host summary.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use vmng_virt::{DomainState, DomainSummary, PowerAction};

use super::widgets::{Meter, OsLogo, StatusLabel, Tone, icon_button};
use crate::backend::HostLive;
use crate::{format, osdb};

/// The OS name to show for a VM.
pub fn os_label(d: &DomainSummary) -> String {
    d.config.os_id.as_deref().map_or_else(|| "Unknown OS".to_owned(), osdb::display_name)
}

pub fn logo_key(d: &DomainSummary) -> Option<&'static str> {
    d.config.os_id.as_deref().and_then(osdb::logo_key)
}

/// Width of the list column in pixels.
const LIST_WIDTH: i32 = 340;

struct Row {
    row: gtk::ListBoxRow,
    logo: OsLogo,
    name: gtk::Label,
    status: StatusLabel,
    start: gtk::Button,
    pause: gtk::Button,
    resume: gtk::Button,
    /// Name and state, for search and accessibility.
    search_text: RefCell<String>,
}

type ActionFn = Rc<dyn Fn(&str, PowerAction)>;
type SelectFn = Rc<dyn Fn(Option<&str>)>;

/// The left column of the Machines view.
pub struct VmList {
    pub root: adw::Clamp,
    list: gtk::ListBox,
    summary: gtk::Label,
    rows: RefCell<Vec<(String, Rc<Row>)>>,
    by_row: RefCell<HashMap<gtk::ListBoxRow, String>>,
    filter: Rc<RefCell<String>>,
    on_action: RefCell<Option<ActionFn>>,
    on_select: RefCell<Option<SelectFn>>,
    host: HostSummary,
}

impl std::fmt::Debug for VmList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VmList").finish_non_exhaustive()
    }
}

impl VmList {
    pub fn new() -> Rc<Self> {
        let summary =
            gtk::Label::builder().xalign(0.0).css_classes(["caption", "dim-label", "vm-list-summary"]).build();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .css_classes(["navigation-sidebar", "vm-list"])
            .build();
        list.update_property(&[gtk::accessible::Property::Label("Virtual machines")]);

        let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).margin_top(6).build();
        column.append(&summary);
        column.append(&list);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&column)
            .build();

        let host = HostSummary::new();
        let column_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
        column_box.append(&scroller);
        column_box.append(&host.button);
        // A fixed-width column: long names and captions ellipsize instead
        // of pushing the details panel aside.
        let root = adw::Clamp::builder()
            .maximum_size(LIST_WIDTH)
            .tightening_threshold(LIST_WIDTH)
            .width_request(LIST_WIDTH)
            .hexpand(false)
            .child(&column_box)
            .build();

        let this = Rc::new(Self {
            root,
            list,
            summary,
            rows: RefCell::default(),
            by_row: RefCell::default(),
            filter: Rc::default(),
            on_action: RefCell::default(),
            on_select: RefCell::default(),
            host,
        });

        let weak = Rc::downgrade(&this);
        this.list.connect_row_selected(move |_, row| {
            let Some(this) = weak.upgrade() else { return };
            let uuid = row.and_then(|r| this.by_row.borrow().get(r).cloned());
            let cb = this.on_select.borrow().clone();
            if let Some(f) = cb {
                f(uuid.as_deref());
            }
        });

        let weak = Rc::downgrade(&this);
        this.list.set_filter_func(move |row| {
            let Some(this) = weak.upgrade() else { return true };
            let filter = this.filter.borrow();
            if filter.is_empty() {
                return true;
            }
            let uuid = this.by_row.borrow().get(row).cloned();
            uuid.and_then(|u| this.rows.borrow().iter().find(|(id, _)| *id == u).map(|(_, r)| r.clone()))
                .is_none_or(|r| r.search_text.borrow().contains(filter.as_str()))
        });
        this
    }

    pub fn connect_selected(&self, f: impl Fn(Option<&str>) + 'static) {
        self.on_select.replace(Some(Rc::new(f)));
    }

    pub fn connect_action(&self, f: impl Fn(&str, PowerAction) + 'static) {
        self.on_action.replace(Some(Rc::new(f)));
    }

    pub fn connect_host_clicked(&self, f: impl Fn() + 'static) {
        self.host.button.connect_clicked(move |_| f());
    }

    pub fn set_filter(&self, text: &str) {
        self.filter.replace(text.trim().to_lowercase());
        self.list.invalidate_filter();
    }

    pub fn selected(&self) -> Option<String> {
        let row = self.list.selected_row()?;
        self.by_row.borrow().get(&row).cloned()
    }

    pub fn select(&self, uuid: &str) {
        if let Some((_, r)) = self.rows.borrow().iter().find(|(id, _)| id == uuid) {
            self.list.select_row(Some(&r.row));
        }
    }

    /// Selects the first visible VM if nothing is selected.
    pub fn ensure_selection(&self) {
        if self.list.selected_row().is_none()
            && let Some((_, r)) = self.rows.borrow().first()
        {
            self.list.select_row(Some(&r.row));
        }
    }

    fn action(&self, uuid: &str, action: PowerAction) {
        let cb = self.on_action.borrow().clone();
        if let Some(f) = cb {
            f(uuid, action);
        }
    }

    fn make_row(self: &Rc<Self>, uuid: &str) -> Rc<Row> {
        let logo = OsLogo::new(36);
        let name = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["vm-name"])
            .build();
        let status = StatusLabel::new();
        status.root.add_css_class("caption");
        status.root.add_css_class("dim-label");
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .build();
        text.append(&name);
        text.append(&status.root);

        let start = icon_button("media-playback-start-symbolic", "Start");
        let pause = icon_button("media-playback-pause-symbolic", "Pause");
        let resume = icon_button("media-playback-start-symbolic", "Resume");
        let actions = gtk::Box::builder().spacing(4).css_classes(["row-actions"]).build();
        for b in [&start, &pause, &resume] {
            b.add_css_class("flat");
            b.add_css_class("circular");
            actions.append(b);
        }
        // Quick actions appear on hover and while the row has focus.
        let revealer =
            gtk::Revealer::builder().transition_type(gtk::RevealerTransitionType::Crossfade).child(&actions).build();

        let content = gtk::Box::builder().spacing(12).build();
        content.append(&logo);
        content.append(&text);
        content.append(&revealer);
        let row = gtk::ListBoxRow::builder().child(&content).build();

        let motion = gtk::EventControllerMotion::new();
        let r = revealer.clone();
        motion.connect_enter(move |_, _, _| r.set_reveal_child(true));
        let r = revealer.clone();
        motion.connect_leave(move |_| r.set_reveal_child(false));
        row.add_controller(motion);
        let focus = gtk::EventControllerFocus::new();
        let r = revealer.clone();
        focus.connect_enter(move |_| r.set_reveal_child(true));
        let r = revealer;
        focus.connect_leave(move |_| r.set_reveal_child(false));
        row.add_controller(focus);

        for (button, action) in
            [(&start, PowerAction::Start), (&pause, PowerAction::Pause), (&resume, PowerAction::Resume)]
        {
            let weak = Rc::downgrade(self);
            let uuid = uuid.to_owned();
            button.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.action(&uuid, action);
                }
            });
        }

        Rc::new(Row { row, logo, name, status, start, pause, resume, search_text: RefCell::default() })
    }

    /// Updates the list to match `domains`, reusing rows by UUID.
    pub fn update(self: &Rc<Self>, domains: &[DomainSummary]) {
        let selected = self.selected();
        let same_order = {
            let rows = self.rows.borrow();
            rows.len() == domains.len() && rows.iter().zip(domains).all(|((id, _), d)| *id == d.uuid)
        };
        if !same_order {
            let old: HashMap<String, Rc<Row>> = self.rows.take().into_iter().collect();
            self.by_row.borrow_mut().clear();
            self.list.remove_all();
            let mut rows = Vec::with_capacity(domains.len());
            for d in domains {
                let row = old.get(&d.uuid).cloned().unwrap_or_else(|| self.make_row(&d.uuid));
                self.list.append(&row.row);
                self.by_row.borrow_mut().insert(row.row.clone(), d.uuid.clone());
                rows.push((d.uuid.clone(), row));
            }
            self.rows.replace(rows);
            if let Some(uuid) = selected {
                self.select(&uuid);
            }
        }

        for ((_, row), d) in self.rows.borrow().iter().zip(domains) {
            let name = d.display_name();
            if row.name.label() != name {
                row.name.set_label(name);
            }
            let os = os_label(d);
            row.status.set(Tone::of(d.state), &format!("{} · {os}", state_text(d)));
            row.logo.set_logo(logo_key(d));
            row.start.set_visible(d.state == DomainState::ShutOff || d.state == DomainState::Crashed);
            row.pause.set_visible(d.state == DomainState::Running);
            row.resume.set_visible(d.state == DomainState::Paused);
            row.search_text.replace(format!("{} {} {}", name, os, d.state.label()).to_lowercase());
            row.row.update_property(&[gtk::accessible::Property::Label(&format!("{name}, {}, {os}", state_text(d)))]);
        }

        let count = |pred: fn(DomainState) -> bool| domains.iter().filter(|d| pred(d.state)).count();
        let running = count(|s| matches!(s, DomainState::Running | DomainState::ShuttingDown));
        let paused = count(|s| matches!(s, DomainState::Paused | DomainState::Suspended));
        let off = count(|s| !s.is_active());
        let mut parts = Vec::new();
        if running > 0 {
            parts.push(format!("{running} running"));
        }
        if paused > 0 {
            parts.push(format!("{paused} paused"));
        }
        if off > 0 {
            parts.push(format!("{off} shut off"));
        }
        self.summary.set_label(&parts.join(" · "));
        self.list.invalidate_filter();
    }

    pub fn update_host(&self, info: Option<&vmng_virt::HostInfo>, live: &HostLive) {
        self.host.update(info, live);
    }
}

/// "Saved" is more useful than "Shut off" when a saved state will be restored.
pub fn state_text(d: &DomainSummary) -> &'static str {
    if d.state == DomainState::ShutOff && d.has_managed_save { "Saved" } else { d.state.label() }
}

/// The compact host card below the list.
struct HostSummary {
    button: gtk::Button,
    specs: gtk::Label,
    cpu: Meter,
    memory: Meter,
    storage: Meter,
}

impl HostSummary {
    fn new() -> Self {
        let title = gtk::Box::builder().spacing(8).hexpand(true).build();
        title.append(&gtk::Image::from_icon_name("computer-symbolic"));
        title.append(&gtk::Label::builder().label("Host").css_classes(["heading"]).build());
        let specs = gtk::Label::builder().xalign(1.0).css_classes(["caption", "dim-label"]).build();
        let head = gtk::Box::builder().spacing(8).build();
        head.append(&title);
        head.append(&specs);

        let cpu = Meter::new("CPU");
        let memory = Meter::new("Memory");
        let storage = Meter::new("Storage · default pool");
        for m in [&cpu, &memory, &storage] {
            m.root.add_css_class("caption");
        }

        let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).build();
        body.append(&head);
        body.append(&cpu.root);
        body.append(&memory.root);
        body.append(&storage.root);

        let button = gtk::Button::builder()
            .child(&body)
            .css_classes(["card", "host-summary"])
            .margin_start(12)
            .margin_end(12)
            .margin_bottom(12)
            .margin_top(6)
            .tooltip_text("Open host overview")
            .build();
        button.update_property(&[gtk::accessible::Property::Label("Host resources. Open host overview")]);
        Self { button, specs, cpu, memory, storage }
    }

    fn update(&self, info: Option<&vmng_virt::HostInfo>, live: &HostLive) {
        if let Some(info) = info {
            self.specs.set_label(&format!("{} CPUs · {}", info.cpus, format::bytes(info.memory)));
        }
        match live.cpu_pct {
            Some(p) => self.cpu.set(p / 100.0, &format::percent(p)),
            None => self.cpu.set(0.0, "—"),
        }
        if live.mem_total > 0 {
            self.memory
                .set(live.mem_used as f64 / live.mem_total as f64, &format::used_of(live.mem_used, live.mem_total));
        }
        self.memory.set_caption(
            (live.vm_memory > 0)
                .then(|| format!("{} used by running machines", format::bytes(live.vm_memory)))
                .as_deref(),
        );
        match &live.pool {
            Some(pool) if pool.capacity > 0 => {
                self.storage.root.set_visible(true);
                self.storage.set(
                    pool.allocation as f64 / pool.capacity as f64,
                    &format!("{} free", format::bytes(pool.available)),
                );
            }
            _ => self.storage.root.set_visible(false),
        }
    }
}
