//! Virtual networks: a list on the left, the selected network's settings
//! on the right, with Apply/Revert like the hardware page.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use vmng_virt::{DomainSummary, Ipv4Config, NetworkDetails, NetworkMode, NetworkSettings, suggest_subnet};

use super::feedback::Feedback;
use super::widgets::{FormDialog, ListDetail, StatusLabel, Tone, boxed_list, scrolled_page, section_heading};
use crate::backend::Conn;
use crate::settings::Settings;

fn settings_of(d: &NetworkDetails) -> NetworkSettings {
    NetworkSettings {
        name: d.name.clone(),
        mode: d.mode,
        forward_dev: d.forward_dev.clone(),
        bridge: if d.mode == NetworkMode::Bridge { d.bridge.clone() } else { None },
        ipv4: d.ipv4.clone(),
        ipv6: d.ipv6.clone(),
        dns_domain: d.dns_domain.clone(),
    }
}

const MODES: [(NetworkMode, &str, &str); 4] = [
    (NetworkMode::Nat, "NAT", "Share the host’s connection"),
    (NetworkMode::Routed, "Routed", "Route to the host’s network"),
    (NetworkMode::Isolated, "Isolated", "Machines only see each other"),
    (NetworkMode::Bridge, "Bridge", "Join a physical network"),
];

#[derive(Default)]
struct State {
    conn: Option<Conn>,
    networks: Vec<NetworkDetails>,
    bridges: Vec<String>,
    domains: Vec<DomainSummary>,
    selected: Option<String>,
    base: Option<NetworkSettings>,
    work: Option<NetworkSettings>,
    /// The saved XML as shown, and as edited.
    xml_base: String,
    xml_work: String,
    /// Guards against feedback loops while controls are filled in.
    filling: bool,
}

pub struct NetworksView {
    pub root: gtk::Overlay,
    list: gtk::ListBox,
    stack: gtk::Stack,
    title: gtk::Label,
    status: StatusLabel,
    toggle: gtk::Button,
    modes: Vec<(NetworkMode, gtk::ToggleButton)>,
    forward_dev: adw::EntryRow,
    bridge: adw::ComboRow,
    autostart: adw::SwitchRow,
    ipv4_group: adw::PreferencesGroup,
    ipv4: adw::EntryRow,
    dhcp: adw::SwitchRow,
    range: gtk::Label,
    ipv6: adw::EntryRow,
    dns: adw::EntryRow,
    error: gtk::Label,
    /// The settings groups, locked while the XML is being edited.
    form: Vec<gtk::Widget>,
    xml_buffer: gtk::TextBuffer,
    xml_view: gtk::TextView,
    xml_note: gtk::Label,
    xml_error: gtk::Label,
    apply_bar: gtk::Revealer,
    apply: gtk::Button,
    state: RefCell<State>,
    feedback: Feedback,
    settings: Settings,
}

impl std::fmt::Debug for NetworksView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetworksView").finish_non_exhaustive()
    }
}

impl NetworksView {
    pub fn new(feedback: Feedback, settings: Settings) -> Rc<Self> {
        let list = gtk::ListBox::builder().css_classes(["boxed-list", "hardware-list"]).build();
        list.update_property(&[gtk::accessible::Property::Label("Virtual networks")]);
        let left = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).width_request(280).build();
        left.append(&list);

        let title = gtk::Label::builder().xalign(0.0).css_classes(["title-2"]).build();
        let status = StatusLabel::new();
        status.root.add_css_class("dim-label");
        let toggle = gtk::Button::builder().valign(gtk::Align::Center).build();
        let titles = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).hexpand(true).build();
        titles.append(&title);
        titles.append(&status.root);
        let head = gtk::Box::builder().spacing(12).build();
        head.append(&titles);
        head.append(&toggle);

        // Four across, or two by two in narrow columns.
        let mode_box = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .min_children_per_line(2)
            .max_children_per_line(4)
            .column_spacing(8)
            .row_spacing(8)
            .build();
        let mut modes = Vec::new();
        for (mode, label, desc) in MODES {
            let b = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(3).build();
            b.append(&gtk::Label::builder().label(label).xalign(0.0).css_classes(["heading"]).build());
            b.append(
                // A short natural width keeps all four cards on one line when they fit.
                &gtk::Label::builder()
                    .label(desc)
                    .xalign(0.0)
                    .wrap(true)
                    .max_width_chars(16)
                    .css_classes(["caption", "dim-label"])
                    .build(),
            );
            let t = gtk::ToggleButton::builder().child(&b).css_classes(["method-card"]).build();
            if let Some((_, first)) = modes.first() {
                t.set_group(Some(first));
            }
            mode_box.append(&t);
            // The toggle itself takes focus, not its FlowBox cell.
            if let Some(cell) = t.parent() {
                cell.set_focusable(false);
            }
            modes.push((mode, t));
        }
        let connection = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).build();
        connection.append(&section_heading("Connection"));
        connection.append(&mode_box);

        let general = adw::PreferencesGroup::builder().title("General").build();
        let forward_dev = adw::EntryRow::builder().title("Forward to device (empty = any)").build();
        let bridge = adw::ComboRow::builder().title("Host bridge").build();
        let autostart = adw::SwitchRow::builder().title("Start when the host boots").build();
        general.add(&forward_dev);
        general.add(&bridge);
        general.add(&autostart);

        let ipv4_group = adw::PreferencesGroup::builder().title("IPv4").build();
        let ipv4 = adw::EntryRow::builder().title("Network (address/size)").build();
        let dhcp = adw::SwitchRow::builder().title("Hand out addresses (DHCP)").build();
        let range_row = adw::ActionRow::builder().title("Address range").build();
        // Breaks only between words, so "Off" never splits.
        let range = gtk::Label::builder()
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::Word)
            .justify(gtk::Justification::Right)
            .xalign(1.0)
            .css_classes(["dim-label"])
            .build();
        range_row.add_suffix(&range);
        ipv4_group.add(&ipv4);
        ipv4_group.add(&dhcp);
        ipv4_group.add(&range_row);

        let v6 = adw::PreferencesGroup::builder().title("IPv6 and DNS").build();
        let ipv6 = adw::EntryRow::builder().title("IPv6 network (optional, e.g. fd00:1::1/64)").build();
        let dns = adw::EntryRow::builder().title("DNS domain (optional)").build();
        v6.add(&ipv6);
        v6.add(&dns);

        let error = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).css_classes(["error"]).build();

        // The whole configuration as XML, like a machine's XML page.
        let xml_buffer = gtk::TextBuffer::new(None);
        let xml_view = gtk::TextView::builder()
            .buffer(&xml_buffer)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(12)
            .bottom_margin(12)
            .left_margin(12)
            .right_margin(12)
            .css_classes(["xml-view", "card"])
            .build();
        xml_view.update_property(&[gtk::accessible::Property::Label("Network XML")]);
        let xml_note = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["dim-label", "caption"]).build();
        let xml_error =
            gtk::Label::builder().xalign(0.0).wrap(true).visible(false).css_classes(["error", "caption"]).build();
        let xml_inner = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).margin_top(8).build();
        xml_inner.append(&xml_note);
        xml_inner.append(&xml_view);
        xml_inner.append(&xml_error);
        let xml = gtk::Expander::builder().label("XML").child(&xml_inner).build();
        // Debug builds: VMNG_DEV_NET_XML=1 opens the XML (screenshots).
        #[cfg(debug_assertions)]
        if std::env::var_os("VMNG_DEV_NET_XML").is_some() {
            xml.set_expanded(true);
            let x = xml.clone();
            glib::timeout_add_local_once(std::time::Duration::from_secs(3), move || {
                x.grab_focus();
            });
        }

        let delete_row = adw::ActionRow::builder()
            .title("Delete this network")
            .subtitle("Machines using it lose their network connection")
            .build();
        let delete = gtk::Button::builder()
            .label("Delete…")
            .valign(gtk::Align::Center)
            .css_classes(["destructive-action"])
            .build();
        delete_row.add_suffix(&delete);
        let danger = boxed_list();
        danger.append(&delete_row);

        let right = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(22).hexpand(true).build();
        for w in [
            head.upcast_ref::<gtk::Widget>(),
            connection.upcast_ref(),
            general.upcast_ref(),
            ipv4_group.upcast_ref(),
            v6.upcast_ref(),
            error.upcast_ref(),
            xml.upcast_ref(),
            danger.upcast_ref(),
        ] {
            right.append(w);
        }
        // Room below for the Apply bar.
        let split = ListDetail::new(&left, &right, 28, false, "Networks", 880.0, |b| {
            b.set_margin_bottom(72);
            scrolled_page(b, 1180)
        });
        let s = split.clone();
        list.connect_row_activated(move |_, _| s.show_detail());
        let scroller = split.root().clone();

        let empty_new = gtk::Button::builder()
            .label("New Network")
            .action_name("win.new-network")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        let empty = adw::StatusPage::builder()
            .icon_name("network-wired-symbolic")
            .title("No virtual networks")
            .description("Create one to give machines a private network or share the host’s connection.")
            .child(&empty_new)
            .build();
        let loading = adw::Spinner::builder().halign(gtk::Align::Center).valign(gtk::Align::Center).build();
        let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).vexpand(true).build();
        stack.add_named(&loading, Some("loading"));
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&scroller, Some("content"));

        let revert = gtk::Button::builder().label("Revert").css_classes(["flat", "pill"]).build();
        let apply = gtk::Button::builder().label("Apply").css_classes(["suggested-action", "pill"]).build();
        let bar = gtk::Box::builder().spacing(12).css_classes(["apply-bar"]).build();
        bar.append(
            &gtk::Label::builder()
                .label("Changes apply after the network restarts")
                .wrap(true)
                .xalign(0.0)
                .margin_start(8)
                .build(),
        );
        bar.append(&revert);
        bar.append(&apply);
        let apply_bar = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::End)
            .margin_bottom(16)
            .margin_start(12)
            .margin_end(12)
            .child(&bar)
            .build();
        let root = gtk::Overlay::builder().child(&stack).build();
        root.add_overlay(&apply_bar);

        let form = vec![connection.upcast(), general.clone().upcast(), ipv4_group.clone().upcast(), v6.upcast()];
        let this = Rc::new(Self {
            root,
            list,
            stack,
            title,
            status,
            toggle,
            modes,
            forward_dev,
            bridge,
            autostart,
            ipv4_group,
            ipv4,
            dhcp,
            range,
            ipv6,
            dns,
            error,
            form,
            xml_buffer,
            xml_view,
            xml_note,
            xml_error,
            apply_bar,
            apply,
            state: RefCell::default(),
            feedback,
            settings,
        });
        this.wire(&revert, &delete);
        this
    }

    fn wire(self: &Rc<Self>, revert: &gtk::Button, delete: &gtk::Button) {
        let weak = Rc::downgrade(self);
        self.list.connect_row_selected(move |_, row| {
            let Some(this) = weak.upgrade() else { return };
            let name = row.map(|r| r.widget_name().to_string());
            if name.is_some() && name != this.state.borrow().selected {
                this.state.borrow_mut().selected = name;
                this.show_selected();
            }
        });
        for (mode, button) in &self.modes {
            let (weak, mode) = (Rc::downgrade(self), *mode);
            button.connect_toggled(move |b| {
                if b.is_active()
                    && let Some(this) = weak.upgrade()
                {
                    this.edit(|s| s.mode = mode);
                }
            });
        }
        let weak = Rc::downgrade(self);
        self.forward_dev.connect_changed(move |e| {
            if let Some(this) = weak.upgrade() {
                let v = e.text().trim().to_owned();
                this.edit(|s| s.forward_dev = Some(v).filter(|v| !v.is_empty()));
            }
        });
        let weak = Rc::downgrade(self);
        self.bridge.connect_selected_notify(move |r| {
            if let Some(this) = weak.upgrade() {
                let b = this.state.borrow().bridges.get(r.selected() as usize).cloned();
                this.edit(|s| s.bridge = b);
            }
        });
        let weak = Rc::downgrade(self);
        self.ipv4.connect_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                this.edit_ipv4();
            }
        });
        let weak = Rc::downgrade(self);
        self.dhcp.connect_active_notify(move |_| {
            if let Some(this) = weak.upgrade() {
                this.edit_ipv4();
            }
        });
        let weak = Rc::downgrade(self);
        self.ipv6.connect_changed(move |e| {
            if let Some(this) = weak.upgrade() {
                let v = e.text().trim().to_owned();
                this.edit(|s| s.ipv6 = Some(v).filter(|v| !v.is_empty()));
            }
        });
        let weak = Rc::downgrade(self);
        self.dns.connect_changed(move |e| {
            if let Some(this) = weak.upgrade() {
                let v = e.text().trim().to_owned();
                this.edit(|s| s.dns_domain = Some(v).filter(|v| !v.is_empty()));
            }
        });
        let weak = Rc::downgrade(self);
        self.xml_buffer.connect_changed(move |buf| {
            let Some(this) = weak.upgrade() else { return };
            if this.state.borrow().filling {
                return;
            }
            let text = buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string();
            match vmng_xml::parse(&text) {
                Ok(_) => this.xml_error.set_visible(false),
                Err(e) => {
                    this.xml_error.set_label(&e.to_string());
                    this.xml_error.set_visible(true);
                }
            }
            this.state.borrow_mut().xml_work = text;
            this.update_bar();
        });
        let weak = Rc::downgrade(self);
        self.autostart.connect_active_notify(move |r| {
            let Some(this) = weak.upgrade() else { return };
            if this.state.borrow().filling {
                return;
            }
            let Some(name) = this.state.borrow().selected.clone() else { return };
            let on = r.is_active();
            this.run(move |c| c.set_network_autostart(&name, on), "Couldn't change start at boot", None);
        });
        let weak = Rc::downgrade(self);
        self.toggle.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.toggle_active();
            }
        });
        let weak = Rc::downgrade(self);
        revert.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.show_selected();
            }
        });
        let weak = Rc::downgrade(self);
        self.apply.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.save();
            }
        });
        let weak = Rc::downgrade(self);
        delete.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.ask_delete();
            }
        });
    }

    /// Loads the networks (when the view is shown, and after changes).
    pub fn load(self: &Rc<Self>, conn: Conn, domains: &[DomainSummary]) {
        {
            let mut st = self.state.borrow_mut();
            st.conn = Some(conn.clone());
            st.domains = domains.to_vec();
            if st.networks.is_empty() {
                self.stack.set_visible_child_name("loading");
            }
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn.call(|c| Ok((c.network_details()?, c.list_bridges()))).await;
            let Some(this) = weak.upgrade() else { return };
            match result {
                Ok((nets, bridges)) => {
                    {
                        let mut st = this.state.borrow_mut();
                        st.networks = nets;
                        st.bridges = bridges;
                    }
                    this.fill_list();
                }
                Err(e) => this.feedback.error("Couldn't load the virtual networks", e.message()),
            }
        });
    }

    fn users(&self, d: &NetworkDetails) -> Vec<String> {
        let st = self.state.borrow();
        st.domains
            .iter()
            .filter(|vm| {
                vm.config.interfaces.iter().any(|i| {
                    (i.kind == "network" && i.source.as_deref() == Some(d.name.as_str()))
                        || (d.mode == NetworkMode::Bridge && i.kind == "bridge" && i.source == d.bridge)
                })
            })
            .map(|vm| vm.display_name().to_owned())
            .collect()
    }

    fn fill_list(self: &Rc<Self>) {
        let nets = self.state.borrow().networks.clone();
        self.list.remove_all();
        if nets.is_empty() {
            self.stack.set_visible_child_name("empty");
            self.apply_bar.set_reveal_child(false);
            return;
        }
        for n in &nets {
            let status = StatusLabel::new();
            status.set(if n.active { Tone::Running } else { Tone::Off }, &n.name);
            status.root.add_css_class("heading");
            let desc =
                gtk::Label::builder().label(n.describe()).xalign(0.0).css_classes(["caption", "dim-label"]).build();
            let text = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(2)
                .css_classes(["hardware-row"])
                .build();
            text.append(&status.root);
            text.append(&desc);
            let row = gtk::ListBoxRow::builder().child(&text).name(&n.name).build();
            self.list.append(&row);
        }
        self.stack.set_visible_child_name("content");
        let wanted = self.state.borrow().selected.clone();
        let index = wanted.and_then(|w| nets.iter().position(|n| n.name == w)).unwrap_or(0);
        self.state.borrow_mut().selected = None;
        self.list.select_row(self.list.row_at_index(index as i32).as_ref());
        #[cfg(debug_assertions)]
        self.dev_edit_xml(&nets);
    }

    /// Debug builds: `VMNG_DEV_NET_EDIT=<vmng-test-… network>` selects it,
    /// adds an MTU in its XML and presses Apply (tests).
    #[cfg(debug_assertions)]
    fn dev_edit_xml(self: &Rc<Self>, nets: &[NetworkDetails]) {
        thread_local!(static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
        let Ok(name) = std::env::var("VMNG_DEV_NET_EDIT") else { return };
        if !name.starts_with("vmng-test-") || DONE.with(|d| d.replace(true)) {
            return;
        }
        let Some(i) = nets.iter().position(|n| n.name == name) else { return };
        self.list.select_row(self.list.row_at_index(i as i32).as_ref());
        let text = self.state.borrow().xml_base.replace("</network>", "  <mtu size=\"1400\"/>\n</network>");
        self.xml_buffer.set_text(&text);
        assert!(self.apply_bar.reveals_child() && self.apply.is_sensitive());
        self.apply.emit_clicked();
    }

    fn details(&self) -> Option<NetworkDetails> {
        let st = self.state.borrow();
        let name = st.selected.as_ref()?;
        st.networks.iter().find(|n| &n.name == name).cloned()
    }

    /// Fills the controls from the selected network.
    fn show_selected(self: &Rc<Self>) {
        let Some(d) = self.details() else { return };
        let settings = settings_of(&d);
        {
            let mut st = self.state.borrow_mut();
            st.filling = true;
            st.base = Some(settings.clone());
            st.work = Some(settings.clone());
            st.xml_base = d.config.to_xml().trim_end().to_owned();
            st.xml_work = st.xml_base.clone();
        }
        let xml = self.state.borrow().xml_base.clone();
        self.xml_buffer.set_text(&xml);
        self.xml_error.set_visible(false);
        self.title.set_label(&d.name);
        let users = self.users(&d);
        let used = if users.is_empty() {
            "not used by any machine".to_owned()
        } else {
            format!("used by {}", users.join(", "))
        };
        self.status.set(
            if d.active { Tone::Running } else { Tone::Off },
            &format!("{} · {used}", if d.active { "Active" } else { "Stopped" }),
        );
        self.toggle.set_label(if d.active { "Stop Network" } else { "Start Network" });
        let editable = d.mode != NetworkMode::Other;
        for (mode, b) in &self.modes {
            b.set_active(*mode == d.mode);
            b.set_sensitive(editable);
        }
        let bridges = self.state.borrow().bridges.clone();
        let refs: Vec<&str> = bridges.iter().map(String::as_str).collect();
        self.bridge.set_model(Some(&gtk::StringList::new(&refs)));
        self.bridge
            .set_selected(d.bridge.as_ref().and_then(|b| bridges.iter().position(|x| x == b)).unwrap_or(0) as u32);
        self.forward_dev.set_text(d.forward_dev.as_deref().unwrap_or_default());
        self.ipv4.set_text(&d.ipv4.as_ref().map(Ipv4Config::cidr).unwrap_or_default());
        self.dhcp.set_active(d.ipv4.as_ref().is_some_and(|ip| ip.dhcp.is_some()));
        self.ipv6.set_text(d.ipv6.as_deref().unwrap_or_default());
        self.dns.set_text(d.dns_domain.as_deref().unwrap_or_default());
        self.autostart.set_active(d.autostart);
        self.autostart.set_sensitive(d.persistent);
        self.state.borrow_mut().filling = false;
        self.update_mode_rows();
        self.update_bar();
    }

    fn edit(self: &Rc<Self>, f: impl FnOnce(&mut NetworkSettings)) {
        {
            let mut st = self.state.borrow_mut();
            if st.filling {
                return;
            }
            if let Some(w) = st.work.as_mut() {
                f(w);
            }
        }
        self.update_mode_rows();
        self.update_bar();
    }

    fn edit_ipv4(self: &Rc<Self>) {
        let text = self.ipv4.text().to_string();
        let dhcp = self.dhcp.is_active();
        let parsed = if text.trim().is_empty() { Ok(None) } else { Ipv4Config::from_cidr(&text).map(Some) };
        match parsed {
            Ok(ip) => {
                self.error.set_visible(false);
                self.ipv4.remove_css_class("error");
                self.edit(|s| {
                    // Keep a custom DHCP range when the network didn't change.
                    let same = s.ipv4.as_ref().map(Ipv4Config::cidr) == ip.as_ref().map(Ipv4Config::cidr);
                    let mut ip = if same { s.ipv4.clone().or(ip) } else { ip };
                    if let Some(ip) = ip.as_mut() {
                        if !dhcp {
                            ip.dhcp = None;
                        } else if ip.dhcp.is_none() {
                            ip.dhcp = Ipv4Config::from_cidr(&ip.cidr()).ok().and_then(|f| f.dhcp);
                        }
                    }
                    s.ipv4 = ip;
                });
            }
            Err(msg) => {
                self.error.set_label(msg);
                self.error.set_visible(true);
                self.ipv4.add_css_class("error");
                self.apply.set_sensitive(false);
            }
        }
    }

    fn update_mode_rows(&self) {
        let st = self.state.borrow();
        let Some(w) = &st.work else { return };
        let bridge = w.mode == NetworkMode::Bridge;
        self.bridge.set_visible(bridge);
        self.forward_dev.set_visible(matches!(w.mode, NetworkMode::Nat | NetworkMode::Routed));
        self.ipv4_group.set_visible(!bridge);
        self.ipv6.set_visible(!bridge);
        self.dns.set_visible(!bridge);
        self.range.set_label(
            &w.ipv4.as_ref().and_then(|ip| ip.dhcp).map_or_else(|| "Off".to_owned(), |(a, b)| format!("{a} – {b}")),
        );
    }

    fn update_bar(&self) {
        let st = self.state.borrow();
        let form_dirty = st.base != st.work;
        let xml_dirty = st.xml_base != st.xml_work;
        let dirty = form_dirty || xml_dirty;
        self.apply_bar.set_reveal_child(dirty);
        self.apply.set_sensitive(dirty && !self.error.is_visible() && !self.xml_error.is_visible());
        // One kind of change at a time: the settings or the XML.
        for w in &self.form {
            w.set_sensitive(!xml_dirty);
        }
        let editing = self.settings.get().xml_editing;
        self.xml_view.set_editable(editing && !form_dirty);
        self.xml_note.set_label(if !editing {
            "Turn on “Allow editing the raw XML” in Preferences to change this."
        } else if form_dirty {
            "Apply or revert the changed settings to edit the XML."
        } else if xml_dirty {
            "The settings above update after you apply the XML."
        } else {
            "Changes here replace the network’s configuration when you press Apply."
        });
    }

    fn run(
        self: &Rc<Self>,
        op: impl FnOnce(&vmng_virt::Connection) -> vmng_virt::Result<()> + Send + 'static,
        failure: &'static str,
        success: Option<String>,
    ) {
        let Some(conn) = self.state.borrow().conn.clone() else { return };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn.call(op).await;
            let Some(this) = weak.upgrade() else { return };
            match result {
                Ok(()) => {
                    if let Some(msg) = success {
                        this.feedback.toast(&msg);
                    }
                }
                Err(e) => this.feedback.error(failure, e.message()),
            }
            let (conn, domains) = {
                let st = this.state.borrow();
                (st.conn.clone(), st.domains.clone())
            };
            if let Some(conn) = conn {
                this.load(conn, &domains);
            }
        });
    }

    fn toggle_active(self: &Rc<Self>) {
        let Some(d) = self.details() else { return };
        let name = d.name.clone();
        if d.active {
            let users = self.users(&d);
            let body = if users.is_empty() {
                "No machine uses it right now.".to_owned()
            } else {
                format!("{} will lose their network connection.", users.join(", "))
            };
            let weak = Rc::downgrade(self);
            self.feedback.confirm(&format!("Stop {name}?"), &body, "Stop", true, move || {
                if let Some(this) = weak.upgrade() {
                    let n = name.clone();
                    this.run(
                        move |c| c.set_network_active(&n, false),
                        "Couldn't stop the network",
                        Some(format!("Stopped {name}")),
                    );
                }
            });
        } else {
            let n = name.clone();
            self.run(
                move |c| c.set_network_active(&n, true),
                "Couldn't start the network",
                Some(format!("Started {name}")),
            );
        }
    }

    fn save(self: &Rc<Self>) {
        let (Some(d), Some(work)) = (self.details(), self.state.borrow().work.clone()) else { return };
        let name = d.name.clone();
        let Some(conn) = self.state.borrow().conn.clone() else { return };
        let xml = {
            let st = self.state.borrow();
            (st.xml_work != st.xml_base).then(|| st.xml_work.clone())
        };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let n = name.clone();
            let result = conn
                .call(move |c| match xml {
                    Some(xml) => c.define_network_xml(&n, &xml),
                    None => c.update_network(&n, &work),
                })
                .await;
            let Some(this) = weak.upgrade() else { return };
            match result {
                Err(e) => this.feedback.error("Couldn't save the network", e.message()),
                Ok(()) if d.active => {
                    let weak = Rc::downgrade(&this);
                    let n = name.clone();
                    this.feedback.confirm(
                        &format!("Restart {name} now?"),
                        "The new settings are used after a restart. Machines on this network lose their connection for a moment.",
                        "Restart",
                        false,
                        move || {
                            if let Some(this) = weak.upgrade() {
                                let n2 = n.clone();
                                this.run(move |c| c.restart_network(&n2), "Couldn't restart the network", Some(format!("Restarted {n}")));
                            }
                        },
                    );
                    this.feedback.toast("Saved the network settings");
                }
                Ok(()) => this.feedback.toast("Saved the network settings"),
            }
            let (conn, domains) = {
                let st = this.state.borrow();
                (st.conn.clone(), st.domains.clone())
            };
            if let Some(conn) = conn {
                this.load(conn, &domains);
            }
        });
    }

    fn ask_delete(self: &Rc<Self>) {
        let Some(d) = self.details() else { return };
        let users = self.users(&d);
        let mut body = "This can't be undone.".to_owned();
        if !users.is_empty() {
            body = format!("{} will lose their network connection. {body}", users.join(", "));
        }
        let name = d.name.clone();
        let weak = Rc::downgrade(self);
        self.feedback.confirm(&format!("Delete {name}?"), &body, "Delete", true, move || {
            if let Some(this) = weak.upgrade() {
                this.state.borrow_mut().selected = None;
                let n = name.clone();
                this.run(move |c| c.delete_network(&n), "Couldn't delete the network", Some(format!("Deleted {name}")));
            }
        });
    }

    /// The New Network dialog.
    pub fn new_network(self: &Rc<Self>, parent: &impl IsA<gtk::Widget>) {
        let form = FormDialog::new("New Virtual Network", "Create");
        let st = self.state.borrow();
        let suggested = suggest_subnet(&st.networks);
        let taken: Vec<String> = st.networks.iter().map(|n| n.name.clone()).collect();
        let bridges = st.bridges.clone();
        drop(st);
        let group = adw::PreferencesGroup::new();
        let default_name = vmng_virt::unique_name("network", |n| taken.iter().any(|t| t == n));
        // Debug builds: VMNG_DEV_FORM_NAME=<name> pre-fills the name (tests).
        #[cfg(debug_assertions)]
        let default_name = std::env::var("VMNG_DEV_FORM_NAME").unwrap_or(default_name);
        let name = adw::EntryRow::builder().title("Name").text(&default_name).build();
        let labels: Vec<&str> = MODES.iter().map(|(_, l, _)| *l).collect();
        let mode = adw::ComboRow::builder().title("Connection").model(&gtk::StringList::new(&labels)).build();
        let subnet = adw::EntryRow::builder().title("IPv4 network").text(&suggested).build();
        let refs: Vec<&str> = bridges.iter().map(String::as_str).collect();
        let bridge =
            adw::ComboRow::builder().title("Host bridge").model(&gtk::StringList::new(&refs)).visible(false).build();
        let autostart = adw::SwitchRow::builder().title("Start when the host boots").active(true).build();
        for w in [
            name.upcast_ref::<gtk::Widget>(),
            mode.upcast_ref(),
            subnet.upcast_ref(),
            bridge.upcast_ref(),
            autostart.upcast_ref(),
        ] {
            group.add(w);
        }
        form.add(&group);
        let (s, b) = (subnet.clone(), bridge.clone());
        mode.connect_selected_notify(move |m| {
            let is_bridge = MODES[m.selected() as usize].0 == NetworkMode::Bridge;
            s.set_visible(!is_bridge);
            b.set_visible(is_bridge);
        });
        let weak = Rc::downgrade(self);
        form.connect_primary(move |form| {
            let Some(this) = weak.upgrade() else { return };
            let chosen = MODES[mode.selected() as usize].0;
            let ipv4 = if chosen == NetworkMode::Bridge {
                None
            } else {
                match Ipv4Config::from_cidr(&subnet.text()) {
                    Ok(ip) => Some(ip),
                    Err(m) => return form.set_error(Some(m)),
                }
            };
            let n = name.text().trim().to_owned();
            if taken.contains(&n) {
                return form.set_error(Some("Another network already has this name."));
            }
            let settings = NetworkSettings {
                name: n.clone(),
                mode: chosen,
                forward_dev: None,
                bridge: bridges.get(bridge.selected() as usize).cloned(),
                ipv4,
                ipv6: None,
                dns_domain: None,
            };
            if let Err(m) = vmng_virt::apply_network_settings(&mut vmng_xml::Element::new("network"), &settings) {
                return form.set_error(Some(m));
            }
            let auto = autostart.is_active();
            form.close();
            this.state.borrow_mut().selected = Some(n.clone());
            this.run(
                move |c| c.create_network(&settings, auto),
                "Couldn't create the network",
                Some(format!("Created {n}")),
            );
        });
        form.present(parent);
    }
}
