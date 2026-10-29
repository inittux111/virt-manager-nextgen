//! The New VM wizard: Install → Resources → Storage → Review.

mod create;
mod defaults;
mod install;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use adw::prelude::*;
use gtk::{gio, glib};
use vmng_osinfo::OsInfo;
use vmng_virt::{
    Created, DomainCaps, DomainSummary, FirmwareChoice, GuestFamily, HostCaps, HostInfo, InstallImage, InstallSource,
    NetworkChoice, NetworkInstall, NetworkSummary, NewDisk, NewVm, PoolSummary, unique_name, validate_vm_name,
};

use super::feedback::Feedback;
use super::os_chooser::choose_os;
use super::widgets::{OsLogo, boxed_list, fact_row};
use crate::backend::Conn;
use crate::backend::net_install::{self, TreeInstaller};
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
    Network,
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

/// What the Storage step does for the machine's disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiskMode {
    Create,
    Existing,
    None,
}

const DISK_MODES: [(DiskMode, &str); 3] =
    [(DiskMode::Create, "New disk"), (DiskMode::Existing, "Existing image"), (DiskMode::None, "None")];

/// What a file chooser picks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Browse {
    Media,
    Import,
    ExistingDisk,
}

#[derive(Default)]
struct State {
    step: usize,
    method: Option<Method>,
    /// (label, path) of the installer choices in the combo row.
    media: Vec<(String, String)>,
    import_path: Option<String>,
    /// An existing image used as the new machine's disk (Storage step).
    existing_path: Option<String>,
    /// The installer of the network install tree, once read.
    tree: Option<TreeInstaller>,
    tree_error: Option<String>,
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
    network_group: adw::PreferencesGroup,
    tree_url: adw::EntryRow,
    kernel_args: adw::EntryRow,
    manual_group: adw::PreferencesGroup,
    pxe: adw::SwitchRow,
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
    disk_mode: adw::ComboRow,
    disk_size: adw::SpinRow,
    pool_row: adw::ComboRow,
    existing_row: adw::ActionRow,
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
    on_created: Box<dyn Fn(Created, bool)>,
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

/// Opens the wizard. `on_created(created, customize)` runs after creation.
pub fn open(parent: &impl IsA<gtk::Widget>, ctx: WizardContext, on_created: impl Fn(Created, bool) + 'static) {
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
    fn build(ctx: WizardContext, on_created: Box<dyn Fn(Created, bool)>) -> Rc<Self> {
        // Step indicator.
        let indicator = gtk::Box::builder().spacing(6).css_classes(["step-indicator"]).build();
        let mut steps = Vec::new();
        let mut step_labels = Vec::new();
        for (i, label) in STEPS.iter().enumerate() {
            if i > 0 {
                indicator
                    .append(&gtk::Box::builder().valign(gtk::Align::Center).css_classes(["step-connector"]).build());
            }
            let s = gtk::Box::builder().spacing(6).css_classes(["step"]).build();
            s.append(&gtk::Label::builder().label((i + 1).to_string()).css_classes(["step-number"]).build());
            let l = gtk::Label::builder().label(*label).css_classes(["step-label"]).build();
            s.append(&l);
            step_labels.push(l);
            indicator.append(&s);
            steps.push(s);
        }

        // Install page.
        let (install_page, install) = page("How do you want to install the operating system?");
        let cards = gtk::Box::builder().spacing(12).homogeneous(true).build();
        let media_card = method_card("media-optical-symbolic", "Local ISO or CD-ROM", "Boot an installer image");
        let network_card = method_card("network-server-symbolic", "Network install", "From an HTTP(S) install tree");
        let import_card = method_card("drive-harddisk-symbolic", "Import existing disk", "Use a qcow2 or raw image");
        let manual_card = method_card("document-edit-symbolic", "Manual install", "Empty machine, set up later");
        network_card.set_group(Some(&media_card));
        import_card.set_group(Some(&media_card));
        manual_card.set_group(Some(&media_card));
        for c in [&media_card, &network_card, &import_card, &manual_card] {
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

        let tree_url = adw::EntryRow::builder().title("Install tree address").show_apply_button(true).build();
        tree_url.set_input_purpose(gtk::InputPurpose::Url);
        let kernel_args = adw::EntryRow::builder().title("Extra kernel options (optional)").build();
        let network_group = adw::PreferencesGroup::builder()
            .title("Network install")
            .description(
                "The address of a Fedora, RHEL-like, openSUSE or Debian install tree (…/os/ or …/dists/trixie/).",
            )
            .build();
        network_group.add(&tree_url);
        network_group.add(&kernel_args);
        install.append(&network_group);

        let pxe = adw::SwitchRow::builder()
            .title("Boot from the network (PXE)")
            .subtitle("Starts from a PXE server on the machine's network until a system is on the disk")
            .build();
        let manual_group = adw::PreferencesGroup::new();
        manual_group.add(&pxe);
        install.append(&manual_group);

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
        let firmware = adw::ComboRow::builder()
            .title("Boot firmware")
            .use_subtitle(true)
            .model(&gtk::StringList::new(&["UEFI", "BIOS"]))
            .list_factory(&super::widgets::full_text_list())
            .build();
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
        let disk_mode =
            adw::ComboRow::builder().title("Disk").model(&gtk::StringList::new(&DISK_MODES.map(|(_, l)| l))).build();
        let disk_size = adw::SpinRow::with_range(1.0, 65536.0, 1.0);
        disk_size.set_title("Size (GiB)");
        let pool_row = adw::ComboRow::builder()
            .title("Location")
            .subtitle("The storage pool for the new disk; add a pool under Storage to use another folder or drive")
            .build();
        let existing_row =
            adw::ActionRow::builder().title("Disk image").subtitle("No file chosen").visible(false).build();
        let browse_existing = gtk::Button::builder().label("Browse…").valign(gtk::Align::Center).build();
        existing_row.add_suffix(&browse_existing);
        disk_group.add(&disk_mode);
        disk_group.add(&disk_size);
        disk_group.add(&pool_row);
        disk_group.add(&existing_row);
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
        // Narrow: numbered steps only (the page title says where you are),
        // and the install methods stacked.
        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            600.0,
            adw::LengthUnit::Px,
        ));
        for l in &step_labels {
            narrow.add_setter(l, "visible", Some(&false.to_value()));
        }
        narrow.add_setter(&cards, "orientation", Some(&gtk::Orientation::Vertical.to_value()));
        narrow.add_setter(&cards, "homogeneous", Some(&false.to_value()));
        dialog.add_breakpoint(narrow);

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
                (Method::Network, network_card),
                (Method::Import, import_card),
                (Method::Manual, manual_card),
            ],
            media_group,
            media_row,
            import_group,
            import_row,
            network_group,
            tree_url,
            kernel_args,
            manual_group,
            pxe,
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
            disk_mode,
            existing_row,
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
        let weak = Rc::downgrade(&w);
        browse_existing.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.browse(Browse::ExistingDisk);
            }
        });
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
        self.tree_url.connect_apply(move |_| {
            if let Some(w) = weak.upgrade() {
                w.detect_tree();
            }
        });
        let weak = Rc::downgrade(self);
        self.tree_url.connect_changed(move |_| {
            if let Some(w) = weak.upgrade() {
                // A new address has to be read again.
                let stale = {
                    let mut st = w.state.borrow_mut();
                    let stale = st.tree.is_some() || st.tree_error.is_some();
                    st.tree = None;
                    st.tree_error = None;
                    stale
                };
                if stale {
                    w.show_os();
                }
                w.update_buttons();
            }
        });
        let weak = Rc::downgrade(self);
        browse_media.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.browse(Browse::Media);
            }
        });
        let weak = Rc::downgrade(self);
        browse_disk.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.browse(Browse::Import);
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
        self.disk_mode.connect_selected_notify(move |_| {
            if let Some(w) = weak.upgrade() {
                let mode = w.disk_mode();
                w.disk_size.set_visible(mode == DiskMode::Create);
                w.pool_row.set_visible(mode == DiskMode::Create);
                w.existing_row.set_visible(mode == DiskMode::Existing);
                w.update_buttons();
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

    /// Whether the current step is complete.
    fn step_ready(&self) -> bool {
        let st = self.state.borrow();
        match st.step {
            0 => match st.method {
                Some(Method::Media) => !st.detecting && self.media_path().is_some(),
                Some(Method::Import) => st.import_path.is_some(),
                Some(Method::Network) => !st.detecting && st.tree.is_some(),
                Some(Method::Manual) => true,
                None => false,
            },
            2 => match self.disk_mode() {
                DiskMode::Create => !st.pools.is_empty(),
                DiskMode::Existing => st.existing_path.is_some(),
                DiskMode::None => true,
            },
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
}
