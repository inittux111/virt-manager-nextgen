//! The Hardware tab: every setting and device of a VM, with an editor for
//! the selected one. Edits collect in a working copy and are saved together
//! with Apply (or dropped with Revert).

mod add;
mod editors;
mod labels;

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use vmng_virt::{DeviceKind, DomainSummary, HostInfo, MemorySize, list_devices};
use vmng_xml::Element;

use self::editors::{Bind, Choices, Live, Target};
use super::feedback::Feedback;
use crate::backend::Conn;
use crate::settings::Settings;

/// An entry of the hardware list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    Overview,
    CpuMemory,
    Boot,
    Device(usize),
    Xml,
}

struct Row {
    item: Item,
    row: gtk::ListBoxRow,
    title: gtk::Label,
    subtitle: gtk::Label,
}

#[derive(Default)]
struct HwState {
    conn: Option<Conn>,
    uuid: Option<String>,
    name: String,
    active: bool,
    base: Option<Element>,
    rows: Vec<Row>,
    selected: Option<Item>,
    host: Option<HostInfo>,
    choices: Choices,
    /// Bumped on every load so late results of an old load are ignored.
    generation: u32,
}

pub struct HardwareView {
    pub root: gtk::Overlay,
    list: gtk::ListBox,
    title: gtk::Label,
    subtitle: gtk::Label,
    running_note: gtk::Label,
    remove: gtk::Button,
    editor: gtk::Box,
    stack: gtk::Stack,
    apply_bar: gtk::Revealer,
    apply: gtk::Button,
    /// Right-click menu of the device list.
    menu: gtk::PopoverMenu,
    menu_model: gio::Menu,
    /// `hw.remove`, disabled for devices that can't be removed.
    remove_action: RefCell<Option<gio::SimpleAction>>,
    work: Rc<RefCell<Element>>,
    state: RefCell<HwState>,
    feedback: Feedback,
    settings: Settings,
    on_changed: Rc<dyn Fn()>,
}

impl std::fmt::Debug for HardwareView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HardwareView").finish_non_exhaustive()
    }
}

impl Drop for HardwareView {
    fn drop(&mut self) {
        self.menu.unparent();
    }
}

impl HardwareView {
    /// `on_changed` runs after the VM was changed (so the window refreshes).
    pub fn new(feedback: Feedback, settings: Settings, on_changed: impl Fn() + 'static) -> Rc<Self> {
        let list = gtk::ListBox::builder().css_classes(["boxed-list", "hardware-list"]).build();
        list.update_property(&[gtk::accessible::Property::Label("Devices")]);
        let add_button = super::widgets::labeled_button("list-add-symbolic", "Add Hardware");
        add_button.set_halign(gtk::Align::Fill);
        let left = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).width_request(300).build();
        left.append(&list);
        left.append(&add_button);

        let title = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["title-2"]).build();
        let subtitle = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["dim-label"]).build();
        let running_note = gtk::Label::builder()
            .label("The machine is running: most changes take effect after it restarts.")
            .xalign(0.0)
            .wrap(true)
            .visible(false)
            .css_classes(["caption", "warning"])
            .build();
        let remove = gtk::Button::builder()
            .label("Remove")
            .valign(gtk::Align::Center)
            .css_classes(["destructive-action"])
            .build();
        let head_text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).hexpand(true).build();
        head_text.append(&title);
        head_text.append(&subtitle);
        head_text.append(&running_note);
        let head = gtk::Box::builder().spacing(12).build();
        head.append(&head_text);
        head.append(&remove);
        let editor = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(22).build();
        let right = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(22).hexpand(true).build();
        right.append(&head);
        right.append(&editor);

        // Room below for the Apply bar.
        let split = super::widgets::ListDetail::new(&left, &right, 28, false, "Devices", 660.0, |b| {
            b.set_margin_bottom(72);
            super::widgets::scrolled_page(b, 1180)
        });
        let s = split.clone();
        list.connect_row_activated(move |_, _| s.show_detail());
        let scroller = split.root().clone();
        let loading = adw::Spinner::builder().halign(gtk::Align::Center).valign(gtk::Align::Center).build();
        let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
        stack.add_named(&loading, Some("loading"));
        stack.add_named(&scroller, Some("content"));

        // "You have unsaved changes · Revert · Apply"
        let revert = gtk::Button::builder().label("Revert").css_classes(["flat", "pill"]).build();
        let apply = gtk::Button::builder().label("Apply").css_classes(["suggested-action", "pill"]).build();
        let bar = gtk::Box::builder().spacing(12).css_classes(["apply-bar"]).build();
        bar.append(
            &gtk::Label::builder().label("You have unsaved changes").wrap(true).xalign(0.0).margin_start(8).build(),
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

        // Filled for the device each time it opens (see `popup_menu`).
        let menu_model = gio::Menu::new();
        let menu = gtk::PopoverMenu::from_model(Some(&menu_model));
        // Parented to the column, not the list: a list removes all of its
        // children when it is refilled, which a popover must survive.
        menu.set_parent(&left);
        menu.set_has_arrow(false);
        menu.set_halign(gtk::Align::Start);

        let this = Rc::new(Self {
            root,
            list,
            title,
            subtitle,
            running_note,
            remove,
            editor,
            stack,
            apply_bar,
            apply,
            menu,
            menu_model,
            remove_action: RefCell::default(),
            work: Rc::default(),
            state: RefCell::default(),
            feedback,
            settings,
            on_changed: Rc::new(on_changed),
        });

        let weak = Rc::downgrade(&this);
        this.list.connect_row_selected(move |_, row| {
            let Some(this) = weak.upgrade() else { return };
            let item = row.and_then(|r| this.state.borrow().rows.iter().find(|x| &x.row == r).map(|x| x.item));
            if let Some(item) = item
                && this.state.borrow().selected != Some(item)
            {
                this.state.borrow_mut().selected = Some(item);
                this.show_editor();
            }
        });
        let weak = Rc::downgrade(&this);
        revert.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.revert();
            }
        });
        let weak = Rc::downgrade(&this);
        this.apply.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.save();
            }
        });
        let weak = Rc::downgrade(&this);
        this.remove.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.ask_remove();
            }
        });
        // Right-click (or Delete) removes a device; devices that can't be
        // removed show the menu item disabled.
        let remove_action = gio::SimpleAction::new("remove", None);
        let weak = Rc::downgrade(&this);
        remove_action.connect_activate(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.ask_remove();
            }
        });
        let actions = gio::SimpleActionGroup::new();
        actions.add_action(&remove_action);
        for (name, f) in [
            ("add", Self::add_hardware as fn(&Rc<Self>)),
            ("eject", Self::eject_selected),
            ("connect-cable", |this: &Rc<Self>| this.set_cable(true)),
            ("disconnect-cable", |this: &Rc<Self>| this.set_cable(false)),
        ] {
            let a = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(&this);
            a.connect_activate(move |_, _| {
                if let Some(this) = weak.upgrade() {
                    f(&this);
                }
            });
            actions.add_action(&a);
        }
        this.remove_action.replace(Some(remove_action.clone()));
        this.root.insert_action_group("hw", Some(&actions));
        let click = gtk::GestureClick::builder().button(gtk::gdk::BUTTON_SECONDARY).build();
        let weak = Rc::downgrade(&this);
        click.connect_pressed(move |g, _, x, y| {
            let Some(this) = weak.upgrade() else { return };
            let Some(row) = this.list.row_at_y(y as i32) else { return };
            g.set_state(gtk::EventSequenceState::Claimed);
            let point = gtk::graphene::Point::new(x as f32, y as f32);
            let p = this.list.compute_point(&row, &point).unwrap_or(point);
            this.popup_menu(&row, f64::from(p.x()), f64::from(p.y()));
        });
        this.list.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(&this);
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(this) = weak.upgrade() else { return glib::Propagation::Proceed };
            let menu_key = key == gtk::gdk::Key::Menu
                || (key == gtk::gdk::Key::F10 && mods.contains(gtk::gdk::ModifierType::SHIFT_MASK));
            if key == gtk::gdk::Key::Delete && this.remove.is_visible() {
                this.ask_remove();
                glib::Propagation::Stop
            } else if menu_key && let Some(row) = this.list.selected_row() {
                this.popup_menu(&row, f64::from(row.width()) / 2.0, f64::from(row.height()) / 2.0);
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        this.list.add_controller(keys);
        // Long-press on touch screens opens the menu too.
        let long_press = gtk::GestureLongPress::builder().touch_only(true).build();
        let weak = Rc::downgrade(&this);
        long_press.connect_pressed(move |_, x, y| {
            let Some(this) = weak.upgrade() else { return };
            let Some(row) = this.list.row_at_y(y as i32) else { return };
            let point = gtk::graphene::Point::new(x as f32, y as f32);
            let p = this.list.compute_point(&row, &point).unwrap_or(point);
            this.popup_menu(&row, f64::from(p.x()), f64::from(p.y()));
        });
        this.list.add_controller(long_press);

        let weak = Rc::downgrade(&this);
        add_button.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.add_hardware();
            }
        });
        this
    }

    /// Whether there are edits that have not been applied.
    pub fn is_dirty(&self) -> bool {
        self.state.borrow().base.as_ref().is_some_and(|b| *b != *self.work.borrow())
    }

    /// Shows `vm`. Reloads when it is another VM; keeps unsaved edits
    /// across the regular refreshes.
    pub fn set_target(self: &Rc<Self>, conn: Conn, vm: &DomainSummary, host: Option<HostInfo>) {
        let same = self.state.borrow().uuid.as_deref() == Some(vm.uuid.as_str());
        {
            let mut st = self.state.borrow_mut();
            st.conn = Some(conn);
            st.active = vm.state.is_active();
            st.name = vm.config.name.clone();
            st.host = host;
        }
        self.running_note.set_visible(vm.state.is_active() && self.is_editable_item());
        if !same {
            self.state.borrow_mut().uuid = Some(vm.uuid.clone());
            self.state.borrow_mut().selected = None;
            self.load();
        }
    }

    fn is_editable_item(&self) -> bool {
        !matches!(self.state.borrow().selected, Some(Item::Xml) | None)
    }

    /// (Re)reads the configuration and the host's choices.
    pub fn load(self: &Rc<Self>) {
        let (conn, uuid, generation) = {
            let mut st = self.state.borrow_mut();
            st.generation += 1;
            match (&st.conn, &st.uuid) {
                (Some(c), Some(u)) => (c.clone(), u.clone(), st.generation),
                _ => return,
            }
        };
        if self.state.borrow().base.is_none() {
            self.stack.set_visible_child_name("loading");
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn
                .call(move |c| {
                    let xml = c.domain_config_xml(&uuid)?;
                    // Firmware files for this machine's architecture and type.
                    let arch = xml.find_attr("os/type", "arch").unwrap_or("x86_64").to_owned();
                    let machine = xml.find_attr("os/type", "machine").map(str::to_owned);
                    let virt = xml.attr("type").unwrap_or("kvm").to_owned();
                    let caps = c.domain_caps(&arch, machine.as_deref(), &virt).unwrap_or_default();
                    let choices = Choices {
                        networks: c.list_networks().unwrap_or_default().into_iter().filter(|n| n.active).collect(),
                        bridges: c.list_bridges(),
                        images: c.list_install_images().unwrap_or_default(),
                        pools: c
                            .list_pools()
                            .unwrap_or_default()
                            .into_iter()
                            .map(|(p, _)| p)
                            .filter(|p| p.active && p.name != vmng_virt::BOOT_POOL)
                            .collect(),
                        session: matches!(c.kind(), vmng_virt::ConnectionKind::Session),
                        arch,
                        loaders: caps.loaders,
                        cpu_models: caps.cpu_models,
                        cpu_security: caps.cpu_security_features,
                        host_nics: c.list_host_nics(),
                    };
                    Ok((xml, choices))
                })
                .await;
            let Some(this) = weak.upgrade() else { return };
            if this.state.borrow().generation != generation {
                return;
            }
            match result {
                Ok((xml, choices)) => {
                    *this.work.borrow_mut() = xml.clone();
                    {
                        let mut st = this.state.borrow_mut();
                        st.base = Some(xml);
                        st.choices = choices;
                    }
                    this.rebuild_list();
                    this.stack.set_visible_child_name("content");
                    this.update_apply_bar();
                }
                Err(e) => this.feedback.error("Couldn't read the machine's configuration", e.message()),
            }
        });
    }

    fn rebuild_list(self: &Rc<Self>) {
        let work = self.work.borrow().clone();
        let mut items = vec![Item::Overview, Item::CpuMemory, Item::Boot];
        items.extend(list_devices(&work).iter().filter(|d| labels::is_listed(d)).map(|d| Item::Device(d.index)));
        items.push(Item::Xml);

        let previous = self.state.borrow().selected;
        self.list.remove_all();
        let mut rows = Vec::new();
        for item in items {
            let icon = gtk::Image::from_icon_name(match item {
                Item::Overview => "dialog-information-symbolic",
                Item::CpuMemory => "applications-system-symbolic",
                Item::Boot => "system-run-symbolic",
                Item::Xml => "text-x-generic-symbolic",
                Item::Device(i) => {
                    let kind = list_devices(&work)
                        .into_iter()
                        .find(|d| d.index == i)
                        .map_or(DeviceKind::Other(String::new()), |d| d.kind);
                    labels::icon(&kind)
                }
            });
            let title = gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["heading"])
                .build();
            let subtitle = gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .css_classes(["caption", "dim-label"])
                .build();
            let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(1).hexpand(true).build();
            text.append(&title);
            text.append(&subtitle);
            let content = gtk::Box::builder().spacing(12).css_classes(["hardware-row"]).build();
            content.append(&icon);
            content.append(&text);
            let row = gtk::ListBoxRow::builder().child(&content).build();
            self.list.append(&row);
            rows.push(Row { item, row, title, subtitle });
        }
        self.state.borrow_mut().rows = rows;
        self.update_labels();
        let target =
            previous.filter(|p| self.state.borrow().rows.iter().any(|r| r.item == *p)).unwrap_or(Item::CpuMemory);
        self.state.borrow_mut().selected = None;
        let row = self.state.borrow().rows.iter().find(|r| r.item == target).map(|r| r.row.clone());
        self.list.select_row(row.as_ref());
        #[cfg(debug_assertions)]
        self.dev_hooks();
    }

    /// Debug builds: `VMNG_DEV_HW_ROW=<n>` selects a row of the list and
    /// `VMNG_DEV_HW_ADD=1` opens Add Hardware, for unattended screenshots.
    #[cfg(debug_assertions)]
    fn dev_hooks(self: &Rc<Self>) {
        thread_local!(static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
        if DONE.with(|d| d.replace(true)) {
            return;
        }
        if let Some(n) = std::env::var("VMNG_DEV_HW_ROW").ok().and_then(|v| v.parse::<i32>().ok()) {
            // Negative numbers count from the end (-1 = XML).
            let count = self.state.borrow().rows.len() as i32;
            let index = if n < 0 { count + n } else { n };
            self.list.select_row(self.list.row_at_index(index).as_ref());
        }
        // VMNG_DEV_HW_REMOVE=1: remove the selected device through the menu action.
        if std::env::var_os("VMNG_DEV_HW_REMOVE").is_some() {
            let _ = WidgetExt::activate_action(&self.list, "hw.remove", None);
        }
        // VMNG_DEV_HW_MENU=1: open the right-click menu on the selected device.
        if std::env::var_os("VMNG_DEV_HW_MENU").is_some()
            && let Some(row) = self.list.selected_row()
        {
            self.popup_menu(&row, 120.0, 30.0);
        }
        if std::env::var_os("VMNG_DEV_HW_ADD").is_some() {
            self.add_hardware();
        }
        // VMNG_DEV_HW_MEMORY=<MiB>: change memory through the editor binding and Apply.
        if let Some(mib) = std::env::var("VMNG_DEV_HW_MEMORY").ok().and_then(|v| v.parse::<u64>().ok()) {
            self.bind(Target::Domain).set(|e| {
                for n in ["memory", "currentMemory"] {
                    let m = e.child_or_insert(n);
                    m.set_attr("unit", "KiB");
                    m.set_text((mib * 1024).to_string());
                }
            });
            assert!(self.is_dirty() && self.apply_bar.reveals_child());
            self.save();
        }
    }

    /// Refreshes the titles in the list from the working copy.
    fn update_labels(&self) {
        let work = self.work.borrow().clone();
        let devices = list_devices(&work);
        let st = self.state.borrow();
        for r in &st.rows {
            let (title, sub) = match r.item {
                Item::Overview => {
                    let c = vmng_virt::DomainConfig::from_xml(&work);
                    ("Overview".to_owned(), c.title.unwrap_or(c.name))
                }
                Item::CpuMemory => {
                    let c = vmng_virt::DomainConfig::from_xml(&work);
                    ("Processor & memory".to_owned(), format!("{} vCPU · {}", c.vcpus, crate::format::bytes(c.memory)))
                }
                Item::Boot => {
                    let c = vmng_virt::DomainConfig::from_xml(&work);
                    let fw = match c.firmware {
                        Some(vmng_virt::Firmware::Uefi { secure_boot: true }) => "UEFI · Secure Boot",
                        Some(vmng_virt::Firmware::Uefi { .. }) => "UEFI",
                        _ => "BIOS",
                    };
                    ("Boot & firmware".to_owned(), fw.to_owned())
                }
                Item::Xml => ("XML".to_owned(), "The complete configuration".to_owned()),
                Item::Device(i) => devices.iter().find(|d| d.index == i).map(labels::describe).unwrap_or_default(),
            };
            r.title.set_label(&title);
            r.subtitle.set_label(&sub);
            r.subtitle.set_visible(!sub.is_empty());
        }
    }

    fn bind(self: &Rc<Self>, target: Target) -> Bind {
        let weak = Rc::downgrade(self);
        Bind {
            work: self.work.clone(),
            target,
            changed: Rc::new(move || {
                if let Some(this) = weak.upgrade() {
                    this.update_labels();
                    this.update_apply_bar();
                }
            }),
        }
    }

    fn live(self: &Rc<Self>, index: usize) -> Live {
        let weak = Rc::downgrade(self);
        Rc::new(move |device: Element, message: String| {
            if let Some(this) = weak.upgrade() {
                this.update_live(index, device, message);
            }
        })
    }

    fn show_editor(self: &Rc<Self>) {
        while let Some(c) = self.editor.first_child() {
            self.editor.remove(&c);
        }
        let Some(item) = self.state.borrow().selected else { return };
        let (host_cpus, host_mem, choices, active) = {
            let st = self.state.borrow();
            let h = st.host.as_ref();
            (h.map_or(1, |h| h.cpus), h.map_or(64 << 30, |h| h.memory), st.choices.clone(), st.active)
        };
        let xml_editing = self.settings.get().xml_editing;
        let work = self.work.borrow().clone();
        self.remove.set_visible(false);
        self.running_note.set_visible(active && item != Item::Xml);
        let (title, subtitle, body) = match item {
            Item::Overview => (
                "Overview".to_owned(),
                "A title and notes to recognise this machine.".to_owned(),
                editors::overview(&self.bind(Target::Domain)),
            ),
            Item::CpuMemory => (
                "Processor & memory".to_owned(),
                "How much of the host this machine may use.".to_owned(),
                editors::cpu_memory(&self.bind(Target::Domain), &choices, host_cpus, host_mem),
            ),
            Item::Boot => (
                "Boot & firmware".to_owned(),
                "What the machine starts from.".to_owned(),
                editors::boot(&self.bind(Target::Domain), &choices),
            ),
            Item::Xml => {
                let b = editors::xml_view(&self.bind(Target::Domain), xml_editing, true);
                ("XML".to_owned(), "The complete libvirt configuration of this machine.".to_owned(), b)
            }
            Item::Device(index) => {
                let Some(dev) = list_devices(&work).into_iter().find(|d| d.index == index) else { return };
                let (title, sub) = labels::describe(&dev);
                self.remove.set_visible(dev.kind.removable(&dev.element));
                let bind = self.bind(Target::Device(index));
                let body = match &dev.kind {
                    DeviceKind::Disk => editors::disk(&bind),
                    DeviceKind::Cdrom => editors::cdrom(&bind, &choices, &self.live(index)),
                    DeviceKind::Interface => editors::interface(&bind, &choices, &self.live(index)),
                    DeviceKind::Graphics => editors::graphics(&bind),
                    DeviceKind::Video => editors::video(&bind),
                    DeviceKind::Sound => editors::sound(&bind),
                    DeviceKind::Input => editors::input(&bind),
                    DeviceKind::Filesystem => editors::filesystem(&bind),
                    DeviceKind::Controller => editors::controller(&bind),
                    DeviceKind::Tpm => editors::tpm(&bind),
                    other => editors::generic(&bind, other),
                };
                body.append(&editors::xml_view(&bind, xml_editing, false));
                (title, sub, body)
            }
        };
        self.title.set_label(&title);
        self.subtitle.set_label(&subtitle);
        self.subtitle.set_visible(!subtitle.is_empty());
        self.editor.append(&body);
    }

    fn update_apply_bar(&self) {
        let dirty = self.is_dirty();
        self.apply_bar.set_reveal_child(dirty);
        if dirty {
            self.apply.grab_focus();
        }
    }

    fn revert(self: &Rc<Self>) {
        let base = self.state.borrow().base.clone();
        if let Some(base) = base {
            *self.work.borrow_mut() = base;
        }
        self.rebuild_list();
        self.show_editor();
        self.update_apply_bar();
    }

    fn conn_uuid(&self) -> Option<(Conn, String)> {
        let st = self.state.borrow();
        Some((st.conn.clone()?, st.uuid.clone()?))
    }

    fn save(self: &Rc<Self>) {
        let Some((conn, uuid)) = self.conn_uuid() else { return };
        let xml = self.work.borrow().clone();
        let base = self.state.borrow().base.clone();
        let active = self.state.borrow().active;
        // What a running machine can take right away: its title and
        // description, and a new memory size up to its maximum.
        let text = |e: &Element, n: &str| e.find_text(n).filter(|t| !t.is_empty());
        let live_title = base
            .as_ref()
            .filter(|b| {
                active
                    && (text(b, "title") != text(&xml, "title") || text(b, "description") != text(&xml, "description"))
            })
            .map(|_| (text(&xml, "title"), text(&xml, "description")));
        let only_memory_and_text = base.as_ref().is_some_and(|b| {
            let strip = |e: &Element| {
                let mut e = e.clone();
                e.remove_children(|c| matches!(c.name.as_str(), "title" | "description" | "currentMemory"));
                e
            };
            strip(b) == strip(&xml)
        });
        let live_memory = base
            .as_ref()
            .map(MemorySize::of)
            .filter(|b| active && b.current != MemorySize::of(&xml).current)
            .map(|_| MemorySize::of(&xml).current);
        self.apply.set_sensitive(false);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn
                .call(move |c| {
                    c.save_domain_config(&uuid, &xml)?;
                    let mut live = true;
                    if let Some((title, description)) = live_title {
                        live &= c.set_live_title(&uuid, title.as_deref(), description.as_deref()).is_ok();
                    }
                    if let Some(bytes) = live_memory {
                        live &= c.set_live_memory(&uuid, bytes).is_ok();
                    }
                    Ok(live)
                })
                .await;
            let Some(this) = weak.upgrade() else { return };
            this.apply.set_sensitive(true);
            match result {
                Ok(live) => {
                    this.feedback.toast(if !active {
                        "Saved the changes"
                    } else if live && only_memory_and_text {
                        "Saved and applied to the running machine"
                    } else {
                        "Saved. Restart the machine to use the new settings."
                    });
                    this.state.borrow_mut().base = None;
                    this.load();
                    (this.on_changed)();
                }
                Err(e) => this.feedback.error("Couldn't save the changes", e.message()),
            }
        });
    }

    /// Applies an immediate change (CD, network cable) to one device, then
    /// redraws the editor so every label shows the new state.
    fn update_live(self: &Rc<Self>, index: usize, device: Element, message: String) {
        let Some((conn, uuid)) = self.conn_uuid() else { return };
        let weak = Rc::downgrade(self);
        let dev = device.clone();
        glib::spawn_future_local(async move {
            let result = conn.call(move |c| c.update_device(&uuid, &dev)).await;
            let Some(this) = weak.upgrade() else { return };
            match result {
                Ok(now) => {
                    // Keep unsaved edits elsewhere: patch both copies.
                    let _ = vmng_virt::replace_device(&mut this.work.borrow_mut(), index, device.clone());
                    if let Some(base) = this.state.borrow_mut().base.as_mut() {
                        let _ = vmng_virt::replace_device(base, index, device);
                    }
                    this.update_labels();
                    this.update_apply_bar();
                    this.show_editor();
                    this.feedback.toast(&if now { message } else { format!("{message} (after a restart)") });
                    (this.on_changed)();
                }
                Err(e) => {
                    let mut details = e.message().to_owned();
                    // QEMU runs as its own user on the System connection and
                    // may not be allowed into the file's folder.
                    if details.contains("Permission denied") {
                        details.push_str(
                            "\n\nThe virtual machine can't read this file from where it is. Move it to a folder \
                             it can reach, such as Downloads or a storage pool, and choose it again.",
                        );
                    }
                    this.feedback.error("Couldn't change the device", &details);
                    this.show_editor();
                }
            }
        });
    }

    /// The selected device in the working copy, with its position.
    fn selected_device(&self) -> Option<vmng_virt::DeviceRef> {
        let Some(Item::Device(index)) = self.state.borrow().selected else { return None };
        list_devices(&self.work.borrow()).into_iter().find(|d| d.index == index)
    }

    /// Takes the disc out of the selected CD/DVD drive, right away.
    fn eject_selected(self: &Rc<Self>) {
        let Some(d) = self.selected_device() else { return };
        let mut dev = d.element;
        dev.remove_children(|c| c.name == "source");
        self.update_live(d.index, dev, "Ejected the disc".into());
    }

    /// Plugs in or pulls the virtual cable of the selected network card.
    fn set_cable(self: &Rc<Self>, connected: bool) {
        let Some(d) = self.selected_device() else { return };
        let mut dev = d.element;
        dev.child_or_insert("link").set_attr("state", if connected { "up" } else { "down" });
        let msg = if connected { "Connected the cable" } else { "Disconnected the cable" };
        self.update_live(d.index, dev, msg.into());
    }

    /// Selects `row` and opens the device menu at (`x`, `y`) in the row:
    /// quick actions for the device, Add Hardware, and Remove (like the
    /// machine menu: grouped, the destructive item last).
    fn popup_menu(&self, row: &gtk::ListBoxRow, x: f64, y: f64) {
        self.list.select_row(Some(row));
        if let Some(remove) = self.remove_action.borrow().as_ref() {
            remove.set_enabled(self.remove.is_visible());
        }
        let m = &self.menu_model;
        m.remove_all();
        let quick = gio::Menu::new();
        if let Some(d) = self.selected_device() {
            match d.kind {
                vmng_virt::DeviceKind::Cdrom if d.element.child("source").is_some() => {
                    quick.append(Some("Eject Disc"), Some("hw.eject"));
                }
                vmng_virt::DeviceKind::Interface => {
                    if d.element.find_attr("link", "state") == Some("down") {
                        quick.append(Some("Connect Cable"), Some("hw.connect-cable"));
                    } else {
                        quick.append(Some("Disconnect Cable"), Some("hw.disconnect-cable"));
                    }
                }
                _ => {}
            }
        }
        if quick.n_items() > 0 {
            m.append_section(None, &quick);
        }
        let add = gio::Menu::new();
        add.append(Some("Add Hardware…"), Some("hw.add"));
        m.append_section(None, &add);
        let danger = gio::Menu::new();
        danger.append(Some("Remove…"), Some("hw.remove"));
        m.append_section(None, &danger);
        // The menu lives on the column (see `new`), so convert the point.
        let point = gtk::graphene::Point::new(x as f32, y as f32);
        let p = self.menu.parent().and_then(|parent| row.compute_point(&parent, &point)).unwrap_or(point);
        self.menu.set_pointing_to(Some(&gtk::gdk::Rectangle::new(p.x() as i32, p.y() as i32, 1, 1)));
        self.menu.popup();
    }

    fn ask_remove(self: &Rc<Self>) {
        let Some(Item::Device(index)) = self.state.borrow().selected else { return };
        let base = self.state.borrow().base.clone();
        let Some(dev) = base.as_ref().and_then(|b| list_devices(b).into_iter().find(|d| d.index == index)) else {
            return;
        };
        let (title, _) = labels::describe(&dev);
        let element = dev.element;
        let mut body = "It is removed from the machine's configuration.".to_owned();
        if dev.kind == DeviceKind::Disk {
            body.push_str(" The disk image itself is kept.");
        }
        if self.is_dirty() {
            body.push_str(" Your unsaved changes are discarded.");
        }
        let heading = format!("Remove {title}?");
        let weak = Rc::downgrade(self);
        let run = move || {
            if let Some(this) = weak.upgrade() {
                this.remove_device(element.clone(), title.clone());
            }
        };
        if self.settings.get().confirm_remove_device {
            self.feedback.confirm(&heading, &body, "Remove", true, run);
        } else {
            run();
        }
    }

    fn remove_device(self: &Rc<Self>, element: Element, title: String) {
        let Some((conn, uuid)) = self.conn_uuid() else { return };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn.call(move |c| c.remove_device(&uuid, &element)).await;
            let Some(this) = weak.upgrade() else { return };
            match result {
                Ok(now) => {
                    this.feedback.toast(&if now {
                        format!("Removed {title}")
                    } else {
                        format!("Removed {title}. It disappears after a restart.")
                    });
                    this.state.borrow_mut().selected = None;
                    this.load();
                    (this.on_changed)();
                }
                Err(e) => this.feedback.error(&format!("Couldn't remove {title}"), e.message()),
            }
        });
    }

    /// Called when the person leaves the page with unapplied edits: asks
    /// whether to apply or discard them (if that prompt is turned on).
    pub fn leave(self: &Rc<Self>) {
        if !self.is_dirty() {
            return;
        }
        if !self.settings.get().confirm_unapplied_changes {
            self.revert();
            return;
        }
        let name = self.state.borrow().name.clone();
        let dialog = adw::AlertDialog::builder()
            .heading(format!("Apply the changes to {name}?"))
            .body("You changed the hardware but didn't apply the changes.")
            .build();
        dialog.add_responses(&[("discard", "Discard"), ("apply", "Apply")]);
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        dialog.set_response_appearance("apply", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("apply"));
        dialog.set_close_response("apply");
        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, response| {
            if let Some(this) = weak.upgrade() {
                if response == "discard" {
                    this.revert();
                } else {
                    this.save();
                }
            }
        });
        dialog.present(Some(self.feedback.parent()));
    }

    fn add_hardware(self: &Rc<Self>) {
        let Some((conn, uuid)) = self.conn_uuid() else { return };
        let (choices, name, active) = {
            let st = self.state.borrow();
            (st.choices.clone(), st.name.clone(), st.active)
        };
        let domain = self.work.borrow().clone();
        let weak = Rc::downgrade(self);
        add::open(&self.feedback, add::AddContext { conn, uuid, name, active, domain, choices }, move || {
            if let Some(this) = weak.upgrade() {
                this.load();
                (this.on_changed)();
            }
        });
    }
}
