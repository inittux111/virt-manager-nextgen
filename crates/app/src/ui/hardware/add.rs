//! The Add Hardware dialog: device types on the left, a short form on the
//! right, Cancel / Add at the bottom.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use vmng_virt::{disk_prefix, next_disk_target};
use vmng_xml::Element;

use super::editors::Choices;
use crate::backend::Conn;
use crate::format;
use crate::ui::feedback::Feedback;

const GIB: u64 = 1 << 30;

pub struct AddContext {
    pub conn: Conn,
    pub uuid: String,
    pub name: String,
    pub active: bool,
    /// The current configuration (for free disk names and checks).
    pub domain: Element,
    pub choices: Choices,
}

/// What pressing Add does.
enum Plan {
    Device(Element),
    /// Create an empty image, then attach `device` with it as source.
    NewDisk {
        pool: String,
        size: u64,
        format: String,
        device: Element,
    },
    /// virtiofs needs shared memory, which may have to be turned on too.
    SharedFolder(Element),
}

type Builder = Box<dyn Fn() -> Result<Plan, String>>;

struct Page {
    title: &'static str,
    icon: &'static str,
    section: Option<&'static str>,
    widget: gtk::Widget,
    build: Builder,
}

fn el(name: &str) -> Element {
    Element::new(name)
}

fn form(description: &str) -> (gtk::Box, adw::PreferencesGroup) {
    let b = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(18).build();
    let g = adw::PreferencesGroup::builder().description(description).build();
    b.append(&g);
    (b, g)
}

fn choice(title: &str, options: &[(&str, &str)]) -> (adw::ComboRow, Rc<Vec<String>>) {
    let labels: Vec<&str> = options.iter().map(|(_, l)| *l).collect();
    let row = adw::ComboRow::builder().title(title).model(&gtk::StringList::new(&labels)).build();
    (row, Rc::new(options.iter().map(|(v, _)| (*v).to_owned()).collect()))
}

fn value(row: &adw::ComboRow, values: &[String]) -> String {
    values.get(row.selected() as usize).cloned().unwrap_or_default()
}

/// Lets the person pick a file or folder and shows it in `row`.
fn browse_button(row: &adw::ActionRow, path: Rc<RefCell<Option<String>>>, folder: bool) -> gtk::Button {
    let button = gtk::Button::builder().label("Browse…").valign(gtk::Align::Center).build();
    let row = row.clone();
    button.connect_clicked(move |b| {
        let dialog = gtk::FileDialog::builder()
            .title(if folder { "Choose a Folder to Share" } else { "Choose a Disk Image" })
            .modal(true)
            .build();
        let window = b.root().and_downcast::<gtk::Window>();
        let (row, path) = (row.clone(), path.clone());
        glib::spawn_future_local(async move {
            let file = if folder {
                dialog.select_folder_future(window.as_ref()).await
            } else {
                dialog.open_future(window.as_ref()).await
            };
            if let Some(p) = file.ok().and_then(|f| f.path()) {
                let p = p.to_string_lossy().into_owned();
                row.set_subtitle(&glib::markup_escape_text(&p));
                path.replace(Some(p));
            }
        });
    });
    button
}

fn storage_page(ctx: &AddContext) -> Page {
    let (b, g) = form("Add a disk or CD-ROM drive.");
    let create = gtk::ToggleButton::builder()
        .label("Create a new disk")
        .active(true)
        .hexpand(true)
        .css_classes(["method-card"])
        .build();
    let existing = gtk::ToggleButton::builder()
        .label("Use an existing image")
        .hexpand(true)
        .css_classes(["method-card"])
        .group(&create)
        .build();
    let modes = gtk::Box::builder().spacing(10).homogeneous(true).build();
    for t in [&create, &existing] {
        // Two lines rather than overflowing a narrow dialog.
        if let Some(label) = t.child().and_downcast::<gtk::Label>() {
            label.set_wrap(true);
            label.set_justify(gtk::Justification::Center);
        }
        modes.append(t);
    }
    b.prepend(&modes);

    let (device, devices) = choice("Device type", &[("disk", "Disk"), ("cdrom", "CD-ROM")]);
    let size = adw::SpinRow::with_range(1.0, 65536.0, 1.0);
    size.set_title("Size (GiB)");
    size.set_value(20.0);
    let pools: Vec<String> = ctx.choices.pools.iter().map(|p| p.name.clone()).collect();
    let pool_labels: Vec<String> =
        ctx.choices.pools.iter().map(|p| format!("{} · {} free", p.name, format::bytes(p.available))).collect();
    let pool = adw::ComboRow::builder()
        .title("Storage pool")
        .model(&gtk::StringList::new(&pool_labels.iter().map(String::as_str).collect::<Vec<_>>()))
        .build();
    if let Some(i) = pools.iter().position(|p| p == "default") {
        pool.set_selected(i as u32);
    }
    let (fmt, fmts) =
        choice("Format", &[("qcow2", "qcow2 (grows as needed, snapshots)"), ("raw", "raw (fastest, full size)")]);
    let path = Rc::new(RefCell::new(None::<String>));
    let file_row = adw::ActionRow::builder().title("Image file").subtitle("No file chosen").build();
    file_row.add_suffix(&browse_button(&file_row, path.clone(), false));
    let (bus, buses) =
        choice("Bus", &[("virtio", "VirtIO (fastest)"), ("sata", "SATA"), ("scsi", "SCSI"), ("usb", "USB")]);
    for w in [
        device.upcast_ref::<gtk::Widget>(),
        size.upcast_ref(),
        pool.upcast_ref(),
        fmt.upcast_ref(),
        file_row.upcast_ref(),
        bus.upcast_ref(),
    ] {
        g.add(w);
    }

    let sync = {
        let (create, device, size, pool, fmt, file_row) =
            (create.clone(), device.clone(), size.clone(), pool.clone(), fmt.clone(), file_row.clone());
        move || {
            let new = create.is_active();
            let cd = device.selected() == 1;
            size.set_visible(new && !cd);
            pool.set_visible(new && !cd);
            fmt.set_visible(new && !cd);
            file_row.set_visible(!new || cd);
        }
    };
    sync();
    let s = sync.clone();
    create.connect_toggled(move |_| s());
    device.connect_selected_notify(move |_| sync());

    let domain = ctx.domain.clone();
    let build: Builder = Box::new(move || {
        let kind = value(&device, &devices);
        let mut bus_v = value(&bus, &buses);
        if kind == "cdrom" && bus_v == "virtio" {
            bus_v = "sata".into();
        }
        let target = next_disk_target(&domain, disk_prefix(&bus_v));
        let mut d = el("disk").with_attr("type", "file").with_attr("device", &kind);
        let format = if kind == "cdrom" { "raw".to_owned() } else { value(&fmt, &fmts) };
        let mut driver = el("driver").with_attr("name", "qemu");
        if create.is_active() && kind == "disk" {
            driver.set_attr("type", &format);
            d.push(driver);
            d.push(el("target").with_attr("dev", &target).with_attr("bus", &bus_v));
            let pool = pools.get(pool.selected() as usize).cloned().ok_or("Choose a storage pool.")?;
            return Ok(Plan::NewDisk { pool, size: size.value() as u64 * GIB, format, device: d });
        }
        let file = path.borrow().clone();
        if kind == "disk" && file.is_none() {
            return Err("Choose the image file.".into());
        }
        let image_format = match &file {
            Some(f) if f.ends_with(".qcow2") => "qcow2",
            _ => "raw",
        };
        driver.set_attr("type", image_format);
        d.push(driver);
        if let Some(f) = file {
            d.push(el("source").with_attr("file", f));
        }
        d.push(el("target").with_attr("dev", &target).with_attr("bus", &bus_v));
        if kind == "cdrom" {
            d.push(el("readonly"));
        }
        Ok(Plan::Device(d))
    });
    Page {
        title: "Storage",
        icon: "drive-harddisk-symbolic",
        section: Some("Disks & controllers"),
        widget: b.upcast(),
        build,
    }
}

fn controller_page() -> Page {
    let (b, g) = form("Add a USB, SCSI or VirtIO serial controller.");
    let (kind, kinds) = choice(
        "Type",
        &[("usb:qemu-xhci", "USB 3"), ("scsi:virtio-scsi", "SCSI (VirtIO)"), ("virtio-serial:", "VirtIO serial")],
    );
    g.add(&kind);
    let build: Builder = Box::new(move || {
        let v = value(&kind, &kinds);
        let (t, model) = v.split_once(':').unwrap_or((&v, ""));
        let mut c = el("controller").with_attr("type", t);
        if !model.is_empty() {
            c.set_attr("model", model);
        }
        Ok(Plan::Device(c))
    });
    Page { title: "Controller", icon: "drive-multidisk-symbolic", section: None, widget: b.upcast(), build }
}

fn shared_folder_page() -> Page {
    let (b, g) = form("Share a folder of the host with the guest (virtiofs). Shared memory is turned on when needed.");
    let path = Rc::new(RefCell::new(None::<String>));
    let folder = adw::ActionRow::builder().title("Folder on the host").subtitle("No folder chosen").build();
    folder.add_suffix(&browse_button(&folder, path.clone(), true));
    let tag = adw::EntryRow::builder().title("Name in the guest (mount tag)").build();
    g.add(&folder);
    g.add(&tag);
    let build: Builder = Box::new(move || {
        let dir = path.borrow().clone().ok_or("Choose the folder to share.")?;
        let name = tag.text().trim().to_owned();
        let name = if name.is_empty() { dir.rsplit('/').next().unwrap_or("share").to_owned() } else { name };
        if name.contains(char::is_whitespace) {
            return Err("The name can't contain spaces.".into());
        }
        Ok(Plan::SharedFolder(
            el("filesystem")
                .with_attr("type", "mount")
                .with_attr("accessmode", "passthrough")
                .with_child(el("driver").with_attr("type", "virtiofs"))
                .with_child(el("source").with_attr("dir", dir))
                .with_child(el("target").with_attr("dir", name)),
        ))
    });
    Page { title: "Shared folder", icon: "folder-symbolic", section: None, widget: b.upcast(), build }
}

fn network_page(choices: &Choices) -> Page {
    let (b, g) = form("Add a network card.");
    let mut options: Vec<(String, String)> = Vec::new();
    if choices.session {
        options.push(("user:".into(), "User networking".into()));
    }
    for n in &choices.networks {
        options.push((format!("network:{}", n.name), format!("Virtual network “{}”", n.name)));
    }
    for br in &choices.bridges {
        options.push((format!("bridge:{br}"), format!("Bridge {br}")));
    }
    if !choices.session {
        options.push(("user:".into(), "User networking".into()));
    }
    let refs: Vec<(&str, &str)> = options.iter().map(|(v, l)| (v.as_str(), l.as_str())).collect();
    let (source, sources) = choice("Connected to", &refs);
    let (model, models) =
        choice("Model", &[("virtio", "VirtIO (fastest)"), ("e1000e", "Intel e1000e"), ("rtl8139", "Realtek RTL8139")]);
    g.add(&source);
    g.add(&model);
    let build: Builder = Box::new(move || {
        let v = value(&source, &sources);
        let (kind, name) = v.split_once(':').unwrap_or((&v, ""));
        let mut i = el("interface").with_attr("type", kind);
        match kind {
            "network" => i.push(el("source").with_attr("network", name)),
            "bridge" => i.push(el("source").with_attr("bridge", name)),
            _ => &mut i,
        };
        i.push(el("model").with_attr("type", value(&model, &models)));
        Ok(Plan::Device(i))
    });
    Page { title: "Network", icon: "network-wired-symbolic", section: Some("Network"), widget: b.upcast(), build }
}

fn simple_page(
    title: &'static str,
    icon: &'static str,
    section: Option<&'static str>,
    description: &str,
    row_title: &str,
    options: &[(&str, &str)],
    make: impl Fn(&str) -> Element + 'static,
) -> Page {
    let (b, g) = form(description);
    let (row, values) = choice(row_title, options);
    g.add(&row);
    let build: Builder = Box::new(move || Ok(Plan::Device(make(&value(&row, &values)))));
    Page { title, icon, section, widget: b.upcast(), build }
}

fn pages(ctx: &AddContext) -> Vec<Page> {
    vec![
        storage_page(ctx),
        controller_page(),
        shared_folder_page(),
        network_page(&ctx.choices),
        simple_page(
            "Graphics",
            "video-display-symbolic",
            Some("Display & input"),
            "Add a graphical display.",
            "Protocol",
            &[("spice", "SPICE (recommended)"), ("vnc", "VNC")],
            |v| el("graphics").with_attr("type", v).with_attr("autoport", "yes"),
        ),
        simple_page(
            "Video",
            "video-display-symbolic",
            None,
            "Add a video card.",
            "Model",
            &[("virtio", "VirtIO"), ("qxl", "QXL"), ("vga", "VGA"), ("bochs", "Bochs"), ("ramfb", "ramfb")],
            |v| el("video").with_child(el("model").with_attr("type", v)),
        ),
        simple_page(
            "Input",
            "input-keyboard-symbolic",
            None,
            "Add a tablet, keyboard or mouse.",
            "Device",
            &[
                ("tablet:usb", "Tablet (USB, absolute pointer)"),
                ("keyboard:usb", "Keyboard (USB)"),
                ("mouse:usb", "Mouse (USB)"),
                ("keyboard:virtio", "Keyboard (VirtIO)"),
                ("tablet:virtio", "Tablet (VirtIO)"),
            ],
            |v| {
                let (t, bus) = v.split_once(':').unwrap_or((v, "usb"));
                el("input").with_attr("type", t).with_attr("bus", bus)
            },
        ),
        simple_page(
            "Sound",
            "audio-speakers-symbolic",
            None,
            "Add a sound card.",
            "Model",
            &[("ich9", "Intel HDA (ICH9)"), ("ich6", "Intel HDA (ICH6)"), ("ac97", "AC97"), ("usb", "USB audio")],
            |v| el("sound").with_attr("model", v),
        ),
        simple_page(
            "Serial",
            "utilities-terminal-symbolic",
            Some("Character devices"),
            "Add a serial port, reachable as a text console.",
            "Type",
            &[("pty", "Pseudo terminal (pty)")],
            |v| el("serial").with_attr("type", v),
        ),
        simple_page(
            "Console",
            "utilities-terminal-symbolic",
            None,
            "Add a VirtIO text console.",
            "Type",
            &[("pty", "Pseudo terminal (pty)")],
            |v| el("console").with_attr("type", v).with_child(el("target").with_attr("type", "virtio")),
        ),
        simple_page(
            "Channel",
            "utilities-terminal-symbolic",
            None,
            "Add a communication channel between host and guest.",
            "Channel",
            &[("guest-agent", "qemu-guest-agent"), ("spice-agent", "SPICE agent (clipboard, resizing)")],
            |v| {
                if v == "spice-agent" {
                    el("channel")
                        .with_attr("type", "spicevmc")
                        .with_child(el("target").with_attr("type", "virtio").with_attr("name", "com.redhat.spice.0"))
                } else {
                    el("channel").with_attr("type", "unix").with_child(
                        el("target").with_attr("type", "virtio").with_attr("name", "org.qemu.guest_agent.0"),
                    )
                }
            },
        ),
        simple_page(
            "TPM",
            "security-high-symbolic",
            Some("Other"),
            "Add an emulated TPM 2.0 (needed by Windows 11).",
            "Model",
            &[("tpm-crb", "CRB (recommended)"), ("tpm-tis", "TIS")],
            |v| {
                el("tpm")
                    .with_attr("model", v)
                    .with_child(el("backend").with_attr("type", "emulator").with_attr("version", "2.0"))
            },
        ),
        simple_page(
            "Random numbers",
            "dialog-password-symbolic",
            None,
            "Give the guest a source of randomness from the host.",
            "Source",
            &[("/dev/urandom", "/dev/urandom")],
            |v| {
                el("rng")
                    .with_attr("model", "virtio")
                    .with_child(el("backend").with_attr("model", "random").with_text(v))
            },
        ),
    ]
}

/// Opens the dialog; `done` runs after a device was added.
pub fn open(feedback: &Feedback, ctx: AddContext, done: impl Fn() + 'static) {
    let pages = pages(&ctx);
    let nav = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
    let sections: Vec<Option<&'static str>> = pages.iter().map(|p| p.section).collect();
    nav.set_header_func(move |row, _| match sections.get(row.index() as usize).copied().flatten() {
        Some(section) => row.set_header(Some(
            &gtk::Label::builder()
                .label(section)
                .xalign(0.0)
                .css_classes(["caption-heading", "dim-label", "nav-heading"])
                .build(),
        )),
        None => row.set_header(None::<&gtk::Widget>),
    });
    let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
    for (i, p) in pages.iter().enumerate() {
        let b = gtk::Box::builder().spacing(10).build();
        b.append(&gtk::Image::from_icon_name(p.icon));
        b.append(&gtk::Label::new(Some(p.title)));
        nav.append(&gtk::ListBoxRow::builder().child(&b).build());
        let title = gtk::Label::builder().label(p.title).xalign(0.0).css_classes(["title-2"]).build();
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            .css_classes(["page-content"])
            .build();
        page.append(&title);
        page.append(&p.widget);
        stack.add_named(
            &gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&page).build(),
            Some(&i.to_string()),
        );
    }
    let s = stack.clone();
    nav.connect_row_selected(move |_, row| {
        if let Some(r) = row {
            s.set_visible_child_name(&r.index().to_string());
        }
    });
    nav.select_row(nav.row_at_index(0).as_ref());

    let error = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .hexpand(true)
        .visible(false)
        .css_classes(["error", "caption"])
        .build();
    let cancel = gtk::Button::builder().label("Cancel").build();
    let add = gtk::Button::builder().label("Add").css_classes(["suggested-action"]).build();
    let footer = gtk::Box::builder().spacing(10).css_classes(["wizard-footer"]).build();
    footer.append(&cancel);
    // The error sits between the buttons, so Add stays on the right.
    let middle = gtk::Box::builder().hexpand(true).margin_start(8).margin_end(8).build();
    middle.append(&error);
    footer.append(&middle);
    footer.append(&add);

    let side =
        gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).width_request(230).child(&nav).build();
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    let split =
        crate::ui::widgets::ListDetail::new(&side, &stack, 0, true, "Device types", 600.0, |b| b.clone().upcast());
    let s = split.clone();
    nav.connect_row_activated(move |_, _| s.show_detail());
    let body = split.root().clone();

    let toolbar = adw::ToolbarView::builder().content(&body).bottom_bar_style(adw::ToolbarStyle::Raised).build();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.add_bottom_bar(&footer);
    let dialog = adw::Dialog::builder()
        .title(format!("Add Hardware to {}", ctx.name))
        .content_width(860)
        .content_height(600)
        .child(&toolbar)
        .build();
    dialog.set_default_widget(Some(&add));

    let d = dialog.clone();
    cancel.connect_clicked(move |_| {
        d.close();
    });
    let pages = Rc::new(pages);
    let ctx = Rc::new(ctx);
    let done = Rc::new(done);
    let parent = feedback.parent().clone();
    let feedback = feedback.clone();
    let d = dialog.clone();
    #[cfg(debug_assertions)]
    let add_button = add.clone();
    add.connect_clicked(move |button| {
        let Some(row) = nav.selected_row() else { return };
        let page = &pages[row.index() as usize];
        let plan = match (page.build)() {
            Ok(p) => p,
            Err(msg) => {
                error.set_label(&msg);
                error.set_visible(true);
                return;
            }
        };
        error.set_visible(false);
        button.set_sensitive(false);
        let (ctx, done, feedback, dialog, error, button, title) = (
            ctx.clone(),
            done.clone(),
            feedback.clone(),
            d.clone(),
            error.clone(),
            button.clone(),
            page.title.to_lowercase(),
        );
        glib::spawn_future_local(async move {
            let result = execute(&ctx, plan).await;
            button.set_sensitive(true);
            match result {
                Ok(true) => {
                    feedback.toast(&format!("Added {title} to {}", ctx.name));
                    dialog.close();
                    done();
                }
                Ok(false) => {
                    feedback.toast(&format!("Added {title}. It appears after {} restarts.", ctx.name));
                    dialog.close();
                    done();
                }
                Err(msg) => {
                    error.set_label(&msg);
                    error.set_visible(true);
                }
            }
        });
    });
    dialog.present(Some(&parent));
    // Debug builds: VMNG_DEV_HW_ADD_CLICK=1 presses Add on the first page.
    #[cfg(debug_assertions)]
    if std::env::var_os("VMNG_DEV_HW_ADD_CLICK").is_some() {
        glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || add_button.emit_clicked());
    }
}

/// Carries out a plan. `Ok(false)` means it applies after a restart.
async fn execute(ctx: &AddContext, plan: Plan) -> Result<bool, String> {
    let (conn, uuid) = (ctx.conn.clone(), ctx.uuid.clone());
    match plan {
        Plan::Device(device) => {
            conn.call(move |c| c.add_device(&uuid, &device)).await.map_err(|e| e.message().to_owned())
        }
        Plan::NewDisk { pool, size, format, mut device } => {
            let base = format!("{}-{}", ctx.name, device.find_attr("target", "dev").unwrap_or("disk"));
            conn.call(move |c| {
                let path = c.create_disk(&pool, &base, size, &format)?;
                let pos = device.children.len().saturating_sub(1);
                device.children.insert(pos, vmng_xml::Node::Element(el("source").with_attr("file", &path)));
                c.add_device(&uuid, &device).inspect_err(|_| {
                    let _ = c.delete_volume_at(&path);
                })
            })
            .await
            .map_err(|e| e.message().to_owned())
        }
        Plan::SharedFolder(device) => {
            let shared = ctx.domain.find_attr("memoryBacking/access", "mode") == Some("shared");
            if shared {
                return conn.call(move |c| c.add_device(&uuid, &device)).await.map_err(|e| e.message().to_owned());
            }
            if ctx.active {
                return Err(
                    "Shared folders need shared memory, which can only be turned on while the machine is shut off."
                        .into(),
                );
            }
            conn.call(move |c| {
                let mut xml = c.domain_config_xml(&uuid)?;
                let mb = xml.child_or_insert("memoryBacking");
                mb.child_or_insert("source").set_attr("type", "memfd");
                mb.child_or_insert("access").set_attr("mode", "shared");
                xml.child_or_insert("devices").push(device);
                c.save_domain_config(&uuid, &xml).map(|()| true)
            })
            .await
            .map_err(|e| e.message().to_owned())
        }
    }
}
