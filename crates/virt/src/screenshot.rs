//! A still image of a running machine's screen, for the console preview.
//!
//! QEMU sends a PPM image (or PNG with newer versions). The image comes
//! from the hypervisor side, so it is size-limited and parsed strictly.

use virt::stream::Stream;

use crate::connection::Connection;
use crate::error::{Error, Result};

/// Largest image accepted, in bytes (an 8K screen as PPM is ~100 MiB, so
/// this allows 4K with room to spare).
const MAX_BYTES: usize = 64 << 20;
/// Largest width or height accepted.
const MAX_SIDE: u32 = 16384;

/// One captured screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screenshot {
    /// 8-bit RGB pixels, `width * 3` bytes per row, no padding.
    Rgb { width: u32, height: u32, pixels: Vec<u8> },
    /// A PNG file, for the UI toolkit to decode.
    Png(Vec<u8>),
}

impl Connection {
    /// Captures the first screen of a running machine.
    pub fn screenshot(&self, uuid: &str) -> Result<Screenshot> {
        let dom = self.domain(uuid)?;
        let stream = Stream::new(self.raw(), 0)?;
        let mime = dom.screenshot(&stream, 0, 0)?;
        let mut data = Vec::new();
        let mut buf = vec![0u8; 256 << 10];
        loop {
            let n = match stream.recv(&mut buf) {
                Ok(n) => n,
                Err(e) => {
                    let _ = stream.abort();
                    return Err(e.into());
                }
            };
            if n == 0 {
                break;
            }
            if data.len() + n > MAX_BYTES {
                let _ = stream.abort();
                return Err(Error::Invalid("The screen image is too large".into()));
            }
            data.extend_from_slice(&buf[..n]);
        }
        stream.finish()?;
        match mime.as_str() {
            "image/png" => Ok(Screenshot::Png(data)),
            _ => parse_ppm(&data),
        }
    }
}

/// Parses a binary PPM (`P6`) image with 8-bit samples.
pub(crate) fn parse_ppm(data: &[u8]) -> Result<Screenshot> {
    let bad = || Error::Invalid("Unexpected screen image format".into());
    let mut pos = 0;
    // The header: "P6", width, height, maxval, separated by whitespace
    // and comments, then exactly one whitespace byte before the pixels.
    let field = |pos: &mut usize| -> Option<&[u8]> {
        loop {
            while data.get(*pos).is_some_and(u8::is_ascii_whitespace) {
                *pos += 1;
            }
            if data.get(*pos) == Some(&b'#') {
                while data.get(*pos).is_some_and(|&b| b != b'\n') {
                    *pos += 1;
                }
                continue;
            }
            break;
        }
        let start = *pos;
        while data.get(*pos).is_some_and(|b| !b.is_ascii_whitespace()) {
            *pos += 1;
        }
        (*pos > start).then(|| &data[start..*pos])
    };
    let number = |s: &[u8]| -> Option<u32> {
        if s.len() > 9 || !s.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(s).ok()?.parse().ok()
    };
    if field(&mut pos) != Some(b"P6".as_slice()) {
        return Err(bad());
    }
    let width = field(&mut pos).and_then(number).ok_or_else(bad)?;
    let height = field(&mut pos).and_then(number).ok_or_else(bad)?;
    let maxval = field(&mut pos).and_then(number).ok_or_else(bad)?;
    if maxval != 255 || width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(bad());
    }
    if !data.get(pos).is_some_and(u8::is_ascii_whitespace) {
        return Err(bad());
    }
    pos += 1;
    let len = width as usize * height as usize * 3;
    let pixels = data.get(pos..pos + len).ok_or_else(bad)?;
    Ok(Screenshot::Rgb { width, height, pixels: pixels.to_vec() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ppm_with_comment() {
        let mut img = b"P6\n# made by qemu\n2 1\n255\n".to_vec();
        img.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
        assert_eq!(parse_ppm(&img), Ok(Screenshot::Rgb { width: 2, height: 1, pixels: vec![1, 2, 3, 4, 5, 6] }));
    }

    #[test]
    fn rejects_bad_images() {
        for img in [
            b"P5\n1 1\n255\n\0".as_slice(),             // greyscale
            b"P6\n1 1\n65535\n\0\0\0\0\0\0".as_slice(), // 16-bit
            b"P6\n2 2\n255\n\0\0\0".as_slice(),         // truncated
            b"P6\n0 1\n255\n".as_slice(),               // empty
            b"P6\n99999 99999\n255\n".as_slice(),       // too large
            b"P6\n1 1\n255".as_slice(),                 // no pixels
            b"P6\n-1 1\n255\n\0\0\0".as_slice(),        // not a number
        ] {
            assert!(parse_ppm(img).is_err(), "{:?}", String::from_utf8_lossy(img));
        }
    }
}
