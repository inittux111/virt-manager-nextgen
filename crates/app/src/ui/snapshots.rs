//! The Snapshots tab: a timeline of saved states with a details card.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use vmng_virt::{DomainSummary, SnapshotInfo, SnapshotLocation, validate_snapshot_name};

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
        aside.append(&aside_facts);
        aside.append(&aside_note);
        aside.append(&buttons);

        let content = gtk::Box::builder().spacing(28).css_classes(["page-content"]).build();
        content.append(&left);
        content.append(&aside);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&adw::Clamp::builder().maximum_size(1100).child(&content).build())
            .build();

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
            for b in [&self.restore, &self.edit, &self.delete] {
                b.set_visible(false);
            }
            return;
        };
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
                        move |c, uuid| c.delete_snapshot(uuid, &name),
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
    let default_name = glib::DateTime::now_local()
        .and_then(|d| d.format("Snapshot %Y-%m-%d %H.%M"))
        .map(|s| s.to_string())
        .unwrap_or_else(|_| "Snapshot".into());
    let group = adw::PreferencesGroup::builder()
        .description(if vm.state.is_active() {
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

    let f = form.clone();
    name.connect_changed(move |e| f.set_error(validate_snapshot_name(&e.text()).err()));

    let uuid = vm.uuid.clone();
    let vm_name = vm.display_name().to_owned();
    let parent = feedback.parent().clone();
    let feedback = feedback.clone();
    let done = Rc::new(done);
    form.connect_primary(move |form| {
        let snap_name = name.text().trim().to_owned();
        if let Err(msg) = validate_snapshot_name(&snap_name) {
            form.set_error(Some(msg));
            return;
        }
        let desc = description.text().to_string();
        form.set_busy(true);
        form.set_primary_label("Taking Snapshot…");
        let (conn, uuid, form, feedback, done, vm_name) =
            (conn.clone(), uuid.clone(), form.clone(), feedback.clone(), done.clone(), vm_name.clone());
        glib::spawn_future_local(async move {
            let n = snap_name.clone();
            let result = conn.call(move |c| c.create_snapshot(&uuid, &n, &desc)).await;
            form.set_busy(false);
            form.set_primary_label("Take Snapshot");
            match result {
                Ok(()) => {
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
