//! Build-time application constants.

pub const APP_ID: &str = "io.github.inittux111.VirtManagerNextgen";
pub const APP_NAME: &str = "Virtual Machine Manager";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Directory name used under the XDG config directory.
pub const CONFIG_DIR: &str = "virt-manager-nextgen";
pub const WEBSITE: &str = "https://github.com/inittux111/virt-manager-nextgen";
