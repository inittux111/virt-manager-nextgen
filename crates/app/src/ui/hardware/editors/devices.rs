//! Editors for storage, network and the other devices.

use super::*;

/// A disk (not CD-ROM).
pub fn disk(bind: &Bind) -> gtk::Box {
    let b = vbox();
    let info = group("Disk");
    fact(&info, "Source", &bind.get(|e| crate::ui::hardware::labels::disk_source(e).unwrap_or_default()));
    fact(&info, "Format", &bind.get(|e| e.find_attr("driver", "type").unwrap_or_default().to_owned()));
    b.append(&info);

    let opts = group("Options");
    opts.add(&bus_combo(bind, &[("virtio", "VirtIO (fastest)"), ("sata", "SATA"), ("scsi", "SCSI"), ("usb", "USB")]));
    opts.add(&combo(
        bind,
        "Cache mode",
        &[
            ("", "Default"),
            ("none", "None (safest with host crashes)"),
            ("writeback", "Write-back (faster)"),
            ("writethrough", "Write-through"),
            ("unsafe", "Unsafe (fastest, risky)"),
        ],
        |e| e.find_attr("driver", "cache").unwrap_or_default().to_owned(),
        |e, v| set_attr_path(e, "driver", "cache", v),
    ));
    opts.add(&combo(
        bind,
        "Discard",
        &[("", "Default"), ("unmap", "Pass to the image (frees space)"), ("ignore", "Ignore")],
        |e| e.find_attr("driver", "discard").unwrap_or_default().to_owned(),
        |e, v| set_attr_path(e, "driver", "discard", v),
    ));
    let bus = bind.get(|e| e.find_attr("target", "bus").unwrap_or_default().to_owned());
    if matches!(bus.as_str(), "usb" | "scsi") {
        opts.add(&switch(
            bind,
            "Removable",
            "The guest sees it as a removable drive, like a USB stick",
            |e| e.find_attr("target", "removable") == Some("on"),
            |e, on| set_attr_path(e, "target", "removable", if on { "on" } else { "" }),
        ));
    }
    b.append(&opts);

    let perf = group("Performance");
    perf.add(&combo(
        bind,
        "Detect zeroes",
        &[
            ("", "Default (off)"),
            ("unmap", "Free the space (needs Discard: pass to the image)"),
            ("on", "Store zeroes efficiently"),
            ("off", "Off"),
        ],
        |e| e.find_attr("driver", "detect_zeroes").unwrap_or_default().to_owned(),
        |e, v| set_attr_path(e, "driver", "detect_zeroes", v),
    ));
    perf.add(&combo(
        bind,
        "I/O mode",
        &[
            ("", "Default"),
            ("native", "Native (Linux AIO; needs cache mode None)"),
            ("threads", "Threads"),
            ("io_uring", "io_uring (newer kernels)"),
        ],
        |e| e.find_attr("driver", "io").unwrap_or_default().to_owned(),
        |e, v| set_attr_path(e, "driver", "io", v),
    ));
    b.append(&perf);

    let access = group("Access");
    access.add(&switch(
        bind,
        "Read-only",
        "The guest can't change the disk",
        |e| e.child("readonly").is_some(),
        |e, on| toggle_child(e, "readonly", on),
    ));
    access.add(&switch(
        bind,
        "Shareable",
        "Several machines may use this disk at the same time",
        |e| e.child("shareable").is_some(),
        |e, on| toggle_child(e, "shareable", on),
    ));
    access.add(&entry(
        bind,
        "Serial number (optional)",
        |e| e.find_text("serial").unwrap_or_default(),
        |e, v| {
            if v.is_empty() {
                e.remove_children(|c| c.name == "serial");
            } else {
                e.child_or_insert("serial").set_text(v);
            }
        },
    ));
    b.append(&access);
    b
}

/// Changing the bus also changes the target name (vda ↔ sda).
pub(super) fn bus_combo(bind: &Bind, options: &[(&str, &str)]) -> adw::ComboRow {
    let mut opts: Vec<(String, String)> = options.iter().map(|(v, l)| ((*v).to_owned(), (*l).to_owned())).collect();
    let current = bind.get(|e| e.find_attr("target", "bus").unwrap_or_default().to_owned());
    if !opts.iter().any(|(v, _)| *v == current) {
        opts.push((current.clone(), current.clone()));
    }
    let labels: Vec<&str> = opts.iter().map(|(_, l)| l.as_str()).collect();
    let row = adw::ComboRow::builder().title("Bus").model(&gtk::StringList::new(&labels)).build();
    row.set_selected(opts.iter().position(|(v, _)| *v == current).unwrap_or(0) as u32);
    let bind = bind.clone();
    row.connect_selected_notify(move |r| {
        let Some((bus, _)) = opts.get(r.selected() as usize).cloned() else { return };
        if bind.get(|e| e.find_attr("target", "bus") == Some(bus.as_str())) {
            return;
        }
        // Pick a free name on the new bus, ignoring this disk's own name.
        let own = bind.get(|e| e.find_attr("target", "dev").map(str::to_owned));
        let mut others = bind.work.borrow().clone();
        if let Some(devices) = others.child_mut("devices") {
            devices.remove_children(|d| d.name == "disk" && d.find_attr("target", "dev").map(str::to_owned) == own);
        }
        let target = next_disk_target(&others, disk_prefix(&bus));
        bind.set(|e| {
            let t = e.child_or_insert("target");
            t.set_attr("bus", &bus);
            t.set_attr("dev", &target);
            if !matches!(bus.as_str(), "usb" | "scsi") {
                t.remove_attr("removable");
            }
            drop_address(e);
        });
    });
    row
}

/// A CD-ROM drive. Changing the disc applies immediately.
pub fn cdrom(bind: &Bind, choices: &Choices, live: &Live) -> gtk::Box {
    let b = vbox();
    let media = group("Disc");
    let current = bind.get(crate::ui::hardware::labels::disk_source);
    let mut options: Vec<(String, Option<String>)> = vec![("Empty".into(), None)];
    for img in &choices.images {
        options.push((img.name.clone(), Some(img.path.clone())));
    }
    if let Some(c) = &current
        && !options.iter().any(|(_, p)| p.as_ref() == Some(c))
    {
        options.push((c.rsplit('/').next().unwrap_or(c).to_owned(), Some(c.clone())));
    }
    let labels: Vec<&str> = options.iter().map(|(l, _)| l.as_str()).collect();
    let model = gtk::StringList::new(&labels);
    let row = adw::ComboRow::builder()
        .title("Inserted disc")
        .subtitle("Changes right away, also while the machine runs")
        .model(&model)
        .build();
    row.set_selected(options.iter().position(|(_, p)| *p == current).unwrap_or(0) as u32);
    let options = Rc::new(RefCell::new(options));
    // Set while the list is changed in code, so that isn't taken as a choice.
    let quiet = Rc::new(std::cell::Cell::new(false));
    let insert = {
        let (bind, live) = (bind.clone(), live.clone());
        move |label: &str, path: Option<&str>| {
            let mut dev = bind.get(|e| e.clone());
            dev.remove_children(|c| c.name == "source");
            if let Some(p) = path {
                let pos =
                    dev.children.iter().position(|n| matches!(n, Node::Element(c) if c.name == "target")).unwrap_or(0);
                dev.set_attr("type", "file");
                dev.children.insert(pos, Node::Element(Element::new("source").with_attr("file", p)));
            }
            let msg = if path.is_some() { format!("Inserted {label}") } else { "Ejected the disc".into() };
            live(dev, msg);
        }
    };
    let insert = Rc::new(insert);
    let (opts2, quiet2, insert2) = (options.clone(), quiet.clone(), insert.clone());
    row.connect_selected_notify(move |r| {
        if quiet2.get() {
            return;
        }
        let Some((label, path)) = opts2.borrow().get(r.selected() as usize).cloned() else { return };
        insert2(&label, path.as_deref());
    });
    // A chosen file: shown in the list (once), selected, then inserted.
    let choose = {
        let (row, model) = (row.clone(), model.clone());
        Rc::new(move |path: String| {
            let label = path.rsplit('/').next().unwrap_or(&path).to_owned();
            let index = {
                let mut opts = options.borrow_mut();
                opts.iter().position(|(_, p)| p.as_deref() == Some(path.as_str())).unwrap_or_else(|| {
                    opts.push((label.clone(), Some(path.clone())));
                    model.append(&label);
                    opts.len() - 1
                })
            };
            quiet.set(true);
            row.set_selected(index as u32);
            quiet.set(false);
            insert(&label, Some(&path));
        })
    };
    // Debug builds: `VMNG_DEV_CDROM_FILE=<path>` acts as if it was browsed to.
    #[cfg(debug_assertions)]
    if let Ok(path) = std::env::var("VMNG_DEV_CDROM_FILE") {
        let choose = choose.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || choose(path));
    }
    // Any ISO file, also one libvirt doesn't know yet (e.g. in Downloads).
    let browse = gtk::Button::builder().label("Browse…").valign(gtk::Align::Center).build();
    browse.set_tooltip_text(Some("Choose a disc image file"));
    let remote = choices.remote.clone();
    browse.connect_clicked(move |button| {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Disc images"));
        filter.add_suffix("iso");
        filter.add_mime_type("application/x-cd-image");
        let choose = choose.clone();
        crate::ui::widgets::pick_host_path(
            button.upcast_ref(),
            remote.as_deref(),
            false,
            "Choose a Disc Image",
            Some(filter),
            move |p| choose(p),
        );
    });
    row.add_suffix(&browse);
    media.add(&row);
    b.append(&media);
    let opts = group("Options");
    opts.add(&bus_combo(bind, &[("sata", "SATA"), ("scsi", "SCSI"), ("ide", "IDE"), ("usb", "USB")]));
    b.append(&opts);
    b
}

/// A network card.
pub fn interface(bind: &Bind, choices: &Choices, live: &Live) -> gtk::Box {
    let b = vbox();
    let g = group("Connection");
    let mut options: Vec<(String, String)> = Vec::new();
    for n in &choices.networks {
        options.push((
            format!("network:{}", n.name),
            format!("Virtual network “{}” · {}", n.name, n.forward.to_uppercase()),
        ));
    }
    for br in &choices.bridges {
        options.push((format!("bridge:{br}"), format!("Bridge {br} · on your local network")));
    }
    for nic in &choices.host_nics {
        options.push((format!("direct:{nic}"), format!("Directly on {nic} (macvtap)")));
    }
    options.push(("user:".into(), "User networking".into()));
    let refs: Vec<(&str, &str)> = options.iter().map(|(v, l)| (v.as_str(), l.as_str())).collect();
    g.add(&combo(
        bind,
        "Connected to",
        &refs,
        |e| match e.attr("type") {
            Some("network") => format!("network:{}", e.find_attr("source", "network").unwrap_or_default()),
            Some("bridge") => format!("bridge:{}", e.find_attr("source", "bridge").unwrap_or_default()),
            Some("user") => "user:".into(),
            Some(other) => format!("{other}:{}", e.find_attr("source", "dev").unwrap_or_default()),
            None => String::new(),
        },
        |e, v| {
            let (kind, name) = v.split_once(':').unwrap_or((v, ""));
            e.set_attr("type", kind);
            e.remove_children(|c| c.name == "source" || c.name == "target" || c.name == "virtualport");
            let source = match kind {
                "network" => Some(Element::new("source").with_attr("network", name)),
                "bridge" => Some(Element::new("source").with_attr("bridge", name)),
                // Bridge mode lets machines on the same card see each other.
                "direct" => Some(Element::new("source").with_attr("dev", name).with_attr("mode", "bridge")),
                _ => None,
            };
            if let Some(source) = source {
                e.children.insert(0, Node::Element(source));
            }
        },
    ));
    if bind.get(|e| e.attr("type") == Some("direct")) || !choices.host_nics.is_empty() {
        g.add(&note_row(
            "A direct (macvtap) connection puts the machine on your local network, but the host itself can't reach it over that card.",
        ));
    }
    g.add(&combo(
        bind,
        "Model",
        &[
            ("virtio", "VirtIO (fastest)"),
            ("e1000e", "Intel e1000e"),
            ("e1000", "Intel e1000"),
            ("rtl8139", "Realtek RTL8139"),
        ],
        |e| e.find_attr("model", "type").unwrap_or_default().to_owned(),
        |e, v| {
            set_attr_path(e, "model", "type", v);
            drop_address(e);
        },
    ));
    let mac = adw::EntryRow::builder()
        .title("MAC address")
        .text(bind.get(|e| e.find_attr("mac", "address").unwrap_or_default().to_owned()))
        .build();
    {
        let bind = bind.clone();
        mac.connect_changed(move |r| match normalize_mac(&r.text()) {
            Ok(m) => {
                r.remove_css_class("error");
                if bind.get(|e| e.find_attr("mac", "address") != Some(m.as_str())) {
                    bind.set(|e| e.child_or_insert("mac").set_attr("address", &m));
                }
            }
            Err(msg) => {
                r.add_css_class("error");
                r.set_tooltip_text(Some(msg));
            }
        });
    }
    g.add(&mac);
    b.append(&g);

    let state = group("State");
    let cable = adw::SwitchRow::builder()
        .title("Cable connected")
        .subtitle("Changes right away, also while the machine runs")
        .active(bind.get(|e| e.find_attr("link", "state") != Some("down")))
        .build();
    let (bind2, live2) = (bind.clone(), live.clone());
    cable.connect_active_notify(move |r| {
        let mut dev = bind2.get(|e| e.clone());
        dev.child_or_insert("link").set_attr("state", if r.is_active() { "up" } else { "down" });
        live2(
            dev,
            if r.is_active() { "Connected the network cable".into() } else { "Unplugged the network cable".into() },
        );
    });
    state.add(&cable);
    b.append(&state);
    b
}

pub fn sound(bind: &Bind) -> gtk::Box {
    let b = vbox();
    let g = group("Sound");
    g.add(&combo(
        bind,
        "Model",
        &[("ich9", "Intel HDA (ICH9)"), ("ich6", "Intel HDA (ICH6)"), ("ac97", "AC97"), ("usb", "USB audio")],
        |e| e.attr("model").unwrap_or_default().to_owned(),
        |e, v| {
            e.set_attr("model", v);
            drop_address(e);
        },
    ));
    b.append(&g);
    b
}

pub fn input(bind: &Bind) -> gtk::Box {
    let b = vbox();
    let g = group("Input");
    fact(&g, "Type", &bind.get(|e| e.attr("type").unwrap_or_default().to_owned()));
    if bind.get(|e| e.attr("bus") != Some("ps2")) {
        g.add(&combo(
            bind,
            "Bus",
            &[("usb", "USB"), ("virtio", "VirtIO")],
            |e| e.attr("bus").unwrap_or_default().to_owned(),
            |e, v| {
                e.set_attr("bus", v);
                drop_address(e);
            },
        ));
    } else {
        g.add(
            &adw::ActionRow::builder()
                .title("Built into the machine")
                .subtitle("PS/2 devices come with the chipset and can't be changed")
                .build(),
        );
    }
    b.append(&g);
    b
}

pub fn filesystem(bind: &Bind) -> gtk::Box {
    let b = vbox();
    let g = group("Shared folder");
    g.add(&entry(
        bind,
        "Folder on the host",
        |e| e.find_attr("source", "dir").unwrap_or_default().to_owned(),
        |e, v| {
            e.child_or_insert("source").set_attr("dir", v);
        },
    ));
    g.add(&entry(
        bind,
        "Name in the guest (mount tag)",
        |e| e.find_attr("target", "dir").unwrap_or_default().to_owned(),
        |e, v| {
            e.child_or_insert("target").set_attr("dir", v);
        },
    ));
    g.add(&switch(
        bind,
        "Read-only",
        "The guest can read the folder but not change it",
        |e| e.child("readonly").is_some(),
        |e, on| toggle_child(e, "readonly", on),
    ));
    let method = combo(
        bind,
        "Method",
        &[(SHARE_VIRTIOFS, "virtiofs (fast; needs shared memory)"), (SHARE_9P, "9p (older guests)")],
        |e| share_method(e).to_owned(),
        set_share_method,
    );
    {
        // virtiofs needs the machine's memory to be shared.
        let bind = bind.clone();
        method.connect_selected_notify(move |_| {
            if bind.get(|e| share_method(e) == SHARE_VIRTIOFS) {
                bind.set_domain(enable_shared_memory);
            }
        });
    }
    g.add(&method);
    b.append(&g);
    let hint = note(&mount_hint(&bind.get(|e| e.clone())));
    {
        let (bind, hint) = (bind.clone(), hint.clone());
        method.connect_selected_notify(move |_| hint.set_label(&mount_hint(&bind.get(|e| e.clone()))));
    }
    b.append(&hint);
    b
}

pub const SHARE_VIRTIOFS: &str = "virtiofs";

pub const SHARE_9P: &str = "9p";

/// How a shared folder reaches the guest: virtiofs or 9p (libvirt's
/// default driver when none is given).
pub fn share_method(e: &Element) -> &'static str {
    if e.find_attr("driver", "type") == Some("virtiofs") { SHARE_VIRTIOFS } else { SHARE_9P }
}

/// Switches a shared folder between virtiofs and 9p. virtiofs only
/// supports passing files through as they are; 9p maps ownership so
/// guest users can write.
pub fn set_share_method(e: &mut Element, method: &str) {
    e.remove_children(|c| c.name == "driver" || c.name == "binary");
    if method == SHARE_VIRTIOFS {
        e.set_attr("accessmode", "passthrough");
        e.children.insert(0, Node::Element(Element::new("driver").with_attr("type", "virtiofs")));
    } else {
        e.set_attr("accessmode", "mapped");
        e.children.insert(0, Node::Element(Element::new("driver").with_attr("type", "path")));
    }
    drop_address(e);
}

/// Shares the machine's memory with helper processes (virtiofs needs it).
pub fn enable_shared_memory(domain: &mut Element) {
    let mb = domain.child_or_insert("memoryBacking");
    mb.child_or_insert("source").set_attr("type", "memfd");
    mb.child_or_insert("access").set_attr("mode", "shared");
}

pub(super) fn mount_hint(e: &Element) -> String {
    let tag = e.find_attr("target", "dir").unwrap_or("<name>");
    if share_method(e) == SHARE_VIRTIOFS {
        format!("In a Linux guest, mount it with: mount -t virtiofs {tag} /mnt/{tag}")
    } else {
        format!("In a Linux guest, mount it with: mount -t 9p -o trans=virtio {tag} /mnt/{tag}")
    }
}

pub fn controller(bind: &Bind) -> gtk::Box {
    let b = vbox();
    let g = group("Controller");
    let kind = bind.get(|e| e.attr("type").unwrap_or_default().to_owned());
    fact(&g, "Type", &kind.to_uppercase());
    let options: &[(&str, &str)] = match kind.as_str() {
        "usb" => &[
            ("qemu-xhci", "USB 3 (qemu-xhci)"),
            ("nec-xhci", "USB 3 (NEC)"),
            ("ich9-ehci1", "USB 2 (ICH9)"),
            ("piix3-uhci", "USB 1 (PIIX3)"),
        ],
        "scsi" => &[("virtio-scsi", "VirtIO SCSI"), ("lsilogic", "LSI Logic")],
        _ => &[],
    };
    if !options.is_empty() {
        g.add(&combo(
            bind,
            "Model",
            options,
            |e| e.attr("model").unwrap_or_default().to_owned(),
            |e, v| {
                e.set_attr("model", v);
                drop_address(e);
            },
        ));
    }
    b.append(&g);
    b
}

pub fn tpm(bind: &Bind) -> gtk::Box {
    let b = vbox();
    let g = group("TPM");
    g.add(&combo(
        bind,
        "Model",
        &[("tpm-crb", "CRB (recommended)"), ("tpm-tis", "TIS")],
        |e| e.attr("model").unwrap_or_default().to_owned(),
        |e, v| e.set_attr("model", v),
    ));
    fact(&g, "Version", &bind.get(|e| e.find_attr("backend", "version").unwrap_or("2.0").to_owned()));
    fact(&g, "Backend", &bind.get(|e| e.find_attr("backend", "type").unwrap_or_default().to_owned()));
    b.append(&g);
    b
}
