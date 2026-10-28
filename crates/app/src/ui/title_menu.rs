//! The clickable window title: shows the current view and connection, and
//! opens a menu to switch between them. This replaces a permanent sidebar
//! so the VM list gets the whole window.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use vmng_virt::ConnectionKind;

use super::widgets::{StatusLabel, Tone};

/// The top-level views of the main window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Machines,
    Host,
    Networks,
    Storage,
}

impl View {
    pub const ALL: [View; 4] = [View::Machines, View::Host, View::Networks, View::Storage];

    pub fn title(self) -> &'static str {
        match self {
            Self::Machines => "Virtual Machines",
            Self::Host => "Host Overview",
            Self::Networks => "Virtual Networks",
            Self::Storage => "Storage Pools",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Machines => "computer-symbolic",
            Self::Host => "utilities-system-monitor-symbolic",
            Self::Networks => "network-wired-symbolic",
            Self::Storage => "drive-harddisk-symbolic",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Machines => "machines",
            Self::Host => "host",
            Self::Networks => "networks",
            Self::Storage => "storage",
        }
    }
}

/// How a connection is doing, as shown in the title and menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnStatus {
    Connecting,
    Connected,
    Failed,
    Disconnected,
}

impl ConnStatus {
    fn tone(self) -> Tone {
        match self {
            Self::Connected => Tone::Running,
            Self::Connecting => Tone::Paused,
            Self::Failed => Tone::Error,
            Self::Disconnected => Tone::Off,
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::Connecting => "Connecting…",
            Self::Connected => "Connected",
            Self::Failed => "Not connected",
            Self::Disconnected => "Not connected",
        }
    }
}

/// The connections offered in the menu.
pub const CONNECTIONS: [ConnectionKind; 2] = [ConnectionKind::System, ConnectionKind::Session];

/// Short name for the connection switcher.
pub fn connection_name(kind: &ConnectionKind) -> &str {
    match kind {
        ConnectionKind::System => "System",
        ConnectionKind::Session => "User session",
        ConnectionKind::Other(uri) => uri,
    }
}

type Callback<T> = RefCell<Option<Box<dyn Fn(T)>>>;

#[derive(Default)]
struct Handlers {
    view: Callback<View>,
    connection: Callback<ConnectionKind>,
}

/// The title button and its popover.
#[derive(Clone)]
pub struct TitleMenu {
    pub button: gtk::MenuButton,
    title: gtk::Label,
    subtitle: StatusLabel,
    popover: gtk::Popover,
    view_checks: Rc<Vec<(View, gtk::Image)>>,
    conn_rows: Rc<Vec<(ConnectionKind, gtk::Image, StatusLabel)>>,
    handlers: Rc<Handlers>,
}

impl std::fmt::Debug for TitleMenu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TitleMenu").finish_non_exhaustive()
    }
}

fn menu_row(icon: Option<&str>, content: &impl IsA<gtk::Widget>) -> (gtk::ListBoxRow, gtk::Image) {
    let b = gtk::Box::builder().spacing(12).build();
    if let Some(icon) = icon {
        b.append(&gtk::Image::from_icon_name(icon));
    }
    content.set_hexpand(true);
    b.append(content);
    let check = gtk::Image::builder().icon_name("object-select-symbolic").css_classes(["check"]).build();
    b.append(&check);
    (gtk::ListBoxRow::builder().child(&b).build(), check)
}

fn heading(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).css_classes(["caption-heading", "dim-label", "menu-heading"]).build()
}

impl TitleMenu {
    pub fn new() -> Self {
        let title =
            gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["title"]).build();
        let subtitle = StatusLabel::new();
        subtitle.root.add_css_class("caption");
        subtitle.root.add_css_class("dim-label");
        let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).valign(gtk::Align::Center).build();
        text.append(&title);
        text.append(&subtitle.root);
        let arrow = gtk::Image::from_icon_name("pan-down-symbolic");
        let content = gtk::Box::builder().spacing(10).build();
        content.append(&text);
        content.append(&arrow);

        let popover = gtk::Popover::builder().has_arrow(false).halign(gtk::Align::Start).build();
        popover.add_css_class("title-menu");
        popover.add_css_class("menu");
        let button = gtk::MenuButton::builder()
            .child(&content)
            .popover(&popover)
            .css_classes(["flat", "title-menu-button"])
            .tooltip_text("Switch view or connection")
            .build();

        let handlers = Rc::new(Handlers::default());
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .width_request(280)
            .margin_top(4)
            .margin_bottom(4)
            .build();

        // Views.
        body.append(&heading("View"));
        let views_list = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
        let mut view_checks = Vec::new();
        for view in View::ALL {
            let label = gtk::Label::builder().label(view.title()).xalign(0.0).build();
            let (row, check) = menu_row(Some(view.icon()), &label);
            views_list.append(&row);
            view_checks.push((view, check));
        }
        body.append(&views_list);
        body.append(&gtk::Separator::builder().margin_top(6).margin_bottom(2).build());

        // Connections.
        body.append(&heading("Connection"));
        let conn_list = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
        let mut conn_rows = Vec::new();
        for kind in CONNECTIONS {
            let name = gtk::Label::builder().label(connection_name(&kind)).xalign(0.0).build();
            let status = StatusLabel::new();
            status.root.add_css_class("caption");
            status.root.add_css_class("dim-label");
            status.set(Tone::Off, kind.uri());
            let col = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
            col.append(&name);
            col.append(&status.root);
            let (row, check) = menu_row(None, &col);
            conn_list.append(&row);
            conn_rows.push((kind, check, status));
        }
        body.append(&conn_list);
        popover.set_child(Some(&body));

        let menu = Self {
            button,
            title,
            subtitle,
            popover,
            view_checks: Rc::new(view_checks),
            conn_rows: Rc::new(conn_rows),
            handlers,
        };

        let m = menu.clone();
        views_list.connect_row_activated(move |_, row| {
            let view = View::ALL[row.index() as usize];
            m.popover.popdown();
            if let Some(f) = m.handlers.view.borrow().as_ref() {
                f(view);
            }
        });
        let m = menu.clone();
        conn_list.connect_row_activated(move |_, row| {
            let kind = CONNECTIONS[row.index() as usize].clone();
            m.popover.popdown();
            if let Some(f) = m.handlers.connection.borrow().as_ref() {
                f(kind);
            }
        });
        menu
    }

    pub fn connect_view_selected(&self, f: impl Fn(View) + 'static) {
        self.handlers.view.replace(Some(Box::new(f)));
    }

    pub fn connect_connection_selected(&self, f: impl Fn(ConnectionKind) + 'static) {
        self.handlers.connection.replace(Some(Box::new(f)));
    }

    /// Shows the current view and connection in the title and menu.
    pub fn set_current(&self, view: View, conn: &ConnectionKind, status: ConnStatus) {
        self.title.set_label(view.title());
        let text = match status {
            ConnStatus::Connected => format!("QEMU/KVM · {}", connection_name(conn)),
            other => format!("{} · {}", connection_name(conn), other.word()),
        };
        self.subtitle.set(status.tone(), &text);
        for (v, check) in self.view_checks.iter() {
            check.set_opacity(if *v == view { 1.0 } else { 0.0 });
        }
        for (k, check, _) in self.conn_rows.iter() {
            check.set_opacity(if k == conn { 1.0 } else { 0.0 });
        }
    }

    /// Updates the status line of one connection in the menu.
    pub fn set_connection_status(&self, conn: &ConnectionKind, status: ConnStatus) {
        if let Some((_, _, label)) = self.conn_rows.iter().find(|(k, _, _)| k == conn) {
            let text = match status {
                ConnStatus::Connected => conn.uri().to_owned(),
                other => format!("{} · {}", conn.uri(), other.word()),
            };
            label.set(status.tone(), &text);
        }
    }
}
