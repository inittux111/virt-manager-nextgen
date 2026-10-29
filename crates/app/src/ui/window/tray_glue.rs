//! The tray icon: shown to match Preferences, its machine menu and commands.

use super::*;

impl MainWindow {
    /// Shows or removes the tray icon to match Preferences.
    pub(super) fn update_tray(self: &Rc<Self>) {
        let prefs = self.settings.get();
        let wanted = prefs.tray_icon;
        let icon = if prefs.tray_icon_color {
            crate::tray::TrayIcon::Image(crate::config::APP_ICON_SVG)
        } else {
            crate::tray::TrayIcon::Named(crate::config::TRAY_ICON)
        };
        let conn = self.window.application().and_then(|a| a.dbus_connection());
        let can = conn.as_ref().is_some_and(crate::tray::available);
        if !(wanted && can) {
            self.tray.replace(None);
            return;
        }
        if let Some(tray) = self.tray.borrow().as_ref() {
            tray.set_icon(icon);
            return;
        }
        let Some(conn) = conn else { return };
        let weak = Rc::downgrade(self);
        match crate::tray::Tray::new(&conn, icon, move |cmd| {
            if let Some(this) = weak.upgrade() {
                this.tray_command(cmd);
            }
        }) {
            Ok(tray) => {
                self.tray.replace(Some(tray));
                self.update_tray_menu();
            }
            Err(e) => glib::g_warning!("tray", "couldn't create the tray icon: {e}"),
        }
    }

    /// Shows the window in front. GNOME (Wayland) only lets a window raise
    /// itself in response to input inside the app, and a tray menu click
    /// isn't that; a window that appears anew is put in front, though. So a
    /// window that is open but behind others (or minimised) is hidden and
    /// shown again.
    pub(super) fn bring_to_front(&self) {
        if self.window.is_visible() && !self.window.is_active() {
            self.window.set_visible(false);
        }
        self.window.present();
    }

    pub(super) fn update_tray_menu(&self) {
        let tray = self.tray.borrow();
        let Some(tray) = tray.as_ref() else { return };
        let vms: Vec<crate::tray::TrayVm> = self
            .state
            .borrow()
            .domains
            .iter()
            .map(|d| crate::tray::TrayVm { uuid: d.uuid.clone(), name: d.display_name().to_owned(), state: d.state })
            .collect();
        tray.set_vms(&vms);
    }

    pub(super) fn tray_command(self: &Rc<Self>, cmd: crate::tray::TrayCommand) {
        use crate::tray::TrayCommand;
        match cmd {
            TrayCommand::ShowWindow => self.bring_to_front(),
            TrayCommand::Quit => {
                if let Some(app) = self.window.application() {
                    app.quit();
                }
            }
            TrayCommand::Power(uuid, action) => self.request_power(uuid, action),
            TrayCommand::Console(uuid) => {
                if let Some(vm) = self.vm_by_uuid(&uuid) {
                    self.open_console(&vm);
                }
            }
        }
    }
}
