//! Editors for the machine's settings and each kind of device.
//!
//! Every control reads from and writes to a working copy of the domain XML
//! through a [`Bind`]; nothing reaches libvirt until the person presses
//! Apply. A few changes that make sense immediately on a running machine
//! (changing the CD, plugging the network cable) go through [`Live`].

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use vmng_virt::{
    CpuMode, DeviceKind, DomainConfig, FirmwareChoice, InstallImage, MemorySize, NetworkSummary, PoolSummary, Topology,
    disk_prefix, mitigations_on, next_disk_target, normalize_mac, set_cpu_mode, set_firmware, set_memory,
    set_mitigations, set_os_id, set_topology, set_video_3d, units, video_3d,
};
use vmng_xml::{Element, Node};

use crate::format;

/// Which part of the working XML an editor changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Domain,
    Device(usize),
}

/// Connects controls to the working copy of the domain XML.
#[derive(Clone)]
pub struct Bind {
    pub work: Rc<RefCell<Element>>,
    pub target: Target,
    pub changed: Rc<dyn Fn()>,
}

impl Bind {
    fn with<R>(el: &mut Element, target: Target, f: impl FnOnce(&mut Element) -> R) -> Option<R> {
        match target {
            Target::Domain => Some(f(el)),
            Target::Device(i) => match el.child_mut("devices")?.children.get_mut(i)? {
                Node::Element(e) => Some(f(e)),
                _ => None,
            },
        }
    }

    pub fn get<R: Default>(&self, f: impl FnOnce(&Element) -> R) -> R {
        let mut w = self.work.borrow_mut();
        Self::with(&mut w, self.target, |e| f(e)).unwrap_or_default()
    }

    pub fn set(&self, f: impl FnOnce(&mut Element)) {
        {
            let mut w = self.work.borrow_mut();
            Self::with(&mut w, self.target, f);
        }
        (self.changed)();
    }

    /// The whole domain, for edits that touch more than the device.
    pub fn set_domain(&self, f: impl FnOnce(&mut Element)) {
        f(&mut self.work.borrow_mut());
        (self.changed)();
    }
}

/// An immediate change to a running (or stopped) machine.
pub type Live = Rc<dyn Fn(Element, String)>;

/// Things the host offers that editors let people choose from.
#[derive(Clone, Debug, Default)]
pub struct Choices {
    pub networks: Vec<NetworkSummary>,
    pub bridges: Vec<String>,
    pub images: Vec<InstallImage>,
    pub pools: Vec<PoolSummary>,
    pub session: bool,
    /// The machine's architecture, and the UEFI firmware files libvirt
    /// offers for it.
    pub arch: String,
    pub loaders: Vec<String>,
    /// Named CPU models the host can run, and its CPU security mitigations.
    pub cpu_models: Vec<String>,
    pub cpu_security: Vec<String>,
    /// The host's wired network cards (for a direct connection).
    pub host_nics: Vec<String>,
    /// The other computer's name when the machine runs elsewhere (SSH).
    pub remote: Option<String>,
}

// ── Row helpers ─────────────────────────────────────────────────────────

fn group(title: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder().title(title).build()
}

fn fact(group: &adw::PreferencesGroup, title: &str, value: &str) {
    let row = adw::ActionRow::builder().title(title).build();
    let label = gtk::Label::builder()
        .label(if value.is_empty() { "—" } else { value })
        .tooltip_text(value)
        .selectable(true)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .xalign(1.0)
        .css_classes(["dim-label"])
        .build();
    row.add_suffix(&label);
    group.add(&row);
}

/// A drop-down bound to a value. Unknown current values are kept as an
/// extra option so nothing is changed by just looking.
fn combo(
    bind: &Bind,
    title: &str,
    options: &[(&str, &str)],
    get: impl Fn(&Element) -> String + 'static,
    set: impl Fn(&mut Element, &str) + 'static,
) -> adw::ComboRow {
    let mut opts: Vec<(String, String)> = options.iter().map(|(v, l)| ((*v).to_owned(), (*l).to_owned())).collect();
    let current = bind.get(&get);
    if !opts.iter().any(|(v, _)| *v == current) {
        opts.push((current.clone(), if current.is_empty() { "Default".into() } else { current.clone() }));
    }
    let labels: Vec<&str> = opts.iter().map(|(_, l)| l.as_str()).collect();
    let row = adw::ComboRow::builder().title(title).model(&gtk::StringList::new(&labels)).build();
    // Set before the selection: changing it later resets the selection,
    // which would count as an edit. It lets long lists be searched.
    row.set_expression(Some(gtk::PropertyExpression::new(
        gtk::StringObject::static_type(),
        None::<gtk::Expression>,
        "string",
    )));
    row.set_selected(opts.iter().position(|(v, _)| *v == current).unwrap_or(0) as u32);
    let bind = bind.clone();
    row.connect_selected_notify(move |r| {
        if let Some((v, _)) = opts.get(r.selected() as usize) {
            let v = v.clone();
            bind.set(|e| set(e, &v));
        }
    });
    row
}

fn switch(
    bind: &Bind,
    title: &str,
    subtitle: &str,
    get: impl Fn(&Element) -> bool,
    set: impl Fn(&mut Element, bool) + 'static,
) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder().title(title).subtitle(subtitle).active(bind.get(get)).build();
    let bind = bind.clone();
    row.connect_active_notify(move |r| {
        let on = r.is_active();
        bind.set(|e| set(e, on));
    });
    row
}

fn entry(
    bind: &Bind,
    title: &str,
    get: impl Fn(&Element) -> String,
    set: impl Fn(&mut Element, &str) + 'static,
) -> adw::EntryRow {
    let row = adw::EntryRow::builder().title(title).text(bind.get(get)).build();
    let bind = bind.clone();
    row.connect_changed(move |r| {
        let text = r.text().to_string();
        bind.set(|e| set(e, text.trim()));
    });
    row
}

fn spin(
    bind: &Bind,
    title: &str,
    (min, max, step, digits): (f64, f64, f64, u32),
    get: impl Fn(&Element) -> f64,
    set: impl Fn(&mut Element, f64) + 'static,
) -> adw::SpinRow {
    let row = adw::SpinRow::with_range(min, max, step);
    row.set_title(title);
    row.set_digits(digits);
    row.set_value(bind.get(get));
    let bind = bind.clone();
    row.connect_value_notify(move |r| {
        let v = r.value();
        bind.set(|e| set(e, v));
    });
    row
}

fn vbox() -> gtk::Box {
    gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(22).build()
}

/// A note inside a group of rows.
fn note_row(text: &str) -> gtk::Label {
    let l = note(text);
    l.set_margin_top(8);
    l
}

fn note(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).wrap(true).css_classes(["dim-label", "caption"]).build()
}

// ── XML helpers ─────────────────────────────────────────────────────────

fn set_attr_path(e: &mut Element, child: &str, attr: &str, value: &str) {
    if value.is_empty() {
        if let Some(c) = e.child_mut(child) {
            c.remove_attr(attr);
        }
    } else {
        e.child_or_insert(child).set_attr(attr, value);
    }
}

fn toggle_child(e: &mut Element, name: &str, on: bool) {
    let present = e.child(name).is_some();
    if on && !present {
        e.push(Element::new(name));
    } else if !on {
        e.remove_children(|c| c.name == name);
    }
}

/// libvirt assigns addresses for a bus; after a bus change the old one is
/// invalid, so it is dropped and libvirt picks a new one.
fn drop_address(e: &mut Element) {
    e.remove_children(|c| c.name == "address");
}

// ── Editors ─────────────────────────────────────────────────────────────

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
fn set_text_child(e: &mut Element, name: &str, text: &str) {
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
fn bootable(e: &Element) -> bool {
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
fn operating_system(bind: &Bind) -> adw::PreferencesGroup {
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

fn boot_candidates(e: &Element) -> Vec<(usize, Option<u32>, String)> {
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
            (d.index, order, super::labels::describe(&d).0)
        })
        .collect();
    out.sort_by_key(|(i, o, _)| (o.unwrap_or(u32::MAX), *i));
    out
}

/// Rewrites boot orders from a list of (device index, enabled) in order.
fn apply_boot_order(e: &mut Element, rows: &[(usize, bool)]) {
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

fn fill_boot_list(bind: &Bind, list: &gtk::ListBox) {
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

/// A disk (not CD-ROM).
pub fn disk(bind: &Bind) -> gtk::Box {
    let b = vbox();
    let info = group("Disk");
    fact(&info, "Source", &bind.get(|e| super::labels::disk_source(e).unwrap_or_default()));
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
fn bus_combo(bind: &Bind, options: &[(&str, &str)]) -> adw::ComboRow {
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
    let current = bind.get(super::labels::disk_source);
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
        super::super::widgets::pick_host_path(
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

/// The graphical display (SPICE/VNC).
pub fn graphics(bind: &Bind) -> gtk::Box {
    let b = vbox();
    let g = group("Display");
    g.add(&combo(
        bind,
        "Protocol",
        &[("spice", "SPICE (recommended)"), ("vnc", "VNC")],
        |e| e.attr("type").unwrap_or_default().to_owned(),
        |e, v| {
            e.set_attr("type", v);
            // Protocol-specific settings don't carry over.
            e.remove_children(|c| {
                matches!(
                    c.name.as_str(),
                    "image" | "streaming" | "clipboard" | "mouse" | "filetransfer" | "gl" | "channel"
                )
            });
            e.remove_attr("tlsPort");
        },
    ));
    g.add(&combo(
        bind,
        "Reachable from",
        &[("127.0.0.1", "This computer only"), ("0.0.0.0", "Any network (less safe)"), ("none", "Local socket only")],
        |e| match e.find_attr("listen", "type") {
            Some("none") => "none".into(),
            _ => e.find_attr("listen", "address").or(e.attr("listen")).unwrap_or("127.0.0.1").to_owned(),
        },
        |e, v| {
            e.remove_children(|c| c.name == "listen");
            e.remove_attr("listen");
            let listen = if v == "none" {
                Element::new("listen").with_attr("type", "none")
            } else {
                e.set_attr("listen", v);
                Element::new("listen").with_attr("type", "address").with_attr("address", v)
            };
            e.push(listen);
        },
    ));
    g.add(&switch(
        bind,
        "Choose port automatically",
        "Recommended when several machines run",
        |e| e.attr("autoport") != Some("no"),
        |e, on| {
            e.set_attr("autoport", if on { "yes" } else { "no" });
            if on {
                e.remove_attr("port");
            } else if e.attr("port").is_none_or(|p| p == "-1") {
                e.set_attr("port", "5900");
            }
        },
    ));
    g.add(&spin(
        bind,
        "Port (when not automatic)",
        (5900.0, 65535.0, 1.0, 0),
        |e| e.attr("port").and_then(|p| p.parse().ok()).filter(|&p: &f64| p > 0.0).unwrap_or(5900.0),
        |e, v| {
            if e.attr("autoport") == Some("no") {
                e.set_attr("port", (v as u32).to_string());
            }
        },
    ));
    let vnc = bind.get(|e| e.attr("type") == Some("vnc"));
    let password = adw::PasswordEntryRow::builder()
        .title(if vnc { "Password (optional, up to 8 characters)" } else { "Password (optional)" })
        .text(bind.get(|e| e.attr("passwd").unwrap_or_default().to_owned()))
        .build();
    let bind2 = bind.clone();
    password.connect_changed(move |r| {
        let text = r.text().to_string();
        // VNC passwords are at most 8 characters; QEMU won't start with a
        // longer one, so it isn't saved until it fits.
        let too_long = bind2.get(|e| e.attr("type") == Some("vnc")) && text.chars().count() > VNC_PASSWORD_MAX;
        if too_long {
            r.add_css_class("error");
            return;
        }
        r.remove_css_class("error");
        bind2.set(|e| {
            if text.is_empty() {
                e.remove_attr("passwd");
            } else {
                e.set_attr("passwd", text);
            }
        });
    });
    g.add(&password);
    b.append(&g);
    b
}

/// The longest password VNC accepts.
const VNC_PASSWORD_MAX: usize = 8;

pub fn video(bind: &Bind) -> gtk::Box {
    let b = vbox();
    let g = group("Video");
    let model = combo(
        bind,
        "Model",
        &[
            ("virtio", "VirtIO (recommended for Linux)"),
            ("qxl", "QXL"),
            ("vga", "VGA"),
            ("bochs", "Bochs"),
            ("ramfb", "ramfb"),
            ("cirrus", "Cirrus (very old systems)"),
            ("none", "None"),
        ],
        |e| e.find_attr("model", "type").unwrap_or_default().to_owned(),
        |e, v| {
            let m = e.child_or_insert("model");
            *m = Element::new("model").with_attr("type", v);
            drop_address(e);
        },
    );
    g.add(&model);
    let Target::Device(index) = bind.target else { return b };
    let accel = adw::SwitchRow::builder()
        .title("3D acceleration")
        .subtitle("Draws with the host's graphics card: smoother Linux desktops. VirtIO only")
        .active(bind.get(video_3d))
        .build();
    let is_virtio = |b: &Bind| b.get(|e| e.find_attr("model", "type") == Some("virtio"));
    accel.set_sensitive(is_virtio(bind));
    {
        let bind = bind.clone();
        accel.connect_active_notify(move |r| {
            let on = r.is_active();
            bind.set_domain(|d| set_video_3d(d, index, on));
        });
    }
    {
        // Another model has no 3D acceleration.
        let (bind, accel) = (bind.clone(), accel.clone());
        model.connect_selected_notify(move |_| {
            let virtio = is_virtio(&bind);
            accel.set_sensitive(virtio);
            if !virtio {
                accel.set_active(false);
                bind.set_domain(|d| set_video_3d(d, index, false));
            }
        });
    }
    g.add(&accel);
    b.append(&g);
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

fn mount_hint(e: &Element) -> String {
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

/// Read-only facts for devices without a dedicated editor.
pub fn generic(bind: &Bind, kind: &DeviceKind) -> gtk::Box {
    let b = vbox();
    let g = group("Details");
    let attrs: Vec<(String, String)> = bind.get(|e| {
        let mut v: Vec<(String, String)> = e.attrs.clone();
        for c in e.elements().filter(|c| c.name != "address" && c.name != "alias") {
            for (k, val) in &c.attrs {
                v.push((format!("{} {k}", c.name), val.clone()));
            }
            let t = c.text();
            if !t.is_empty() {
                v.push((c.name.clone(), t));
            }
        }
        v
    });
    if matches!(kind, DeviceKind::Channel)
        && bind.get(|e| e.find_attr("target", "name") == Some("org.qemu.guest_agent.0"))
    {
        b.append(&note(
            "Lets the qemu-guest-agent in the guest report its IP address and hostname, and shut down cleanly. Install the qemu-guest-agent package in the guest.",
        ));
    }
    for (k, v) in attrs {
        fact(&g, &k, &v);
    }
    b.prepend(&g);
    b
}

/// The XML of the device (or whole machine): read-only unless XML editing
/// is turned on in Preferences.
pub fn xml_view(bind: &Bind, editable: bool, expanded: bool) -> gtk::Box {
    let b = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).build();
    let buffer = gtk::TextBuffer::new(None);
    buffer.set_text(bind.get(|e| e.to_xml()).trim_end());
    let view = gtk::TextView::builder()
        .buffer(&buffer)
        .monospace(true)
        .editable(editable)
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(12)
        .bottom_margin(12)
        .left_margin(12)
        .right_margin(12)
        .css_classes(["xml-view", "card"])
        .build();
    view.update_property(&[gtk::accessible::Property::Label("XML")]);
    let error = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).css_classes(["error", "caption"]).build();
    let expander = gtk::Expander::builder().label("XML").expanded(expanded).build();
    let inner = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).margin_top(8).build();
    if editable {
        let bind = bind.clone();
        let err = error.clone();
        buffer.connect_changed(move |buf| {
            let text = buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string();
            match vmng_xml::parse(&text) {
                Ok(el) => {
                    err.set_visible(false);
                    if bind.get(|e| *e != el) {
                        bind.set(|e| *e = el);
                    }
                }
                Err(e) => {
                    err.set_label(&e.to_string());
                    err.set_visible(true);
                }
            }
        });
    } else {
        inner.append(&note("Turn on “Allow editing the raw XML” in Preferences to change this."));
    }
    inner.append(&view);
    inner.append(&error);
    expander.set_child(Some(&inner));
    b.append(&expander);
    b
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
