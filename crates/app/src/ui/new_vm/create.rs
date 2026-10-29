//! The Review step and creating the machine (including downloading a network
//! installer).

use super::defaults::family_of;
use super::*;

impl Wizard {
    pub(super) fn validate_name(&self) -> bool {
        let name = self.name.text().trim().to_owned();
        let error = validate_vm_name(&name).err().or_else(|| {
            self.ctx.domains.iter().any(|d| d.config.name == name).then_some("Another machine already has this name.")
        });
        self.name_error.set_visible(error.is_some());
        self.name_error.set_label(error.unwrap_or_default());
        if error.is_some() {
            self.name.add_css_class("error");
        } else {
            self.name.remove_css_class("error");
        }
        let last = self.state.borrow().step == STEPS.len() - 1;
        if last {
            self.next.set_sensitive(error.is_none());
        }
        error.is_none()
    }

    pub(super) fn fill_summary(&self) {
        while let Some(child) = self.summary.first_child() {
            self.summary.remove(&child);
        }
        let st = self.state.borrow();
        let add = |title: &str, value: String| {
            let (row, label) = fact_row(title);
            label.set_label(&value);
            self.summary.append(&row);
        };
        add("Operating system", st.os.as_ref().map_or_else(|| "Unknown".to_owned(), |o| o.name.clone()));
        add(
            "Install from",
            match st.method {
                Some(Method::Media) => {
                    self.media_path().map(|p| p.rsplit('/').next().unwrap_or(&p).to_owned()).unwrap_or_default()
                }
                Some(Method::Import) => "Existing disk".into(),
                Some(Method::Network) => self.tree_url.text().trim().to_owned(),
                _ if self.pxe.is_active() => "The network (PXE)".into(),
                _ => "Nothing (manual)".into(),
            },
        );
        add("Memory", format::bytes((self.memory.value() * GIB as f64) as u64));
        add("Virtual CPUs", format!("{}", self.cpus.value() as u32));
        let disk = match st.method {
            Some(Method::Import) => st.import_path.clone().unwrap_or_default(),
            _ if self.disk_mode() == DiskMode::Existing => st.existing_path.clone().unwrap_or_default(),
            _ if self.disk_mode() == DiskMode::Create => {
                let pool = st.pools.get(self.pool_row.selected() as usize).map_or("", |p| p.name.as_str());
                format!(
                    "{} {} in {pool}",
                    format::bytes(self.disk_size.value() as u64 * GIB),
                    self.ctx.prefs.new_vm_disk_format
                )
            }
            _ => "None".into(),
        };
        add("Disk", disk);
        let mut fw = match self.firmware_choice() {
            FirmwareChoice::Bios => "BIOS".to_owned(),
            FirmwareChoice::Uefi { .. } => "UEFI".to_owned(),
            FirmwareChoice::Loader(path) => format!("UEFI · {}", path.rsplit('/').next().unwrap_or(&path)),
        };
        if self.secure_boot.is_active() {
            fw.push_str(" · Secure Boot");
        }
        if self.tpm.is_active() {
            fw.push_str(" · TPM");
        }
        add("Firmware", fw);
    }

    /// Turns the choices into a request.
    pub(super) fn spec(&self) -> NewVm {
        let st = self.state.borrow();
        let os = st.os.as_ref();
        let family = family_of(os);
        let windows = family == GuestFamily::Windows;
        let graphics = {
            let pref = self.ctx.prefs.new_vm_graphics.clone();
            let supported = st.caps.as_ref().is_none_or(|c| c.graphics_types.contains(&pref));
            if supported { pref } else { "vnc".into() }
        };
        let has_video = |m: &str| st.caps.as_ref().is_none_or(|c| c.video_models.iter().any(|v| v == m));
        let video = if windows {
            if graphics == "spice" && has_video("qxl") { "qxl" } else { "vga" }
        } else if has_video("virtio") {
            "virtio"
        } else {
            "vga"
        };
        let install = match st.method {
            Some(Method::Media) => InstallSource::Media(self.media_path().unwrap_or_default()),
            Some(Method::Import) => InstallSource::ImportDisk(st.import_path.clone().unwrap_or_default()),
            // Filled in once the installer is downloaded (see `create`).
            Some(Method::Network) => InstallSource::Network(NetworkInstall {
                kernel: String::new(),
                initrd: String::new(),
                args: String::new(),
            }),
            _ if self.pxe.is_active() => InstallSource::NetworkBoot,
            _ => InstallSource::Manual,
        };
        let disk = (st.method != Some(Method::Import) && self.disk_mode() == DiskMode::Create)
            .then(|| st.pools.get(self.pool_row.selected() as usize))
            .flatten()
            .map(|p| NewDisk {
                pool: p.name.clone(),
                size: self.disk_size.value() as u64 * GIB,
                format: self.ctx.prefs.new_vm_disk_format.clone(),
            });
        NewVm {
            name: self.name.text().trim().to_owned(),
            os_id: os.map(|o| o.id.clone()),
            family,
            arch: self.selected_arch(),
            machine: self.selected_machine(),
            virt_type: self.selected_virt(),
            memory: ((self.memory.value() * GIB as f64) as u64 / MIB) * MIB,
            vcpus: self.cpus.value() as u32,
            firmware: self.firmware_choice(),
            tpm: self.tpm.is_active(),
            cpu_model: self.ctx.prefs.new_vm_cpu_model.clone(),
            install,
            disk,
            existing_disk: (st.method != Some(Method::Import) && self.disk_mode() == DiskMode::Existing)
                .then(|| st.existing_path.clone())
                .flatten(),
            network: st
                .networks
                .get(self.network_row.selected() as usize)
                .map_or(NetworkChoice::None, |(_, c)| c.clone()),
            graphics,
            virtio: !windows,
            video: video.to_owned(),
        }
    }

    pub(super) fn set_busy(&self, busy: bool) {
        self.next_spinner.set_visible(busy);
        self.next_label.set_label(if busy { "Creating…" } else { "Create" });
        self.next.set_sensitive(!busy);
        self.back.set_sensitive(!busy);
        self.stack.set_sensitive(!busy);
        self.dialog.set_can_close(!busy);
    }

    pub(super) fn create(self: &Rc<Self>) {
        let mut spec = self.spec();
        let customize = self.customize.is_active();
        let network = matches!(spec.install, InstallSource::Network(_));
        let tree = self.state.borrow().tree.clone().filter(|_| network);
        if network && tree.is_none() {
            return;
        }
        let extra_args = self.kernel_args.text().trim().to_owned();
        self.set_busy(true);
        let conn = self.ctx.conn.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            if let Some(tree) = tree {
                let Some(w) = weak.upgrade() else { return };
                match w.download_installer(&tree).await {
                    Ok((kernel, initrd)) => {
                        let args = [tree.args.as_str(), extra_args.as_str()]
                            .iter()
                            .filter(|a| !a.is_empty())
                            .copied()
                            .collect::<Vec<_>>()
                            .join(" ");
                        spec.install = InstallSource::Network(NetworkInstall { kernel, initrd, args });
                    }
                    Err(e) => {
                        w.set_busy(false);
                        w.ctx.feedback.error("Couldn't download the installer", &e);
                        return;
                    }
                }
            }
            let s = spec.clone();
            let result = conn.call(move |c| c.create_vm(&s, !customize)).await;
            let Some(w) = weak.upgrade() else { return };
            w.set_busy(false);
            match result {
                Ok(created) => {
                    match &created.start_error {
                        Some(e) => {
                            w.ctx.feedback.error(&format!("Created {}, but couldn't start it", spec.name), e.message())
                        }
                        None if customize => {
                            w.ctx.feedback.toast(&format!("Created {}. Start it when you're ready.", spec.name))
                        }
                        None => w.ctx.feedback.toast(&format!("Created and started {}", spec.name)),
                    }
                    w.dialog.close();
                    (w.on_created)(created, customize);
                }
                Err(e) => {
                    if let InstallSource::Network(n) = &spec.install {
                        let (k, i) = (n.kernel.clone(), n.initrd.clone());
                        let _ = conn
                            .call(move |c| {
                                c.remove_boot_file(&k);
                                c.remove_boot_file(&i);
                                Ok(())
                            })
                            .await;
                    }
                    w.ctx.feedback.error(&format!("Couldn't create {}", spec.name), e.message())
                }
            }
        });
    }

    /// Downloads the network installer's kernel and initrd and stores them
    /// where QEMU can read them. Shows the progress on the Create button.
    pub(super) async fn download_installer(self: &Rc<Self>, tree: &TreeInstaller) -> Result<(String, String), String> {
        let done = Arc::new(AtomicU64::new(0));
        let total = Arc::new(AtomicU64::new(0));
        let label = self.next_label.clone();
        let (d, t) = (done.clone(), total.clone());
        let ticker = glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
            let (d, t) = (d.load(Ordering::Relaxed), t.load(Ordering::Relaxed));
            label.set_label(&match (d * 100).checked_div(t) {
                Some(percent) => format!("Downloading… {percent}%"),
                None => format!("Downloading… {}", format::bytes(d)),
            });
            glib::ControlFlow::Continue
        });
        let mut stored = Vec::new();
        let mut result = Ok(());
        for (i, url) in [&tree.kernel_url, &tree.initrd_url].into_iter().enumerate() {
            let (url, d, t) = (url.clone(), done.clone(), total.clone());
            d.store(0, Ordering::Relaxed);
            t.store(0, Ordering::Relaxed);
            let data = gio::spawn_blocking(move || {
                net_install::fetch(&url, net_install::MAX_FILE, &|got, all| {
                    d.store(got, Ordering::Relaxed);
                    t.store(all.unwrap_or(0), Ordering::Relaxed);
                })
            })
            .await
            .unwrap_or_else(|_| Err("The download stopped unexpectedly.".into()));
            let data = match data {
                Ok(d) => d,
                Err(e) => {
                    result = Err(e);
                    break;
                }
            };
            self.next_label.set_label("Storing the installer…");
            let name = format!("{}-{}", self.name.text().trim(), if i == 0 { "kernel" } else { "initrd" });
            match self.ctx.conn.call(move |c| c.store_boot_file(&name, &data)).await {
                Ok(path) => stored.push(path),
                Err(e) => {
                    result = Err(e.message().to_owned());
                    break;
                }
            }
        }
        ticker.remove();
        self.next_label.set_label("Creating…");
        match (result, stored.as_slice()) {
            (Ok(()), [kernel, initrd]) => Ok((kernel.clone(), initrd.clone())),
            (result, _) => {
                let paths = stored.clone();
                let _ = self
                    .ctx
                    .conn
                    .call(move |c| {
                        for p in &paths {
                            c.remove_boot_file(p);
                        }
                        Ok(())
                    })
                    .await;
                Err(result.err().unwrap_or_else(|| "The installer files are incomplete.".into()))
            }
        }
    }
}
