//! Choosing a machine's boot firmware: BIOS, UEFI picked by libvirt, or a
//! specific UEFI firmware file (as virt-manager offers them).

use vmng_xml::Element;

/// A firmware choice, as written into the domain XML.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FirmwareChoice {
    Bios,
    /// libvirt picks a matching UEFI build (`<os firmware='efi'>`).
    Uefi {
        secure_boot: bool,
    },
    /// A specific firmware file (`<loader>`), e.g. one of
    /// [`DomainCaps::loaders`](crate::DomainCaps::loaders).
    Loader(String),
}

/// Whether a firmware file is a Secure Boot build (by its name, which is
/// how distributions mark them, e.g. `OVMF_CODE_4M.secboot.qcow2`).
pub fn loader_is_secure(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.contains("secboot") || name.contains(".secure")
}

impl FirmwareChoice {
    /// The firmware a domain is set up with.
    pub fn of(domain: &Element) -> Self {
        let Some(os) = domain.child("os") else { return Self::Bios };
        if os.attr("firmware") == Some("efi") {
            let secure_boot = os.child("firmware").is_some_and(|f| {
                f.children_named("feature")
                    .any(|x| x.attr("name") == Some("secure-boot") && x.attr("enabled") == Some("yes"))
            }) || os.find_attr("loader", "secure") == Some("yes");
            return Self::Uefi { secure_boot };
        }
        match os.child("loader").map(|l| l.text().trim().to_owned()) {
            Some(path) if !path.is_empty() => Self::Loader(path),
            _ => Self::Bios,
        }
    }

    pub fn secure_boot(&self) -> bool {
        match self {
            Self::Bios => false,
            Self::Uefi { secure_boot } => *secure_boot,
            Self::Loader(path) => loader_is_secure(path),
        }
    }

    /// The label virt-manager uses, e.g. "UEFI x86_64: /usr/share/…".
    pub fn label(&self, arch: &str) -> String {
        match self {
            Self::Bios => "BIOS".into(),
            Self::Uefi { .. } => "UEFI".into(),
            Self::Loader(path) => format!("UEFI {arch}: {path}"),
        }
    }
}

/// Sets the domain's firmware. A previous firmware's NVRAM (variable store)
/// reference is dropped, so libvirt makes one that fits the new firmware.
pub fn set_firmware(domain: &mut Element, choice: &FirmwareChoice) {
    let os = domain.child_or_insert("os");
    os.remove_attr("firmware");
    os.remove_children(|c| matches!(c.name.as_str(), "loader" | "nvram" | "firmware"));
    match choice {
        FirmwareChoice::Bios => {}
        FirmwareChoice::Uefi { secure_boot } => {
            os.set_attr("firmware", "efi");
            let yes_no = if *secure_boot { "yes" } else { "no" };
            os.push(
                Element::new("firmware")
                    .with_child(Element::new("feature").with_attr("enabled", yes_no).with_attr("name", "enrolled-keys"))
                    .with_child(Element::new("feature").with_attr("enabled", yes_no).with_attr("name", "secure-boot")),
            );
        }
        FirmwareChoice::Loader(path) => {
            let mut loader = Element::new("loader").with_attr("readonly", "yes").with_attr("type", "pflash");
            if loader_is_secure(path) {
                loader.set_attr("secure", "yes");
            }
            if path.ends_with(".qcow2") {
                loader.set_attr("format", "qcow2");
            }
            os.push(loader.with_text(path.as_str()));
        }
    }
    // A Secure Boot firmware file needs SMM; libvirt adds it itself when
    // it picks the firmware.
    if matches!(choice, FirmwareChoice::Loader(p) if loader_is_secure(p)) {
        domain.child_or_insert("features").child_or_insert("smm").set_attr("state", "on");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dom(os: &str) -> Element {
        vmng_xml::parse(&format!("<domain><os><type arch='x86_64'>hvm</type>{os}</os></domain>")).unwrap()
    }

    #[test]
    fn reads_the_current_choice() {
        assert_eq!(FirmwareChoice::of(&dom("")), FirmwareChoice::Bios);
        let auto = vmng_xml::parse(
            "<domain><os firmware='efi'><firmware><feature enabled='yes' name='secure-boot'/></firmware></os></domain>",
        )
        .unwrap();
        assert_eq!(FirmwareChoice::of(&auto), FirmwareChoice::Uefi { secure_boot: true });
        let file = "/usr/share/edk2/ovmf/OVMF_CODE_4M.secboot.qcow2";
        let manual = dom(&format!("<loader readonly='yes' type='pflash'>{file}</loader><nvram>/x</nvram>"));
        assert_eq!(FirmwareChoice::of(&manual), FirmwareChoice::Loader(file.into()));
    }

    #[test]
    fn writes_a_firmware_file() {
        let mut d = dom("<nvram>/var/lib/libvirt/qemu/nvram/old_VARS.fd</nvram>");
        let file = "/usr/share/edk2/ovmf/OVMF_CODE_4M.secboot.qcow2";
        set_firmware(&mut d, &FirmwareChoice::Loader(file.into()));
        assert_eq!(d.find_text("os/loader").as_deref(), Some(file));
        assert_eq!(d.find_attr("os/loader", "secure"), Some("yes"));
        assert_eq!(d.find_attr("os/loader", "format"), Some("qcow2"));
        assert!(d.find("os/nvram").is_none(), "the old variable store is dropped");
        assert_eq!(d.find_attr("features/smm", "state"), Some("on"));
        assert_eq!(FirmwareChoice::of(&d), FirmwareChoice::Loader(file.into()));

        set_firmware(&mut d, &FirmwareChoice::Bios);
        assert!(d.find("os/loader").is_none());
        assert_eq!(FirmwareChoice::of(&d), FirmwareChoice::Bios);

        set_firmware(&mut d, &FirmwareChoice::Uefi { secure_boot: false });
        assert_eq!(d.find_attr("os", "firmware"), Some("efi"));
        assert_eq!(FirmwareChoice::of(&d), FirmwareChoice::Uefi { secure_boot: false });
    }

    #[test]
    fn plain_files_are_not_secure() {
        assert!(!loader_is_secure("/usr/share/edk2/ovmf/OVMF_CODE.fd"));
        assert!(loader_is_secure("/usr/share/OVMF/OVMF_CODE_4M.secboot.fd"));
    }
}
