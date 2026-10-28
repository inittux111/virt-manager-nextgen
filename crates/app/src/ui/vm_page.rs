//! The full-window page of one VM, with tabs for its hardware, snapshots
//! and performance. Opened from the details panel; Back returns to the list.

use std::rc::Rc;

use adw::prelude::*;
use vmng_virt::{DomainSummary, HostInfo};

use super::hardware::HardwareView;
use super::snapshots::SnapshotsView;
use super::vm_actions::VmButtons;
use super::vm_list::{logo_key, os_label, state_text};
use super::widgets::{OsLogo, StatusLabel, Tone};
use crate::backend::Conn;

/// The tabs of the VM page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VmTab {
    Hardware,
    Snapshots,
    Performance,
}

impl VmTab {
    fn id(self) -> &'static str {
        match self {
            Self::Hardware => "hardware",
            Self::Snapshots => "snapshots",
            Self::Performance => "performance",
        }
    }
}

pub struct VmPage {
    pub page: adw::NavigationPage,
    stack: adw::ViewStack,
    logo: OsLogo,
    name: gtk::Label,
    status: StatusLabel,
    buttons: VmButtons,
    pub snapshots: Rc<SnapshotsView>,
    pub hardware: Rc<HardwareView>,
}

impl std::fmt::Debug for VmPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VmPage").finish_non_exhaustive()
    }
}

fn later(icon: &str, title: &str, text: &str) -> adw::StatusPage {
    adw::StatusPage::builder().icon_name(icon).title(title).description(text).build()
}

impl VmPage {
    pub const TAG: &'static str = "vm";

    pub fn new(snapshots: Rc<SnapshotsView>, hardware: Rc<HardwareView>) -> Self {
        let stack = adw::ViewStack::new();
        stack.add_titled_with_icon(&hardware.root, Some(VmTab::Hardware.id()), "Hardware", "emblem-system-symbolic");
        stack.add_titled_with_icon(&snapshots.root, Some(VmTab::Snapshots.id()), "Snapshots", "camera-photo-symbolic");
        stack.add_titled_with_icon(
            &later(
                "utilities-system-monitor-symbolic",
                "Performance",
                "Detailed performance graphs arrive in a later update.",
            ),
            Some(VmTab::Performance.id()),
            "Performance",
            "utilities-system-monitor-symbolic",
        );
        let switcher = adw::InlineViewSwitcher::builder().stack(&stack).build();
        switcher.add_css_class("flat");

        let logo = OsLogo::new(30);
        let name = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["heading"])
            .build();
        let status = StatusLabel::new();
        status.root.add_css_class("caption");
        status.root.add_css_class("dim-label");
        let titles = gtk::Box::builder().orientation(gtk::Orientation::Vertical).valign(gtk::Align::Center).build();
        titles.append(&name);
        titles.append(&status.root);
        let identity = gtk::Box::builder().spacing(10).build();
        identity.append(&logo);
        identity.append(&titles);

        let buttons = VmButtons::new();
        let header = adw::HeaderBar::builder().title_widget(&switcher).build();
        header.pack_start(&identity);
        header.pack_end(&buttons.root);

        let toolbar = adw::ToolbarView::builder().content(&stack).build();
        toolbar.add_top_bar(&header);
        let page = adw::NavigationPage::builder().tag(Self::TAG).title("Machine").child(&toolbar).build();
        Self { page, stack, logo, name, status, buttons, snapshots, hardware }
    }

    pub fn select(&self, tab: VmTab) {
        self.stack.set_visible_child_name(tab.id());
    }

    /// Shows `vm`; called on every refresh while the page is open.
    pub fn show(&self, conn: Conn, vm: &DomainSummary, host: Option<HostInfo>) {
        self.page.set_title(vm.display_name());
        self.logo.set_logo(logo_key(vm));
        self.name.set_label(vm.display_name());
        self.status.set(Tone::of(vm.state), &format!("{} · {}", state_text(vm), os_label(vm)));
        self.buttons.update(vm.state);
        self.hardware.set_target(conn.clone(), vm, host);
        self.snapshots.set_target(conn, vm);
    }
}
