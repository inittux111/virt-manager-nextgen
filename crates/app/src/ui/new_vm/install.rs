//! Step 1, Install: the install method, installer media or network tree, OS
//! detection and the OS chooser, and file browsing.

use super::*;

impl Wizard {
    pub(super) fn set_method(self: &Rc<Self>, method: Method) {
        self.state.borrow_mut().method = Some(method);
        self.media_group.set_visible(method == Method::Media);
        self.import_group.set_visible(method == Method::Import);
        self.network_group.set_visible(method == Method::Network);
        self.manual_group.set_visible(method == Method::Manual);
        self.update_firmware_rows();
        // The network installer only runs when the machine starts now.
        self.customize.set_sensitive(method != Method::Network);
        if method == Method::Network {
            self.customize.set_active(false);
        }
        match method {
            Method::Media => self.detect(),
            Method::Network if self.state.borrow().tree.is_some() => {}
            Method::Network => {
                {
                    let mut st = self.state.borrow_mut();
                    if st.os_detected {
                        st.os = None;
                        st.os_detected = false;
                    }
                    st.detecting = false;
                }
                self.show_os();
            }
            _ => {
                let mut st = self.state.borrow_mut();
                if st.os_detected {
                    st.os = None;
                    st.os_detected = false;
                }
                st.detecting = false;
                drop(st);
                self.show_os();
            }
        }
        self.update_buttons();
    }

    pub(super) fn media_path(&self) -> Option<String> {
        let st = self.state.borrow();
        st.media.get(self.media_row.selected() as usize).map(|(_, p)| p.clone())
    }

    /// Detects the OS on the selected installer image.
    pub(super) fn detect(self: &Rc<Self>) {
        let Some(path) = self.media_path() else {
            self.show_os();
            return;
        };
        let seq = {
            let mut st = self.state.borrow_mut();
            st.detect_seq += 1;
            st.detecting = true;
            if st.os_detected {
                st.os = None;
            }
            st.detect_seq
        };
        self.show_os();
        self.update_buttons();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = osdb::identify_media(path).await;
            let Some(w) = weak.upgrade() else { return };
            let mut st = w.state.borrow_mut();
            if st.detect_seq != seq {
                return;
            }
            st.detecting = false;
            if let Ok(Some(os)) = result {
                st.os = Some(os);
                st.os_detected = true;
            } else if st.os_detected {
                st.os = None;
                st.os_detected = false;
            }
            drop(st);
            w.show_os();
            w.apply_os_defaults();
            w.update_buttons();
        });
    }

    /// Reads the network install tree at the typed address.
    pub(super) fn detect_tree(self: &Rc<Self>) {
        let url = self.tree_url.text().trim().to_owned();
        if let Err(msg) = net_install::validate_url(&url) {
            self.state.borrow_mut().tree_error = Some(msg.to_owned());
            self.show_os();
            self.update_buttons();
            return;
        }
        let seq = {
            let mut st = self.state.borrow_mut();
            st.detect_seq += 1;
            st.detecting = true;
            st.tree = None;
            st.tree_error = None;
            st.detect_seq
        };
        self.show_os();
        self.update_buttons();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = match osdb::identify_tree(url.clone()).await {
                Ok(tree) => net_install::from_treeinfo(&url, &tree)
                    .ok_or_else(|| "The install tree has no installer kernel for this architecture.".to_owned()),
                // No .treeinfo: maybe a Debian mirror.
                Err(e) => {
                    net_install::debian(&url).ok_or_else(|| format!("This doesn't look like an install tree ({e})."))
                }
            };
            let Some(w) = weak.upgrade() else { return };
            let mut st = w.state.borrow_mut();
            if st.detect_seq != seq {
                return;
            }
            st.detecting = false;
            match result {
                Ok(tree) => {
                    if let Some(os) = tree.os_id.as_deref().and_then(osdb::lookup) {
                        st.os = Some(os.clone());
                        st.os_detected = true;
                    } else if st.os_detected {
                        st.os = None;
                        st.os_detected = false;
                    }
                    st.tree = Some(tree);
                }
                Err(e) => st.tree_error = Some(e),
            }
            drop(st);
            w.show_os();
            w.apply_os_defaults();
            w.update_buttons();
        });
    }

    pub(super) fn show_os(&self) {
        let st = self.state.borrow();
        self.os_spinner.set_visible(st.detecting);
        self.os_button.set_sensitive(!st.detecting);
        if st.detecting {
            self.os_logo.set_logo(None);
            self.os_title.set_label("Detecting the operating system…");
            self.os_subtitle.set_label(if st.method == Some(Method::Network) {
                "Reading the install tree"
            } else {
                "Reading the install media"
            });
            self.os_button.set_label("Choose");
            return;
        }
        match &st.os {
            Some(os) => {
                self.os_logo.set_logo(osdb::logo_key(&os.id));
                self.os_title.set_label(&os.name);
                self.os_subtitle.set_label(if st.os_detected && st.method == Some(Method::Network) {
                    "Detected automatically from the install tree"
                } else if st.os_detected {
                    "Detected automatically from the install media"
                } else {
                    "Chosen by you"
                });
                self.os_button.set_label("Change");
            }
            None => {
                self.os_logo.set_logo(None);
                let (title, sub) = match st.method {
                    Some(Method::Media) if self.media_path().is_some() => {
                        ("Couldn't detect the operating system", "Choose it, so the machine gets the right settings")
                    }
                    Some(Method::Import) => {
                        ("Which operating system is on the disk?", "Choose it, so the machine gets the right settings")
                    }
                    Some(Method::Network) if st.tree_error.is_some() => {
                        ("Couldn't read the install tree", st.tree_error.as_deref().unwrap_or_default())
                    }
                    Some(Method::Network) if st.tree.is_some() => {
                        ("Couldn't detect the operating system", "Choose it, so the machine gets the right settings")
                    }
                    Some(Method::Network) => {
                        ("Enter the install tree's address", "Then press Enter or the check mark to read it")
                    }
                    _ => ("Operating system", "Choose it, so the machine gets the right settings"),
                };
                self.os_title.set_label(title);
                self.os_subtitle.set_label(sub);
                self.os_button.set_label("Choose…");
            }
        }
    }

    pub(super) fn pick_os(self: &Rc<Self>) {
        // Start the search from the file name, e.g. "EndeavourOS".
        let query = match self.state.borrow().method {
            Some(Method::Media) => self.media_path(),
            Some(Method::Import) => self.state.borrow().import_path.clone(),
            _ => None,
        }
        .and_then(|p| {
            let file = p.rsplit('/').next()?.to_owned();
            file.split(|c: char| !c.is_alphanumeric()).find(|s| s.len() > 2).map(str::to_owned)
        })
        .filter(|q| osdb::all().any(|o| o.name.to_lowercase().contains(&q.to_lowercase())))
        .unwrap_or_default();
        let weak = Rc::downgrade(self);
        choose_os(&self.dialog, &query, move |os| {
            if let Some(w) = weak.upgrade() {
                {
                    let mut st = w.state.borrow_mut();
                    st.os = Some(os.clone());
                    st.os_detected = false;
                }
                w.show_os();
                w.apply_os_defaults();
                w.update_buttons();
            }
        });
    }

    pub(super) fn disk_mode(&self) -> DiskMode {
        DISK_MODES.get(self.disk_mode.selected() as usize).map_or(DiskMode::Create, |(m, _)| *m)
    }

    pub(super) fn browse(self: &Rc<Self>, target: Browse) {
        let media = target == Browse::Media;
        let filter = gtk::FileFilter::new();
        if media {
            filter.set_name(Some("Disc images"));
            filter.add_suffix("iso");
            filter.add_mime_type("application/x-cd-image");
        } else {
            filter.set_name(Some("Disk images"));
            for s in ["qcow2", "img", "raw", "qcow"] {
                filter.add_suffix(s);
            }
        }
        let weak = Rc::downgrade(self);
        crate::ui::widgets::pick_host_path(
            self.dialog.upcast_ref(),
            self.ctx.conn.remote_name().as_deref(),
            false,
            if media { "Choose an Installer Image" } else { "Choose a Disk Image" },
            Some(filter),
            move |path| {
                let Some(w) = weak.upgrade() else { return };
                if media {
                    let index = {
                        let mut st = w.state.borrow_mut();
                        match st.media.iter().position(|(_, p)| *p == path) {
                            Some(i) => i,
                            None => {
                                let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
                                st.media.push((name, path.clone()));
                                st.media.len() - 1
                            }
                        }
                    };
                    let labels: Vec<String> = w.state.borrow().media.iter().map(|(l, _)| l.clone()).collect();
                    w.updating.set(true);
                    w.media_row.set_model(Some(&string_list(&labels)));
                    w.media_row.set_selected(index as u32);
                    w.updating.set(false);
                    w.detect();
                } else if target == Browse::ExistingDisk {
                    w.set_existing_disk(path);
                } else {
                    w.import_row.set_subtitle(&glib::markup_escape_text(&path));
                    w.state.borrow_mut().import_path = Some(path);
                    w.show_os();
                    w.update_buttons();
                }
            },
        );
    }

    pub(super) fn set_existing_disk(&self, path: String) {
        self.existing_row.set_subtitle(&glib::markup_escape_text(&path));
        self.state.borrow_mut().existing_path = Some(path);
        self.update_buttons();
    }
}
