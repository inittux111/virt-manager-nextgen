//! Size units as used in libvirt XML.

/// Converts a libvirt `unit` attribute plus value to bytes.
///
/// libvirt accepts `b`/`bytes`, `KB` (1000), `k`/`KiB` (1024), and so on up
/// to exbibytes. A missing unit means `default` (KiB for memory, bytes for
/// storage), which the caller supplies.
pub fn to_bytes(value: u64, unit: Option<&str>, default_multiplier: u64) -> u64 {
    let mult = match unit.map(str::trim) {
        None | Some("") => default_multiplier,
        Some(u) => match u {
            "b" | "bytes" => 1,
            "KB" => 1000,
            "k" | "K" | "KiB" => 1 << 10,
            "MB" => 1000_u64.pow(2),
            "M" | "MiB" => 1 << 20,
            "GB" => 1000_u64.pow(3),
            "G" | "GiB" => 1 << 30,
            "TB" => 1000_u64.pow(4),
            "T" | "TiB" => 1 << 40,
            "PB" => 1000_u64.pow(5),
            "P" | "PiB" => 1 << 50,
            "EB" => 1000_u64.pow(6),
            "E" | "EiB" => 1 << 60,
            _ => default_multiplier,
        },
    };
    value.saturating_mul(mult)
}

pub const KIB: u64 = 1 << 10;
pub const MIB: u64 = 1 << 20;
pub const GIB: u64 = 1 << 30;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units() {
        assert_eq!(to_bytes(2, None, KIB), 2048);
        assert_eq!(to_bytes(2, Some("GiB"), KIB), 2 * GIB);
        assert_eq!(to_bytes(1, Some("GB"), 1), 1_000_000_000);
        assert_eq!(to_bytes(5, Some("bytes"), KIB), 5);
        assert_eq!(to_bytes(u64::MAX, Some("EiB"), 1), u64::MAX);
    }
}
