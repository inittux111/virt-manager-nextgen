//! Virtual Machine Manager: a modern desktop front-end for QEMU/KVM.

mod application;
mod backend;
mod config;
#[cfg(debug_assertions)]
mod dev;
mod format;
mod osdb;
mod settings;
mod tray;
mod ui;

fn main() -> gtk::glib::ExitCode {
    vmng_virt::init();
    osdb::preload();
    application::run()
}
