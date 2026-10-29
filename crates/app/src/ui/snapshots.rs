//! The Snapshots tab: a timeline of saved states with a details card.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use vmng_virt::{DomainSummary, SnapshotInfo, SnapshotLocation, SnapshotOptions, validate_snapshot_name_for};

use super::feedback::Feedback;
use super::widgets::{FormDialog, card, labeled_button};
use crate::backend::Conn;

/// "28 Oct 2026, 16:40"
pub fn format_time(unix: i64) -> String {
    glib::DateTime::from_unix_local(unix)
        .and_then(|d| d.format("%e %b %Y, %H:%M"))
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|_| "Unknown date".to_owned())
}

fn kind_text(s: &SnapshotInfo) -> &'static str {
    match s.state.as_str() {
        "running" => "Running (with memory)",
        "paused" => "Paused (with memory)",
        "disk-snapshot" => "Disks only",
        _ => "Shut off",
    }
}

/// Where the picture of the screen taken with a snapshot is kept (like
/// virt-manager, in the cache: libvirt has no place for it). The name is
/// hex-encoded, so any snapshot name makes a safe file name.
fn screen_file(uuid: &str, snapshot: &str) -> std::path::PathBuf {
    let hex: String = snapshot.bytes().map(|b| format!("{b:02x}")).collect();
    glib::user_cache_dir().join("virt-manager-nextgen").join("snapshot-screens").join(uuid).join(format!("{hex}.png"))
}

fn save_screen(uuid: &str, snapshot: &str, texture: &gtk::gdk::Texture) {
    let path = screen_file(uuid, snapshot);
    let saved = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .map_err(|e| e.to_string())
        .and_then(|()| texture.save_to_png(&path).map_err(|e| e.to_string()));
    if let Err(e) = saved {
        glib::g_warning!("virt-manager-nextgen", "couldn't keep the snapshot's screen picture: {e}");
    }
}

type Changed = Rc<dyn Fn()>;

struct Target {
    conn: Conn,
    vm: DomainSummary,
}

pub struct SnapshotsView {
    pub root: gtk::Stack,
    list: gtk::ListBox,
    busy: adw::Spinner,
    take: gtk::Button,
    aside: gtk::Box,
    aside_title: gtk::Label,
    aside_screen: gtk::Picture,
    aside_facts: gtk::Grid,
    aside_note: gtk::Label,
    restore: gtk::Button,
    edit: gtk::Button,
    delete: gtk::Button,
    snapshots: RefCell<Vec<SnapshotInfo>>,
    target: RefCell<Option<Target>>,
    feedback: Feedback,
    on_changed: Changed,
}

impl std::fmt::Debug for SnapshotsView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnapshotsView").finish_non_exhaustive()
    }
}

impl SnapshotsView {
    /// `on_changed` is called after anything changed the VM (e.g. restore).
    pub fn new(feedback: Feedback, on_changed: impl Fn() + 'static) -> Rc<Self> {
        let title = gtk::Label::builder().label("Snapshots").xalign(0.0).css_classes(["title-2"]).build();
        let subtitle = gtk::Label::builder()
            .label("Save the machine's state and return to it later.")
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim-label"])
            .build();
        let titles = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).hexpand(true).build();
        titles.append(&title);
        titles.append(&subtitle);
        let busy = adw::Spinner::builder().visible(false).build();
        let take = labeled_button("camera-photo-symbolic", "Take Snapshot");
        take.set_action_name(Some("vm.take-snapshot"));
        let head = gtk::Box::builder().spacing(12).build();
        head.append(&titles);
        head.append(&busy);
        head.append(&take);

        let list = gtk::ListBox::builder().css_classes(["snapshot-timeline"]).build();
        list.update_property(&[gtk::accessible::Property::Label("Snapshots")]);
        let left = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(16).hexpand(true).build();
        left.append(&head);
        left.append(&list);

        // The details card for the selected snapshot.
        let aside_title = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["title-3"]).build();
        // The screen as it was when the snapshot was taken.
        let aside_screen = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .height_request(170)
            .visible(false)
            .css_classes(["snapshot-screen"])
            .build();
        let aside_facts = gtk::Grid::builder().row_spacing(12).column_spacing(16).column_homogeneous(true).build();
        let aside_note = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["dim-label"]).build();
        let restore = gtk::Button::builder().label("Restore").hexpand(true).css_classes(["suggested-action"]).build();
        let edit = gtk::Button::builder().label("Edit Description…").build();
        let delete = gtk::Button::builder().label("Delete").css_classes(["destructive-action"]).build();
        let buttons = gtk::Box::builder().spacing(8).margin_top(4).build();
        buttons.append(&restore);
        buttons.append(&edit);
        buttons.append(&delete);
        let aside = card(gtk::Orientation::Vertical, 12);
        aside.add_css_class("snapshot-details");
        aside.set_valign(gtk::Align::Start);
        aside.set_width_request(320);
        aside.append(&aside_title);
        aside.append(&aside_screen);
        aside.append(&aside_facts);
        aside.append(&aside_note);
        aside.append(&buttons);

        let split = super::widgets::ListDetail::new(&left, &aside, 28, false, "Snapshots", 700.0, |b| {
            super::widgets::scrolled_page(b, 1100)
        });
        let s = split.clone();
        list.connect_row_activated(move |_, _| s.show_detail());
        let scroller = split.root().clone();

        let empty_take = gtk::Button::builder()
            .label("Take Snapshot")
            .action_name("vm.take-snapshot")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        let empty = adw::StatusPage::builder()
            .icon_name("camera-photo-symbolic")
            .title("No snapshots yet")
            .description("Take a snapshot before risky changes, so you can go back to how things were.")
            .child(&empty_take)
            .build();
        let loading = adw::Spinner::builder().halign(gtk::Align::Center).valign(gtk::Align::Center).build();

        let root = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
        root.add_named(&loading, Some("loading"));
        root.add_named(&empty, Some("empty"));
        root.add_named(&scroller, Some("list"));

        let this = Rc::new(Self {
            root,
            list,
            busy,
            take,
            aside,
            aside_title,
            aside_screen,
            aside_facts,
            aside_note,
            restore,
            edit,
            delete,
            snapshots: RefCell::default(),
            target: RefCell::default(),
            feedback,
            on_changed: Rc::new(on_changed),
        });

        let weak = Rc::downgrade(&this);
        this.list.connect_row_selected(move |_, row| {
            if let Some(this) = weak.upgrade() {
                this.show_aside(row.map(|r| r.index()));
            }
        });
        let weak = Rc::downgrade(&this);
        this.restore.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.ask_restore();
            }
        });
        let weak = Rc::downgrade(&this);
        this.delete.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.ask_delete();
            }
        });
        let weak = Rc::downgrade(&this);
        this.edit.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.edit_description();
            }
        });
        this
    }

    /// Shows the snapshots of `vm`, reloading them if it is another VM.
    pub fn set_target(self: &Rc<Self>, conn: Conn, vm: &DomainSummary) {
        let same = self.target.borrow().as_ref().is_some_and(|t| t.vm.uuid == vm.uuid);
        let state_changed = self.target.borrow().as_ref().is_some_and(|t| t.vm.state != vm.state);
        self.target.replace(Some(Target { conn, vm: vm.clone() }));
        if !same {
            self.snapshots.borrow_mut().clear();
            self.root.set_visible_child_name("loading");
            self.reload();
        } else if state_changed {
            // The "Now" row shows the current state.
            self.render();
        }
    }

    pub fn reload(self: &Rc<Self>) {
        let Some((conn, uuid)) = self.target.borrow().as_ref().map(|t| (t.conn.clone(), t.vm.uuid.clone())) else {
            return;
        };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let id = uuid.clone();
            let result = conn.call(move |c| c.list_snapshots(&id)).await;
            let Some(this) = weak.upgrade() else { return };
            if this.target.borrow().as_ref().map(|t| t.vm.uuid.as_str()) != Some(uuid.as_str()) {
                return;
            }
            match result {
                Ok(list) => {
                    this.snapshots.replace(list);
                    this.render();
                }
                Err(e) => {
                    this.root.set_visible_child_name("empty");
                    this.feedback.error("Couldn't load the snapshots", e.message());
                }
            }
        });
    }

    fn render(&self) {
        let snaps = self.snapshots.borrow();
        if snaps.is_empty() {
            self.root.set_visible_child_name("empty");
            return;
        }
        let selected = self.list.selected_row().map(|r| r.index());
        self.list.remove_all();
        let state = self.target.borrow().as_ref().map(|t| t.vm.state);
        let now_text = match state {
            Some(s) => format!("{} · Current state", s.label()),
            None => "Current state".to_owned(),
        };
        let count = snaps.len() + 1;
        self.list.append(&timeline_row("Now", &now_text, None, false, true, 0, count));
        for (i, s) in snaps.iter().enumerate() {
            let meta = format!("{} · {}", format_time(s.created), kind_text(s));
            self.list.append(&timeline_row(
                &s.name,
                &meta,
                s.description.as_deref(),
                s.is_current,
                false,
                i + 1,
                count,
            ));
        }
        drop(snaps);
        // Keep the selection; by default select the current snapshot.
        let default = self.snapshots.borrow().iter().position(|s| s.is_current).map_or(1, |p| p as i32 + 1);
        let index = selected.unwrap_or(default).min(count as i32 - 1);
        self.list.select_row(self.list.row_at_index(index).as_ref());
        self.root.set_visible_child_name("list");
    }

    fn selected(&self) -> Option<SnapshotInfo> {
        let index = self.list.selected_row()?.index();
        (index > 0).then(|| self.snapshots.borrow().get(index as usize - 1).cloned()).flatten()
    }

    fn show_aside(&self, index: Option<i32>) {
        while let Some(child) = self.aside_facts.first_child() {
            self.aside_facts.remove(&child);
        }
        let fact = |col: i32, row: i32, title: &str, value: &str| {
            let b = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).build();
            b.append(&gtk::Label::builder().label(title).xalign(0.0).css_classes(["caption", "dim-label"]).build());
            b.append(&gtk::Label::builder().label(value).xalign(0.0).wrap(true).css_classes(["heading"]).build());
            self.aside_facts.attach(&b, col, row, 1, 1);
        };
        let snap = match index {
            Some(i) if i > 0 => self.snapshots.borrow().get(i as usize - 1).cloned(),
            _ => None,
        };
        let Some(s) = snap else {
            // "Now": the live machine.
            self.aside.set_visible(index == Some(0));
            self.aside_title.set_label("Current state");
            self.aside_note.set_label("Take a snapshot to save this state, so you can return to it later.");
            self.aside_note.set_visible(true);
            self.aside_screen.set_visible(false);
            for b in [&self.restore, &self.edit, &self.delete] {
                b.set_visible(false);
            }
            return;
        };
        let file = self.target.borrow().as_ref().map(|t| screen_file(&t.vm.uuid, &s.name)).filter(|f| f.exists());
        self.aside_screen.set_filename(file.as_ref());
        self.aside_screen.set_visible(file.is_some());
        self.aside.set_visible(true);
        self.aside_title.set_label(&s.name);
        fact(0, 0, "Taken", &format_time(s.created));
        fact(1, 0, "Type", kind_text(&s));
        fact(
            0,
            1,
            "Stored",
            if s.location == SnapshotLocation::Internal { "Inside the disk" } else { "Separate files" },
        );
        fact(1, 1, "Children", &s.children.to_string());
        self.aside_note.set_label(s.description.as_deref().unwrap_or_default());
        self.aside_note.set_visible(s.description.is_some());
        for b in [&self.restore, &self.edit, &self.delete] {
            b.set_visible(true);
        }
    }

    fn set_busy(&self, busy: bool) {
        self.busy.set_visible(busy);
        self.list.set_sensitive(!busy);
        self.aside.set_sensitive(!busy);
        self.take.set_sensitive(!busy);
    }

    /// Runs an operation on the selected snapshot with busy indication.
    fn run(
        self: &Rc<Self>,
        op: impl FnOnce(&vmng_virt::Connection, &str) -> vmng_virt::Result<()> + Send + 'static,
        failure: String,
        success: Option<String>,
    ) {
        let Some((conn, uuid)) = self.target.borrow().as_ref().map(|t| (t.conn.clone(), t.vm.uuid.clone())) else {
            return;
        };
        self.set_busy(true);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn.call(move |c| op(c, &uuid)).await;
            let Some(this) = weak.upgrade() else { return };
            this.set_busy(false);
            match result {
                Ok(()) => {
                    if let Some(msg) = success {
                        this.feedback.toast(&msg);
                    }
                }
                Err(e) => this.feedback.error(&failure, e.message()),
            }
            this.reload();
            (this.on_changed)();
        });
    }

    fn ask_restore(self: &Rc<Self>) {
        let Some(s) = self.selected() else { return };
        let body = format!(
            "The machine goes back to how it was on {}. Anything changed since then is lost, unless you take a snapshot first.",
            format_time(s.created)
        );
        let weak = Rc::downgrade(self);
        self.feedback.confirm(&format!("Restore “{}”?", s.name), &body, "Restore", true, move || {
            if let Some(this) = weak.upgrade() {
                let name = s.name.clone();
                this.run(
                    move |c, uuid| c.revert_snapshot(uuid, &name),
                    format!("Couldn't restore “{}”", s.name),
                    Some(format!("Restored “{}”", s.name)),
                );
            }
        });
    }

    fn ask_delete(self: &Rc<Self>) {
        let Some(s) = self.selected() else { return };
        let weak = Rc::downgrade(self);
        self.feedback.confirm(
            &format!("Delete snapshot “{}”?", s.name),
            "You can no longer return to this state. The machine itself is not changed.",
            "Delete",
            true,
            move || {
                if let Some(this) = weak.upgrade() {
                    let name = s.name.clone();
                    this.run(
                        move |c, uuid| {
                            c.delete_snapshot(uuid, &name)?;
                            let _ = std::fs::remove_file(screen_file(uuid, &name));
                            Ok(())
                        },
                        format!("Couldn't delete “{}”", s.name),
                        Some(format!("Deleted snapshot “{}”", s.name)),
                    );
                }
            },
        );
    }

    fn edit_description(self: &Rc<Self>) {
        let Some(s) = self.selected() else { return };
        let form = FormDialog::new("Edit Description", "Save");
        let group = adw::PreferencesGroup::builder().title(&s.name).build();
        let entry = adw::EntryRow::builder()
            .title("Description")
            .text(s.description.as_deref().unwrap_or_default())
            .activates_default(true)
            .build();
        group.add(&entry);
        form.add(&group);
        let weak = Rc::downgrade(self);
        form.connect_primary(move |form| {
            let Some(this) = weak.upgrade() else { return };
            let (name, text) = (s.name.clone(), entry.text().to_string());
            this.run(
                move |c, uuid| c.set_snapshot_description(uuid, &name, &text),
                format!("Couldn't change the description of “{}”", s.name),
                None,
            );
            form.close();
        });
        form.present(self.feedback.parent());
    }
}

/// Opens the "Take Snapshot" dialog for `vm`. `done` runs after success.
pub fn take_snapshot_dialog(feedback: &Feedback, conn: Conn, vm: &DomainSummary, done: impl Fn() + 'static) {
    let form = FormDialog::new("Take Snapshot", "Take Snapshot");
    let running = vm.state.is_active();
    let default_name = glib::DateTime::now_local()
        // No spaces: QEMU can't take a running machine's internal snapshot
        // with them (see `validate_snapshot_name_for`).
        .and_then(|d| d.format("Snapshot-%Y-%m-%d-%H.%M"))
        .map(|s| s.to_string())
        .unwrap_or_else(|_| "Snapshot".into());
    let group = adw::PreferencesGroup::builder()
        .description(if running {
            "The machine's memory is saved too: restoring resumes it exactly where it was. It pauses briefly while the snapshot is taken."
        } else {
            "The machine is shut off, so its disks are saved."
        })
        .build();
    let name = adw::EntryRow::builder().title("Name").text(&default_name).activates_default(true).build();
    let description = adw::EntryRow::builder().title("Description (optional)").activates_default(true).build();
    group.add(&name);
    group.add(&description);
    form.add(&group);

    // Where it's stored: virt-manager's "Snapshot Mode".
    let storage = adw::PreferencesGroup::builder()
        .title("Storage")
        .description(
            "Inside the disk keeps the snapshot in the qcow2 disk images. Separate files puts it in new files \
             next to each disk, which works with any disk format.",
        )
        .build();
    let mode = adw::ComboRow::builder()
        .title("Stored")
        .model(&gtk::StringList::new(&["Inside the disk", "Separate files"]))
        .sensitive(false)
        .build();
    let memory_dir: Rc<RefCell<Option<String>>> = Rc::default();
    let memory = adw::ActionRow::builder()
        .title("Memory file folder")
        .subtitle("Next to the machine's disk")
        .visible(false)
        .build();
    let choose = gtk::Button::builder().label("Choose…").valign(gtk::Align::Center).build();
    memory.add_suffix(&choose);
    storage.add(&mode);
    storage.add(&memory);
    form.add(&storage);
    let external = |m: &adw::ComboRow| m.selected() == 1;
    {
        let memory = memory.clone();
        mode.connect_selected_notify(move |m| memory.set_visible(running && external(m)));
    }
    {
        let (memory, memory_dir) = (memory.clone(), memory_dir.clone());
        let remote = conn.remote_name();
        choose.connect_clicked(move |b| {
            let (memory, memory_dir) = (memory.clone(), memory_dir.clone());
            super::widgets::pick_host_path(
                b.upcast_ref(),
                remote.as_deref(),
                true,
                "Folder for the Memory File",
                None,
                move |path| {
                    memory.set_subtitle(&path);
                    memory_dir.replace(Some(path));
                },
            );
        });
    }
    {
        // Which modes the machine allows decides the choice offered.
        let (conn, uuid, mode, f) = (conn.clone(), vm.uuid.clone(), mode.clone(), form.clone());
        glib::spawn_future_local(async move {
            match conn.call(move |c| c.snapshot_modes(&uuid)).await {
                Ok(m) => {
                    mode.set_selected(u32::from(m.default == SnapshotLocation::External));
                    mode.notify("selected");
                    mode.set_sensitive(m.internal);
                    if !m.internal {
                        mode.set_subtitle("This machine's disks can't hold them");
                    }
                }
                Err(e) => f.set_error(Some(&format!("Couldn't read the machine's disks: {}", e.message()))),
            }
        });
    }

    let location = move |m: &adw::ComboRow| {
        if external(m) { SnapshotLocation::External } else { SnapshotLocation::Internal }
    };
    let check = {
        let (f, name, mode) = (form.clone(), name.clone(), mode.clone());
        move || f.set_error(validate_snapshot_name_for(name.text().trim(), running, location(&mode)).err())
    };
    {
        let check = check.clone();
        name.connect_changed(move |_| check());
    }
    mode.connect_selected_notify(move |_| check());

    let uuid = vm.uuid.clone();
    let vm_name = vm.display_name().to_owned();
    let parent = feedback.parent().clone();
    let feedback = feedback.clone();
    let done = Rc::new(done);
    let has_screen = running && !vm.config.graphics.is_empty();
    form.connect_primary(move |form| {
        let snap_name = name.text().trim().to_owned();
        if let Err(msg) = validate_snapshot_name_for(&snap_name, running, location(&mode)) {
            form.set_error(Some(msg));
            return;
        }
        let desc = description.text().to_string();
        let options = SnapshotOptions { location: location(&mode), memory_dir: memory_dir.borrow().clone() };
        form.set_busy(true);
        form.set_primary_label("Taking Snapshot…");
        let (conn, uuid, form, feedback, done, vm_name) =
            (conn.clone(), uuid.clone(), form.clone(), feedback.clone(), done.clone(), vm_name.clone());
        glib::spawn_future_local(async move {
            // The screen just before, to show with the snapshot.
            let screen = if has_screen {
                let id = uuid.clone();
                conn.call(move |c| c.screenshot(&id)).await.ok().and_then(super::window::screenshot_texture)
            } else {
                None
            };
            let (n, id) = (snap_name.clone(), uuid.clone());
            let result = conn.call(move |c| c.create_snapshot(&id, &n, &desc, Some(&options))).await;
            form.set_busy(false);
            form.set_primary_label("Take Snapshot");
            match result {
                Ok(()) => {
                    if let Some(t) = screen {
                        save_screen(&uuid, &snap_name, &t);
                    }
                    form.close();
                    feedback.toast(&format!("Took snapshot “{snap_name}” of {vm_name}"));
                    done();
                }
                Err(e) => form.set_error(Some(&format!("Couldn't take the snapshot: {}", e.message()))),
            }
        });
    });
    form.present(&parent);
}

/// One entry of the timeline: gutter (line, dot, line) and text.
fn timeline_row(
    title: &str,
    meta: &str,
    note: Option<&str>,
    current: bool,
    now: bool,
    index: usize,
    count: usize,
) -> gtk::ListBoxRow {
    let line = |visible: bool| {
        let l = gtk::Box::builder().vexpand(true).halign(gtk::Align::Center).css_classes(["timeline-line"]).build();
        l.set_opacity(if visible { 1.0 } else { 0.0 });
        l
    };
    let dot = gtk::Box::builder().halign(gtk::Align::Center).css_classes(["timeline-dot"]).build();
    if now {
        dot.add_css_class("now");
    }
    let gutter = gtk::Box::builder().orientation(gtk::Orientation::Vertical).width_request(20).build();
    gutter.append(&line(index > 0));
    gutter.append(&dot);
    gutter.append(&line(index + 1 < count));

    let name = gtk::Label::builder().label(title).xalign(0.0).wrap(true).css_classes(["heading"]).build();
    let name_row = gtk::Box::builder().spacing(8).build();
    name_row.append(&name);
    if current {
        name_row
            .append(&gtk::Label::builder().label("Current").valign(gtk::Align::Center).css_classes(["badge"]).build());
    }
    let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(3).hexpand(true).build();
    text.append(&name_row);
    text.append(&gtk::Label::builder().label(meta).xalign(0.0).css_classes(["caption", "dim-label"]).build());
    if let Some(note) = note {
        text.append(&gtk::Label::builder().label(note).xalign(0.0).wrap(true).css_classes(["dim-label"]).build());
    }
    let body = gtk::Box::builder().css_classes(["snapshot-entry"]).hexpand(true).build();
    body.append(&text);

    let content = gtk::Box::builder().spacing(14).build();
    content.append(&gutter);
    content.append(&body);
    let row = gtk::ListBoxRow::builder().child(&content).build();
    row.update_property(&[gtk::accessible::Property::Label(&format!("{title}, {meta}"))]);
    row
}
