//! Build-time application constants.

pub const APP_ID: &str = "io.github.inittux111.VirtManagerNextgen";
pub const APP_NAME: &str = "Virtual Machine Manager";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Directory name used under the XDG config directory.
pub const CONFIG_DIR: &str = "virt-manager-nextgen";
pub const WEBSITE: &str = "https://github.com/inittux111/virt-manager-nextgen";
/// Symbolic app icon, used in the tray.
pub const TRAY_ICON: &str = "io.github.inittux111.VirtManagerNextgen-symbolic";
/// Where development builds find the app icons, which only an installed
/// copy has in the system icon theme.
#[cfg(debug_assertions)]
pub const DEV_ICON_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/icons");
