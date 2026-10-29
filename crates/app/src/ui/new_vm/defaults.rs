//! What the host offers (architectures, machine types, firmware) and the
//! defaults that follow from it and from the chosen OS.

use super::*;

pub(super) fn machine_label(m: &str) -> String {
    match m {
        "q35" => "Q35 (modern, recommended)".into(),
        "pc" => "i440FX (for very old systems)".into(),
        "virt" => "virt (recommended)".into(),
        other => other.into(),
    }
}

pub(super) fn family_of(os: Option<&OsInfo>) -> GuestFamily {
    match os.and_then(|o| o.family.as_deref()) {
        Some(f) if f.starts_with("win") => GuestFamily::Windows,
        Some("linux") => GuestFamily::Linux,
        Some(_) => GuestFamily::Other,
        None => GuestFamily::Linux,
    }
}

/// Windows 11 and Server 2022+ need a TPM; they also expect Secure Boot.
pub(super) fn needs_tpm(os: Option<&OsInfo>) -> bool {
    os.is_some_and(|o| ["/win/11", "/win/2k22", "/win/2k25"].iter().any(|v| o.id.contains(v)))
}

/// Distributions whose installers boot with Secure Boot enabled.
pub(super) fn secure_boot_ok(os: Option<&OsInfo>) -> bool {
    const SIGNED: [&str; 11] =
        ["fedora", "rhel", "centos", "almalinux", "rocky", "ubuntu", "debian", "opensuse", "sles", "win", "ol"];
    os.and_then(|o| o.distro.as_deref()).is_some_and(|d| SIGNED.contains(&d))
}

pub(super) fn default_name(os: Option<&OsInfo>, domains: &[DomainSummary]) -> String {
    let base = os
        .map(|o| o.short_id.trim_end_matches("-unknown").trim_end_matches("unknown").to_owned())
        .filter(|s| !s.is_empty() && validate_vm_name(s).is_ok())
        .unwrap_or_else(|| "vm".into());
    unique_name(&base, |n| domains.iter().any(|d| d.config.name == n))
}

impl Wizard {
    /// Loads everything the wizard offers from the host.
    pub(super) fn load(self: &Rc<Self>) {
        self.stack.set_visible_child_name("loading");
        self.update_buttons();
        let conn = self.ctx.conn.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn
                .call(|c| {
                    Ok(HostData {
                        caps: c.host_caps()?,
                        images: c.list_install_images().unwrap_or_default(),
                        // Not the network installers' temporary pool.
                        pools: c
                            .list_pools()?
                            .into_iter()
                            .map(|(p, _)| p)
                            .filter(|p| p.active && p.name != vmng_virt::BOOT_POOL)
                            .collect(),
                        networks: c.list_networks().unwrap_or_default(),
                        bridges: c.list_bridges(),
                    })
                })
                .await;
            let Some(w) = weak.upgrade() else { return };
            match result {
                Ok(data) => w.apply_host(data),
                Err(e) => {
                    w.ctx.feedback.error("Couldn't read the host's capabilities", e.message());
                    w.dialog.close();
                }
            }
        });
    }

    pub(super) fn apply_host(self: &Rc<Self>, data: HostData) {
        self.updating.set(true);
        let mut st = self.state.borrow_mut();

        // Installer images: pools first, then an optical drive if present.
        st.media =
            data.images.iter().map(|i| (format!("{} · {}", i.name, format::bytes(i.size)), i.path.clone())).collect();
        // This computer's optical drive, unless the machines run elsewhere.
        if !self.ctx.conn.kind().is_remote() && std::path::Path::new("/dev/sr0").exists() {
            st.media.push(("Optical drive (/dev/sr0)".into(), "/dev/sr0".into()));
        }
        let labels: Vec<String> = st.media.iter().map(|(l, _)| l.clone()).collect();
        self.media_row.set_model(Some(&string_list(&labels)));
        // Debug builds: `VMNG_DEV_WIZARD_MEDIA=<text>` picks the first image
        // whose name contains it.
        #[cfg(debug_assertions)]
        if let Ok(want) = std::env::var("VMNG_DEV_WIZARD_MEDIA")
            && let Some(i) = st.media.iter().position(|(l, _)| l.contains(&want))
        {
            self.media_row.set_selected(i as u32);
        }

        // Architectures: the host's own first.
        let host_arch = if data.caps.host_arch.is_empty() { "x86_64".to_owned() } else { data.caps.host_arch.clone() };
        let mut arches: Vec<String> = data.caps.guests.iter().map(|g| g.arch.clone()).collect();
        arches.sort_by_key(|a| (a != &host_arch, a.clone()));
        self.arch_row.set_model(Some(&string_list(&arches)));
        st.arches = arches;

        // Pools, default pool first.
        let mut pools = data.pools;
        pools.sort_by_key(|p| (p.name != "default", p.name.clone()));
        let pool_labels: Vec<String> =
            pools.iter().map(|p| format!("{} · {} free", p.name, format::bytes(p.available))).collect();
        self.pool_row.set_model(Some(&string_list(&pool_labels)));
        st.pools = pools;

        // Networks: the one most existing VMs use comes first.
        let mut nets: Vec<(String, NetworkChoice)> = Vec::new();
        if self.ctx.session {
            nets.push(("User networking (no setup needed)".into(), NetworkChoice::User));
        }
        for n in data.networks.iter().filter(|n| n.active) {
            nets.push((
                format!("Virtual network “{}” · {}", n.name, n.forward.to_uppercase()),
                NetworkChoice::Network(n.name.clone()),
            ));
        }
        for b in &data.bridges {
            nets.push((format!("Bridge {b} · on your local network"), NetworkChoice::Bridge(b.clone())));
        }
        nets.push(("No network".into(), NetworkChoice::None));
        let usage = |choice: &NetworkChoice| {
            self.ctx
                .domains
                .iter()
                .flat_map(|d| &d.config.interfaces)
                .filter(|i| match choice {
                    NetworkChoice::Network(n) => i.kind == "network" && i.source.as_deref() == Some(n),
                    NetworkChoice::Bridge(b) => i.kind == "bridge" && i.source.as_deref() == Some(b),
                    _ => false,
                })
                .count()
        };
        let best = nets.iter().enumerate().max_by_key(|(i, (_, c))| (usage(c), usize::MAX - i)).map_or(0, |(i, _)| i);
        let labels: Vec<String> = nets.iter().map(|(l, _)| l.clone()).collect();
        self.network_row.set_model(Some(&string_list(&labels)));
        self.network_row.set_selected(best as u32);
        st.networks = nets;
        drop(st);

        if let Some(host) = &self.ctx.host {
            self.memory.set_range(0.5, (host.memory / GIB).max(1) as f64);
            self.cpus.set_range(1.0, f64::from(host.cpus.max(1)));
        }
        self.fill_machines_with(&data.caps);
        {
            let mut st = self.state.borrow_mut();
            st.caps = None;
            st.host_caps = Some(data.caps);
        }
        self.updating.set(false);

        let method = if self.ctx.import {
            Method::Import
        } else if self.state.borrow().media.is_empty() {
            Method::Manual
        } else {
            Method::Media
        };
        if let Some((_, b)) = self.method_buttons.iter().find(|(m, _)| *m == method) {
            b.set_active(true);
        }
        self.set_method(method);
        self.load_domain_caps();
        self.go(0);
        // Debug builds: `VMNG_DEV_WIZARD_URL=<address>` reads a network
        // install tree; `VMNG_DEV_WIZARD_PXE=1` picks Manual with PXE.
        #[cfg(debug_assertions)]
        {
            let pick = |m: Method| {
                if let Some((_, b)) = self.method_buttons.iter().find(|(x, _)| *x == m) {
                    b.set_active(true);
                }
            };
            if let Ok(url) = std::env::var("VMNG_DEV_WIZARD_URL") {
                pick(Method::Network);
                self.tree_url.set_text(&url);
                self.detect_tree();
            } else if std::env::var_os("VMNG_DEV_WIZARD_PXE").is_some() {
                pick(Method::Manual);
                self.pxe.set_active(true);
            }
            // `VMNG_DEV_WIZARD_EXISTING=<image>` uses an existing disk.
            if let Ok(path) = std::env::var("VMNG_DEV_WIZARD_EXISTING") {
                self.disk_mode.set_selected(1);
                self.set_existing_disk(path);
            }
        }
        #[cfg(debug_assertions)]
        if let Some(step) = std::env::var("VMNG_DEV_WIZARD_STEP").ok().and_then(|s| s.parse::<usize>().ok()) {
            let weak = Rc::downgrade(self);
            glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
                if let Some(w) = weak.upgrade() {
                    w.go(step.min(STEPS.len() - 1));
                    if std::env::var_os("VMNG_DEV_WIZARD_CHOOSE").is_some() {
                        w.pick_os();
                    }
                    // VMNG_DEV_WIZARD_CREATE=<name>: create without network.
                    if let Ok(name) = std::env::var("VMNG_DEV_WIZARD_CREATE") {
                        w.name.set_text(&name);
                        let none = w.state.borrow().networks.len().saturating_sub(1);
                        w.network_row.set_selected(none as u32);
                        // A network install waits until its tree is read.
                        let weak = Rc::downgrade(&w);
                        glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
                            let Some(w) = weak.upgrade() else { return glib::ControlFlow::Break };
                            let st = w.state.borrow();
                            if st.method == Some(Method::Network) && (st.detecting || st.tree.is_none()) {
                                return glib::ControlFlow::Continue;
                            }
                            drop(st);
                            w.advance();
                            glib::ControlFlow::Break
                        });
                    }
                }
            });
        }
    }

    pub(super) fn fill_machines(self: &Rc<Self>) {
        let caps = self.state.borrow().host_caps.clone();
        if let Some(caps) = caps {
            self.fill_machines_with(&caps);
        }
    }

    pub(super) fn fill_machines_with(&self, caps: &HostCaps) {
        let was = self.updating.replace(true);
        let arch = self.selected_arch();
        let guest = caps.guest(&arch);
        let all = guest.map(|g| g.machines.clone()).unwrap_or_default();
        let mut machines: Vec<String> =
            ["q35", "pc", "virt"].iter().filter(|m| all.iter().any(|a| a == *m)).map(|m| (*m).to_owned()).collect();
        if machines.is_empty() {
            machines = all.into_iter().take(1).collect();
        }
        let labels: Vec<String> = machines.iter().map(|m| machine_label(m)).collect();
        self.machine_row.set_model(Some(&string_list(&labels)));
        self.machine_row.set_selected(0);
        let mut virt = Vec::new();
        if guest.is_some_and(|g| g.kvm) {
            virt.push("kvm".to_owned());
        }
        virt.push("qemu".to_owned());
        let labels: Vec<String> =
            virt.iter()
                .map(|v| {
                    if v == "kvm" {
                        "KVM (hardware accelerated)".to_owned()
                    } else {
                        "QEMU (emulated, slow)".to_owned()
                    }
                })
                .collect();
        self.virt_row.set_model(Some(&string_list(&labels)));
        self.virt_row.set_selected(0);
        let mut st = self.state.borrow_mut();
        st.machines = machines;
        st.virt_types = virt;
        drop(st);
        self.updating.set(was);
    }

    pub(super) fn selected_arch(&self) -> String {
        let st = self.state.borrow();
        st.arches.get(self.arch_row.selected() as usize).cloned().unwrap_or_else(|| "x86_64".into())
    }

    pub(super) fn selected_machine(&self) -> Option<String> {
        self.state.borrow().machines.get(self.machine_row.selected() as usize).cloned()
    }

    pub(super) fn selected_virt(&self) -> String {
        self.state.borrow().virt_types.get(self.virt_row.selected() as usize).cloned().unwrap_or_else(|| "qemu".into())
    }

    /// Fetches firmware/TPM support for the chosen arch and machine.
    pub(super) fn load_domain_caps(self: &Rc<Self>) {
        let (arch, machine, virt) = (self.selected_arch(), self.selected_machine(), self.selected_virt());
        let conn = self.ctx.conn.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = conn.call(move |c| c.domain_caps(&arch, machine.as_deref(), &virt)).await;
            let Some(w) = weak.upgrade() else { return };
            w.state.borrow_mut().caps = result.ok();
            w.apply_os_defaults();
        });
    }

    /// Applies the chosen OS's recommendations to values the person has
    /// not changed yet.
    pub(super) fn apply_os_defaults(&self) {
        let st = self.state.borrow();
        let os_owned = st.os.clone();
        let os = os_owned.as_ref();
        let arch = self.selected_arch();
        let res = os.map(|o| o.resources_for(&arch)).unwrap_or_default();
        let windows = family_of(os) == GuestFamily::Windows;
        let caps = st.caps.clone();
        let (resources_edited, disk_edited, name_edited) = (st.resources_edited, st.disk_edited, st.name_edited);
        drop(st);

        self.updating.set(true);
        if !resources_edited {
            let host_mem = self.ctx.host.as_ref().map_or(u64::MAX, |h| h.memory);
            let ram = res
                .ram
                .unwrap_or(if windows { 4 * GIB } else { 2 * GIB })
                .clamp(512 * MIB, host_mem.saturating_sub(GIB).max(512 * MIB));
            self.memory.set_value(ram as f64 / GIB as f64);
            let host_cpus = self.ctx.host.as_ref().map_or(2, |h| h.cpus);
            self.cpus.set_value(f64::from(res.cpus.unwrap_or(2).max(2).min(host_cpus.max(1))));
        }
        if !disk_edited {
            let size = res.storage.unwrap_or(if windows { 64 * GIB } else { 20 * GIB }).max(GIB);
            self.disk_size.set_value((size / GIB) as f64);
        }
        let uefi_ok = caps.as_ref().is_none_or(|c| c.uefi);
        let want_uefi = uefi_ok && (self.ctx.prefs.new_vm_firmware == "uefi" || needs_tpm(os));
        // UEFI (libvirt picks the file), BIOS, then each firmware file.
        let arch = self.selected_arch();
        let mut labels = vec!["UEFI".to_owned(), "BIOS".to_owned()];
        labels.extend(caps.iter().flat_map(|c| &c.loaders).map(|p| FirmwareChoice::Loader(p.clone()).label(&arch)));
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        self.firmware.set_model(Some(&gtk::StringList::new(&labels)));
        self.firmware.set_selected(if want_uefi { 0 } else { 1 });
        self.firmware.set_sensitive(uefi_ok);
        self.secure_boot.set_active(secure_boot_ok(os));
        self.tpm.set_active(needs_tpm(os));
        self.tpm.set_sensitive(caps.as_ref().is_none_or(|c| c.tpm));
        if !name_edited {
            self.name.set_text(&default_name(os, &self.ctx.domains));
        }
        // Tell the person what the OS asks for and what the host has.
        let who = os.map_or_else(|| "Recommended".to_owned(), |o| format!("Recommended for {}", o.name));
        let host = self.ctx.host.as_ref();
        let mem_hint = match (res.ram, host) {
            (Some(r), Some(h)) => format!("{who}: {} · host has {}", format::bytes(r), format::bytes(h.memory)),
            (Some(r), None) => format!("{who}: {}", format::bytes(r)),
            (None, Some(h)) => format!("Host has {}", format::bytes(h.memory)),
            (None, None) => String::new(),
        };
        self.memory.set_subtitle(&mem_hint);
        let cpu_hint = match (res.cpus, host) {
            (Some(c), Some(h)) => format!("{who}: {c} · host has {}", h.cpus),
            (_, Some(h)) => format!("Host has {} logical CPUs", h.cpus),
            (Some(c), None) => format!("{who}: {c}"),
            (None, None) => String::new(),
        };
        self.cpus.set_subtitle(&cpu_hint);
        self.disk_size.set_subtitle(&res.storage.map(|s| format!("{who}: {}", format::bytes(s))).unwrap_or_default());
        self.updating.set(false);
        self.update_firmware_rows();
    }

    /// The firmware chosen in the dropdown (see `apply_os_defaults`).
    pub(super) fn firmware_choice(&self) -> FirmwareChoice {
        let choice = self.firmware_choice_raw();
        // See `update_firmware_rows`: no Secure Boot for a network install.
        if self.state.borrow().method == Some(Method::Network) && choice.secure_boot() {
            return FirmwareChoice::Uefi { secure_boot: false };
        }
        choice
    }

    pub(super) fn firmware_choice_raw(&self) -> FirmwareChoice {
        match self.firmware.selected() {
            0 => FirmwareChoice::Uefi { secure_boot: self.secure_boot.is_active() },
            1 => FirmwareChoice::Bios,
            n => self
                .state
                .borrow()
                .caps
                .as_ref()
                .and_then(|c| c.loaders.get(n as usize - 2).cloned())
                .map_or(FirmwareChoice::Bios, FirmwareChoice::Loader),
        }
    }

    pub(super) fn update_firmware_rows(&self) {
        let choice = self.firmware.selected();
        let secure_ok = self.state.borrow().caps.as_ref().is_none_or(|c| c.secure_boot);
        // Secure Boot refuses a network installer's kernel (it isn't
        // started through a signed boot loader).
        let network = self.state.borrow().method == Some(Method::Network);
        // Secure Boot is a choice for automatic UEFI; a firmware file
        // either is a Secure Boot build or isn't.
        self.secure_boot.set_sensitive(choice == 0 && secure_ok && !network);
        self.secure_boot.set_subtitle(if network {
            "Not available for a network install: it would refuse the installer"
        } else {
            "Only lets signed operating systems start"
        });
        if network {
            self.secure_boot.set_active(false);
        }
        match self.firmware_choice() {
            FirmwareChoice::Bios => self.secure_boot.set_active(false),
            FirmwareChoice::Loader(path) => self.secure_boot.set_active(vmng_virt::loader_is_secure(&path)),
            FirmwareChoice::Uefi { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(id: &str, short: &str, family: &str, distro: &str) -> OsInfo {
        OsInfo {
            id: id.into(),
            short_id: short.into(),
            name: short.into(),
            version: None,
            vendor: None,
            family: Some(family.into()),
            distro: Some(distro.into()),
            release_date: None,
            eol_date: None,
            minimum: Vec::new(),
            recommended: Vec::new(),
        }
    }

    #[test]
    fn defaults_follow_the_os() {
        let win11 = os("http://microsoft.com/win/11", "win11", "winnt", "win");
        let fedora = os("http://fedoraproject.org/fedora/unknown", "fedora-unknown", "linux", "fedora");
        let arch = os("http://archlinux.org/archlinux/rolling", "archlinux", "linux", "archlinux");
        assert_eq!(family_of(Some(&win11)), GuestFamily::Windows);
        assert!(needs_tpm(Some(&win11)) && !needs_tpm(Some(&fedora)));
        assert!(secure_boot_ok(Some(&fedora)) && !secure_boot_ok(Some(&arch)));
        assert_eq!(default_name(Some(&fedora), &[]), "fedora");
        assert_eq!(default_name(None, &[]), "vm");
    }
}
