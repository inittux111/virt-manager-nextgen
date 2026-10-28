//! Keyboard translation: Linux evdev key codes (what GTK reports as the
//! hardware key code, minus 8) to PC/XT set 1 scan codes, which SPICE and
//! QEMU's VNC server expect.

/// Bit marking a scan code that is sent with an `E0` prefix.
pub const EXTENDED: u32 = 0x100;

/// Translates an evdev key code to an XT set 1 scan code. Extended keys
/// carry [`EXTENDED`]. Returns `None` for keys without a scan code.
pub fn evdev_to_xt(code: u32) -> Option<u32> {
    let xt = match code {
        // KEY_ESC (1) … KEY_KPDOT (83) map one to one.
        1..=83 => code,
        86 => 0x56,             // KEY_102ND
        87 => 0x57,             // F11
        88 => 0x58,             // F12
        89 => 0x73,             // KEY_RO
        92 => 0x79,             // KEY_HENKAN
        93 => 0x70,             // KEY_KATAKANAHIRAGANA
        94 => 0x7b,             // KEY_MUHENKAN
        96 => EXTENDED | 0x1c,  // KP Enter
        97 => EXTENDED | 0x1d,  // Right Ctrl
        98 => EXTENDED | 0x35,  // KP /
        99 => EXTENDED | 0x37,  // SysRq / Print Screen
        100 => EXTENDED | 0x38, // Right Alt (AltGr)
        102 => EXTENDED | 0x47, // Home
        103 => EXTENDED | 0x48, // Up
        104 => EXTENDED | 0x49, // Page Up
        105 => EXTENDED | 0x4b, // Left
        106 => EXTENDED | 0x4d, // Right
        107 => EXTENDED | 0x4f, // End
        108 => EXTENDED | 0x50, // Down
        109 => EXTENDED | 0x51, // Page Down
        110 => EXTENDED | 0x52, // Insert
        111 => EXTENDED | 0x53, // Delete
        113 => EXTENDED | 0x20, // Mute
        114 => EXTENDED | 0x2e, // Volume down
        115 => EXTENDED | 0x30, // Volume up
        116 => EXTENDED | 0x5e, // Power
        117 => 0x59,            // KP =
        119 => EXTENDED | 0x46, // Pause (as sent by Ctrl+Break)
        121 => 0x7e,            // KP ,
        124 => 0x7d,            // KEY_YEN
        125 => EXTENDED | 0x5b, // Left Super
        126 => EXTENDED | 0x5c, // Right Super
        127 => EXTENDED | 0x5d, // Menu
        _ => return None,
    };
    Some(xt)
}

/// The scan code in QEMU's "qnum" form used by the VNC extended key event:
/// extended keys are `0x80 | code`.
pub fn xt_to_qnum(xt: u32) -> u32 {
    if xt & EXTENDED != 0 { 0x80 | (xt & 0x7f) } else { xt }
}

/// evdev codes of keys used in key combinations.
pub mod keys {
    pub const ESC: u32 = 1;
    pub const BACKSPACE: u32 = 14;
    pub const TAB: u32 = 15;
    pub const LEFT_CTRL: u32 = 29;
    pub const LEFT_SHIFT: u32 = 42;
    pub const LEFT_ALT: u32 = 56;
    pub const F1: u32 = 59;
    pub const DELETE: u32 = 111;
    pub const PRINT: u32 = 99;
    pub const LEFT_SUPER: u32 = 125;

    /// F1…F12 by number.
    pub fn f(n: u32) -> u32 {
        match n {
            1..=10 => F1 + n - 1,
            11 => 87,
            12 => 88,
            _ => F1,
        }
    }
}

/// The X keysym for an evdev code, needed by VNC servers without the
/// extended key event. Only keys used in combinations are covered; regular
/// typing passes the keysym from GTK instead.
pub fn evdev_to_keysym(code: u32) -> u32 {
    match code {
        keys::ESC => 0xff1b,
        keys::BACKSPACE => 0xff08,
        keys::TAB => 0xff09,
        keys::LEFT_CTRL => 0xffe3,
        keys::LEFT_SHIFT => 0xffe1,
        keys::LEFT_ALT => 0xffe9,
        keys::DELETE => 0xffff,
        keys::PRINT => 0xff61,
        keys::LEFT_SUPER => 0xffeb,
        c if (59..=68).contains(&c) => 0xffbe + (c - 59),
        87 => 0xffc8,
        88 => 0xffc9,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_keys() {
        assert_eq!(evdev_to_xt(1), Some(0x01)); // Esc
        assert_eq!(evdev_to_xt(30), Some(0x1e)); // A
        assert_eq!(evdev_to_xt(103), Some(EXTENDED | 0x48)); // Up
        assert_eq!(evdev_to_xt(111), Some(EXTENDED | 0x53)); // Delete
        assert_eq!(evdev_to_xt(200), None);
        assert_eq!(xt_to_qnum(EXTENDED | 0x48), 0xc8);
        assert_eq!(xt_to_qnum(0x1e), 0x1e);
        assert_eq!(keys::f(12), 88);
        assert_eq!(evdev_to_keysym(keys::f(1)), 0xffbe);
    }
}
