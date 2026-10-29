//! The poll timer: fetching a sample from libvirt and updating the list,
//! details, consoles, previews and guest-agent details from it.

use super::*;

impl MainWindow {
    pub(super) fn restart_timer(self: &Rc<Self>) {
        let secs = self.settings.get().stats_interval_secs.max(1);
        if let Some(id) = self.state.borrow_mut().timer.take() {
            id.remove();
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_seconds_local(secs, move || {
            let Some(this) = weak.upgrade() else { return glib::ControlFlow::Break };
            this.refresh();
            glib::ControlFlow::Continue
        });
        self.state.borrow_mut().timer = Some(id);
    }

    /// Polls the current connection once (skipped if a poll is running).
    pub(super) fn refresh(self: &Rc<Self>) {
        let Some(conn) = self.current_conn() else { return };
        {
            let mut st = self.state.borrow_mut();
            if st.polling {
                return;
            }
            st.polling = true;
            st.tick += 1;
        }
        let kind = conn.kind().clone();
        let prefs = self.settings.get();
        let wanted =
            vmng_virt::StatsWanted { cpu: prefs.stats_cpu, memory: prefs.stats_memory, network: prefs.stats_network };
        self.details.set_collect([prefs.stats_cpu, prefs.stats_memory, prefs.stats_disk, prefs.stats_network]);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn
                .call(move |c| {
                    Ok(Sample {
                        at: Instant::now(),
                        domains: c.list_domains()?,
                        stats: c.domain_stats(wanted)?,
                        host: c.host_stats()?,
                        pool: c.pool_summary("default").ok().flatten(),
                    })
                })
                .await;
            let alive = if result.is_err() { conn.call(|c| Ok(c.is_alive())).await.unwrap_or(false) } else { true };
            let Some(this) = weak.upgrade() else { return };
            this.state.borrow_mut().polling = false;
            if this.state.borrow().current != kind {
                return;
            }
            match result {
                Ok(sample) => this.apply_sample(sample),
                Err(e) if !alive => {
                    this.state.borrow_mut().conns.insert(kind, Slot::Failed(e.message().to_owned()));
                    this.update_chrome();
                }
                Err(e) => glib::g_warning!("vmng", "refresh failed: {e}"),
            }
        });
    }

    pub(super) fn apply_sample(self: &Rc<Self>, sample: Sample) {
        let tick = {
            let mut st = self.state.borrow_mut();
            st.monitor.ingest(&sample);
            st.domains = sample.domains;
            let alive: Vec<&str> = st.domains.iter().map(|d| d.uuid.as_str()).collect();
            let stale: Vec<String> = st.guest.keys().filter(|k| !alive.contains(&k.as_str())).cloned().collect();
            for k in stale {
                st.guest.remove(&k);
            }
            st.tick
        };
        let domains = self.state.borrow().domains.clone();
        self.restart_finished_installs(&domains);
        self.vm_list.update(&domains);
        if self.state.borrow().view == View::Host {
            self.update_host_view();
        }
        self.update_tray_menu();
        {
            let st = self.state.borrow();
            self.vm_list.update_host(st.host_info.as_ref(), st.monitor.host());
        }
        if domains.is_empty() {
            self.machines.set_visible_child_name("empty");
        } else {
            self.machines.set_visible_child_name("content");
            let pending = self.state.borrow_mut().pending_select.take();
            if let Some(uuid) = pending {
                self.vm_list.select(&uuid);
            }
            self.vm_list.ensure_selection();
            let page = self.state.borrow_mut().pending_page.take();
            if let Some((uuid, tab)) = page
                && let Some(vm) = self.vm_by_uuid(&uuid)
            {
                self.open_vm_page(&vm, tab);
            }
        }
        // (Taken first: the state can't stay borrowed while switching.)
        let pending_view = if tick == 1 { self.state.borrow_mut().pending_view.take() } else { None };
        if let Some(view) = pending_view {
            self.set_view(view);
        }
        #[cfg(debug_assertions)]
        if tick == 1 {
            self.dev_hooks();
        }
        let selected = self.vm_list.selected();
        self.show_selected(selected.as_deref());
        self.update_consoles();
        if let Some(uuid) = &selected
            && tick % PREVIEW_POLL_EVERY == 0
        {
            self.fetch_preview(uuid.clone());
        }
        if tick % GUEST_POLL_EVERY == 0
            && let Some(uuid) = selected
        {
            self.fetch_guest(uuid);
        }
        self.update_chrome();
    }

    /// Captures the selected machine's screen for the details preview, when
    /// that preview is actually on screen.
    pub(super) fn fetch_preview(self: &Rc<Self>, uuid: String) {
        let showing = self.window.is_visible()
            && self.state.borrow().view == View::Machines
            && self.nav.visible_page().and_then(|p| p.tag()).is_some_and(|t| t == "main")
            && !(self.split.is_narrow() && !self.split.showing_detail());
        let running = self
            .state
            .borrow()
            .domains
            .iter()
            .any(|d| d.uuid == uuid && d.state == vmng_virt::DomainState::Running && !d.config.graphics.is_empty());
        if !showing || !running || self.state.borrow().preview_busy {
            return;
        }
        let Some(conn) = self.current_conn() else { return };
        self.state.borrow_mut().preview_busy = true;
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let id = uuid.clone();
            let result = conn.call(move |c| c.screenshot(&id)).await;
            let Some(this) = weak.upgrade() else { return };
            this.state.borrow_mut().preview_busy = false;
            // No picture (e.g. no display yet) keeps the "Running" placeholder.
            if let Some(texture) = result.ok().and_then(screenshot_texture) {
                this.details.set_screen(&uuid, &texture);
            }
        });
    }

    pub(super) fn show_selected(&self, uuid: Option<&str>) {
        let st = self.state.borrow();
        let vm = uuid.and_then(|u| st.domains.iter().find(|d| d.uuid == u));
        self.actions.set_target(vm);
        match vm {
            Some(d) => {
                self.details.show(d, st.monitor.vm(&d.uuid), st.guest.get(&d.uuid));
                self.content.set_visible_child_name("details");
                if self.vm_page_open()
                    && let Some(conn) = self.current_conn_in(&st)
                {
                    self.vm_page.show(conn, d, st.host_info.clone());
                    self.vm_page.performance.update(d, st.monitor.vm(&d.uuid), self.settings.get().stats_interval_secs);
                }
            }
            None => {
                self.content.set_visible_child_name("none");
                if self.vm_page_open() {
                    // The VM went away (deleted elsewhere).
                    self.nav.pop_to_tag("main");
                }
            }
        }
    }

    /// Opens `vm`'s console window, or brings it forward if it is open.
    pub(super) fn open_console(self: &Rc<Self>, vm: &DomainSummary) {
        let Some(conn) = self.current_conn() else { return };
        let Some(app) = self.window.application().and_downcast::<adw::Application>() else { return };
        let existing = self.consoles.borrow().get(&vm.uuid).cloned();
        let console = existing.unwrap_or_else(|| {
            let weak = Rc::downgrade(self);
            let console = ConsoleWindow::new(&app, self.settings.clone(), move |uuid, action| {
                if let Some(this) = weak.upgrade() {
                    this.vm_action(uuid.to_owned(), action);
                }
            });
            let (weak, uuid) = (Rc::downgrade(self), vm.uuid.clone());
            console.window.connect_close_request(move |_| {
                if let Some(this) = weak.upgrade()
                    && let Some(c) = this.consoles.borrow_mut().remove(&uuid)
                {
                    c.leave();
                }
                glib::Propagation::Proceed
            });
            self.consoles.borrow_mut().insert(vm.uuid.clone(), console.clone());
            #[cfg(debug_assertions)]
            crate::dev::maybe_screenshot(&console.window);
            console
        });
        console.open(conn, vm);
        console.window.present();
    }

    /// Keeps the open console windows in step with their machines; closes
    /// those whose machine is gone.
    pub(super) fn update_consoles(&self) {
        let open: Vec<(String, Rc<ConsoleWindow>)> =
            self.consoles.borrow().iter().map(|(u, c)| (u.clone(), c.clone())).collect();
        for (uuid, console) in open {
            match self.vm_by_uuid(&uuid) {
                Some(vm) => console.update(&vm),
                None if console.is_on(&self.state.borrow().current) => console.window.close(),
                None => {}
            }
        }
    }

    pub(super) fn vm_page_open(&self) -> bool {
        self.nav.visible_page().and_then(|p| p.tag()).is_some_and(|t| t == VmPage::TAG)
    }

    /// Asks the guest (agent) for its hostname and addresses.
    pub(super) fn fetch_guest(self: &Rc<Self>, uuid: String) {
        let active = self.state.borrow().domains.iter().any(|d| d.uuid == uuid && d.state.is_active());
        let Some(conn) = self.current_conn().filter(|_| active) else { return };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let id = uuid.clone();
            let result = conn.call(move |c| c.guest_details(&id)).await;
            let Some(this) = weak.upgrade() else { return };
            if let Ok(details) = result {
                this.state.borrow_mut().guest.insert(uuid.clone(), details);
                if this.vm_list.selected().as_deref() == Some(uuid.as_str()) {
                    this.show_selected(Some(&uuid));
                }
            }
        });
    }

    pub(super) fn domain_name(&self, uuid: &str) -> String {
        self.state
            .borrow()
            .domains
            .iter()
            .find(|d| d.uuid == uuid)
            .map_or_else(|| "the machine".to_owned(), |d| d.display_name().to_owned())
    }

    /// A network installer stops its machine when it's done (the first
    /// boot ran the installer's kernel); start it again from its disk.
    pub(super) fn restart_finished_installs(self: &Rc<Self>, domains: &[DomainSummary]) {
        let finished: Vec<String> = {
            let mut st = self.state.borrow_mut();
            // A machine not listed yet (a poll from before it was made)
            // is kept waiting.
            let done: Vec<String> = st
                .installing
                .iter()
                .filter(|u| domains.iter().any(|d| &d.uuid == *u && !d.state.is_active()))
                .cloned()
                .collect();
            for u in &done {
                st.installing.remove(u);
            }
            done
        };
        for uuid in finished {
            let name = self.domain_name(&uuid);
            self.feedback.toast(&format!("The installer of {name} finished; starting it from its disk"));
            self.power(uuid, PowerAction::Start);
        }
    }
}
