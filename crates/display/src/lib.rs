//! Guest display widgets: a GTK4 screen for SPICE and VNC guests, and the
//! keyboard translation they need. Work in progress (console milestone):
//! the SPICE/VNC protocol backends and the text console are still to come.

#[allow(dead_code)]
mod ffi;
pub mod keymap;
mod screen;

pub use screen::{Backend, Button, GuestScreen, Scaling, parse_keys};
