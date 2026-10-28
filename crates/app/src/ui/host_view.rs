//! The Host overview: what the hypervisor host is, how busy it is, and
//! which machines are using it.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use vmng_virt::{DomainSummary, HostInfo};

use super::vm_list::logo_key;
use super::widgets::{Meter, OsLogo, Sparkline, boxed_list, card, fact_row, reflow, section_heading};
use crate::backend::{HISTORY_LEN, HostLive, Monitor};
use crate::format;

struct GraphCard {
    root: gtk::Box,
    value: gtk::Label,
    sub: gtk::Label,
    graph: Sparkline,
}

fn graph_card(title: &str, metric: &str) -> GraphCard {
    let label =
        gtk::Label::builder().label(title).xalign(0.0).hexpand(true).css_classes(["heading", "dim-label"]).build();
    let value = gtk::Label::builder().xalign(1.0).css_classes(["title-2", "numeric"]).build();
    let head = gtk::Box::builder().spacing(8).build();
    head.append(&label);
    head.append(&value);
    let graph = Sparkline::new(metric, HISTORY_LEN);
    graph.set_height_request(70);
    let sub = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["caption", "dim-label"]).build();
    let root = card(gtk::Orientation::Vertical, 10);
    root.add_css_class("host-card");
    root.set_hexpand(true);
    root.append(&head);
    root.append(&graph);
    root.append(&sub);
    GraphCard { root, value, sub, graph }
}

type OpenVm = Rc<dyn Fn(&str)>;

pub struct HostView {
    pub root: adw::BreakpointBin,
    name: gtk::Label,
    specs: gtk::Label,
    cpu: GraphCard,
    memory: GraphCard,
    storage: Meter,
    storage_sub: gtk::Label,
    running: gtk::ListBox,
    running_empty: gtk::Label,
    libvirt: gtk::Label,
    qemu: gtk::Label,
    kvm: gtk::Label,
    uri: gtk::Label,
    on_open: RefCell<Option<OpenVm>>,
}

impl std::fmt::Debug for HostView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostView").finish_non_exhaustive()
    }
}

impl HostView {
    pub fn new() -> Rc<Self> {
        let icon = gtk::Image::builder()
            .icon_name("computer-symbolic")
            .pixel_size(26)
            .valign(gtk::Align::Center)
            .css_classes(["host-icon"])
            .build();
        // Host names have no spaces, so they may break anywhere.
        let name = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .css_classes(["title-1"])
            .build();
        let specs = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["dim-label"]).build();
        let titles =
            gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(3).valign(gtk::Align::Center).build();
        titles.append(&name);
        titles.append(&specs);
        let header = gtk::Box::builder().spacing(16).build();
        header.append(&icon);
        header.append(&titles);

        let cpu = graph_card("CPU", "cpu");
        let memory = graph_card("Memory", "memory");
        let storage = Meter::new("Storage · default pool");
        let storage_sub = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["caption", "dim-label"]).build();
        let storage_card = card(gtk::Orientation::Vertical, 10);
        storage_card.add_css_class("host-card");
        storage_card.set_hexpand(true);
        storage_card.append(&storage.root);
        storage_card.append(&storage_sub);
        let cards = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .min_children_per_line(1)
            .max_children_per_line(3)
            .column_spacing(16)
            .row_spacing(16)
            .build();
        cards.append(&cpu.root);
        cards.append(&memory.root);
        cards.append(&storage_card);

        let running = boxed_list();
        running.set_selection_mode(gtk::SelectionMode::None);
        let running_empty =
            gtk::Label::builder().label("No machines are running.").xalign(0.0).css_classes(["dim-label"]).build();
        let running_col = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).hexpand(true).build();
        running_col.append(&section_heading("Running machines"));
        running_col.append(&running);
        running_col.append(&running_empty);

        let software = boxed_list();
        let (row, libvirt) = fact_row("libvirt");
        software.append(&row);
        let (row, qemu) = fact_row("QEMU");
        software.append(&row);
        let (row, kvm) = fact_row("KVM");
        software.append(&row);
        let (row, osinfo) = fact_row("osinfo-db");
        software.append(&row);
        let (row, uri) = fact_row("Connection");
        software.append(&row);
        let software_col =
            gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).width_request(300).build();
        software_col.append(&section_heading("Software"));
        software_col.append(&software);

        let bottom = gtk::Box::builder().spacing(20).build();
        bottom.append(&running_col);
        bottom.append(&software_col);

        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(24)
            .css_classes(["page-content"])
            .build();
        body.append(&header);
        body.append(&cards);
        body.append(&bottom);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&adw::Clamp::builder().maximum_size(1180).child(&body).build())
            .build();
        let root = reflow(&scroller, &[&bottom], &[], 760.0);

        let osinfo_version = std::fs::read_to_string("/usr/share/osinfo/VERSION")
            .map(|v| v.trim().to_owned())
            .unwrap_or_else(|_| "—".into());
        osinfo.set_label(&osinfo_version);

        Rc::new(Self {
            root,
            name,
            specs,
            cpu,
            memory,
            storage,
            storage_sub,
            running,
            running_empty,
            libvirt,
            qemu,
            kvm,
            uri,
            on_open: RefCell::default(),
        })
    }

    /// Called with a VM's UUID when a running machine is clicked.
    pub fn connect_open_vm(&self, f: impl Fn(&str) + 'static) {
        self.on_open.replace(Some(Rc::new(f)));
    }

    pub fn update(
        &self,
        info: Option<&HostInfo>,
        kvm: Option<bool>,
        uri: &str,
        monitor: &Monitor,
        domains: &[DomainSummary],
    ) {
        let live: &HostLive = monitor.host();
        if let Some(i) = info {
            self.name.set_label(if i.hostname.is_empty() { "This computer" } else { &i.hostname });
            let model = if i.cpu_model.is_empty() { String::new() } else { format!("{} · ", i.cpu_model) };
            self.specs.set_label(&format!("{model}{} logical CPUs · {} memory", i.cpus, format::bytes(i.memory)));
            self.libvirt.set_label(&i.libvirt_version);
            self.qemu.set_label(&i.hypervisor_version);
        }
        self.kvm.set_label(match kvm {
            Some(true) => "Available",
            Some(false) => "Not available (machines run slowly)",
            None => "—",
        });
        self.uri.set_label(uri);

        let running: Vec<&DomainSummary> = domains.iter().filter(|d| d.state.is_active()).collect();
        self.cpu.value.set_label(&live.cpu_pct.map_or_else(|| "—".to_owned(), format::percent));
        self.cpu.sub.set_label(&match running.len() {
            0 => "No machines running".to_owned(),
            1 => "1 machine running".to_owned(),
            n => format!("{n} machines running"),
        });
        self.cpu.graph.set_max(Some(100.0));
        self.cpu.graph.set_values(live.cpu.values());

        self.memory.value.set_label(&format::bytes(live.mem_used));
        self.memory.sub.set_label(&format!(
            "of {} · {} assigned to running machines",
            format::bytes(live.mem_total),
            format::bytes(live.vm_memory)
        ));
        self.memory.graph.set_max(Some(live.mem_total.max(1) as f64));
        self.memory.graph.set_values(live.mem.values());

        match &live.pool {
            Some(pool) if pool.capacity > 0 => {
                self.storage.set(
                    pool.allocation as f64 / pool.capacity as f64,
                    &format!("{} free", format::bytes(pool.available)),
                );
                self.storage_sub.set_label(&format!(
                    "{} used of {}",
                    format::bytes(pool.allocation),
                    format::bytes(pool.capacity)
                ));
            }
            _ => {
                self.storage.set(0.0, "—");
                self.storage_sub.set_label("There is no storage pool called “default”.");
            }
        }

        while let Some(c) = self.running.first_child() {
            self.running.remove(&c);
        }
        for d in &running {
            let vm = monitor.vm(&d.uuid);
            let logo = OsLogo::new(32);
            logo.set_logo(logo_key(d));
            let cpu =
                vm.and_then(|v| v.cpu_pct).map_or_else(|| "—".to_owned(), |p| format!("CPU {}", format::percent(p)));
            let mem = vm
                .and_then(|v| v.mem_used)
                .map_or_else(|| format::bytes(d.config.memory), |u| format::used_of(u, d.config.memory));
            let row = adw::ActionRow::builder()
                .title(d.display_name())
                .subtitle(format!("{cpu} · {mem}"))
                .activatable(true)
                .build();
            row.add_prefix(&logo);
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            let (uuid, open) = (d.uuid.clone(), self.on_open.borrow().clone());
            row.connect_activated(move |_| {
                if let Some(f) = &open {
                    f(&uuid);
                }
            });
            self.running.append(&row);
        }
        self.running.set_visible(!running.is_empty());
        self.running_empty.set_visible(running.is_empty());
    }
}
