//! Guest display widgets: a GTK4 screen for SPICE and VNC guests and a
//! text console terminal, on top of spice-client-glib, gvnc and VTE.

use std::os::fd::OwnedFd;
use std::rc::Rc;

mod ffi;
pub mod keymap;
mod screen;
mod spice;
mod terminal;
mod vnc;

pub use screen::{Backend, Button, GuestScreen, Scaling, parse_keys};
pub use spice::SpiceBackend;
pub use terminal::Terminal;
pub use vnc::VncBackend;

/// Connection state reported by a backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Connecting,
    Connected,
    /// Closed; with a message when it was an error.
    Disconnected(Option<String>),
}

/// Asks for another socket to the guest display. The answer is delivered
/// to the callback later (getting it may block, so it runs elsewhere).
pub type FdProvider = Rc<dyn Fn(Box<dyn FnOnce(Result<OwnedFd, String>)>)>;
