//! Dialogs for managing a VM: clone, rename and delete.

use std::rc::Rc;

use adw::prelude::*;
use vmng_virt::{CloneDiskMode, DiskConfig, DomainSummary, unique_name, validate_vm_name};

use super::feedback::confirm_dialog;
use super::vm_details::disk_title;
use super::widgets::FormDialog;
use crate::backend::VmLive;
use crate::format;

/// Writable disks that can be copied or deleted (not CD-ROMs or read-only media).
fn own_disks(vm: &DomainSummary) -> impl Iterator<Item = &DiskConfig> {
    vm.config.disks.iter().filter(|d| !d.is_cdrom() && !d.read_only && d.source_path.is_some())
}

/// Other VMs that use the same image file.
fn shared_with<'a>(path: &str, vm: &DomainSummary, all: &'a [DomainSummary]) -> Vec<&'a str> {
    all.iter()
        .filter(|o| o.uuid != vm.uuid && o.config.disks.iter().any(|d| d.source_path.as_deref() == Some(path)))
        .map(|o| o.display_name())
        .collect()
}

fn name_error(name: &str, all: &[DomainSummary]) -> Option<&'static str> {
    validate_vm_name(name)
        .err()
        .or_else(|| all.iter().any(|d| d.config.name == name).then_some("Another machine already has this name."))
}

fn allocation(live: Option<&VmLive>, target: &str, running: bool) -> Option<u64> {
    live?.disks.iter().find(|b| b.name == target)?.used(running)
}

/// Asks for the clone's name and which disks to copy, then calls `start`.
pub fn clone_dialog(
    parent: &impl IsA<gtk::Widget>,
    vm: &DomainSummary,
    live: Option<&VmLive>,
    all: &[DomainSummary],
    start: impl Fn(String, Vec<(String, CloneDiskMode)>) + 'static,
) {
    let form = FormDialog::new(&format!("Clone {}", vm.display_name()), "Clone");
    let suggested = unique_name(&format!("{}-clone", vm.config.name), |n| all.iter().any(|d| d.config.name == n));
    let group = adw::PreferencesGroup::builder()
        .description("The clone gets its own network addresses and UEFI settings.")
        .build();
    let name = adw::EntryRow::builder().title("Name").text(&suggested).activates_default(true).build();
    group.add(&name);
    form.add(&group);

    let disks_group = adw::PreferencesGroup::builder().title("Disks").build();
    let mut switches = Vec::new();
    for d in own_disks(vm) {
        let size = allocation(live, &d.target, vm.state.is_active()).map(format::bytes);
        let copy_text = match &size {
            Some(s) => format!("A copy is made · {s}"),
            None => "A copy is made".to_owned(),
        };
        let row = adw::SwitchRow::builder().title(disk_title(d)).subtitle(&copy_text).active(true).build();
        let shared = "Shared: both machines use the same disk. Only safe if one of them never runs.";
        row.connect_active_notify(move |r| r.set_subtitle(if r.is_active() { &copy_text } else { shared }));
        disks_group.add(&row);
        switches.push((d.target.clone(), row));
    }
    if !switches.is_empty() {
        form.add(&disks_group);
    }

    let names: Vec<DomainSummary> = all.to_vec();
    let f = form.clone();
    name.connect_changed(move |e| f.set_error(name_error(e.text().trim(), &names)));
    let names: Vec<DomainSummary> = all.to_vec();
    let start = Rc::new(start);
    form.connect_primary(move |form| {
        let new_name = name.text().trim().to_owned();
        if let Some(msg) = name_error(&new_name, &names) {
            form.set_error(Some(msg));
            return;
        }
        let plan = switches
            .iter()
            .map(|(t, s)| (t.clone(), if s.is_active() { CloneDiskMode::Copy } else { CloneDiskMode::Share }))
            .collect();
        form.close();
        start(new_name, plan);
    });
    form.present(parent);
}

/// Asks for a new name, then calls `apply`.
pub fn rename_dialog(
    parent: &impl IsA<gtk::Widget>,
    vm: &DomainSummary,
    all: &[DomainSummary],
    apply: impl Fn(String) + 'static,
) {
    let form = FormDialog::new(&format!("Rename {}", vm.display_name()), "Rename");
    let group = adw::PreferencesGroup::new();
    let name = adw::EntryRow::builder().title("Name").text(&vm.config.name).activates_default(true).build();
    group.add(&name);
    form.add(&group);
    let others: Vec<DomainSummary> = all.iter().filter(|d| d.uuid != vm.uuid).cloned().collect();
    let f = form.clone();
    let o = others.clone();
    name.connect_changed(move |e| f.set_error(name_error(e.text().trim(), &o)));
    let current = vm.config.name.clone();
    form.connect_primary(move |form| {
        let new_name = name.text().trim().to_owned();
        if let Some(msg) = name_error(&new_name, &others) {
            form.set_error(Some(msg));
            return;
        }
        form.close();
        if new_name != current {
            apply(new_name);
        }
    });
    form.present(parent);
}

/// Confirms deleting a VM, optionally with its disks, then calls `apply`
/// with the image paths to delete.
pub fn delete_dialog(
    parent: &impl IsA<gtk::Widget>,
    vm: &DomainSummary,
    live: Option<&VmLive>,
    all: &[DomainSummary],
    apply: impl Fn(Vec<String>) + 'static,
) {
    let mut body = "The machine and its snapshots are removed. This can't be undone.".to_owned();
    if vm.state.is_active() {
        body.push_str(" It is running and will be turned off first.");
    }
    let dialog = confirm_dialog(&format!("Delete {}?", vm.display_name()), &body, "Delete", true);

    let mut deletable = Vec::new();
    let mut kept = Vec::new();
    let mut total = 0u64;
    for d in own_disks(vm) {
        let Some(path) = d.source_path.clone() else { continue };
        let users = shared_with(&path, vm, all);
        if users.is_empty() {
            total += allocation(live, &d.target, vm.state.is_active()).unwrap_or(0);
            deletable.push(path);
        } else {
            kept.push(format!("{} is also used by {} and is kept.", file_name(&path), users.join(", ")));
        }
    }

    let extra = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build();
    let check = gtk::CheckButton::builder()
        .label(if total > 0 {
            format!("Also delete its disks ({})", format::bytes(total))
        } else {
            "Also delete its disks".to_owned()
        })
        .build();
    if !deletable.is_empty() {
        extra.append(&check);
        let files: Vec<&str> = deletable.iter().map(|p| file_name(p)).collect();
        extra.append(
            &gtk::Label::builder()
                .label(files.join(", "))
                .xalign(0.0)
                .wrap(true)
                .margin_start(28)
                .css_classes(["caption", "dim-label"])
                .build(),
        );
    }
    for note in &kept {
        extra.append(
            &gtk::Label::builder().label(note).xalign(0.0).wrap(true).css_classes(["caption", "dim-label"]).build(),
        );
    }
    if extra.first_child().is_some() {
        dialog.set_extra_child(Some(&extra));
    }

    dialog.connect_response(Some("confirm"), move |_, _| {
        apply(if check.is_active() { deletable.clone() } else { Vec::new() });
    });
    dialog.present(Some(parent));
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use vmng_virt::{DomainConfig, DomainState};

    use super::*;

    fn vm(uuid: &str, name: &str, disk: &str) -> DomainSummary {
        DomainSummary {
            uuid: uuid.into(),
            state: DomainState::ShutOff,
            persistent: true,
            autostart: false,
            has_managed_save: false,
            config: DomainConfig {
                name: name.into(),
                disks: vec![DiskConfig {
                    device: "disk".into(),
                    target: "vda".into(),
                    bus: None,
                    format: None,
                    source_path: Some(disk.into()),
                    source_volume: None,
                    read_only: false,
                }],
                ..Default::default()
            },
        }
    }

    #[test]
    fn detects_shared_disks_and_names() {
        let a = vm("1", "a", "/img/a.qcow2");
        let b = vm("2", "b", "/img/a.qcow2");
        let c = vm("3", "c", "/img/c.qcow2");
        let all = [a.clone(), b, c];
        assert_eq!(shared_with("/img/a.qcow2", &a, &all), ["b"]);
        assert!(shared_with("/img/c.qcow2", &all[2], &all).is_empty());
        assert!(name_error("b", &all).is_some());
        assert!(name_error("new", &all).is_none());
    }
}
