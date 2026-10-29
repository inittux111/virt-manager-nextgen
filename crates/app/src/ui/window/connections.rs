//! Connections: connecting, switching, and the remote computers in the main menu.

use super::*;

/// Added to a failed SSH connection's message.
pub(super) const REMOTE_HINT: &str = "\n\nThe app logs in with your SSH key. Set it up with “ssh-copy-id user@computer”, and \
connect once with “ssh user@computer” in a terminal to accept the computer's key. The other computer needs \
libvirt running.";

/// The `win.connection` target of a connection.
pub(super) fn connection_id(kind: &ConnectionKind) -> String {
    match kind {
        ConnectionKind::System => "system".into(),
        ConnectionKind::Session => "session".into(),
        ConnectionKind::Other(uri) => uri.clone(),
    }
}

pub(super) fn connection_of(id: &str) -> ConnectionKind {
    match id {
        "system" => ConnectionKind::System,
        "session" => ConnectionKind::Session,
        uri => ConnectionKind::Other(uri.to_owned()),
    }
}

impl MainWindow {
    pub(super) fn status_of(&self, kind: &ConnectionKind) -> ConnStatus {
        match self.state.borrow().conns.get(kind) {
            Some(Slot::Connecting) => ConnStatus::Connecting,
            Some(Slot::Connected(_)) => ConnStatus::Connected,
            Some(Slot::Failed(_)) => ConnStatus::Failed,
            None => ConnStatus::Disconnected,
        }
    }

    pub(super) fn current_conn(&self) -> Option<Conn> {
        self.current_conn_in(&self.state.borrow())
    }

    pub(super) fn current_conn_in(&self, st: &State) -> Option<Conn> {
        match st.conns.get(&st.current) {
            Some(Slot::Connected(c)) => Some(c.clone()),
            _ => None,
        }
    }

    pub(super) fn connect_to(self: &Rc<Self>, kind: ConnectionKind) {
        if matches!(self.state.borrow().conns.get(&kind), Some(Slot::Connecting | Slot::Connected(_))) {
            return;
        }
        self.state.borrow_mut().conns.insert(kind.clone(), Slot::Connecting);
        self.update_chrome();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = Conn::open(kind.clone()).await;
            let Some(this) = weak.upgrade() else { return };
            let is_current = this.state.borrow().current == kind;
            match result {
                Ok(conn) => {
                    this.state.borrow_mut().conns.insert(kind.clone(), Slot::Connected(conn.clone()));
                    if is_current {
                        this.on_connected(conn).await;
                    }
                }
                Err(e) => {
                    let mut msg = e.message().to_owned();
                    if kind.is_remote() {
                        msg.push_str(REMOTE_HINT);
                    }
                    this.state.borrow_mut().conns.insert(kind, Slot::Failed(msg));
                }
            }
            this.update_chrome();
        });
    }

    /// The Connection section: this computer, then the other computers.
    pub(super) fn fill_connection_menu(&self) {
        let menu = &self.conn_menu;
        menu.remove_all();
        menu.append(Some("QEMU/KVM · System"), Some("win.connection::system"));
        menu.append(Some("QEMU/KVM · User Session"), Some("win.connection::session"));
        for uri in self.settings.get().remote_connections {
            let label = format!("QEMU/KVM · {}", ConnectionKind::Other(uri.clone()).label());
            let item = gio::MenuItem::new(Some(&label), None);
            item.set_action_and_target_value(Some("win.connection"), Some(&uri.to_variant()));
            menu.append_item(&item);
        }
        menu.append(Some("Add Remote Computer…"), Some("win.add-connection"));
        menu.append(Some("Remove This Connection"), Some("win.remove-connection"));
    }

    /// Asks for another computer to connect to over SSH.
    pub(super) fn add_connection_dialog(self: &Rc<Self>) {
        let form = FormDialog::new("Add Remote Computer", "Connect");
        let group = adw::PreferencesGroup::builder()
            .description(
                "Manage the virtual machines of another computer over SSH. The app logs in with your SSH key; \
                 set it up first with: ssh-copy-id user@computer",
            )
            .build();
        let host = adw::EntryRow::builder().title("Computer (name or address)").build();
        let user =
            adw::EntryRow::builder().title("User name").text(glib::user_name().to_string_lossy().into_owned()).build();
        let port =
            adw::EntryRow::builder().title("SSH port (empty = 22)").input_purpose(gtk::InputPurpose::Digits).build();
        let kind = adw::ComboRow::builder()
            .title("Machines")
            .model(&gtk::StringList::new(&["System", "User session"]))
            .build();
        let startup = adw::SwitchRow::builder().title("Connect at startup").build();
        for w in [
            host.upcast_ref::<gtk::Widget>(),
            user.upcast_ref(),
            port.upcast_ref(),
            kind.upcast_ref(),
            startup.upcast_ref(),
        ] {
            group.add(w);
        }
        form.add(&group);
        // Debug builds: VMNG_DEV_REMOTE=<address> fills it in (tests).
        #[cfg(debug_assertions)]
        if let Some(t) = std::env::var("VMNG_DEV_REMOTE").ok().and_then(|u| vmng_virt::SshTarget::parse(&u)) {
            host.set_text(&t.host);
            user.set_text(t.user.as_deref().unwrap_or_default());
            port.set_text(&t.port.map(|p| p.to_string()).unwrap_or_default());
        }
        let weak = Rc::downgrade(self);
        form.connect_primary(move |form| {
            let Some(this) = weak.upgrade() else { return };
            let port_text = port.text().trim().to_owned();
            let port = if port_text.is_empty() {
                None
            } else {
                match port_text.parse::<u16>() {
                    Ok(p) if p > 0 => Some(p),
                    _ => return form.set_error(Some("The port must be a number between 1 and 65535.")),
                }
            };
            let user = Some(user.text().trim().to_owned()).filter(|u| !u.is_empty());
            #[allow(unused_mut)]
            let mut target = vmng_virt::SshTarget {
                user,
                host: host.text().trim().to_owned(),
                port,
                session: kind.selected() == 1,
                ..Default::default()
            };
            // Tests reach the test computer with their own key.
            #[cfg(debug_assertions)]
            if let Some(t) = std::env::var("VMNG_DEV_REMOTE").ok().and_then(|u| vmng_virt::SshTarget::parse(&u)) {
                target.keyfile = t.keyfile;
                target.no_verify = t.no_verify;
            }
            if let Err(m) = target.validate() {
                return form.set_error(Some(m));
            }
            let uri = target.uri();
            let at_startup = startup.is_active();
            this.settings.update(|p| {
                if !p.remote_connections.contains(&uri) {
                    p.remote_connections.push(uri.clone());
                }
                if at_startup && !p.autoconnect.contains(&uri) {
                    p.autoconnect.push(uri.clone());
                }
            });
            form.close();
            this.fill_connection_menu();
            this.switch_connection(ConnectionKind::Other(uri));
        });
        form.present(&self.window);
    }

    /// Forgets the current other computer and goes back to this one.
    pub(super) fn ask_remove_connection(self: &Rc<Self>) {
        let kind = self.state.borrow().current.clone();
        if !kind.is_remote() {
            return;
        }
        let weak = Rc::downgrade(self);
        let label = kind.label();
        self.feedback.confirm(
            &format!("Remove {label}?"),
            "The app stops showing this computer. Its machines keep running there.",
            "Remove",
            true,
            move || {
                let Some(this) = weak.upgrade() else { return };
                let uri = kind.uri().to_owned();
                this.settings.update(|p| {
                    p.remote_connections.retain(|u| *u != uri);
                    p.autoconnect.retain(|u| *u != uri);
                });
                this.switch_connection(ConnectionKind::System);
                this.state.borrow_mut().conns.remove(&kind);
                this.fill_connection_menu();
            },
        );
    }

    pub(super) async fn on_connected(self: &Rc<Self>, conn: Conn) {
        if let Ok(info) = conn.call(|c| c.host_info()).await {
            self.state.borrow_mut().host_info = Some(info);
        }
        if let Ok(caps) = conn.call(|c| c.host_caps()).await {
            let kvm = caps.guest(&caps.host_arch).is_some_and(|g| g.kvm);
            self.state.borrow_mut().kvm = Some(kvm);
        }
        let view = self.state.borrow().view;
        self.load_view(view);
        self.refresh();
    }

    pub(super) fn switch_connection(self: &Rc<Self>, kind: ConnectionKind) {
        if self.state.borrow().current == kind {
            return;
        }
        {
            let mut st = self.state.borrow_mut();
            st.current = kind.clone();
            st.monitor.reset();
            st.domains.clear();
            st.guest.clear();
            st.host_info = None;
        }
        self.settings.update(|p| p.last_connection = kind.uri().to_owned());
        self.nav.pop_to_tag("main");
        self.machines.set_visible_child_name("loading");
        self.vm_list.update(&[]);
        self.update_chrome();
        match self.current_conn() {
            Some(conn) => {
                let this = self.clone();
                glib::spawn_future_local(async move { this.on_connected(conn).await });
            }
            None => self.connect_to(kind),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_ids_round_trip() {
        for kind in [
            ConnectionKind::System,
            ConnectionKind::Session,
            ConnectionKind::Other("qemu+ssh://me@server/system".into()),
        ] {
            assert_eq!(connection_of(&connection_id(&kind)), kind);
        }
    }
}
