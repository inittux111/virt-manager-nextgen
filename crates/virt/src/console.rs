//! Access to a domain's graphical display and text console.

use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::Arc;

use virt::stream::Stream;

use crate::connection::Connection;
use crate::error::{Error, Result};

// virDomainOpenGraphicsFlags
const GRAPHICS_SKIPAUTH: u32 = 1;
// virDomainConsoleFlags
const CONSOLE_FORCE: u32 = 1;
const CONSOLE_SAFE: u32 = 2;

/// A connected text console. Reading blocks, so it is done on a worker
/// thread; writing and closing may happen from another thread.
#[derive(Clone)]
pub struct ConsoleStream {
    stream: Arc<Stream>,
}

impl std::fmt::Debug for ConsoleStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsoleStream").finish_non_exhaustive()
    }
}

impl ConsoleStream {
    /// Reads output from the guest. Returns 0 when the console closed.
    pub fn read(&self, buf: &mut [u8]) -> Result<usize> {
        Ok(self.stream.recv(buf)?)
    }

    /// Sends input to the guest.
    pub fn write(&self, mut data: &[u8]) -> Result<()> {
        while !data.is_empty() {
            let n = self.stream.send(data)?;
            if n == 0 {
                return Err(Error::Invalid("The console was closed.".into()));
            }
            data = &data[n.min(data.len())..];
        }
        Ok(())
    }

    /// Closes the console; a blocked `read` returns.
    pub fn close(&self) {
        // SAFETY: the stream pointer is valid for the lifetime of `self`;
        // aborting is allowed while another thread waits in recv.
        unsafe {
            virt::sys::virStreamAbort(self.stream.as_ptr());
        }
    }
}

impl Connection {
    /// Opens a socket to the domain's graphical display (SPICE or VNC)
    /// number `index`, already authenticated through libvirt. On another
    /// computer (SSH), a tunnel to the display's port instead.
    pub fn open_graphics_fd(&self, uuid: &str, index: u32) -> Result<OwnedFd> {
        if let Some(target) = self.kind().ssh_target() {
            let live = vmng_xml::parse(&self.domain(uuid)?.get_xml_desc(0)?)?;
            let dest = crate::remote::display_address(&live, index as usize)
                .ok_or_else(|| Error::Invalid("The display isn't listening on the other computer.".into()))?;
            return target.tunnel(&dest);
        }
        let fd = self.domain(uuid)?.open_graphics_fd(index, GRAPHICS_SKIPAUTH)?;
        let fd = i32::try_from(fd).map_err(|_| Error::Invalid("libvirt returned an invalid descriptor.".into()))?;
        // SAFETY: libvirt hands us a new descriptor that we now own.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// Connects to the domain's first text console (serial port). With
    /// `force`, takes it over from another client (e.g. `virsh console`).
    pub fn open_console(&self, uuid: &str, force: bool) -> Result<ConsoleStream> {
        let dom = self.domain(uuid)?;
        let stream = Stream::new(self.raw(), 0)?;
        let flags = CONSOLE_SAFE | if force { CONSOLE_FORCE } else { 0 };
        dom.open_console(None, &stream, flags)?;
        Ok(ConsoleStream { stream: Arc::new(stream) })
    }
}
