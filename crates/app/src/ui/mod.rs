//! All user interface code.

mod console;
mod feedback;
mod hardware;
mod host_view;
mod networks_view;
mod new_vm;
mod os_chooser;
pub mod preferences;
mod snapshots;
mod storage_view;
mod title_menu;
mod vm_actions;
mod vm_details;
mod vm_dialogs;
mod vm_list;
mod vm_page;
pub mod widgets;
mod window;

pub use window::MainWindow;
