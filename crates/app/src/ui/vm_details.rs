//! The details panel next to the VM list: header with actions, console
//! preview, live graphs, facts and storage usage.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;
use vmng_virt::{DiskConfig, DomainState, DomainSummary, Firmware, GuestDetails};

use super::vm_actions::VmButtons;
use super::vm_list::{logo_key, os_label, state_text};
use super::widgets::{
    Meter, OsLogo, Sparkline, StatusLabel, Tone, boxed_list, card, fact_row, labeled_button, reflow, section_heading,
};
use crate::backend::{HISTORY_LEN, VmLive};
use crate::format;

struct StatCard {
    root: gtk::Box,
    value: gtk::Label,
    sub: gtk::Label,
    graph: Sparkline,
}

impl StatCard {
    fn new(title: &str, metric: &str) -> Self {
        let label =
            gtk::Label::builder().label(title).xalign(0.0).hexpand(true).css_classes(["caption", "dim-label"]).build();
        let value = gtk::Label::builder().xalign(1.0).css_classes(["stat-value", "numeric"]).build();
        let head = gtk::Box::builder().spacing(8).build();
        head.append(&label);
        head.append(&value);
        let graph = Sparkline::new(metric, HISTORY_LEN);
        let sub = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["caption", "dim-label", "numeric"])
            .build();
        let root = card(gtk::Orientation::Vertical, 6);
        root.add_css_class("stat-card");
        root.set_hexpand(true);
        root.append(&head);
        root.append(&graph);
        root.append(&sub);
        root.update_property(&[gtk::accessible::Property::Label(title)]);
        Self { root, value, sub, graph }
    }

    fn set(&self, value: &str, sub: &str, active: bool) {
        self.value.set_label(value);
        self.sub.set_label(sub);
        if active {
            self.graph.remove_css_class("inactive");
        } else {
            self.graph.add_css_class("inactive");
        }
    }
}

type AutostartFn = Rc<dyn Fn(&str, bool)>;

pub struct VmDetails {
    pub root: adw::BreakpointBin,
    uuid: RefCell<Option<String>>,
    logo: OsLogo,
    name: gtk::Label,
    status: StatusLabel,
    buttons: VmButtons,
    preview: gtk::Stack,
    preview_frame: gtk::Box,
    screen: gtk::Picture,
    preview_caption: gtk::Label,
    cpu: StatCard,
    memory: StatCard,
    disk: StatCard,
    net: StatCard,
    ip: gtk::Label,
    hostname: gtk::Label,
    agent: gtk::Label,
    guest_os_row: adw::ActionRow,
    guest_os: gtk::Label,
    users_row: adw::ActionRow,
    users: gtk::Label,
    guest_fs_heading: gtk::Label,
    guest_fs: gtk::Box,
    firmware: gtk::Label,
    network: gtk::Label,
    machine: gtk::Label,
    autostart: adw::SwitchRow,
    autostart_guard: RefCell<bool>,
    /// Which graphs are collected (CPU, memory, disk, network).
    collect: std::cell::Cell<[bool; 4]>,
    storage: gtk::Box,
    /// Built for the current disk count; `None` until first shown.
    storage_meters: RefCell<Option<Vec<Meter>>>,
    on_autostart: RefCell<Option<AutostartFn>>,
}

impl std::fmt::Debug for VmDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VmDetails").field("uuid", &self.uuid).finish_non_exhaustive()
    }
}

impl VmDetails {
    pub fn new() -> Rc<Self> {
        // Header.
        let logo = OsLogo::new(56);
        let name = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["title-2"])
            .build();
        let status = StatusLabel::new();
        status.root.add_css_class("dim-label");
        let titles = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .spacing(2)
            .build();
        titles.append(&name);
        titles.append(&status.root);

        let buttons = VmButtons::new();
        let identity = gtk::Box::builder().spacing(16).hexpand(true).build();
        identity.append(&logo);
        identity.append(&titles);
        let header = gtk::Box::builder().spacing(16).css_classes(["details-header"]).build();
        header.append(&identity);
        header.append(&buttons.root);

        // Console preview.
        let preview = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
        let placeholder = |icon: &str, text: &str| {
            let b = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(10)
                .valign(gtk::Align::Center)
                .halign(gtk::Align::Center)
                .css_classes(["dim-label"])
                .build();
            b.append(&gtk::Image::builder().icon_name(icon).pixel_size(32).build());
            b.append(&gtk::Label::new(Some(text)));
            b
        };
        preview.add_named(&placeholder("system-shutdown-symbolic", "This machine is shut off"), Some("off"));
        preview.add_named(&placeholder("media-playback-pause-symbolic", "Paused"), Some("paused"));
        preview.add_named(&placeholder("video-display-symbolic", "Running"), Some("running"));
        // A live picture of the screen. It sits in an overlay so it fills
        // the preview box without deciding its size (a picture's size
        // follows its image, which would reflow the whole panel).
        let screen = gtk::Picture::builder().content_fit(gtk::ContentFit::Contain).can_shrink(true).build();
        screen.update_property(&[gtk::accessible::Property::Label("Screen preview")]);
        let screen_box = gtk::Overlay::builder().child(&gtk::Box::new(gtk::Orientation::Vertical, 0)).build();
        screen_box.add_overlay(&screen);
        preview.add_named(&screen_box, Some("screen"));
        let preview_caption = gtk::Label::builder().xalign(0.0).css_classes(["caption", "dim-label"]).build();
        let preview_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).hexpand(true).build();
        let frame = gtk::Box::builder().css_classes(["console-preview"]).build();
        preview.set_hexpand(true);
        frame.append(&preview);
        // Clicking a running machine's preview opens its console.
        let click = gtk::GestureClick::new();
        let p = preview.clone();
        click.connect_released(move |g, _, _, _| {
            if matches!(p.visible_child_name().as_deref(), Some("screen" | "running"))
                && let Some(w) = g.widget()
            {
                let _ = w.activate_action("vm.console", None);
            }
        });
        frame.add_controller(click);
        preview_box.append(&frame);
        preview_box.append(&preview_caption);

        // Stats.
        let cpu = StatCard::new("CPU", "cpu");
        let memory = StatCard::new("Memory", "memory");
        let disk = StatCard::new("Disk I/O", "disk");
        let net = StatCard::new("Network", "network");
        // Two by two, or one column when there's no room.
        let stats = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .min_children_per_line(1)
            .max_children_per_line(2)
            .row_spacing(10)
            .column_spacing(10)
            .valign(gtk::Align::Start)
            .build();
        for card in [&cpu.root, &memory.root, &disk.root, &net.root] {
            stats.append(card);
            if let Some(cell) = card.parent() {
                cell.set_focusable(false);
            }
        }
        stats.set_hexpand(true);

        let top = gtk::Box::builder().spacing(16).homogeneous(true).build();
        top.append(&preview_box);
        top.append(&stats);

        // Facts.
        let facts = boxed_list();
        let (row, ip) = fact_row("IP address");
        facts.append(&row);
        let (row, hostname) = fact_row("Hostname");
        facts.append(&row);
        let (row, agent) = fact_row("Guest agent");
        facts.append(&row);
        // Only shown when the guest agent reports them.
        let (guest_os_row, guest_os) = fact_row("Guest system");
        guest_os_row.set_visible(false);
        facts.append(&guest_os_row);
        let (users_row, users) = fact_row("Signed in");
        users_row.set_visible(false);
        facts.append(&users_row);
        let (row, firmware) = fact_row("Firmware");
        facts.append(&row);
        let (row, network) = fact_row("Network");
        facts.append(&row);
        let (row, machine) = fact_row("Chipset");
        facts.append(&row);
        let autostart = adw::SwitchRow::builder().title("Start at boot").subtitle("When the host starts").build();
        facts.append(&autostart);

        let details_col = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).hexpand(true).build();
        details_col.append(&section_heading("Details"));
        details_col.append(&facts);

        let storage = card(gtk::Orientation::Vertical, 0);
        let storage_col = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).hexpand(true).build();
        storage_col.append(&section_heading("Storage"));
        storage_col.append(&storage);
        let guest_fs_heading = section_heading("Filesystems in the guest");
        guest_fs_heading.set_visible(false);
        let guest_fs = card(gtk::Orientation::Vertical, 0);
        guest_fs.set_visible(false);
        storage_col.append(&guest_fs_heading);
        storage_col.append(&guest_fs);
        let pages = gtk::Box::builder().spacing(8).margin_top(4).build();
        let hardware = labeled_button("emblem-system-symbolic", "Hardware");
        hardware.set_action_name(Some("vm.hardware"));
        let snapshots = labeled_button("camera-photo-symbolic", "Snapshots");
        snapshots.set_action_name(Some("vm.snapshots"));
        pages.append(&hardware);
        pages.append(&snapshots);
        storage_col.append(&pages);

        let bottom = gtk::Box::builder().spacing(16).homogeneous(true).valign(gtk::Align::Start).build();
        bottom.append(&details_col);
        bottom.append(&storage_col);

        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(20)
            .css_classes(["details-panel"])
            .build();
        body.append(&header);
        body.append(&top);
        body.append(&bottom);
        let clamp = adw::Clamp::builder().maximum_size(1100).tightening_threshold(800).child(&body).build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .child(&clamp)
            .build();
        let root = reflow(&scroller, &[&header, &top, &bottom], &[&stats], 880.0);

        let this = Rc::new(Self {
            root,
            uuid: RefCell::default(),
            logo,
            name,
            status,
            buttons,
            preview,
            preview_frame: frame,
            screen,
            preview_caption,
            cpu,
            memory,
            disk,
            net,
            ip,
            hostname,
            agent,
            guest_os_row,
            guest_os,
            users_row,
            users,
            guest_fs_heading,
            guest_fs,
            firmware,
            network,
            machine,
            autostart,
            autostart_guard: RefCell::new(false),
            collect: std::cell::Cell::new([true; 4]),
            storage,
            storage_meters: RefCell::default(),
            on_autostart: RefCell::default(),
        });

        let weak = Rc::downgrade(&this);
        this.autostart.connect_active_notify(move |row| {
            let Some(this) = weak.upgrade() else { return };
            if *this.autostart_guard.borrow() {
                return;
            }
            let uuid = this.uuid.borrow().clone();
            let cb = this.on_autostart.borrow().clone();
            if let (Some(uuid), Some(f)) = (uuid, cb) {
                f(&uuid, row.is_active());
            }
        });
        this
    }

    /// Which statistics are collected (from Preferences).
    pub fn set_collect(&self, collect: [bool; 4]) {
        self.collect.set(collect);
    }

    pub fn connect_autostart(&self, f: impl Fn(&str, bool) + 'static) {
        self.on_autostart.replace(Some(Rc::new(f)));
    }

    /// Shows a new picture of `uuid`'s screen (ignored if another VM is
    /// shown by now).
    pub fn set_screen(&self, uuid: &str, texture: &gdk::Texture) {
        if self.uuid.borrow().as_deref() != Some(uuid) {
            return;
        }
        self.screen.set_paintable(Some(texture));
        if self.preview.visible_child_name().as_deref() == Some("running") {
            self.preview.set_visible_child_name("screen");
        }
    }

    /// Shows a VM. `live` and `guest` may be missing while data loads.
    pub fn show(&self, d: &DomainSummary, live: Option<&VmLive>, guest: Option<&GuestDetails>) {
        let changed_vm = self.uuid.borrow().as_deref() != Some(d.uuid.as_str());
        self.uuid.replace(Some(d.uuid.clone()));
        let os = os_label(d);
        self.logo.set_logo(logo_key(d));
        self.name.set_label(d.display_name());
        self.status.set(Tone::of(d.state), &format!("{} · {os}", state_text(d)));

        let s = d.state;
        let off = matches!(s, DomainState::ShutOff | DomainState::Crashed);
        self.buttons.update(s);

        if changed_vm {
            self.screen.set_paintable(None::<&gdk::Paintable>);
        }
        let has_screen = self.screen.paintable().is_some();
        let child = match s {
            _ if off => "off",
            DomainState::Paused | DomainState::Suspended => "paused",
            _ if has_screen => "screen",
            _ => "running",
        };
        self.preview.set_visible_child_name(child);
        let clickable = matches!(child, "screen" | "running");
        self.preview_frame.set_cursor_from_name(clickable.then_some("pointer"));
        self.preview_frame.set_tooltip_text(clickable.then_some("Open Console"));
        let graphics = d.config.graphics.first();
        self.preview_caption.set_label(&match graphics {
            Some(g) => match (g.port, s.is_active()) {
                (Some(port), true) => format!("{} · port {port}", g.kind.to_uppercase()),
                _ => g.kind.to_uppercase(),
            },
            None => "No graphical display".to_owned(),
        });

        self.show_stats(d, live);
        self.show_facts(d, guest, changed_vm);
        self.show_storage(d, live);
    }

    fn show_stats(&self, d: &DomainSummary, live: Option<&VmLive>) {
        let active = d.state.is_active();
        let running = d.state == DomainState::Running;
        let vcpu_text = format!("{} vCPU", d.config.vcpus);
        match live.and_then(|l| l.cpu_pct).filter(|_| running) {
            Some(p) => self.cpu.set(&format::percent(p), &vcpu_text, true),
            None => self.cpu.set("—", &vcpu_text, false),
        }
        let mem_total = format::bytes(d.config.memory);
        match live.and_then(|l| l.mem_used).filter(|_| active) {
            Some(used) => self.memory.set(&format::bytes(used), &format!("of {mem_total}"), true),
            None => self.memory.set("—", &format!("{mem_total} assigned"), false),
        }
        if let Some(l) = live.filter(|_| running) {
            self.disk.set(
                &format::rate(l.disk_read + l.disk_write),
                &format!("read {} · write {}", format::rate(l.disk_read), format::rate(l.disk_write)),
                true,
            );
            self.net.set(
                &format::rate(l.net_rx + l.net_tx),
                &format!("in {} · out {}", format::rate(l.net_rx), format::rate(l.net_tx)),
                true,
            );
        } else {
            self.disk.set("—", "", false);
            self.net.set("—", "", false);
        }

        // Statistics turned off in Preferences say so instead of "—".
        let [c_cpu, c_mem, c_disk, c_net] = self.collect.get();
        for (on, card) in [(c_cpu, &self.cpu), (c_mem, &self.memory), (c_disk, &self.disk), (c_net, &self.net)] {
            if !on {
                card.set("Off", "Turned off in Preferences", false);
            }
        }
        let empty = Vec::<f64>::new();
        let (cpu, mem, disk, net) = match live.filter(|_| active) {
            Some(l) => (
                l.cpu.values().collect(),
                l.mem.values().collect(),
                l.disk.values().collect(),
                l.net.values().collect(),
            ),
            None => (empty.clone(), empty.clone(), empty.clone(), empty),
        };
        self.cpu.graph.set_max(Some(100.0));
        self.cpu.graph.set_values(if c_cpu { cpu } else { Vec::new() });
        self.memory.graph.set_max(Some(d.config.memory.max(1) as f64));
        self.memory.graph.set_values(if c_mem { mem } else { Vec::new() });
        // I/O graphs scale to their own peak, with a floor so idle noise
        // stays flat.
        let peak = |v: &Vec<f64>| Some(v.iter().copied().fold(64.0 * 1024.0, f64::max));
        self.disk.graph.set_max(peak(&disk));
        self.disk.graph.set_values(if c_disk { disk } else { Vec::new() });
        self.net.graph.set_max(peak(&net));
        self.net.graph.set_values(if c_net { net } else { Vec::new() });
    }

    fn show_facts(&self, d: &DomainSummary, guest: Option<&GuestDetails>, changed_vm: bool) {
        let active = d.state.is_active();
        let dash = "—";
        let (ip, host, agent) = match guest.filter(|_| active) {
            Some(g) => (
                g.addresses.first().map_or(dash.to_owned(), Clone::clone),
                g.hostname.clone().unwrap_or_else(|| dash.to_owned()),
                if g.agent_connected {
                    "Connected"
                } else if d.config.has_guest_agent_channel {
                    "Not responding"
                } else {
                    "Not configured"
                },
            ),
            None if !active => (
                dash.into(),
                dash.into(),
                if d.config.has_guest_agent_channel { "Available when running" } else { "Not configured" },
            ),
            None => (dash.into(), dash.into(), "Checking…"),
        };
        self.ip.set_label(&ip);
        self.hostname.set_label(&host);
        self.agent.set_label(agent);
        let info = guest.filter(|_| active).and_then(|g| g.info.as_ref());
        let os = info.and_then(|i| i.os_name.clone());
        self.guest_os_row.set_visible(os.is_some());
        self.guest_os.set_label(os.as_deref().unwrap_or_default());
        let users = info.map(|i| i.users.join(", ")).unwrap_or_default();
        self.users_row.set_visible(info.is_some());
        self.users.set_label(if users.is_empty() { "Nobody" } else { &users });
        self.show_guest_filesystems(info.map(|i| i.filesystems.as_slice()).unwrap_or_default());
        self.firmware.set_label(match d.config.firmware {
            Some(Firmware::Uefi { secure_boot: true }) => "UEFI · Secure Boot",
            Some(Firmware::Uefi { secure_boot: false }) => "UEFI",
            Some(Firmware::Bios) => "BIOS",
            None => dash,
        });
        let nets: Vec<String> = d
            .config
            .interfaces
            .iter()
            .map(|i| match (i.kind.as_str(), i.source.as_deref()) {
                ("network", Some(n)) => n.to_owned(),
                ("bridge", Some(b)) => format!("{b} (bridge)"),
                ("direct", Some(dev)) => format!("{dev} (macvtap)"),
                ("user", _) => "User networking".to_owned(),
                (kind, src) => src.unwrap_or(kind).to_owned(),
            })
            .collect();
        self.network.set_label(&if nets.is_empty() { "None".to_owned() } else { nets.join(", ") });
        let machine = d.config.machine.as_deref().unwrap_or_default();
        let chipset = if machine.contains("q35") {
            "Q35"
        } else if machine.contains("i440fx") || machine.starts_with("pc") {
            "i440FX"
        } else if machine.is_empty() {
            dash
        } else {
            machine
        };
        let arch = d.config.arch.as_deref().unwrap_or_default();
        self.machine.set_label(&if arch.is_empty() { chipset.to_owned() } else { format!("{chipset} · {arch}") });

        if changed_vm || self.autostart.is_active() != d.autostart {
            self.autostart_guard.replace(true);
            self.autostart.set_active(d.autostart);
            self.autostart_guard.replace(false);
        }
        self.autostart.set_sensitive(d.persistent);
    }

    fn show_guest_filesystems(&self, filesystems: &[vmng_virt::GuestFilesystem]) {
        while let Some(c) = self.guest_fs.first_child() {
            self.guest_fs.remove(&c);
        }
        self.guest_fs.set_visible(!filesystems.is_empty());
        self.guest_fs_heading.set_visible(!filesystems.is_empty());
        for (i, f) in filesystems.iter().enumerate() {
            if i > 0 {
                self.guest_fs.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            }
            let m = Meter::new(&f.mountpoint);
            m.root.set_margin_start(16);
            m.root.set_margin_end(16);
            m.root.set_margin_top(12);
            m.root.set_margin_bottom(12);
            match (f.used, f.total) {
                (Some(u), Some(t)) if t > 0 => m.set(u as f64 / t as f64, &format::used_of(u, t)),
                _ => m.set(0.0, "—"),
            }
            // Read-only media (live CDs) are always "full"; don't warn.
            m.set_warning_allowed(!matches!(f.fstype.as_str(), "iso9660" | "erofs" | "squashfs" | "udf"));
            m.set_caption(Some(&f.fstype).filter(|t| !t.is_empty()).map(String::as_str));
            self.guest_fs.append(&m.root);
        }
    }

    fn show_storage(&self, d: &DomainSummary, live: Option<&VmLive>) {
        let disks: Vec<&DiskConfig> = d.config.disks.iter().filter(|x| !x.is_cdrom()).collect();
        let mut built = self.storage_meters.borrow_mut();
        if built.as_ref().is_none_or(|m| m.len() != disks.len()) {
            while let Some(child) = self.storage.first_child() {
                self.storage.remove(&child);
            }
            let mut meters = Vec::with_capacity(disks.len());
            for (i, _) in disks.iter().enumerate() {
                let m = Meter::new("");
                m.root.set_margin_start(16);
                m.root.set_margin_end(16);
                m.root.set_margin_top(12);
                m.root.set_margin_bottom(12);
                if i > 0 {
                    self.storage.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
                }
                self.storage.append(&m.root);
                meters.push(m);
            }
            if disks.is_empty() {
                let none = gtk::Label::builder()
                    .label("No disks")
                    .xalign(0.0)
                    .margin_start(16)
                    .margin_top(12)
                    .margin_bottom(12)
                    .css_classes(["dim-label"])
                    .build();
                self.storage.append(&none);
            }
            *built = Some(meters);
        }
        for (m, disk) in built.iter().flatten().zip(&disks) {
            let title = disk_title(disk);
            m.set_title(&title);
            let usage = live.and_then(|l| l.disks.iter().find(|b| b.name == disk.target));
            let raw = disk.format.as_deref() == Some("raw");
            m.set_warning_allowed(!raw);
            match usage.and_then(|u| u.used(d.state.is_active()).zip(u.capacity)) {
                Some((alloc, cap)) if cap > 0 => m.set(alloc as f64 / cap as f64, &format::used_of(alloc, cap)),
                _ => m.set(0.0, "—"),
            }
            let mut caption = Vec::new();
            if let Some(f) = &disk.format {
                caption.push(f.clone());
            }
            if let Some((pool, vol)) = &disk.source_volume {
                caption.push(format!("{vol} in {pool}"));
            } else if let Some(p) = &disk.source_path {
                caption.push(p.clone());
            }
            m.set_caption(Some(&caption.join(" · ")).filter(|c| !c.is_empty()).map(String::as_str));
        }
    }
}

/// "VirtIO disk (vda)", "SATA disk (sdb)", …
pub fn disk_title(d: &DiskConfig) -> String {
    let bus = match d.bus.as_deref() {
        Some("virtio") => "VirtIO",
        Some("sata") => "SATA",
        Some("scsi") => "SCSI",
        Some("ide") => "IDE",
        Some("usb") => "USB",
        Some("nvme") => "NVMe",
        Some(other) => other,
        None => "",
    };
    let kind = if d.device == "disk" { "disk" } else { d.device.as_str() };
    let name = if bus.is_empty() { format!("Disk {}", d.target) } else { format!("{bus} {kind}") };
    format!("{name} ({})", d.target)
}
