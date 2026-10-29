//! A machine's memory: what it has now (`<currentMemory>`) and the most it
//! may get while running (`<memory>`). The difference is handed over by the
//! guest's balloon driver.

use vmng_xml::Element;

use crate::units::{self, KIB};

/// Memory sizes in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemorySize {
    pub current: u64,
    pub maximum: u64,
}

fn bytes(e: Option<&Element>) -> Option<u64> {
    let m = e?;
    Some(units::to_bytes(m.text().trim().parse().ok()?, m.attr("unit"), KIB))
}

impl MemorySize {
    pub fn of(domain: &Element) -> Self {
        let maximum = bytes(domain.child("memory")).unwrap_or(1 << 30);
        let current = bytes(domain.child("currentMemory")).unwrap_or(maximum).min(maximum);
        Self { current, maximum }
    }
}

/// Sets the current and maximum memory (the maximum is raised to at least
/// the current size).
pub fn set_memory(domain: &mut Element, size: MemorySize) {
    let maximum = size.maximum.max(size.current);
    for (name, value) in [("memory", maximum), ("currentMemory", size.current)] {
        let m = domain.child_or_insert(name);
        m.set_attr("unit", "KiB");
        m.set_text((value / KIB).to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_both_sizes() {
        let mut d = vmng_xml::parse(
            "<domain><memory unit='GiB'>8</memory><currentMemory unit='KiB'>4194304</currentMemory></domain>",
        )
        .unwrap();
        assert_eq!(MemorySize::of(&d), MemorySize { current: 4 << 30, maximum: 8 << 30 });
        set_memory(&mut d, MemorySize { current: 6 << 30, maximum: 2 << 30 });
        assert_eq!(MemorySize::of(&d), MemorySize { current: 6 << 30, maximum: 6 << 30 });
        let only_max = vmng_xml::parse("<domain><memory unit='MiB'>2048</memory></domain>").unwrap();
        assert_eq!(MemorySize::of(&only_max), MemorySize { current: 2 << 30, maximum: 2 << 30 });
    }
}
