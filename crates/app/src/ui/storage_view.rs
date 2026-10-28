//! Storage pools: the pools on the left, the selected pool's volumes and
//! settings on the right.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use vmng_virt::{DomainSummary, NewPool, PoolDetails, PoolKind, VolumeInfo};

use super::feedback::Feedback;
use super::widgets::{FormDialog, Meter, StatusLabel, Tone, boxed_list, icon_button, labeled_button, section_heading};
use crate::backend::Conn;
use crate::format;

const GIB: u64 = 1 << 30;

#[derive(Default)]
struct State {
    conn: Option<Conn>,
    pools: Vec<PoolDetails>,
    volumes: Vec<VolumeInfo>,
    domains: Vec<DomainSummary>,
    selected: Option<String>,
    filling: bool,
}

pub struct StorageView {
    pub root: gtk::Stack,
    list: gtk::ListBox,
    title: gtk::Label,
    subtitle: gtk::Label,
    status: StatusLabel,
    toggle: gtk::Button,
    browse: gtk::Button,
    new_volume: gtk::Button,
    volumes: gtk::ListBox,
    volumes_empty: gtk::Label,
    autostart: adw::SwitchRow,
    kind: gtk::Label,
    source: gtk::Label,
    state: RefCell<State>,
    feedback: Feedback,
}

impl std::fmt::Debug for StorageView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageView").finish_non_exhaustive()
    }
}

/// Which machines use a volume (by path).
fn users_of(path: &str, domains: &[DomainSummary]) -> Vec<String> {
    domains
        .iter()
        .filter(|d| {
            d.config.disks.iter().any(|x| x.source_path.as_deref() == Some(path))
                || d.config.nvram_path.as_deref() == Some(path)
        })
        .map(|d| d.display_name().to_owned())
        .collect()
}

impl StorageView {
    pub fn new(feedback: Feedback) -> Rc<Self> {
        // Left: pools.
        let add_pool = icon_button("list-add-symbolic", "Create a storage pool");
        add_pool.add_css_class("flat");
        let pools_head = gtk::Box::builder().spacing(8).build();
        let heading = section_heading("Pools");
        heading.set_hexpand(true);
        pools_head.append(&heading);
        pools_head.append(&add_pool);
        let list = gtk::ListBox::builder().css_classes(["boxed-list", "hardware-list"]).build();
        list.update_property(&[gtk::accessible::Property::Label("Storage pools")]);
        let left = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).width_request(300).build();
        left.append(&pools_head);
        left.append(&list);

        // Right: the selected pool.
        let title = gtk::Label::builder().xalign(0.0).css_classes(["title-2"]).build();
        let subtitle = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["dim-label"]).build();
        let status = StatusLabel::new();
        status.root.add_css_class("caption");
        status.root.add_css_class("dim-label");
        let titles = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).hexpand(true).build();
        titles.append(&title);
        titles.append(&subtitle);
        titles.append(&status.root);
        let refresh = icon_button("view-refresh-symbolic", "Refresh");
        let toggle = gtk::Button::builder().valign(gtk::Align::Center).build();
        let browse = labeled_button("folder-open-symbolic", "Browse Files");
        let new_volume = labeled_button("list-add-symbolic", "New Volume");
        new_volume.add_css_class("suggested-action");
        let buttons = gtk::Box::builder().spacing(8).valign(gtk::Align::Center).build();
        for w in
            [refresh.upcast_ref::<gtk::Widget>(), toggle.upcast_ref(), browse.upcast_ref(), new_volume.upcast_ref()]
        {
            buttons.append(w);
        }
        let head = gtk::Box::builder().spacing(12).build();
        head.append(&titles);
        head.append(&buttons);

        let volumes = boxed_list();
        let volumes_empty = gtk::Label::builder().xalign(0.0).css_classes(["dim-label"]).build();
        let vol_col = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).build();
        vol_col.append(&section_heading("Volumes"));
        vol_col.append(&volumes);
        vol_col.append(&volumes_empty);

        let settings = adw::PreferencesGroup::builder().title("Settings").build();
        let kind_row = adw::ActionRow::builder().title("Pool type").build();
        let kind = gtk::Label::builder().css_classes(["dim-label"]).build();
        kind_row.add_suffix(&kind);
        let source_row = adw::ActionRow::builder().title("Source").build();
        let source = gtk::Label::builder().css_classes(["dim-label"]).selectable(true).build();
        source_row.add_suffix(&source);
        let autostart = adw::SwitchRow::builder().title("Start when the host boots").build();
        settings.add(&kind_row);
        settings.add(&source_row);
        settings.add(&autostart);

        let remove_row = adw::ActionRow::builder()
            .title("Remove this pool")
            .subtitle("The files in it are kept; the app just stops listing them")
            .build();
        let remove = gtk::Button::builder()
            .label("Remove…")
            .valign(gtk::Align::Center)
            .css_classes(["destructive-action"])
            .build();
        remove_row.add_suffix(&remove);
        let danger = boxed_list();
        danger.append(&remove_row);

        let right = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(22).hexpand(true).build();
        right.append(&head);
        right.append(&vol_col);
        right.append(&settings);
        right.append(&danger);
        let content = gtk::Box::builder().spacing(28).css_classes(["page-content"]).build();
        content.append(&left);
        content.append(&right);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&adw::Clamp::builder().maximum_size(1180).child(&content).build())
            .build();

        let empty_new = gtk::Button::builder()
            .label("Create a Pool")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        let empty = adw::StatusPage::builder()
            .icon_name("drive-harddisk-symbolic")
            .title("No storage pools")
            .description("A pool is a place where disk images live, such as a folder on this computer.")
            .child(&empty_new)
            .build();
        let loading = adw::Spinner::builder().halign(gtk::Align::Center).valign(gtk::Align::Center).build();
        let root = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).vexpand(true).build();
        root.add_named(&loading, Some("loading"));
        root.add_named(&empty, Some("empty"));
        root.add_named(&scroller, Some("content"));

        let this = Rc::new(Self {
            root,
            list,
            title,
            subtitle,
            status,
            toggle,
            browse,
            new_volume,
            volumes,
            volumes_empty,
            autostart,
            kind,
            source,
            state: RefCell::default(),
            feedback,
        });

        let weak = Rc::downgrade(&this);
        this.list.connect_row_selected(move |_, row| {
            let Some(this) = weak.upgrade() else { return };
            let name = row.map(|r| r.widget_name().to_string());
            if name.is_some() && name != this.state.borrow().selected {
                this.state.borrow_mut().selected = name;
                this.show_selected();
                this.load_volumes();
            }
        });
        for b in [&add_pool, &empty_new] {
            let weak = Rc::downgrade(&this);
            b.connect_clicked(move |b| {
                if let Some(this) = weak.upgrade() {
                    this.new_pool(b);
                }
            });
        }
        let weak = Rc::downgrade(&this);
        refresh.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                let Some(name) = this.state.borrow().selected.clone() else { return };
                this.run(move |c| c.refresh_pool(&name), "Couldn't refresh the pool", None);
            }
        });
        let weak = Rc::downgrade(&this);
        this.toggle.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.toggle_active();
            }
        });
        let weak = Rc::downgrade(&this);
        this.browse.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.browse_files();
            }
        });
        let weak = Rc::downgrade(&this);
        this.new_volume.connect_clicked(move |b| {
            if let Some(this) = weak.upgrade() {
                this.new_volume_dialog(b);
            }
        });
        let weak = Rc::downgrade(&this);
        this.autostart.connect_active_notify(move |r| {
            let Some(this) = weak.upgrade() else { return };
            if this.state.borrow().filling {
                return;
            }
            let Some(name) = this.state.borrow().selected.clone() else { return };
            let on = r.is_active();
            this.run(move |c| c.set_pool_autostart(&name, on), "Couldn't change start at boot", None);
        });
        let group = gio::SimpleActionGroup::new();
        let delete = gio::SimpleAction::new("delete-volume", Some(glib::VariantTy::STRING));
        let weak = Rc::downgrade(&this);
        delete.connect_activate(move |_, p| {
            if let (Some(this), Some(name)) = (weak.upgrade(), p.and_then(|v| v.get::<String>())) {
                this.ask_delete_volume(name);
            }
        });
        group.add_action(&delete);
        this.root.insert_action_group("storage", Some(&group));
        let weak = Rc::downgrade(&this);
        remove.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.ask_remove();
            }
        });
        this
    }

    pub fn load(self: &Rc<Self>, conn: Conn, domains: &[DomainSummary]) {
        {
            let mut st = self.state.borrow_mut();
            st.conn = Some(conn.clone());
            st.domains = domains.to_vec();
            if st.pools.is_empty() {
                self.root.set_visible_child_name("loading");
            }
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn.call(|c| c.pool_details()).await;
            let Some(this) = weak.upgrade() else { return };
            match result {
                Ok(pools) => {
                    this.state.borrow_mut().pools = pools;
                    this.fill_list();
                    this.load_volumes();
                }
                Err(e) => this.feedback.error("Couldn't load the storage pools", e.message()),
            }
        });
    }

    fn fill_list(&self) {
        let pools = self.state.borrow().pools.clone();
        // Keep the selection without re-triggering the selection handler.
        let wanted = self.state.borrow().selected.clone();
        self.list.remove_all();
        if pools.is_empty() {
            self.root.set_visible_child_name("empty");
            return;
        }
        for p in &pools {
            let status = StatusLabel::new();
            status.set(if p.active { Tone::Running } else { Tone::Off }, &p.name);
            status.root.add_css_class("heading");
            let path = gtk::Label::builder()
                .label(p.path.as_deref().unwrap_or_default())
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .css_classes(["caption", "dim-label"])
                .build();
            let meter = Meter::new("");
            meter.root.add_css_class("caption");
            if p.active && p.capacity > 0 {
                meter.set(p.allocation as f64 / p.capacity as f64, &format!("{} free", format::bytes(p.available)));
            } else {
                meter.set(0.0, "Start to see usage");
            }
            let b = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(6)
                .css_classes(["hardware-row"])
                .build();
            b.append(&status.root);
            b.append(&path);
            b.append(&meter.root);
            let row = gtk::ListBoxRow::builder().child(&b).name(&p.name).build();
            self.list.append(&row);
        }
        self.root.set_visible_child_name("content");
        let index = wanted.and_then(|w| pools.iter().position(|p| p.name == w)).unwrap_or(0);
        let name = pools[index].name.clone();
        self.state.borrow_mut().selected = Some(name);
        self.list.select_row(self.list.row_at_index(index as i32).as_ref());
        self.show_selected();
    }

    fn details(&self) -> Option<PoolDetails> {
        let st = self.state.borrow();
        let name = st.selected.as_ref()?;
        st.pools.iter().find(|p| &p.name == name).cloned()
    }

    fn show_selected(&self) {
        let Some(p) = self.details() else { return };
        self.title.set_label(&p.name);
        let usage = if p.active && p.capacity > 0 {
            format!("{} free of {}", format::bytes(p.available), format::bytes(p.capacity))
        } else {
            "Usage is shown while the pool is active".to_owned()
        };
        self.subtitle.set_label(&match &p.path {
            Some(path) => format!("{path} · {usage}"),
            None => usage,
        });
        let n = self.state.borrow().volumes.len();
        self.status.set(
            if p.active { Tone::Running } else { Tone::Off },
            &if p.active {
                format!("Active · {n} volume{}", if n == 1 { "" } else { "s" })
            } else {
                "Stopped".to_owned()
            },
        );
        self.toggle.set_label(if p.active { "Stop Pool" } else { "Start Pool" });
        self.browse.set_visible(p.kind == PoolKind::Folder && p.path.is_some());
        self.new_volume.set_sensitive(p.active);
        self.kind.set_label(p.kind.label());
        self.source.set_label(p.source.as_deref().unwrap_or("—"));
        self.state.borrow_mut().filling = true;
        self.autostart.set_active(p.autostart);
        self.autostart.set_sensitive(p.persistent);
        self.state.borrow_mut().filling = false;
    }

    fn load_volumes(self: &Rc<Self>) {
        let (conn, Some(name)) = (self.state.borrow().conn.clone(), self.state.borrow().selected.clone()) else {
            return;
        };
        let Some(conn) = conn else { return };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let n = name.clone();
            let result = conn.call(move |c| c.list_volumes(&n)).await;
            let Some(this) = weak.upgrade() else { return };
            if this.state.borrow().selected.as_deref() != Some(name.as_str()) {
                return;
            }
            match result {
                Ok(vols) => {
                    this.state.borrow_mut().volumes = vols;
                    this.fill_volumes();
                    this.show_selected();
                }
                Err(e) => this.feedback.error("Couldn't list the volumes", e.message()),
            }
        });
    }

    fn fill_volumes(self: &Rc<Self>) {
        while let Some(c) = self.volumes.first_child() {
            self.volumes.remove(&c);
        }
        let (vols, domains, active) = {
            let st = self.state.borrow();
            (st.volumes.clone(), st.domains.clone(), self.details().is_some_and(|p| p.active))
        };
        for v in &vols {
            let users = users_of(&v.path, &domains);
            let used = if users.is_empty() { "Not used".to_owned() } else { format!("Used by {}", users.join(", ")) };
            let mut sub = vec![used, format::bytes(v.capacity)];
            if v.allocation < v.capacity {
                sub.push(format!("{} on disk", format::bytes(v.allocation)));
            }
            if let Some(f) = &v.format {
                sub.push(f.clone());
            }
            let row =
                adw::ActionRow::builder().title(glib::markup_escape_text(&v.name)).subtitle(sub.join(" · ")).build();
            let menu = gio::Menu::new();
            let item = gio::MenuItem::new(Some("Delete…"), None);
            item.set_action_and_target_value(Some("storage.delete-volume"), Some(&v.name.to_variant()));
            menu.append_item(&item);
            let actions = gtk::MenuButton::builder()
                .icon_name("view-more-symbolic")
                .menu_model(&menu)
                .valign(gtk::Align::Center)
                .tooltip_text("Volume actions")
                .css_classes(["flat"])
                .build();
            row.add_suffix(&actions);
            self.volumes.append(&row);
        }
        self.volumes.set_visible(!vols.is_empty());
        self.volumes_empty.set_label(if active {
            "This pool has no volumes yet."
        } else {
            "Start the pool to see its volumes."
        });
        self.volumes_empty.set_visible(vols.is_empty());
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
                    if let Some(m) = success {
                        this.feedback.toast(&m);
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
        let Some(p) = self.details() else { return };
        let name = p.name.clone();
        let on = !p.active;
        let msg = format!("{} {name}", if on { "Started" } else { "Stopped" });
        self.run(
            move |c| c.set_pool_active(&name, on),
            if on { "Couldn't start the pool" } else { "Couldn't stop the pool" },
            Some(msg),
        );
    }

    fn browse_files(&self) {
        let Some(path) = self.details().and_then(|p| p.path) else { return };
        let uri = gio::File::for_path(&path).uri();
        if let Err(e) = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>) {
            self.feedback.error("Couldn't open the folder", &e.to_string());
        }
    }

    fn ask_delete_volume(self: &Rc<Self>, name: String) {
        let Some(pool) = self.state.borrow().selected.clone() else { return };
        let vol = self.state.borrow().volumes.iter().find(|v| v.name == name).cloned();
        let users = vol.as_ref().map(|v| users_of(&v.path, &self.state.borrow().domains)).unwrap_or_default();
        let body = if users.is_empty() {
            "The file is deleted from the disk. This can't be undone.".to_owned()
        } else {
            format!(
                "It is used by {}, which won't start without it. The file is deleted from the disk. This can't be undone.",
                users.join(", ")
            )
        };
        let weak = Rc::downgrade(self);
        self.feedback.confirm(&format!("Delete {name}?"), &body, "Delete", true, move || {
            if let Some(this) = weak.upgrade() {
                let (p, n) = (pool.clone(), name.clone());
                this.run(
                    move |c| c.delete_volume(&p, &n),
                    "Couldn't delete the volume",
                    Some(format!("Deleted {name}")),
                );
            }
        });
    }

    fn ask_remove(self: &Rc<Self>) {
        let Some(p) = self.details() else { return };
        let name = p.name.clone();
        let weak = Rc::downgrade(self);
        self.feedback.confirm(
            &format!("Remove the pool {name}?"),
            "The app stops listing it; the files in it are not deleted. You can add it again later.",
            "Remove",
            true,
            move || {
                if let Some(this) = weak.upgrade() {
                    this.state.borrow_mut().selected = None;
                    let n = name.clone();
                    this.run(move |c| c.remove_pool(&n), "Couldn't remove the pool", Some(format!("Removed {name}")));
                }
            },
        );
    }

    fn new_volume_dialog(self: &Rc<Self>, parent: &impl IsA<gtk::Widget>) {
        let Some(pool) = self.details() else { return };
        let form = FormDialog::new(&format!("New Volume in {}", pool.name), "Create");
        let group = adw::PreferencesGroup::builder()
            .description(format!("{} free in this pool.", format::bytes(pool.available)))
            .build();
        let taken: Vec<String> = self.state.borrow().volumes.iter().map(|v| v.name.clone()).collect();
        let name = adw::EntryRow::builder()
            .title("Name")
            .text(vmng_virt::unique_file_name("disk", ".qcow2", |n| taken.iter().any(|t| t == n)))
            .build();
        let size = adw::SpinRow::with_range(1.0, (pool.available / GIB).max(1) as f64, 1.0);
        size.set_title("Size (GiB)");
        size.set_value(20.0);
        let fmt = adw::ComboRow::builder()
            .title("Format")
            .model(&gtk::StringList::new(&["qcow2 (grows as needed)", "raw (fastest, full size)"]))
            .build();
        group.add(&name);
        group.add(&size);
        group.add(&fmt);
        form.add(&group);
        let n2 = name.clone();
        fmt.connect_selected_notify(move |f| {
            // Keep the extension in step with the format.
            let text = n2.text().to_string();
            let stem = text.trim_end_matches(".qcow2").trim_end_matches(".img");
            n2.set_text(&format!("{stem}{}", if f.selected() == 0 { ".qcow2" } else { ".img" }));
        });
        let weak = Rc::downgrade(self);
        let pool_name = pool.name.clone();
        form.connect_primary(move |form| {
            let Some(this) = weak.upgrade() else { return };
            let n = name.text().trim().to_owned();
            if taken.contains(&n) {
                return form.set_error(Some("A volume with this name already exists."));
            }
            let format = if fmt.selected() == 0 { "qcow2" } else { "raw" };
            let bytes = size.value() as u64 * GIB;
            let p = pool_name.clone();
            let n2 = n.clone();
            form.close();
            this.run(
                move |c| c.create_named_volume(&p, &n2, bytes, format).map(drop),
                "Couldn't create the volume",
                Some(format!("Created {n}")),
            );
        });
        form.present(parent);
    }

    fn new_pool(self: &Rc<Self>, parent: &impl IsA<gtk::Widget>) {
        let form = FormDialog::new("New Storage Pool", "Create");
        let taken: Vec<String> = self.state.borrow().pools.iter().map(|p| p.name.clone()).collect();
        let group = adw::PreferencesGroup::new();
        let name = adw::EntryRow::builder()
            .title("Name")
            .text(vmng_virt::unique_name("pool", |n| taken.iter().any(|t| t == n)))
            .build();
        const KINDS: [(PoolKind, &str); 4] = [
            (PoolKind::Folder, "Folder on this computer"),
            (PoolKind::Lvm, "LVM volume group"),
            (PoolKind::Nfs, "Network share (NFS)"),
            (PoolKind::Iscsi, "iSCSI target"),
        ];
        let labels: Vec<&str> = KINDS.iter().map(|(_, l)| *l).collect();
        let kind = adw::ComboRow::builder().title("Type").model(&gtk::StringList::new(&labels)).build();
        let path = adw::EntryRow::builder().title("Folder").build();
        let browse = gtk::Button::builder()
            .icon_name("folder-open-symbolic")
            .tooltip_text("Choose a folder")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        path.add_suffix(&browse);
        let host = adw::EntryRow::builder().title("Server").visible(false).build();
        let source = adw::EntryRow::builder().title("Shared folder on the server").visible(false).build();
        for w in [
            name.upcast_ref::<gtk::Widget>(),
            kind.upcast_ref(),
            path.upcast_ref(),
            host.upcast_ref(),
            source.upcast_ref(),
        ] {
            group.add(w);
        }
        form.add(&group);
        let (p, h, s) = (path.clone(), host.clone(), source.clone());
        kind.connect_selected_notify(move |k| {
            let kind = KINDS[k.selected() as usize].0;
            p.set_visible(matches!(kind, PoolKind::Folder | PoolKind::Nfs));
            p.set_title(if kind == PoolKind::Nfs { "Mount it at (folder on this computer)" } else { "Folder" });
            h.set_visible(matches!(kind, PoolKind::Nfs | PoolKind::Iscsi));
            s.set_visible(kind != PoolKind::Folder);
            s.set_title(match kind {
                PoolKind::Lvm => "Volume group name",
                PoolKind::Iscsi => "Target (IQN)",
                _ => "Shared folder on the server",
            });
        });
        let p = path.clone();
        browse.connect_clicked(move |b| {
            let dialog = gtk::FileDialog::builder().title("Choose a Folder for the Pool").modal(true).build();
            let window = b.root().and_downcast::<gtk::Window>();
            let p = p.clone();
            glib::spawn_future_local(async move {
                if let Some(path) = dialog.select_folder_future(window.as_ref()).await.ok().and_then(|f| f.path()) {
                    p.set_text(&path.to_string_lossy());
                }
            });
        });
        let weak = Rc::downgrade(self);
        form.connect_primary(move |form| {
            let Some(this) = weak.upgrade() else { return };
            let spec = NewPool {
                name: name.text().trim().to_owned(),
                kind: KINDS[kind.selected() as usize].0,
                path: path.text().trim().to_owned(),
                host: host.text().trim().to_owned(),
                source: source.text().trim().to_owned(),
            };
            if taken.contains(&spec.name) {
                return form.set_error(Some("Another pool already has this name."));
            }
            if let Err(m) = vmng_virt::pool_xml(&spec) {
                return form.set_error(Some(m));
            }
            form.close();
            this.state.borrow_mut().selected = Some(spec.name.clone());
            let n = spec.name.clone();
            this.run(move |c| c.create_pool(&spec), "Couldn't create the pool", Some(format!("Created the pool {n}")));
        });
        form.present(parent);
    }
}
