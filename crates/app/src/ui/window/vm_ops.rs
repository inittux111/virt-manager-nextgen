//! VM operations: power, autostart, the `vm.*` actions, delete and clone
//! (with the window's job bar).

use super::*;

/// A slim progress bar at the bottom of the window for long operations.
#[derive(Clone, Debug)]
pub(super) struct JobBar {
    pub(super) revealer: gtk::Revealer,
    label: gtk::Label,
    bar: gtk::ProgressBar,
}

impl JobBar {
    pub(super) fn new() -> Self {
        let label = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
        let bar = gtk::ProgressBar::new();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .css_classes(["card", "job-bar"])
            .build();
        content.append(&label);
        content.append(&bar);
        content.update_property(&[gtk::accessible::Property::Label("Operation in progress")]);
        let clamp = adw::Clamp::builder()
            .maximum_size(560)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .child(&content)
            .build();
        let revealer =
            gtk::Revealer::builder().transition_type(gtk::RevealerTransitionType::SlideUp).child(&clamp).build();
        Self { revealer, label, bar }
    }

    pub(super) fn start(&self, text: &str) {
        self.label.set_label(text);
        self.bar.set_fraction(0.0);
        self.revealer.set_reveal_child(true);
    }

    pub(super) fn update(&self, text: &str, fraction: f64) {
        self.label.set_label(text);
        self.bar.set_fraction(fraction.clamp(0.0, 1.0));
    }

    pub(super) fn pulse(&self) {
        self.bar.pulse();
    }

    pub(super) fn finish(&self) {
        self.revealer.set_reveal_child(false);
    }
}

/// Plain advice for start failures whose libvirt message doesn't say what
/// to do.
pub(super) fn start_hint(details: &str) -> Option<&'static str> {
    // 3D acceleration: QEMU couldn't use the graphics card. On the System
    // connection QEMU runs confined (SELinux), which NVIDIA's own driver
    // isn't allowed to work in.
    (details.contains("egl:") || details.contains("render node")).then_some(
        "The machine's 3D acceleration couldn't use the graphics card. Turn off 3D acceleration in Hardware → \
         Video, or run the machine on the User session connection. On the System connection, some graphics \
         drivers (such as NVIDIA's own) aren't allowed to run inside the virtual machine's process.",
    )
}

impl MainWindow {
    /// Runs a power action, asking for confirmation first where configured.
    pub(super) fn request_power(self: &Rc<Self>, uuid: String, action: PowerAction) {
        let prefs = self.settings.get();
        let name = self.domain_name(&uuid);
        let confirm = match action {
            PowerAction::ForceOff | PowerAction::Reset => prefs.confirm_force_off,
            PowerAction::Shutdown | PowerAction::Reboot => prefs.confirm_shutdown,
            PowerAction::Pause => prefs.confirm_pause,
            _ => false,
        };
        if !confirm {
            self.power(uuid, action);
            return;
        }
        let (heading, body, verb, destructive) = match action {
            PowerAction::ForceOff => (
                format!("Force off {name}?"),
                "This is like pulling the power plug. Unsaved work in the machine is lost.",
                "Force Off",
                true,
            ),
            PowerAction::Reset => (
                format!("Reset {name}?"),
                "This is like pressing the reset button. Unsaved work in the machine is lost.",
                "Reset",
                true,
            ),
            PowerAction::Shutdown => {
                (format!("Shut down {name}?"), "The guest is asked to shut down cleanly.", "Shut Down", false)
            }
            PowerAction::Reboot => (format!("Reboot {name}?"), "The guest is asked to restart.", "Reboot", false),
            _ => (format!("Pause {name}?"), "The machine stops running until it is resumed.", "Pause", false),
        };
        let dialog = adw::AlertDialog::builder().heading(heading).body(body).build();
        dialog.add_responses(&[("cancel", "Cancel"), ("confirm", verb)]);
        dialog.set_response_appearance(
            "confirm",
            if destructive { adw::ResponseAppearance::Destructive } else { adw::ResponseAppearance::Suggested },
        );
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        dialog.connect_response(Some("confirm"), move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.power(uuid.clone(), action);
            }
        });
        dialog.present(Some(&self.window));
    }

    pub(super) fn power(self: &Rc<Self>, uuid: String, action: PowerAction) {
        let Some(conn) = self.current_conn() else { return };
        let name = self.domain_name(&uuid);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn.call(move |c| c.power(&uuid, action)).await;
            let Some(this) = weak.upgrade() else { return };
            if let Err(e) = result {
                let mut details = e.message().to_owned();
                if let Some(hint) = start_hint(&details) {
                    details.push_str("\n\n");
                    details.push_str(hint);
                }
                this.feedback.error(&format!("Couldn't {} {name}", action.label().to_lowercase()), &details);
            }
            this.refresh();
        });
    }

    pub(super) fn set_autostart(self: &Rc<Self>, uuid: String, on: bool) {
        let Some(conn) = self.current_conn() else { return };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn.call(move |c| c.set_autostart(&uuid, on)).await;
            let Some(this) = weak.upgrade() else { return };
            if let Err(e) = result {
                this.feedback.error("Couldn't change the start-at-boot setting", e.message());
            }
            this.refresh();
        });
    }

    pub(super) fn vm_by_uuid(&self, uuid: &str) -> Option<DomainSummary> {
        self.state.borrow().domains.iter().find(|d| d.uuid == uuid).cloned()
    }

    /// Handles a `vm.*` action for the VM `uuid`.
    pub(super) fn vm_action(self: &Rc<Self>, uuid: String, action: VmAction) {
        let Some(vm) = self.vm_by_uuid(&uuid) else { return };
        let Some(conn) = self.current_conn() else { return };
        // Pages and dialogs live in the main window: bring it forward when
        // they are asked for from a console window.
        if !matches!(action, VmAction::Power(_) | VmAction::Console | VmAction::SyncTime) {
            self.bring_to_front();
        }
        match action {
            VmAction::Power(p) => self.request_power(uuid, p),
            VmAction::Console => self.open_console(&vm),
            VmAction::SyncTime => {
                let name = vm.display_name().to_owned();
                let id = uuid.clone();
                self.run_vm_op(
                    move |c| c.sync_guest_time(&id),
                    format!("Couldn't sync the clock of {name}"),
                    format!("Synced the clock of {name} with this computer"),
                );
            }
            VmAction::Hardware => self.open_vm_page(&vm, VmTab::Hardware),
            VmAction::Snapshots => self.open_vm_page(&vm, VmTab::Snapshots),
            VmAction::Performance => self.open_vm_page(&vm, VmTab::Performance),
            VmAction::TakeSnapshot => {
                let weak = Rc::downgrade(self);
                take_snapshot_dialog(&self.feedback, conn, &vm, move || {
                    if let Some(this) = weak.upgrade() {
                        this.vm_page.snapshots.reload();
                        this.refresh();
                    }
                });
            }
            VmAction::Clone => {
                let (live, all) = {
                    let st = self.state.borrow();
                    (st.monitor.vm(&uuid).cloned(), st.domains.clone())
                };
                let weak = Rc::downgrade(self);
                clone_dialog(&self.window, &vm, live.as_ref(), &all, move |name, disks| {
                    if let Some(this) = weak.upgrade() {
                        this.clone_vm(uuid.clone(), name, disks);
                    }
                });
            }
            VmAction::Rename => {
                let all = self.state.borrow().domains.clone();
                let weak = Rc::downgrade(self);
                let old_name = vm.display_name().to_owned();
                rename_dialog(&self.window, &vm, &all, move |name| {
                    if let Some(this) = weak.upgrade() {
                        let (id, n) = (uuid.clone(), name.clone());
                        this.run_vm_op(
                            move |c| c.rename_domain(&id, &n),
                            format!("Couldn't rename {old_name}"),
                            format!("Renamed {old_name} to {name}"),
                        );
                    }
                });
            }
            VmAction::DiscardSavedState => {
                let weak = Rc::downgrade(self);
                let name = vm.display_name().to_owned();
                self.feedback.confirm(
                    &format!("Discard the saved state of {name}?"),
                    "The machine starts fresh next time instead of resuming. Unsaved work in it is lost.",
                    "Discard",
                    true,
                    move || {
                        if let Some(this) = weak.upgrade() {
                            let id = uuid.clone();
                            this.run_vm_op(
                                move |c| c.discard_saved_state(&id),
                                format!("Couldn't discard the saved state of {name}"),
                                format!("Discarded the saved state of {name}"),
                            );
                        }
                    },
                );
            }
            VmAction::Delete => {
                if self.settings.get().confirm_delete {
                    let (live, all) = {
                        let st = self.state.borrow();
                        (st.monitor.vm(&uuid).cloned(), st.domains.clone())
                    };
                    let weak = Rc::downgrade(self);
                    delete_dialog(&self.window, &vm, live.as_ref(), &all, move |paths| {
                        if let Some(this) = weak.upgrade() {
                            this.delete_vm(uuid.clone(), paths);
                        }
                    });
                } else {
                    self.delete_vm(uuid, Vec::new());
                }
            }
        }
    }

    pub(super) fn open_vm_page(self: &Rc<Self>, vm: &DomainSummary, tab: VmTab) {
        let Some(conn) = self.current_conn() else { return };
        self.vm_page.select(tab);
        let host = self.state.borrow().host_info.clone();
        self.vm_page.show(conn, vm, host);
        if !self.vm_page_open() {
            self.nav.push_by_tag(VmPage::TAG);
        }
    }

    /// Runs a quick operation on the current connection, then refreshes.
    pub(super) fn run_vm_op(
        self: &Rc<Self>,
        op: impl FnOnce(&vmng_virt::Connection) -> vmng_virt::Result<()> + Send + 'static,
        failure: String,
        success: String,
    ) {
        let Some(conn) = self.current_conn() else { return };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn.call(op).await;
            let Some(this) = weak.upgrade() else { return };
            match result {
                Ok(()) => this.feedback.toast(&success),
                Err(e) => this.feedback.error(&failure, e.message()),
            }
            this.refresh();
        });
    }

    pub(super) fn delete_vm(self: &Rc<Self>, uuid: String, paths: Vec<String>) {
        let Some(conn) = self.current_conn() else { return };
        let name = self.domain_name(&uuid);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let id = uuid.clone();
            let result = conn.call(move |c| c.delete_domain(&id, &paths)).await;
            let Some(this) = weak.upgrade() else { return };
            match result {
                Ok(failed) if failed.is_empty() => this.feedback.toast(&format!("Deleted {name}")),
                Ok(failed) => {
                    let details: Vec<String> = failed.iter().map(|(p, e)| format!("{p}: {}", e.message())).collect();
                    this.feedback.error(&format!("Deleted {name}, but some disks were kept"), &details.join("\n"));
                }
                Err(e) => this.feedback.error(&format!("Couldn't delete {name}"), e.message()),
            }
            if this.vm_page_open() {
                this.nav.pop_to_tag("main");
            }
            this.refresh();
        });
    }

    pub(super) fn clone_vm(self: &Rc<Self>, uuid: String, new_name: String, disks: Vec<(String, CloneDiskMode)>) {
        let Some(conn) = self.current_conn() else { return };
        let source = self.domain_name(&uuid);
        let progress = Arc::new((AtomicU64::new(0), AtomicU64::new(0)));
        self.job.start(&format!("Cloning {source} as {new_name}"));

        // The copy runs on a worker thread; the bar follows its progress.
        let p = progress.clone();
        let job = self.job.clone();
        let label = format!("Cloning {source}");
        let ticker = glib::timeout_add_local(std::time::Duration::from_millis(400), move || {
            let (done, total) = (p.0.load(Ordering::Relaxed), p.1.load(Ordering::Relaxed));
            if total > 0 {
                job.update(&format!("{label} — {}", format::used_of(done, total)), done as f64 / total as f64);
            } else {
                job.pulse();
            }
            glib::ControlFlow::Continue
        });

        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let n = new_name.clone();
            let result = conn
                .call(move |c| {
                    c.clone_domain(&uuid, &n, &disks, &|done, total| {
                        progress.0.store(done, Ordering::Relaxed);
                        progress.1.store(total, Ordering::Relaxed);
                    })
                })
                .await;
            ticker.remove();
            let Some(this) = weak.upgrade() else { return };
            this.job.finish();
            match result {
                Ok(new_uuid) => {
                    this.feedback.toast(&format!("Cloned {source} as {new_name}"));
                    this.state.borrow_mut().pending_select = Some(new_uuid);
                }
                Err(e) => this.feedback.error(&format!("Couldn't clone {source}"), e.message()),
            }
            this.refresh();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_failures_get_advice_only_when_known() {
        assert!(start_hint("qemu: egl: no drm render node available").is_some());
        assert!(start_hint("Requested operation is not valid: network 'default' is not active").is_none());
    }
}
