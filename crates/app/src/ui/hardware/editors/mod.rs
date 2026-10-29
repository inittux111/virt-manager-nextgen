//! Editors for the machine's settings and each kind of device.
//!
//! Every control reads from and writes to a working copy of the domain XML
//! through a [`Bind`]; nothing reaches libvirt until the person presses
//! Apply. A few changes that make sense immediately on a running machine
//! (changing the CD, plugging the network cable) go through [`Live`].

mod devices;
mod display;
mod machine;

pub use devices::{
    SHARE_9P, SHARE_VIRTIOFS, cdrom, controller, disk, enable_shared_memory, filesystem, input, interface,
    set_share_method, sound, tpm,
};
pub use display::{graphics, video};
pub use machine::{boot, cpu_memory, overview};

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
