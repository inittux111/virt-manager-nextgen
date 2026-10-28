//! Human-friendly formatting of sizes, rates and durations.
//!
//! All sizes use binary units (GiB), matching libvirt and virt-manager.

const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];

/// Formats a byte count, e.g. `8 GiB`, `412 GiB`, `1.5 GiB`, `23.4 GiB`.
pub fn bytes(n: u64) -> String {
    let (value, unit) = scale(n as f64);
    format!("{} {unit}", number(value))
}

/// Formats a transfer rate in bytes per second, e.g. `12 MB/s`.
///
/// Rates use decimal units like file managers and network tools do.
pub fn rate(bytes_per_sec: f64) -> String {
    const RATE_UNITS: [&str; 5] = ["B/s", "KB/s", "MB/s", "GB/s", "TB/s"];
    let mut v = bytes_per_sec.max(0.0);
    let mut i = 0;
    while v >= 1000.0 && i < RATE_UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    format!("{} {}", number(v), RATE_UNITS[i])
}

/// Formats "used of total", sharing the unit when both scale the same way:
/// `18.4 of 64 GiB`.
pub fn used_of(used: u64, total: u64) -> String {
    let (t, unit) = scale(total as f64);
    let (u, used_unit) = scale(used as f64);
    if unit == used_unit {
        format!("{} of {} {unit}", number(u), number(t))
    } else {
        format!("{} of {}", bytes(used), bytes(total))
    }
}

/// Formats a percentage without decimals: `23%`.
pub fn percent(p: f64) -> String {
    format!("{:.0}%", p.clamp(0.0, 100.0))
}

fn scale(mut v: f64) -> (f64, &'static str) {
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    (v, UNITS[i])
}

/// Up to three significant digits, no trailing zeros: 8, 23.4, 1.25, 412.
fn number(v: f64) -> String {
    let decimals = if v >= 100.0 || v == v.trunc() {
        0
    } else if v >= 10.0 {
        1
    } else {
        2
    };
    let s = format!("{v:.decimals$}");
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(8 << 30), "8 GiB");
        assert_eq!(bytes(1536 << 20), "1.5 GiB");
        assert_eq!(bytes((23.4 * (1u64 << 30) as f64) as u64), "23.4 GiB");
        assert_eq!(bytes(412 << 30), "412 GiB");
    }

    #[test]
    fn rates() {
        assert_eq!(rate(0.0), "0 B/s");
        assert_eq!(rate(12_000_000.0), "12 MB/s");
        assert_eq!(rate(220_000.0), "220 KB/s");
        assert_eq!(rate(-5.0), "0 B/s");
    }

    #[test]
    fn used_of_total() {
        assert_eq!(used_of(18 << 30, 64 << 30), "18 of 64 GiB");
        assert_eq!(used_of(512 << 20, 64 << 30), "512 MiB of 64 GiB");
    }

    #[test]
    fn percentages() {
        assert_eq!(percent(23.4), "23%");
        assert_eq!(percent(140.0), "100%");
    }
}
