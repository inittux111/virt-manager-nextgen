//! Domain (virtual machine) state and lifecycle actions.

use crate::config::DomainConfig;

/// The run state of a domain, simplified to what a person cares about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DomainState {
    Running,
    Paused,
    /// Shutting down, still running.
    ShuttingDown,
    ShutOff,
    Crashed,
    /// Suspended to RAM by the guest (power management).
    Suspended,
    Unknown,
}

impl DomainState {
    /// Maps a raw `virDomainState` value.
    pub fn from_raw(state: u32) -> Self {
        match state {
            1 | 2 => Self::Running, // RUNNING, BLOCKED
            3 => Self::Paused,
            4 => Self::ShuttingDown,
            5 => Self::ShutOff,
            6 => Self::Crashed,
            7 => Self::Suspended,
            _ => Self::Unknown,
        }
    }

    /// Whether the domain has a QEMU process (it uses host resources and
    /// has a console).
    pub fn is_active(self) -> bool {
        matches!(self, Self::Running | Self::Paused | Self::ShuttingDown | Self::Suspended)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Paused => "Paused",
            Self::ShuttingDown => "Shutting down",
            Self::ShutOff => "Shut off",
            Self::Crashed => "Crashed",
            Self::Suspended => "Suspended",
            Self::Unknown => "Unknown",
        }
    }
}

/// Everything the VM list needs to know about one domain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DomainSummary {
    pub uuid: String,
    pub state: DomainState,
    pub persistent: bool,
    pub autostart: bool,
    /// Whether a managed save image exists (the VM was "saved").
    pub has_managed_save: bool,
    pub config: DomainConfig,
}

impl DomainSummary {
    /// The name shown to people: the title if set, otherwise the name.
    pub fn display_name(&self) -> &str {
        self.config.title.as_deref().unwrap_or(&self.config.name)
    }
}

/// A lifecycle operation on a domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerAction {
    /// Start a shut-off domain (restores a saved state if there is one).
    Start,
    /// Ask the guest to shut down (ACPI / guest agent).
    Shutdown,
    /// Ask the guest to reboot.
    Reboot,
    /// Hard reset, like pressing the reset button.
    Reset,
    /// Pull the plug.
    ForceOff,
    Pause,
    Resume,
    /// Save the memory state to disk and stop (managed save).
    Save,
}

impl PowerAction {
    /// Present-tense label for buttons and menus.
    pub fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Shutdown => "Shut Down",
            Self::Reboot => "Reboot",
            Self::Reset => "Reset",
            Self::ForceOff => "Force Off",
            Self::Pause => "Pause",
            Self::Resume => "Resume",
            Self::Save => "Save",
        }
    }
}
