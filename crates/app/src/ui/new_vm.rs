//! The New VM wizard: Install → Resources → Storage → Review.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use vmng_osinfo::OsInfo;
use vmng_virt::{
    DomainCaps, DomainSummary, Firmware, GuestFamily, HostCaps, HostInfo, InstallImage, InstallSource, NetworkChoice,
    NetworkSummary, NewDisk, NewVm, PoolSummary, unique_name, validate_vm_name,
};

use super::feedback::Feedback;
use super::os_chooser::choose_os;
use super::widgets::{OsLogo, boxed_list, fact_row};
use crate::backend::Conn;
use crate::settings::Prefs;
use crate::{format, osdb};

const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;
const STEPS: [&str; 4] = ["Install", "Resources", "Storage", "Review"];

/// Everything the wizard needs from the main window.
pub struct WizardContext {
    pub conn: Conn,
    pub session: bool,
    pub prefs: Prefs,
    pub host: Option<HostInfo>,
    pub domains: Vec<DomainSummary>,
    pub feedback: Feedback,
    /// Start on "Import existing disk" instead of the installer.
    pub import: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Method {
    Media,
    Import,
    Manual,
}

/// Facts about the host, loaded when the wizard opens.
struct HostData {
    caps: HostCaps,
    images: Vec<InstallImage>,
    pools: Vec<PoolSummary>,
    networks: Vec<NetworkSummary>,
    bridges: Vec<String>,
}

#[derive(Default)]
struct State {
    step: usize,
    method: Option<Method>,
    /// (label, path) of the installer choices in the combo row.
    media: Vec<(String, String)>,
    import_path: Option<String>,
    os: Option<OsInfo>,
    os_detected: bool,
    detecting: bool,
    detect_seq: u32,
    arches: Vec<String>,
    machines: Vec<String>,
    virt_types: Vec<String>,
    caps: Option<DomainCaps>,
    host_caps: Option<HostCaps>,
    pools: Vec<PoolSummary>,
    networks: Vec<(String, NetworkChoice)>,
    name_edited: bool,
    resources_edited: bool,
    disk_edited: bool,
}

struct Wizard {
    ctx: WizardContext,
    dialog: adw::Dialog,
    stack: gtk::Stack,
    steps: Vec<gtk::Box>,
    back: gtk::Button,
    next: gtk::Button,
    next_label: gtk::Label,
    next_spinner: adw::Spinner,
    // Install.
    method_buttons: Vec<(Method, gtk::ToggleButton)>,
    media_group: adw::PreferencesGroup,
    media_row: adw::ComboRow,
    import_group: adw::PreferencesGroup,
    import_row: adw::ActionRow,
    os_logo: OsLogo,
    os_title: gtk::Label,
    os_subtitle: gtk::Label,
    os_spinner: adw::Spinner,
    os_button: gtk::Button,
    arch_row: adw::ComboRow,
    machine_row: adw::ComboRow,
    virt_row: adw::ComboRow,
    // Resources.
    memory: adw::SpinRow,
    cpus: adw::SpinRow,
    firmware: adw::ComboRow,
    secure_boot: adw::SwitchRow,
    tpm: adw::SwitchRow,
    // Storage.
    disk_group: adw::PreferencesGroup,
    create_disk: adw::SwitchRow,
    disk_size: adw::SpinRow,
    pool_row: adw::ComboRow,
    import_note: adw::PreferencesGroup,
    // Review.
    name: adw::EntryRow,
    name_error: gtk::Label,
    summary: gtk::ListBox,
    network_row: adw::ComboRow,
    customize: adw::SwitchRow,
    state: RefCell<State>,
    /// Set while values are changed by code, so handlers can tell them
    /// apart from changes the person made.
    updating: Cell<bool>,
    on_created: Box<dyn Fn(String, bool)>,
}

fn string_list(items: &[String]) -> gtk::StringList {
    let refs: Vec<&str> = items.iter().map(String::as_str).collect();
    gtk::StringList::new(&refs)
}

fn page(title: &str) -> (gtk::ScrolledWindow, gtk::Box) {
    let body =
        gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(22).css_classes(["wizard-page"]).build();
    body.append(&gtk::Label::builder().label(title).xalign(0.0).wrap(true).css_classes(["title-4"]).build());
    let clamp = adw::Clamp::builder().maximum_size(720).child(&body).build();
    let scroller =
        gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&clamp).build();
    (scroller, body)
}

fn method_card(icon: &str, title: &str, subtitle: &str) -> gtk::ToggleButton {
    let b = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build();
    b.append(
        &gtk::Image::builder()
            .icon_name(icon)
            .pixel_size(22)
            .halign(gtk::Align::Start)
            .css_classes(["method-icon"])
            .build(),
    );
    b.append(&gtk::Label::builder().label(title).xalign(0.0).wrap(true).css_classes(["heading"]).build());
    b.append(
        &gtk::Label::builder().label(subtitle).xalign(0.0).wrap(true).css_classes(["caption", "dim-label"]).build(),
    );
    let t = gtk::ToggleButton::builder().child(&b).hexpand(true).css_classes(["method-card"]).build();
    t.update_property(&[gtk::accessible::Property::Label(title), gtk::accessible::Property::Description(subtitle)]);
    t
}

fn machine_label(m: &str) -> String {
    match m {
        "q35" => "Q35 (modern, recommended)".into(),
        "pc" => "i440FX (for very old systems)".into(),
        "virt" => "virt (recommended)".into(),
        other => other.into(),
    }
}

fn family_of(os: Option<&OsInfo>) -> GuestFamily {
    match os.and_then(|o| o.family.as_deref()) {
        Some(f) if f.starts_with("win") => GuestFamily::Windows,
        Some("linux") => GuestFamily::Linux,
        Some(_) => GuestFamily::Other,
        None => GuestFamily::Linux,
    }
}

/// Windows 11 and Server 2022+ need a TPM; they also expect Secure Boot.
fn needs_tpm(os: Option<&OsInfo>) -> bool {
    os.is_some_and(|o| ["/win/11", "/win/2k22", "/win/2k25"].iter().any(|v| o.id.contains(v)))
}

/// Distributions whose installers boot with Secure Boot enabled.
fn secure_boot_ok(os: Option<&OsInfo>) -> bool {
    const SIGNED: [&str; 11] =
        ["fedora", "rhel", "centos", "almalinux", "rocky", "ubuntu", "debian", "opensuse", "sles", "win", "ol"];
    os.and_then(|o| o.distro.as_deref()).is_some_and(|d| SIGNED.contains(&d))
}

fn default_name(os: Option<&OsInfo>, domains: &[DomainSummary]) -> String {
    let base = os
        .map(|o| o.short_id.trim_end_matches("-unknown").trim_end_matches("unknown").to_owned())
        .filter(|s| !s.is_empty() && validate_vm_name(s).is_ok())
        .unwrap_or_else(|| "vm".into());
    unique_name(&base, |n| domains.iter().any(|d| d.config.name == n))
}

/// Opens the wizard. `on_created(uuid, customize)` runs after creation.
pub fn open(parent: &impl IsA<gtk::Widget>, ctx: WizardContext, on_created: impl Fn(String, bool) + 'static) {
    let w = Wizard::build(ctx, Box::new(on_created));
    // Everything inside holds weak references; the open dialog owns the
    // wizard until it closes.
    let alive = RefCell::new(Some(w.clone()));
    w.dialog.connect_closed(move |_| {
        alive.take();
    });
    w.dialog.present(Some(parent));
    w.load();
}

impl Wizard {
    fn build(ctx: WizardContext, on_created: Box<dyn Fn(String, bool)>) -> Rc<Self> {
        // Step indicator.
        let indicator = gtk::Box::builder().spacing(6).css_classes(["step-indicator"]).build();
        let mut steps = Vec::new();
        for (i, label) in STEPS.iter().enumerate() {
            if i > 0 {
                indicator
                    .append(&gtk::Box::builder().valign(gtk::Align::Center).css_classes(["step-connector"]).build());
            }
            let s = gtk::Box::builder().spacing(6).css_classes(["step"]).build();
            s.append(&gtk::Label::builder().label((i + 1).to_string()).css_classes(["step-number"]).build());
            s.append(&gtk::Label::builder().label(*label).css_classes(["step-label"]).build());
            indicator.append(&s);
            steps.push(s);
        }

        // Install page.
        let (install_page, install) = page("How do you want to install the operating system?");
        let cards = gtk::Box::builder().spacing(12).homogeneous(true).build();
        let media_card = method_card("media-optical-symbolic", "Local ISO or CD-ROM", "Boot an installer image");
        let import_card = method_card("drive-harddisk-symbolic", "Import existing disk", "Use a qcow2 or raw image");
        let manual_card = method_card("document-edit-symbolic", "Manual install", "Empty machine, set up later");
        import_card.set_group(Some(&media_card));
        manual_card.set_group(Some(&media_card));
        for c in [&media_card, &import_card, &manual_card] {
            cards.append(c);
        }
        install.append(&cards);

        let media_row = adw::ComboRow::builder().title("Installer image").build();
        let browse_media = gtk::Button::builder().label("Browse…").valign(gtk::Align::Center).build();
        media_row.add_suffix(&browse_media);
        let media_group = adw::PreferencesGroup::builder().title("Installation media").build();
        media_group.add(&media_row);
        install.append(&media_group);

        let import_row = adw::ActionRow::builder().title("Disk image").subtitle("No file chosen").build();
        let browse_disk = gtk::Button::builder().label("Browse…").valign(gtk::Align::Center).build();
        import_row.add_suffix(&browse_disk);
        let import_group = adw::PreferencesGroup::builder()
            .title("Existing disk")
            .description("The machine boots from this disk. It is used in place, not copied.")
            .build();
        import_group.add(&import_row);
        install.append(&import_group);

        let os_logo = OsLogo::new(40);
        let os_title = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["heading"]).build();
        let os_subtitle = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["caption", "dim-label"]).build();
        let os_text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        os_text.append(&os_title);
        os_text.append(&os_subtitle);
        let os_spinner = adw::Spinner::builder().visible(false).build();
        let os_button = gtk::Button::builder().label("Change").valign(gtk::Align::Center).css_classes(["flat"]).build();
        let os_card = gtk::Box::builder().spacing(12).css_classes(["card", "os-card"]).build();
        os_card.append(&os_logo);
        os_card.append(&os_text);
        os_card.append(&os_spinner);
        os_card.append(&os_button);
        install.append(&os_card);

        let advanced = adw::ExpanderRow::builder().title("Architecture and machine type").build();
        let arch_row = adw::ComboRow::builder().title("Architecture").build();
        let machine_row = adw::ComboRow::builder().title("Machine type").build();
        let virt_row = adw::ComboRow::builder().title("Virtualization").build();
        advanced.add_row(&arch_row);
        advanced.add_row(&machine_row);
        advanced.add_row(&virt_row);
        let advanced_list = boxed_list();
        advanced_list.append(&advanced);
        install.append(&advanced_list);

        // Resources page.
        let (resources_page, resources) = page("How much of the host should this machine get?");
        let group = adw::PreferencesGroup::new();
        let memory = adw::SpinRow::with_range(0.5, 1024.0, 0.5);
        memory.set_title("Memory (GiB)");
        memory.set_digits(1);
        let cpus = adw::SpinRow::with_range(1.0, 512.0, 1.0);
        cpus.set_title("Virtual CPUs");
        group.add(&memory);
        group.add(&cpus);
        resources.append(&group);
        let fw_group = adw::PreferencesGroup::builder().title("Firmware").build();
        let firmware =
            adw::ComboRow::builder().title("Boot firmware").model(&gtk::StringList::new(&["UEFI", "BIOS"])).build();
        let secure_boot =
            adw::SwitchRow::builder().title("Secure Boot").subtitle("Only lets signed operating systems start").build();
        let tpm = adw::SwitchRow::builder().title("TPM 2.0").subtitle("Required by Windows 11").build();
        fw_group.add(&firmware);
        fw_group.add(&secure_boot);
        fw_group.add(&tpm);
        resources.append(&fw_group);

        // Storage page.
        let (storage_page, storage) = page("Where should the machine's disk live?");
        let disk_group = adw::PreferencesGroup::new();
        let create_disk = adw::SwitchRow::builder().title("Create a disk").active(true).build();
        let disk_size = adw::SpinRow::with_range(1.0, 65536.0, 1.0);
        disk_size.set_title("Size (GiB)");
        let pool_row = adw::ComboRow::builder().title("Storage pool").build();
        disk_group.add(&create_disk);
        disk_group.add(&disk_size);
        disk_group.add(&pool_row);
        storage.append(&disk_group);
        let import_note = adw::PreferencesGroup::builder()
            .title("Imported disk")
            .description("The machine uses the disk you chose in the first step. No new disk is created.")
            .build();
        storage.append(&import_note);

        // Review page.
        let (review_page, review) = page("Ready to create the machine");
        let name_group = adw::PreferencesGroup::new();
        let name = adw::EntryRow::builder().title("Name").activates_default(true).build();
        name_group.add(&name);
        let name_error =
            gtk::Label::builder().xalign(0.0).visible(false).css_classes(["error", "caption"]).margin_top(6).build();
        name_group.add(&name_error);
        review.append(&name_group);
        let summary = boxed_list();
        review.append(&summary);
        let options = adw::PreferencesGroup::new();
        let network_row = adw::ComboRow::builder().title("Network").build();
        let customize = adw::SwitchRow::builder()
            .title("Customize before installing")
            .subtitle("Opens the machine's hardware page instead of starting the installation")
            .build();
        options.add(&network_row);
        options.add(&customize);
        review.append(&options);

        let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::SlideLeftRight).build();
        let loading = adw::Spinner::builder()
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .width_request(32)
            .height_request(32)
            .build();
        stack.add_named(&loading, Some("loading"));
        for (i, p) in [install_page, resources_page, storage_page, review_page].iter().enumerate() {
            stack.add_named(p, Some(&i.to_string()));
        }

        // Header and footer.
        let header = adw::HeaderBar::builder().title_widget(&indicator).build();
        let cancel = gtk::Button::builder().label("Cancel").build();
        let back = gtk::Button::builder().label("Back").build();
        let next_label = gtk::Label::new(Some("Continue"));
        let next_spinner = adw::Spinner::builder().visible(false).build();
        let next_inner = gtk::Box::builder().spacing(8).build();
        next_inner.append(&next_spinner);
        next_inner.append(&next_label);
        let next = gtk::Button::builder().child(&next_inner).css_classes(["suggested-action"]).build();
        let footer = gtk::Box::builder().spacing(10).css_classes(["wizard-footer"]).build();
        footer.append(&cancel);
        footer.append(&gtk::Box::builder().hexpand(true).build());
        footer.append(&back);
        footer.append(&next);

        let toolbar = adw::ToolbarView::builder().content(&stack).bottom_bar_style(adw::ToolbarStyle::Raised).build();
        toolbar.add_top_bar(&header);
        toolbar.add_bottom_bar(&footer);
        let dialog = adw::Dialog::builder()
            .title("New Virtual Machine")
            .content_width(820)
            .content_height(660)
            .child(&toolbar)
            .build();
        dialog.set_default_widget(Some(&next));

        let w = Rc::new(Self {
            ctx,
            dialog,
            stack,
            steps,
            back,
            next,
            next_label,
            next_spinner,
            method_buttons: vec![
                (Method::Media, media_card),
                (Method::Import, import_card),
                (Method::Manual, manual_card),
            ],
            media_group,
            media_row,
            import_group,
            import_row,
            os_logo,
            os_title,
            os_subtitle,
            os_spinner,
            os_button,
            arch_row,
            machine_row,
            virt_row,
            memory,
            cpus,
            firmware,
            secure_boot,
            tpm,
            disk_group,
            create_disk,
            disk_size,
            pool_row,
            import_note,
            name,
            name_error,
            summary,
            network_row,
            customize,
            state: RefCell::default(),
            updating: Cell::new(false),
            on_created,
        });
        w.wire(&cancel, &browse_media, &browse_disk);
        w
    }

    fn wire(self: &Rc<Self>, cancel: &gtk::Button, browse_media: &gtk::Button, browse_disk: &gtk::Button) {
        let d = self.dialog.clone();
        cancel.connect_clicked(move |_| {
            d.close();
        });
        let weak = Rc::downgrade(self);
        self.back.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                let step = w.state.borrow().step;
                w.go(step.saturating_sub(1));
            }
        });
        let weak = Rc::downgrade(self);
        self.next.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.advance();
            }
        });
        for (method, button) in &self.method_buttons {
            let (weak, method) = (Rc::downgrade(self), *method);
            button.connect_toggled(move |b| {
                if b.is_active()
                    && let Some(w) = weak.upgrade()
                {
                    w.set_method(method);
                }
            });
        }
        let weak = Rc::downgrade(self);
        self.media_row.connect_selected_notify(move |_| {
            if let Some(w) = weak.upgrade()
                && !w.updating.get()
            {
                w.detect();
            }
        });
        let weak = Rc::downgrade(self);
        browse_media.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.browse(true);
            }
        });
        let weak = Rc::downgrade(self);
        browse_disk.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.browse(false);
            }
        });
        let weak = Rc::downgrade(self);
        self.os_button.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.pick_os();
            }
        });
        for row in [&self.arch_row, &self.machine_row, &self.virt_row] {
            let weak = Rc::downgrade(self);
            let is_arch = row == &self.arch_row;
            row.connect_selected_notify(move |_| {
                if let Some(w) = weak.upgrade()
                    && !w.updating.get()
                {
                    if is_arch {
                        w.fill_machines();
                    }
                    w.load_domain_caps();
                }
            });
        }
        let weak = Rc::downgrade(self);
        let mark_resources = move || {
            if let Some(w) = weak.upgrade()
                && !w.updating.get()
            {
                w.state.borrow_mut().resources_edited = true;
            }
        };
        let m = mark_resources.clone();
        self.memory.connect_value_notify(move |_| m());
        self.cpus.connect_value_notify(move |_| mark_resources());
        let weak = Rc::downgrade(self);
        self.disk_size.connect_value_notify(move |_| {
            if let Some(w) = weak.upgrade()
                && !w.updating.get()
            {
                w.state.borrow_mut().disk_edited = true;
            }
        });
        let weak = Rc::downgrade(self);
        self.firmware.connect_selected_notify(move |_| {
            if let Some(w) = weak.upgrade() {
                w.update_firmware_rows();
            }
        });
        let weak = Rc::downgrade(self);
        self.create_disk.connect_active_notify(move |r| {
            if let Some(w) = weak.upgrade() {
                w.disk_size.set_sensitive(r.is_active());
                w.pool_row.set_sensitive(r.is_active());
            }
        });
        let weak = Rc::downgrade(self);
        self.name.connect_changed(move |_| {
            if let Some(w) = weak.upgrade() {
                if !w.updating.get() {
                    w.state.borrow_mut().name_edited = true;
                }
                w.validate_name();
            }
        });
    }

    /// Loads everything the wizard offers from the host.
    fn load(self: &Rc<Self>) {
        self.stack.set_visible_child_name("loading");
        self.update_buttons();
        let conn = self.ctx.conn.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn
                .call(|c| {
                    Ok(HostData {
                        caps: c.host_caps()?,
                        images: c.list_install_images().unwrap_or_default(),
                        pools: c.list_pools()?.into_iter().map(|(p, _)| p).filter(|p| p.active).collect(),
                        networks: c.list_networks().unwrap_or_default(),
                        bridges: c.list_bridges(),
                    })
                })
                .await;
            let Some(w) = weak.upgrade() else { return };
            match result {
                Ok(data) => w.apply_host(data),
                Err(e) => {
                    w.ctx.feedback.error("Couldn't read the host's capabilities", e.message());
                    w.dialog.close();
                }
            }
        });
    }

    fn apply_host(self: &Rc<Self>, data: HostData) {
        self.updating.set(true);
        let mut st = self.state.borrow_mut();

        // Installer images: pools first, then an optical drive if present.
        st.media =
            data.images.iter().map(|i| (format!("{} · {}", i.name, format::bytes(i.size)), i.path.clone())).collect();
        if std::path::Path::new("/dev/sr0").exists() {
            st.media.push(("Optical drive (/dev/sr0)".into(), "/dev/sr0".into()));
        }
        let labels: Vec<String> = st.media.iter().map(|(l, _)| l.clone()).collect();
        self.media_row.set_model(Some(&string_list(&labels)));

        // Architectures: the host's own first.
        let host_arch = if data.caps.host_arch.is_empty() { "x86_64".to_owned() } else { data.caps.host_arch.clone() };
        let mut arches: Vec<String> = data.caps.guests.iter().map(|g| g.arch.clone()).collect();
        arches.sort_by_key(|a| (a != &host_arch, a.clone()));
        self.arch_row.set_model(Some(&string_list(&arches)));
        st.arches = arches;

        // Pools, default pool first.
        let mut pools = data.pools;
        pools.sort_by_key(|p| (p.name != "default", p.name.clone()));
        let pool_labels: Vec<String> =
            pools.iter().map(|p| format!("{} · {} free", p.name, format::bytes(p.available))).collect();
        self.pool_row.set_model(Some(&string_list(&pool_labels)));
        st.pools = pools;

        // Networks: the one most existing VMs use comes first.
        let mut nets: Vec<(String, NetworkChoice)> = Vec::new();
        if self.ctx.session {
            nets.push(("User networking (no setup needed)".into(), NetworkChoice::User));
        }
        for n in data.networks.iter().filter(|n| n.active) {
            nets.push((
                format!("Virtual network “{}” · {}", n.name, n.forward.to_uppercase()),
                NetworkChoice::Network(n.name.clone()),
            ));
        }
        for b in &data.bridges {
            nets.push((format!("Bridge {b} · on your local network"), NetworkChoice::Bridge(b.clone())));
        }
        nets.push(("No network".into(), NetworkChoice::None));
        let usage = |choice: &NetworkChoice| {
            self.ctx
                .domains
                .iter()
                .flat_map(|d| &d.config.interfaces)
                .filter(|i| match choice {
                    NetworkChoice::Network(n) => i.kind == "network" && i.source.as_deref() == Some(n),
                    NetworkChoice::Bridge(b) => i.kind == "bridge" && i.source.as_deref() == Some(b),
                    _ => false,
                })
                .count()
        };
        let best = nets.iter().enumerate().max_by_key(|(i, (_, c))| (usage(c), usize::MAX - i)).map_or(0, |(i, _)| i);
        let labels: Vec<String> = nets.iter().map(|(l, _)| l.clone()).collect();
        self.network_row.set_model(Some(&string_list(&labels)));
        self.network_row.set_selected(best as u32);
        st.networks = nets;
        drop(st);

        if let Some(host) = &self.ctx.host {
            self.memory.set_range(0.5, (host.memory / GIB).max(1) as f64);
            self.cpus.set_range(1.0, f64::from(host.cpus.max(1)));
        }
        self.fill_machines_with(&data.caps);
        {
            let mut st = self.state.borrow_mut();
            st.caps = None;
            st.host_caps = Some(data.caps);
        }
        self.updating.set(false);

        let method = if self.ctx.import {
            Method::Import
        } else if self.state.borrow().media.is_empty() {
            Method::Manual
        } else {
            Method::Media
        };
        if let Some((_, b)) = self.method_buttons.iter().find(|(m, _)| *m == method) {
            b.set_active(true);
        }
        self.set_method(method);
        self.load_domain_caps();
        self.go(0);
        #[cfg(debug_assertions)]
        if let Some(step) = std::env::var("VMNG_DEV_WIZARD_STEP").ok().and_then(|s| s.parse::<usize>().ok()) {
            let weak = Rc::downgrade(self);
            glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
                if let Some(w) = weak.upgrade() {
                    w.go(step.min(STEPS.len() - 1));
                    if std::env::var_os("VMNG_DEV_WIZARD_CHOOSE").is_some() {
                        w.pick_os();
                    }
                    // VMNG_DEV_WIZARD_CREATE=<name>: create without network.
                    if let Ok(name) = std::env::var("VMNG_DEV_WIZARD_CREATE") {
                        w.name.set_text(&name);
                        let none = w.state.borrow().networks.len().saturating_sub(1);
                        w.network_row.set_selected(none as u32);
                        w.advance();
                    }
                }
            });
        }
    }

    fn fill_machines(self: &Rc<Self>) {
        let caps = self.state.borrow().host_caps.clone();
        if let Some(caps) = caps {
            self.fill_machines_with(&caps);
        }
    }

    fn fill_machines_with(&self, caps: &HostCaps) {
        let was = self.updating.replace(true);
        let arch = self.selected_arch();
        let guest = caps.guest(&arch);
        let all = guest.map(|g| g.machines.clone()).unwrap_or_default();
        let mut machines: Vec<String> =
            ["q35", "pc", "virt"].iter().filter(|m| all.iter().any(|a| a == *m)).map(|m| (*m).to_owned()).collect();
        if machines.is_empty() {
            machines = all.into_iter().take(1).collect();
        }
        let labels: Vec<String> = machines.iter().map(|m| machine_label(m)).collect();
        self.machine_row.set_model(Some(&string_list(&labels)));
        self.machine_row.set_selected(0);
        let mut virt = Vec::new();
        if guest.is_some_and(|g| g.kvm) {
            virt.push("kvm".to_owned());
        }
        virt.push("qemu".to_owned());
        let labels: Vec<String> =
            virt.iter()
                .map(|v| {
                    if v == "kvm" {
                        "KVM (hardware accelerated)".to_owned()
                    } else {
                        "QEMU (emulated, slow)".to_owned()
                    }
                })
                .collect();
        self.virt_row.set_model(Some(&string_list(&labels)));
        self.virt_row.set_selected(0);
        let mut st = self.state.borrow_mut();
        st.machines = machines;
        st.virt_types = virt;
        drop(st);
        self.updating.set(was);
    }

    fn selected_arch(&self) -> String {
        let st = self.state.borrow();
        st.arches.get(self.arch_row.selected() as usize).cloned().unwrap_or_else(|| "x86_64".into())
    }

    fn selected_machine(&self) -> Option<String> {
        self.state.borrow().machines.get(self.machine_row.selected() as usize).cloned()
    }

    fn selected_virt(&self) -> String {
        self.state.borrow().virt_types.get(self.virt_row.selected() as usize).cloned().unwrap_or_else(|| "qemu".into())
    }

    /// Fetches firmware/TPM support for the chosen arch and machine.
    fn load_domain_caps(self: &Rc<Self>) {
        let (arch, machine, virt) = (self.selected_arch(), self.selected_machine(), self.selected_virt());
        let conn = self.ctx.conn.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn.call(move |c| c.domain_caps(&arch, machine.as_deref(), &virt)).await;
            let Some(w) = weak.upgrade() else { return };
            w.state.borrow_mut().caps = result.ok();
            w.apply_os_defaults();
        });
    }

    fn set_method(self: &Rc<Self>, method: Method) {
        self.state.borrow_mut().method = Some(method);
        self.media_group.set_visible(method == Method::Media);
        self.import_group.set_visible(method == Method::Import);
        match method {
            Method::Media => self.detect(),
            _ => {
                let mut st = self.state.borrow_mut();
                if st.os_detected {
                    st.os = None;
                    st.os_detected = false;
                }
                st.detecting = false;
                drop(st);
                self.show_os();
            }
        }
        self.update_buttons();
    }

    fn media_path(&self) -> Option<String> {
        let st = self.state.borrow();
        st.media.get(self.media_row.selected() as usize).map(|(_, p)| p.clone())
    }

    /// Detects the OS on the selected installer image.
    fn detect(self: &Rc<Self>) {
        let Some(path) = self.media_path() else {
            self.show_os();
            return;
        };
        let seq = {
            let mut st = self.state.borrow_mut();
            st.detect_seq += 1;
            st.detecting = true;
            if st.os_detected {
                st.os = None;
            }
            st.detect_seq
        };
        self.show_os();
        self.update_buttons();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = osdb::identify_media(path).await;
            let Some(w) = weak.upgrade() else { return };
            let mut st = w.state.borrow_mut();
            if st.detect_seq != seq {
                return;
            }
            st.detecting = false;
            if let Ok(Some(os)) = result {
                st.os = Some(os);
                st.os_detected = true;
            } else if st.os_detected {
                st.os = None;
                st.os_detected = false;
            }
            drop(st);
            w.show_os();
            w.apply_os_defaults();
            w.update_buttons();
        });
    }

    fn show_os(&self) {
        let st = self.state.borrow();
        self.os_spinner.set_visible(st.detecting);
        self.os_button.set_sensitive(!st.detecting);
        if st.detecting {
            self.os_logo.set_logo(None);
            self.os_title.set_label("Detecting the operating system…");
            self.os_subtitle.set_label("Reading the install media");
            self.os_button.set_label("Choose");
            return;
        }
        match &st.os {
            Some(os) => {
                self.os_logo.set_logo(osdb::logo_key(&os.id));
                self.os_title.set_label(&os.name);
                self.os_subtitle.set_label(if st.os_detected {
                    "Detected automatically from the install media"
                } else {
                    "Chosen by you"
                });
                self.os_button.set_label("Change");
            }
            None => {
                self.os_logo.set_logo(None);
                let (title, sub) = match st.method {
                    Some(Method::Media) if self.media_path().is_some() => {
                        ("Couldn't detect the operating system", "Choose it, so the machine gets the right settings")
                    }
                    Some(Method::Import) => {
                        ("Which operating system is on the disk?", "Choose it, so the machine gets the right settings")
                    }
                    _ => ("Operating system", "Choose it, so the machine gets the right settings"),
                };
                self.os_title.set_label(title);
                self.os_subtitle.set_label(sub);
                self.os_button.set_label("Choose…");
            }
        }
    }

    fn pick_os(self: &Rc<Self>) {
        // Start the search from the file name, e.g. "EndeavourOS".
        let query = match self.state.borrow().method {
            Some(Method::Media) => self.media_path(),
            Some(Method::Import) => self.state.borrow().import_path.clone(),
            _ => None,
        }
        .and_then(|p| {
            let file = p.rsplit('/').next()?.to_owned();
            file.split(|c: char| !c.is_alphanumeric()).find(|s| s.len() > 2).map(str::to_owned)
        })
        .filter(|q| osdb::all().any(|o| o.name.to_lowercase().contains(&q.to_lowercase())))
        .unwrap_or_default();
        let weak = Rc::downgrade(self);
        choose_os(&self.dialog, &query, move |os| {
            if let Some(w) = weak.upgrade() {
                {
                    let mut st = w.state.borrow_mut();
                    st.os = Some(os.clone());
                    st.os_detected = false;
                }
                w.show_os();
                w.apply_os_defaults();
                w.update_buttons();
            }
        });
    }

    fn browse(self: &Rc<Self>, media: bool) {
        let filter = gtk::FileFilter::new();
        if media {
            filter.set_name(Some("Disc images"));
            filter.add_suffix("iso");
            filter.add_mime_type("application/x-cd-image");
        } else {
            filter.set_name(Some("Disk images"));
            for s in ["qcow2", "img", "raw", "qcow"] {
                filter.add_suffix(s);
            }
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title(if media { "Choose an Installer Image" } else { "Choose a Disk Image" })
            .filters(&filters)
            .default_filter(&filter)
            .modal(true)
            .build();
        let window = self.dialog.root().and_downcast::<gtk::Window>();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(file) = dialog.open_future(window.as_ref()).await else { return };
            let Some(path) = file.path().map(|p| p.to_string_lossy().into_owned()) else { return };
            let Some(w) = weak.upgrade() else { return };
            if media {
                let index = {
                    let mut st = w.state.borrow_mut();
                    match st.media.iter().position(|(_, p)| *p == path) {
                        Some(i) => i,
                        None => {
                            let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
                            st.media.push((name, path.clone()));
                            st.media.len() - 1
                        }
                    }
                };
                let labels: Vec<String> = w.state.borrow().media.iter().map(|(l, _)| l.clone()).collect();
                w.updating.set(true);
                w.media_row.set_model(Some(&string_list(&labels)));
                w.media_row.set_selected(index as u32);
                w.updating.set(false);
                w.detect();
            } else {
                w.import_row.set_subtitle(&glib::markup_escape_text(&path));
                w.state.borrow_mut().import_path = Some(path);
                w.show_os();
                w.update_buttons();
            }
        });
    }

    /// Applies the chosen OS's recommendations to values the person has
    /// not changed yet.
    fn apply_os_defaults(&self) {
        let st = self.state.borrow();
        let os_owned = st.os.clone();
        let os = os_owned.as_ref();
        let arch = self.selected_arch();
        let res = os.map(|o| o.resources_for(&arch)).unwrap_or_default();
        let windows = family_of(os) == GuestFamily::Windows;
        let caps = st.caps.clone();
        let (resources_edited, disk_edited, name_edited) = (st.resources_edited, st.disk_edited, st.name_edited);
        drop(st);

        self.updating.set(true);
        if !resources_edited {
            let host_mem = self.ctx.host.as_ref().map_or(u64::MAX, |h| h.memory);
            let ram = res
                .ram
                .unwrap_or(if windows { 4 * GIB } else { 2 * GIB })
                .clamp(512 * MIB, host_mem.saturating_sub(GIB).max(512 * MIB));
            self.memory.set_value(ram as f64 / GIB as f64);
            let host_cpus = self.ctx.host.as_ref().map_or(2, |h| h.cpus);
            self.cpus.set_value(f64::from(res.cpus.unwrap_or(2).max(2).min(host_cpus.max(1))));
        }
        if !disk_edited {
            let size = res.storage.unwrap_or(if windows { 64 * GIB } else { 20 * GIB }).max(GIB);
            self.disk_size.set_value((size / GIB) as f64);
        }
        let uefi_ok = caps.as_ref().is_none_or(|c| c.uefi);
        let want_uefi = uefi_ok && (self.ctx.prefs.new_vm_firmware == "uefi" || needs_tpm(os));
        self.firmware.set_selected(if want_uefi { 0 } else { 1 });
        self.firmware.set_sensitive(uefi_ok);
        self.secure_boot.set_active(secure_boot_ok(os));
        self.tpm.set_active(needs_tpm(os));
        self.tpm.set_sensitive(caps.as_ref().is_none_or(|c| c.tpm));
        if !name_edited {
            self.name.set_text(&default_name(os, &self.ctx.domains));
        }
        // Tell the person what the OS asks for and what the host has.
        let who = os.map_or_else(|| "Recommended".to_owned(), |o| format!("Recommended for {}", o.name));
        let host = self.ctx.host.as_ref();
        let mem_hint = match (res.ram, host) {
            (Some(r), Some(h)) => format!("{who}: {} · host has {}", format::bytes(r), format::bytes(h.memory)),
            (Some(r), None) => format!("{who}: {}", format::bytes(r)),
            (None, Some(h)) => format!("Host has {}", format::bytes(h.memory)),
            (None, None) => String::new(),
        };
        self.memory.set_subtitle(&mem_hint);
        let cpu_hint = match (res.cpus, host) {
            (Some(c), Some(h)) => format!("{who}: {c} · host has {}", h.cpus),
            (_, Some(h)) => format!("Host has {} logical CPUs", h.cpus),
            (Some(c), None) => format!("{who}: {c}"),
            (None, None) => String::new(),
        };
        self.cpus.set_subtitle(&cpu_hint);
        self.disk_size.set_subtitle(&res.storage.map(|s| format!("{who}: {}", format::bytes(s))).unwrap_or_default());
        self.updating.set(false);
        self.update_firmware_rows();
    }

    fn update_firmware_rows(&self) {
        let uefi = self.firmware.selected() == 0;
        let secure_ok = self.state.borrow().caps.as_ref().is_none_or(|c| c.secure_boot);
        self.secure_boot.set_sensitive(uefi && secure_ok);
        if !uefi {
            self.secure_boot.set_active(false);
        }
    }

    fn validate_name(&self) -> bool {
        let name = self.name.text().trim().to_owned();
        let error = validate_vm_name(&name).err().or_else(|| {
            self.ctx.domains.iter().any(|d| d.config.name == name).then_some("Another machine already has this name.")
        });
        self.name_error.set_visible(error.is_some());
        self.name_error.set_label(error.unwrap_or_default());
        if error.is_some() {
            self.name.add_css_class("error");
        } else {
            self.name.remove_css_class("error");
        }
        let last = self.state.borrow().step == STEPS.len() - 1;
        if last {
            self.next.set_sensitive(error.is_none());
        }
        error.is_none()
    }

    /// Whether the current step is complete.
    fn step_ready(&self) -> bool {
        let st = self.state.borrow();
        match st.step {
            0 => match st.method {
                Some(Method::Media) => !st.detecting && self.media_path().is_some(),
                Some(Method::Import) => st.import_path.is_some(),
                Some(Method::Manual) => true,
                None => false,
            },
            2 => !self.create_disk.is_active() || !st.pools.is_empty(),
            _ => true,
        }
    }

    fn update_buttons(&self) {
        let step = self.state.borrow().step;
        let loading = self.stack.visible_child_name().is_some_and(|n| n == "loading");
        self.back.set_visible(step > 0 && !loading);
        self.next_label.set_label(if step == STEPS.len() - 1 { "Create" } else { "Continue" });
        self.next.set_sensitive(!loading && self.step_ready());
        for (i, s) in self.steps.iter().enumerate() {
            s.remove_css_class("current");
            s.remove_css_class("done");
            if i == step {
                s.add_css_class("current");
            } else if i < step {
                s.add_css_class("done");
            }
        }
    }

    fn go(self: &Rc<Self>, step: usize) {
        self.state.borrow_mut().step = step;
        if step == 2 {
            let import = self.state.borrow().method == Some(Method::Import);
            self.disk_group.set_visible(!import);
            self.import_note.set_visible(import);
        }
        if step == 3 {
            self.fill_summary();
            self.validate_name();
        }
        self.stack.set_visible_child_name(&step.to_string());
        self.update_buttons();
        if step == 3 {
            self.validate_name();
        }
    }

    fn advance(self: &Rc<Self>) {
        let step = self.state.borrow().step;
        if !self.step_ready() {
            return;
        }
        if step + 1 < STEPS.len() {
            self.go(step + 1);
        } else if self.validate_name() {
            self.create();
        }
    }

    fn fill_summary(&self) {
        while let Some(child) = self.summary.first_child() {
            self.summary.remove(&child);
        }
        let st = self.state.borrow();
        let add = |title: &str, value: String| {
            let (row, label) = fact_row(title);
            label.set_label(&value);
            self.summary.append(&row);
        };
        add("Operating system", st.os.as_ref().map_or_else(|| "Unknown".to_owned(), |o| o.name.clone()));
        add(
            "Install from",
            match st.method {
                Some(Method::Media) => {
                    self.media_path().map(|p| p.rsplit('/').next().unwrap_or(&p).to_owned()).unwrap_or_default()
                }
                Some(Method::Import) => "Existing disk".into(),
                _ => "Nothing (manual)".into(),
            },
        );
        add("Memory", format::bytes((self.memory.value() * GIB as f64) as u64));
        add("Virtual CPUs", format!("{}", self.cpus.value() as u32));
        let disk = match st.method {
            Some(Method::Import) => st.import_path.clone().unwrap_or_default(),
            _ if self.create_disk.is_active() => {
                let pool = st.pools.get(self.pool_row.selected() as usize).map_or("", |p| p.name.as_str());
                format!(
                    "{} {} in {pool}",
                    format::bytes(self.disk_size.value() as u64 * GIB),
                    self.ctx.prefs.new_vm_disk_format
                )
            }
            _ => "None".into(),
        };
        add("Disk", disk);
        let mut fw = if self.firmware.selected() == 0 { "UEFI".to_owned() } else { "BIOS".to_owned() };
        if self.secure_boot.is_active() {
            fw.push_str(" · Secure Boot");
        }
        if self.tpm.is_active() {
            fw.push_str(" · TPM");
        }
        add("Firmware", fw);
    }

    /// Turns the choices into a request.
    fn spec(&self) -> NewVm {
        let st = self.state.borrow();
        let os = st.os.as_ref();
        let family = family_of(os);
        let windows = family == GuestFamily::Windows;
        let graphics = {
            let pref = self.ctx.prefs.new_vm_graphics.clone();
            let supported = st.caps.as_ref().is_none_or(|c| c.graphics_types.contains(&pref));
            if supported { pref } else { "vnc".into() }
        };
        let has_video = |m: &str| st.caps.as_ref().is_none_or(|c| c.video_models.iter().any(|v| v == m));
        let video = if windows {
            if graphics == "spice" && has_video("qxl") { "qxl" } else { "vga" }
        } else if has_video("virtio") {
            "virtio"
        } else {
            "vga"
        };
        let install = match st.method {
            Some(Method::Media) => InstallSource::Media(self.media_path().unwrap_or_default()),
            Some(Method::Import) => InstallSource::ImportDisk(st.import_path.clone().unwrap_or_default()),
            _ => InstallSource::Manual,
        };
        let disk = (st.method != Some(Method::Import) && self.create_disk.is_active())
            .then(|| st.pools.get(self.pool_row.selected() as usize))
            .flatten()
            .map(|p| NewDisk {
                pool: p.name.clone(),
                size: self.disk_size.value() as u64 * GIB,
                format: self.ctx.prefs.new_vm_disk_format.clone(),
            });
        NewVm {
            name: self.name.text().trim().to_owned(),
            os_id: os.map(|o| o.id.clone()),
            family,
            arch: self.selected_arch(),
            machine: self.selected_machine(),
            virt_type: self.selected_virt(),
            memory: ((self.memory.value() * GIB as f64) as u64 / MIB) * MIB,
            vcpus: self.cpus.value() as u32,
            firmware: if self.firmware.selected() == 0 {
                Firmware::Uefi { secure_boot: self.secure_boot.is_active() }
            } else {
                Firmware::Bios
            },
            tpm: self.tpm.is_active(),
            cpu_model: self.ctx.prefs.new_vm_cpu_model.clone(),
            install,
            disk,
            network: st
                .networks
                .get(self.network_row.selected() as usize)
                .map_or(NetworkChoice::None, |(_, c)| c.clone()),
            graphics,
            virtio: !windows,
            video: video.to_owned(),
        }
    }

    fn set_busy(&self, busy: bool) {
        self.next_spinner.set_visible(busy);
        self.next_label.set_label(if busy { "Creating…" } else { "Create" });
        self.next.set_sensitive(!busy);
        self.back.set_sensitive(!busy);
        self.stack.set_sensitive(!busy);
        self.dialog.set_can_close(!busy);
    }

    fn create(self: &Rc<Self>) {
        let spec = self.spec();
        let customize = self.customize.is_active();
        self.set_busy(true);
        let conn = self.ctx.conn.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let s = spec.clone();
            let result = conn.call(move |c| c.create_vm(&s, !customize)).await;
            let Some(w) = weak.upgrade() else { return };
            w.set_busy(false);
            match result {
                Ok(created) => {
                    match &created.start_error {
                        Some(e) => {
                            w.ctx.feedback.error(&format!("Created {}, but couldn't start it", spec.name), e.message())
                        }
                        None if customize => {
                            w.ctx.feedback.toast(&format!("Created {}. Start it when you're ready.", spec.name))
                        }
                        None => w.ctx.feedback.toast(&format!("Created and started {}", spec.name)),
                    }
                    w.dialog.close();
                    (w.on_created)(created.uuid, customize);
                }
                Err(e) => w.ctx.feedback.error(&format!("Couldn't create {}", spec.name), e.message()),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(id: &str, short: &str, family: &str, distro: &str) -> OsInfo {
        OsInfo {
            id: id.into(),
            short_id: short.into(),
            name: short.into(),
            version: None,
            vendor: None,
            family: Some(family.into()),
            distro: Some(distro.into()),
            release_date: None,
            eol_date: None,
            minimum: Vec::new(),
            recommended: Vec::new(),
        }
    }

    #[test]
    fn defaults_follow_the_os() {
        let win11 = os("http://microsoft.com/win/11", "win11", "winnt", "win");
        let fedora = os("http://fedoraproject.org/fedora/unknown", "fedora-unknown", "linux", "fedora");
        let arch = os("http://archlinux.org/archlinux/rolling", "archlinux", "linux", "archlinux");
        assert_eq!(family_of(Some(&win11)), GuestFamily::Windows);
        assert!(needs_tpm(Some(&win11)) && !needs_tpm(Some(&fedora)));
        assert!(secure_boot_ok(Some(&fedora)) && !secure_boot_ok(Some(&arch)));
        assert_eq!(default_name(Some(&fedora), &[]), "fedora");
        assert_eq!(default_name(None, &[]), "vm");
    }
}
