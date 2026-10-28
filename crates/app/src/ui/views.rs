//! The main window's views (header tabs) and connection status.

use vmng_virt::ConnectionKind;

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

    /// The short name on the view's header tab.
    pub fn tab_title(self) -> &'static str {
        match self {
            Self::Machines => "Machines",
            Self::Host => "Host",
            Self::Networks => "Networks",
            Self::Storage => "Storage",
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

/// How a connection is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnStatus {
    Connecting,
    Connected,
    Failed,
    Disconnected,
}

/// Short name of a connection, e.g. "System".
pub fn connection_name(kind: &ConnectionKind) -> &str {
    match kind {
        ConnectionKind::System => "System",
        ConnectionKind::Session => "User session",
        ConnectionKind::Other(uri) => uri,
    }
}
