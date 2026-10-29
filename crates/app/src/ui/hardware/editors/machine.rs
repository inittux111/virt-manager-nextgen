//! Editors for the machine itself: Overview, Processor & memory, and Boot &
//! firmware (boot order, operating system).

use super::*;

/// Name, title and description (domain level).
pub fn overview(bind: &Bind) -> gtk::Box {
    let b = vbox();
    let g = group("Machine");
    fact(&g, "Name", &bind.get(|e| e.find_text("name").unwrap_or_default()));
    fact(&g, "UUID", &bind.get(|e| e.find_text("uuid").unwrap_or_default()));
    let title = entry(
        bind,
        "Title (optional)",
        |e| e.find_text("title").unwrap_or_default(),
        |e, v| set_text_child(e, "title", v),
    );
    g.add(&title);
    b.append(&g);
    b.append(&note("The title is shown instead of the name in the list. Use Rename… to change the name."));

    let d = group("Description");
    let buffer = gtk::TextBuffer::new(None);
    buffer.set_text(&bind.get(|e| e.find_text("description").unwrap_or_default()));
    let view = gtk::TextView::builder()
        .buffer(&buffer)
        .wrap_mode(gtk::WrapMode::WordChar)
        .accepts_tab(false)
        .height_request(120)
        .top_margin(12)
        .bottom_margin(12)
        .left_margin(12)
        .right_margin(12)
        .css_classes(["card", "description-view"])
        .build();
    view.update_property(&[gtk::accessible::Property::Label("Description")]);
    let bind = bind.clone();
    buffer.connect_changed(move |buf| {
        let text = buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string();
        bind.set(|e| set_text_child(e, "description", text.trim_end()));
    });
    d.add(&view);
    b.append(&d);
    b
}

/// Sets a text element of the domain (`<title>`, `<description>`), right
/// after `<name>`/`<uuid>` as libvirt writes it, or removes it when empty.
pub(super) fn set_text_child(e: &mut Element, name: &str, text: &str) {
    if text.is_empty() {
        e.remove_children(|c| c.name == name);
        return;
    }
    if let Some(c) = e.child_mut(name) {
        c.set_text(text);
        return;
    }
    let after = e
        .children
        .iter()
        .rposition(|n| matches!(n, Node::Element(c) if matches!(c.name.as_str(), "name" | "uuid" | "title")))
        .map_or(0, |i| i + 1);
    e.children.insert(after, Node::Element(Element::new(name).with_text(text)));
}

/// Processor & memory (domain level).
pub fn cpu_memory(bind: &Bind, choices: &Choices, host_cpus: u32, host_memory: u64) -> gtk::Box {
    let b = vbox();
    let cpu = group("Processor");
    let max_cpus = f64::from(host_cpus.max(1)) * 2.0;
    let vcpus = spin(
        bind,
        "Virtual CPUs",
        (1.0, max_cpus, 1.0, 0),
        |e| {
            e.child("vcpu").map_or(1.0, |v| {
                v.attr("current").and_then(|c| c.parse().ok()).unwrap_or_else(|| v.text().parse().unwrap_or(1.0))
            })
        },
        |e, v| {
            // A set layout decides the count; it's kept in step below.
            if Topology::of(e).is_none() {
                let vcpu = e.child_or_insert("vcpu");
                vcpu.remove_attr("current");
                vcpu.set_text((v as u32).max(1).to_string());
            }
        },
    );
    vcpus.set_subtitle(&format!("The host has {host_cpus} logical CPUs"));
    cpu.add(&vcpus);

    // Which CPU the guest sees: the host's own, a model like it, QEMU's
    // default, or one of the named models the host can run.
    let mut options: Vec<(String, String)> = vec![
        ("host-passthrough".into(), "Copy the host CPU (fastest)".into()),
        ("host-model".into(), "Similar to the host (portable)".into()),
        (String::new(), "Hypervisor default".into()),
    ];
    options.extend(choices.cpu_models.iter().map(|m| (format!("model:{m}"), m.clone())));
    let options: Vec<(&str, &str)> = options.iter().map(|(v, l)| (v.as_str(), l.as_str())).collect();
    let model = combo(
        bind,
        "CPU model",
        &options,
        |e| match CpuMode::of(e) {
            CpuMode::HostPassthrough => "host-passthrough".into(),
            CpuMode::HostModel => "host-model".into(),
            CpuMode::Model(m) => format!("model:{m}"),
            CpuMode::Default => String::new(),
        },
        |e, v| {
            let mode = match v {
                "host-passthrough" => CpuMode::HostPassthrough,
                "host-model" => CpuMode::HostModel,
                "" => CpuMode::Default,
                m => CpuMode::Model(m.trim_start_matches("model:").to_owned()),
            };
            set_cpu_mode(e, &mode);
        },
    );
    model.set_enable_search(true);
    cpu.add(&model);

    let available = choices.cpu_security.clone();
    let mitigations = switch(
        bind,
        "Security mitigations",
        "Pass the host CPU's security fixes to a named CPU model",
        |e| mitigations_on(e, &choices.cpu_security),
        move |e, on| set_mitigations(e, &available, on),
    );
    let named = |b: &Bind| b.get(|e| matches!(CpuMode::of(e), CpuMode::Model(_)));
    mitigations.set_sensitive(!choices.cpu_security.is_empty() && named(bind));
    if choices.cpu_security.is_empty() {
        mitigations.set_subtitle("The host CPU reports none");
    }
    {
        let (bind, mitigations, has) = (bind.clone(), mitigations.clone(), !choices.cpu_security.is_empty());
        let available = choices.cpu_security.clone();
        model.connect_selected_notify(move |_| {
            let is_named = named(&bind);
            mitigations.set_sensitive(has && is_named);
            // A newly picked model keeps the switch's choice.
            if is_named && mitigations.is_active() {
                bind.set(|e| set_mitigations(e, &available, true));
            }
        });
    }
    cpu.add(&mitigations);
    b.append(&cpu);

    // The CPU layout: virt-manager's "Manually set CPU topology".
    let layout = group("CPU layout");
    layout.set_description(Some(
        "How the virtual CPUs are arranged. Some systems, such as Windows desktop editions, only use one or two \
         sockets, so give them more cores instead.",
    ));
    let current = bind.get(Topology::of);
    let manual = adw::SwitchRow::builder()
        .title("Set the layout manually")
        .subtitle("Otherwise QEMU decides")
        .active(current.is_some())
        .build();
    layout.add(&manual);
    let start = current.unwrap_or_else(|| Topology::for_vcpus(vcpus.value() as u32));
    let rows: Vec<adw::SpinRow> =
        [("Sockets", start.sockets), ("Cores per socket", start.cores), ("Threads per core", start.threads)]
            .into_iter()
            .map(|(title, value)| {
                let row = adw::SpinRow::with_range(1.0, max_cpus, 1.0);
                row.set_title(title);
                row.set_value(f64::from(value));
                row.set_visible(current.is_some());
                layout.add(&row);
                row
            })
            .collect();
    let dies = start.dies;
    let apply = {
        let (bind, manual, vcpus, rows) = (bind.clone(), manual.clone(), vcpus.clone(), rows.clone());
        Rc::new(move || {
            let on = manual.is_active();
            let topology = on.then(|| {
                let v = |i: usize| rows[i].value() as u32;
                Topology { sockets: v(0), dies, cores: v(1), threads: v(2) }
            });
            for r in &rows {
                r.set_visible(on);
            }
            vcpus.set_sensitive(!on);
            bind.set(|e| set_topology(e, topology));
            if let Some(t) = topology {
                vcpus.set_value(f64::from(t.total()));
                vcpus.set_subtitle(&format!("{} from the layout; the host has {host_cpus} logical CPUs", t.total()));
            } else {
                vcpus.set_subtitle(&format!("The host has {host_cpus} logical CPUs"));
            }
        })
    };
    vcpus.set_sensitive(current.is_none());
    if let Some(t) = current {
        vcpus.set_subtitle(&format!("{} from the layout; the host has {host_cpus} logical CPUs", t.total()));
    }
    {
        let (apply, rows, vcpus) = (apply.clone(), rows.clone(), vcpus.clone());
        manual.connect_active_notify(move |m| {
            if m.is_active() {
                // Start from the current count: one socket, that many cores.
                let t = Topology::for_vcpus(vcpus.value() as u32);
                rows[0].set_value(f64::from(t.sockets));
                rows[1].set_value(f64::from(t.cores));
                rows[2].set_value(f64::from(t.threads));
            }
            apply();
        });
    }
    for r in &rows {
        let apply = apply.clone();
        r.connect_value_notify(move |_| apply());
    }
    b.append(&layout);

    let mem = group("Memory");
    let gib = |bytes: u64| bytes as f64 / units::GIB as f64;
    let to_bytes = |v: f64| ((v * units::GIB as f64) as u64 / units::MIB) * units::MIB;
    let host_gib = gib(host_memory).max(1.0);
    let start = bind.get(MemorySize::of);
    let current = adw::SpinRow::with_range(0.25, host_gib, 0.25);
    current.set_title("Memory (GiB)");
    current.set_digits(2);
    current.set_value(gib(start.current));
    current.set_subtitle(&format!("The host has {}", format::bytes(host_memory)));
    let raise = adw::SwitchRow::builder()
        .title("Allow raising it while running")
        .subtitle("Sets a maximum the memory can grow to without a restart; needs the guest's balloon driver")
        .active(start.maximum > start.current)
        .build();
    let maximum = adw::SpinRow::with_range(0.25, host_gib, 0.25);
    maximum.set_title("Maximum memory (GiB)");
    maximum.set_digits(2);
    maximum.set_value(gib(start.maximum));
    maximum.set_visible(raise.is_active());
    let apply = {
        let (bind, current, raise, maximum) = (bind.clone(), current.clone(), raise.clone(), maximum.clone());
        Rc::new(move || {
            let cur = to_bytes(current.value());
            let max = if raise.is_active() { to_bytes(maximum.value()).max(cur) } else { cur };
            maximum.set_visible(raise.is_active());
            let size = MemorySize { current: cur, maximum: max };
            if bind.get(MemorySize::of) != size {
                bind.set(|e| set_memory(e, size));
            }
        })
    };
    {
        let (apply, maximum) = (apply.clone(), maximum.clone());
        current.connect_value_notify(move |c| {
            if maximum.value() < c.value() {
                maximum.set_value(c.value());
            }
            apply();
        });
    }
    {
        let (apply, maximum, current) = (apply.clone(), maximum.clone(), current.clone());
        raise.connect_active_notify(move |r| {
            if r.is_active() && maximum.value() <= current.value() {
                maximum.set_value((current.value() * 2.0).min(host_gib));
            }
            apply();
        });
    }
    {
        let (apply, current) = (apply.clone(), current.clone());
        maximum.connect_value_notify(move |m| {
            if m.value() < current.value() {
                m.set_value(current.value());
            }
            apply();
        });
    }
    mem.add(&current);
    mem.add(&raise);
    mem.add(&maximum);
    mem.add(&switch(
        bind,
        "Shared memory",
        "Needed for shared folders (virtiofs)",
        |e| e.find_attr("memoryBacking/access", "mode") == Some("shared"),
        |e, on| {
            if on {
                enable_shared_memory(e);
            } else if let Some(mb) = e.child_mut("memoryBacking") {
                mb.remove_children(|c| c.name == "source" || c.name == "access");
                if mb.children.is_empty() {
                    e.remove_children(|c| c.name == "memoryBacking");
                }
            }
        },
    ));
    b.append(&mem);
    b
}

/// Devices that can be in the boot order.
pub(super) fn bootable(e: &Element) -> bool {
    e.name == "disk" || e.name == "interface" || (e.name == "hostdev" && e.attr("mode") == Some("subsystem"))
}

/// Boot & firmware (domain level).
pub fn boot(bind: &Bind, choices: &Choices) -> gtk::Box {
    let b = vbox();
    b.append(&operating_system(bind));
    let fw = group("Firmware");
    fw.set_description(Some("Changing the firmware of an installed system usually stops it from starting."));
    // BIOS, UEFI (libvirt picks the file), then each firmware file, like
    // virt-manager's list.
    let current = bind.get(|e| Some(FirmwareChoice::of(e))).unwrap_or(FirmwareChoice::Bios);
    let mut options = vec![FirmwareChoice::Bios, FirmwareChoice::Uefi { secure_boot: current.secure_boot() }];
    options.extend(choices.loaders.iter().map(|p| FirmwareChoice::Loader(p.clone())));
    if !options.contains(&current) && !matches!(current, FirmwareChoice::Uefi { .. }) {
        options.push(current.clone());
    }
    let labels: Vec<String> = options.iter().map(|o| o.label(&choices.arch)).collect();
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    // The choice shows as the subtitle, so long file paths can wrap.
    let row = adw::ComboRow::builder()
        .title("Firmware")
        .use_subtitle(true)
        .model(&gtk::StringList::new(&labels))
        .list_factory(&crate::ui::widgets::full_text_list())
        .build();
    let position = options.iter().position(|o| {
        std::mem::discriminant(o) == std::mem::discriminant(&current)
            && (!matches!(o, FirmwareChoice::Loader(_)) || *o == current)
    });
    row.set_selected(position.unwrap_or(0) as u32);
    let secure = adw::SwitchRow::builder()
        .title("Secure Boot")
        .subtitle("Only lets signed operating systems start")
        .active(current.secure_boot())
        .sensitive(matches!(current, FirmwareChoice::Uefi { .. }))
        .build();
    let (bind2, secure2, options2) = (bind.clone(), secure.clone(), options.clone());
    row.connect_selected_notify(move |r| {
        let Some(choice) = options2.get(r.selected() as usize).cloned() else { return };
        let choice = match choice {
            FirmwareChoice::Uefi { .. } => FirmwareChoice::Uefi { secure_boot: secure2.is_active() },
            other => other,
        };
        secure2.set_sensitive(matches!(choice, FirmwareChoice::Uefi { .. }));
        if !matches!(choice, FirmwareChoice::Uefi { .. }) {
            secure2.set_active(choice.secure_boot());
        }
        bind2.set(|e| set_firmware(e, &choice));
    });
    let bind2 = bind.clone();
    secure.connect_active_notify(move |s| {
        // Only automatic UEFI takes Secure Boot as a separate setting.
        if s.is_sensitive() {
            let choice = FirmwareChoice::Uefi { secure_boot: s.is_active() };
            bind2.set(|e| set_firmware(e, &choice));
        }
    });
    fw.add(&row);
    fw.add(&secure);
    fw.add(&switch(
        bind,
        "Show boot menu",
        "Lets you pick a boot device while the machine starts",
        |e| e.find_attr("os/bootmenu", "enable") == Some("yes"),
        |e, on| {
            let os = e.child_or_insert("os");
            if on {
                os.child_or_insert("bootmenu").set_attr("enable", "yes");
            } else {
                os.remove_children(|c| c.name == "bootmenu");
            }
        },
    ));
    b.append(&fw);

    // Boot order: devices with <boot order='n'/>, then the rest.
    let order = group("Boot order");
    order.set_description(Some("Checked devices are tried from top to bottom."));
    let list = gtk::ListBox::builder().css_classes(["boxed-list"]).selection_mode(gtk::SelectionMode::None).build();
    order.add(&list);
    b.append(&order);
    fill_boot_list(bind, &list);
    b
}

/// The operating system the machine is marked as (virt-manager's "OS
/// information"): it decides the logo and is what the New VM wizard based
/// its defaults on. Changing it changes no hardware.
pub(super) fn operating_system(bind: &Bind) -> adw::PreferencesGroup {
    let g = group("Operating system");
    let row = adw::ActionRow::builder().title("Installed system").build();
    let logo = crate::ui::widgets::OsLogo::new(32);
    row.add_prefix(&logo);
    let show = {
        let (row, logo) = (row.clone(), logo.clone());
        move |id: Option<&str>| {
            row.set_subtitle(&id.map_or_else(|| "Unknown".to_owned(), crate::osdb::display_name));
            logo.set_logo(id.and_then(crate::osdb::logo_key));
        }
    };
    show(bind.get(|e| DomainConfig::from_xml(e).os_id).as_deref());
    let change = gtk::Button::builder().label("Change…").valign(gtk::Align::Center).build();
    row.add_suffix(&change);
    let bind = bind.clone();
    change.connect_clicked(move |button| {
        let current = bind.get(|e| DomainConfig::from_xml(e).os_id);
        let query = current.as_deref().and_then(crate::osdb::lookup).map(|o| o.name.clone()).unwrap_or_default();
        let (bind, show) = (bind.clone(), show.clone());
        crate::ui::os_chooser::choose_os(button, &query, move |os| {
            let id = os.id.clone();
            bind.set_domain(|e| set_os_id(e, &id));
            show(Some(&id));
        });
    });
    g.add(&row);
    g
}

pub(super) fn boot_candidates(e: &Element) -> Vec<(usize, Option<u32>, String)> {
    // Older configurations list device types in <os><boot dev='hd'/>
    // instead of numbering devices; translate those into an order.
    let legacy: Vec<String> = e
        .child("os")
        .map(|os| os.children_named("boot").filter_map(|b| b.attr("dev").map(str::to_owned)).collect())
        .unwrap_or_default();
    let legacy_rank = |el: &Element| {
        let dev = match (el.name.as_str(), el.attr("device")) {
            ("disk", Some("cdrom")) => "cdrom",
            ("disk", Some("floppy")) => "fd",
            ("disk", _) => "hd",
            ("interface", _) => "network",
            _ => return None,
        };
        legacy.iter().position(|d| d == dev).map(|p| p as u32 * 100)
    };
    let mut out: Vec<(usize, Option<u32>, String)> = vmng_virt::list_devices(e)
        .into_iter()
        .filter(|d| bootable(&d.element))
        .map(|d| {
            let order = d.element.find_attr("boot", "order").and_then(|o| o.parse().ok());
            let order = order.or_else(|| legacy_rank(&d.element).map(|r| r + d.index as u32));
            (d.index, order, crate::ui::hardware::labels::describe(&d).0)
        })
        .collect();
    out.sort_by_key(|(i, o, _)| (o.unwrap_or(u32::MAX), *i));
    out
}

/// Rewrites boot orders from a list of (device index, enabled) in order.
pub(super) fn apply_boot_order(e: &mut Element, rows: &[(usize, bool)]) {
    // Per-device order and <os><boot dev=…> can't be mixed.
    if let Some(os) = e.child_mut("os") {
        os.remove_children(|c| c.name == "boot");
    }
    let mut n = 1;
    let Some(devices) = e.child_mut("devices") else { return };
    for (index, enabled) in rows {
        if let Some(Node::Element(dev)) = devices.children.get_mut(*index) {
            dev.remove_children(|c| c.name == "boot");
            if *enabled {
                dev.push(Element::new("boot").with_attr("order", n.to_string()));
                n += 1;
            }
        }
    }
}

pub(super) fn fill_boot_list(bind: &Bind, list: &gtk::ListBox) {
    while let Some(c) = list.first_child() {
        list.remove(&c);
    }
    let rows: Vec<(usize, bool, String)> =
        bind.get(|e| boot_candidates(e).into_iter().map(|(i, o, t)| (i, o.is_some(), t)).collect());
    let count = rows.len();
    let state = Rc::new(RefCell::new(rows.iter().map(|(i, on, _)| (*i, *on)).collect::<Vec<_>>()));
    for (pos, (_, on, title)) in rows.iter().enumerate() {
        let check = gtk::CheckButton::builder().active(*on).valign(gtk::Align::Center).build();
        let row = adw::ActionRow::builder().title(title).activatable_widget(&check).build();
        row.add_prefix(&check);
        let up = gtk::Button::builder()
            .icon_name("go-up-symbolic")
            .tooltip_text("Move up")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .sensitive(pos > 0)
            .build();
        let down = gtk::Button::builder()
            .icon_name("go-down-symbolic")
            .tooltip_text("Move down")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .sensitive(pos + 1 < count)
            .build();
        row.add_suffix(&up);
        row.add_suffix(&down);
        list.append(&row);

        let commit = {
            let (bind, list, state) = (bind.clone(), list.clone(), state.clone());
            move || {
                let rows = state.borrow().clone();
                bind.set_domain(|e| apply_boot_order(e, &rows));
                let (bind, list) = (bind.clone(), list.clone());
                // Rebuild after the click handler has returned.
                gtk::glib::idle_add_local_once(move || fill_boot_list(&bind, &list));
            }
        };
        let (s, c) = (state.clone(), commit.clone());
        check.connect_toggled(move |b| {
            s.borrow_mut()[pos].1 = b.is_active();
            c();
        });
        let (s, c) = (state.clone(), commit.clone());
        up.connect_clicked(move |_| {
            s.borrow_mut().swap(pos, pos - 1);
            c();
        });
        let (s, c) = (state.clone(), commit);
        down.connect_clicked(move |_| {
            s.borrow_mut().swap(pos, pos + 1);
            c();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_boot_order_is_understood() {
        let d = vmng_xml::parse(
            "<domain><os><boot dev='cdrom'/><boot dev='hd'/></os><devices>\
             <disk device='disk'><target dev='vda'/></disk>\
             <disk device='cdrom'><target dev='sda'/></disk>\
             <interface type='network'/></devices></domain>",
        )
        .unwrap();
        let c = boot_candidates(&d);
        let order: Vec<(usize, bool)> = c.iter().map(|(i, o, _)| (*i, o.is_some())).collect();
        assert_eq!(order, [(1, true), (0, true), (2, false)]);
    }

    #[test]
    fn boot_order_is_renumbered() {
        let mut d = vmng_xml::parse(
            "<domain><os><boot dev='hd'/></os><devices>\
             <disk device='disk'><target dev='vda'/><boot order='2'/></disk>\
             <disk device='cdrom'><target dev='sda'/><boot order='1'/></disk>\
             <interface type='network'/></devices></domain>",
        )
        .unwrap();
        let c = boot_candidates(&d);
        assert_eq!(c.iter().map(|(i, _, _)| *i).collect::<Vec<_>>(), [1, 0, 2]);
        apply_boot_order(&mut d, &[(0, true), (1, false), (2, true)]);
        assert!(d.find("os/boot").is_none());
        let orders: Vec<_> = vmng_virt::list_devices(&d)
            .iter()
            .map(|x| x.element.find_attr("boot", "order").map(str::to_owned))
            .collect();
        assert_eq!(orders, [Some("1".to_owned()), None, Some("2".to_owned())]);
    }
}
