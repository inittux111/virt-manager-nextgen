//! Virtual Machines: a modern desktop front-end for QEMU/KVM.

mod application;
mod backend;
mod config;
#[cfg(debug_assertions)]
mod dev;
mod format;
mod log;
mod osdb;
mod settings;
mod tray;
mod ui;

fn main() -> gtk::glib::ExitCode {
    log::init();
    vmng_virt::init();
    osdb::preload();
    application::run()
}
