//! The full-window page of one VM, with tabs for its hardware, snapshots
//! and performance. Opened from the details panel; Back returns to the list.

use std::rc::Rc;

use adw::prelude::*;
use vmng_virt::{DomainSummary, HostInfo};

use super::hardware::HardwareView;
use super::performance::PerformanceView;
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
    pub performance: PerformanceView,
}

impl std::fmt::Debug for VmPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VmPage").finish_non_exhaustive()
    }
}

impl VmPage {
    pub const TAG: &'static str = "vm";

    pub fn new(snapshots: Rc<SnapshotsView>, hardware: Rc<HardwareView>) -> Self {
        let stack = adw::ViewStack::new();
        stack.add_titled_with_icon(&hardware.root, Some(VmTab::Hardware.id()), "Hardware", "emblem-system-symbolic");
        stack.add_titled_with_icon(&snapshots.root, Some(VmTab::Snapshots.id()), "Snapshots", "camera-photo-symbolic");
        let performance = PerformanceView::new();
        stack.add_titled_with_icon(
            &performance.root,
            Some(VmTab::Performance.id()),
            "Performance",
            "utilities-system-monitor-symbolic",
        );
        // GNOME's standard header tabs (icon and label).
        let switcher = adw::ViewSwitcher::builder().stack(&stack).policy(adw::ViewSwitcherPolicy::Wide).build();

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

        // Narrow windows move the tabs to the bottom.
        let bottom = adw::ViewSwitcherBar::builder().stack(&stack).build();
        let toolbar = adw::ToolbarView::builder().content(&stack).build();
        toolbar.add_top_bar(&header);
        toolbar.add_bottom_bar(&bottom);
        let bin = adw::BreakpointBin::builder().width_request(360).height_request(300).child(&toolbar).build();
        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            820.0,
            adw::LengthUnit::Px,
        ));
        narrow.add_setter(&switcher, "visible", Some(&false.to_value()));
        narrow.add_setter(&bottom, "reveal", Some(&true.to_value()));
        bin.add_breakpoint(narrow);
        let page = adw::NavigationPage::builder().tag(Self::TAG).title("Machine").child(&bin).build();
        let this = Self { page, stack, logo, name, status, buttons, snapshots, hardware, performance };
        // Below this, the header only fits icon-only buttons. (Only one
        // breakpoint applies at a time, so it repeats the narrow setters.)
        let compact = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            520.0,
            adw::LengthUnit::Px,
        ));
        compact.add_setter(&switcher, "visible", Some(&false.to_value()));
        compact.add_setter(&bottom, "reveal", Some(&true.to_value()));
        compact.add_setter(&this.logo, "visible", Some(&false.to_value()));
        let b = this.buttons.clone();
        compact.connect_apply(move |_| b.set_compact(true));
        let b = this.buttons.clone();
        compact.connect_unapply(move |_| b.set_compact(false));
        bin.add_breakpoint(compact);
        this
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
