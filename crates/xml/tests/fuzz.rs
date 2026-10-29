//! Randomized robustness tests for the parser and writer.
//!
//! libvirt XML reaches the parser from remote hosts and from the XML editors,
//! so it must never panic, and whatever the writer produces must parse back.
//! The inputs come from a seeded generator (no fuzzing crate needed), so a
//! failure names the seed that reproduces it. `VMNG_FUZZ_ITERS=<n>` runs
//! longer, `VMNG_FUZZ_SEED=<n>` starts at another seed.

use vmng_xml::{Element, Node, parse};

/// xorshift64*: small, fast and good enough to shuffle test inputs.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

fn iterations(default: u64) -> u64 {
    std::env::var("VMNG_FUZZ_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn first_seed() -> u64 {
    std::env::var("VMNG_FUZZ_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(1)
}

const DOMAIN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- written by libvirt -->
<domain type='kvm' id='3' xmlns:qemu='http://libvirt.org/schemas/domain/qemu/1.0'>
  <name>fedora &amp; friends</name>
  <uuid>6d1c4b0e-8f0f-4a8e-9a6e-2f6d1d2f1a11</uuid>
  <title>Test &lt;VM&gt;</title>
  <description><![CDATA[line one
line two ]] > & <tags>]]></description>
  <metadata><app:info xmlns:app="urn:x">é ü 日本 &#x1F600;</app:info></metadata>
  <memory unit='KiB'>4194304</memory>
  <vcpu placement="static">4</vcpu>
  <os firmware='efi'><type arch='x86_64' machine='pc-q35-9.2'>hvm</type><boot dev='hd'/></os>
  <cpu mode='host-passthrough' check='none'/>
  <devices>
    <emulator>/usr/bin/qemu-system-x86_64</emulator>
    <disk type='file' device='disk'>
      <driver name='qemu' type='qcow2' discard='unmap'/>
      <source file='/var/lib/libvirt/images/a b.qcow2'/>
      <target dev='vda' bus='virtio'/>
    </disk>
    <interface type='network'><mac address='52:54:00:12:34:56'/><source network='default'/></interface>
    <graphics type='spice' autoport='yes'><listen type='address'/></graphics>
    <!-- a comment inside devices -->
  </devices>
  <qemu:commandline><qemu:arg value='-foo'/></qemu:commandline>
</domain>
"#;

/// Fragments that sit on the parser's edge cases.
const TOKENS: &[&str] = &[
    "<",
    ">",
    "/>",
    "</",
    "&",
    ";",
    "'",
    "\"",
    "=",
    " ",
    "\n",
    "\t",
    "\r",
    "]]>",
    "<!--",
    "-->",
    "--",
    "<![CDATA[",
    "<?",
    "?>",
    "<!DOCTYPE x>",
    "<!ENTITY",
    "&amp;",
    "&lt;",
    "&#0;",
    "&#x0;",
    "&#xD800;",
    "&#1114112;",
    "&#xFFFFFFFFF;",
    "&#;",
    "&#x;",
    "&unknown;",
    "&aaaaaaaaaaaaaaaaaaa;",
    "\u{feff}",
    "é",
    "日本",
    "\u{0}",
    "<a>",
    "</a>",
    "<a/>",
    "<domain>",
    "</domain>",
    "a='1'",
    "a=\"1\"",
    ":",
    "-",
    ".",
    "1",
    "x:y",
];

/// Changes `doc` a few times at random places, keeping it valid UTF-8.
fn mutate(rng: &mut Rng, doc: &str) -> String {
    let mut s = doc.to_owned();
    for _ in 0..=rng.below(4) {
        let boundaries: Vec<usize> = (0..=s.len()).filter(|&i| s.is_char_boundary(i)).collect();
        let at = *rng.pick(&boundaries);
        let to = *rng.pick(&boundaries);
        let (a, b) = (at.min(to), at.max(to));
        // Ranges of at most `n` bytes, ending on a character boundary.
        let upto = |n: usize| boundaries.iter().copied().rfind(|&i| i <= b.min(a + n)).unwrap_or(a);
        match rng.below(6) {
            0 => s.insert_str(at, *rng.pick(TOKENS)),
            1 => s.replace_range(a..upto(16), ""),
            2 => s.truncate(at),
            3 => {
                let piece = s[a..upto(64)].to_owned();
                s.insert_str(to, &piece);
            }
            4 => {
                if let Some(c) = s[at..].chars().next() {
                    s.replace_range(at..at + c.len_utf8(), *rng.pick(TOKENS));
                }
            }
            _ => {
                let c = char::from_u32(rng.next() as u32 % 0x11_0000).unwrap_or('\u{fffd}');
                s.insert(at, c);
            }
        }
    }
    s
}

/// Whatever parses must be written as XML that parses to the same tree, and
/// writing that again must give the same text (one pass normalizes layout).
fn check_round_trip(doc: &Element, context: &dyn Fn() -> String) {
    let written = doc.to_xml();
    let again = parse(&written).unwrap_or_else(|e| panic!("written XML doesn't parse ({e}): {}\n{written}", context()));
    let rewritten = again.to_xml();
    let third = parse(&rewritten).expect("rewritten XML parses");
    assert_eq!(third.to_xml(), rewritten, "writing isn't stable: {}", context());
}

#[test]
fn mutated_documents_never_panic_and_round_trip() {
    parse(DOMAIN).expect("the sample is valid");
    let start = first_seed();
    for seed in start..start + iterations(20_000) {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let input = mutate(&mut rng, DOMAIN);
        if let Ok(doc) = parse(&input) {
            check_round_trip(&doc, &|| format!("seed {seed}"));
        }
    }
}

#[test]
fn random_bytes_never_panic() {
    let start = first_seed();
    for seed in start..start + iterations(20_000) {
        let mut rng = Rng(seed | 1);
        let len = rng.below(200);
        let bytes: Vec<u8> = (0..len)
            .map(|_| match rng.below(4) {
                0 => *rng.pick(b"<>/='\"&;!?[]- \n#x"),
                _ => rng.next() as u8,
            })
            .collect();
        let input = String::from_utf8_lossy(&bytes);
        if let Ok(doc) = parse(&input) {
            check_round_trip(&doc, &|| format!("seed {seed}"));
        }
    }
}

/// Text that is hard to write correctly: markup characters, entity-like
/// sequences, CDATA and comment terminators, whitespace and non-ASCII.
fn nasty_text(rng: &mut Rng) -> String {
    const PARTS: &[&str] = &[
        "plain",
        " ",
        "  ",
        "\n",
        "\t",
        "<",
        ">",
        "&",
        "&amp;",
        "'",
        "\"",
        "]]>",
        "]]",
        "--",
        "-",
        "-->",
        "<!--",
        "é",
        "日本",
        "\u{1F600}",
        "a b",
        "=",
    ];
    (0..rng.below(6)).map(|_| *rng.pick(PARTS)).collect()
}

fn random_element(rng: &mut Rng, depth: usize) -> Element {
    const NAMES: &[&str] = &["domain", "devices", "disk", "a", "x:y", "qemu:arg", "_n", "n-1.b", "é"];
    let mut el = Element::new(*rng.pick(NAMES));
    for i in 0..rng.below(3) {
        el.set_attr(&format!("{}{i}", rng.pick(NAMES)), nasty_text(rng));
    }
    if depth < 6 {
        for _ in 0..rng.below(4) {
            let node = match rng.below(5) {
                0 | 1 => Node::Element(random_element(rng, depth + 1)),
                2 => Node::Text(nasty_text(rng)),
                3 => Node::CData(nasty_text(rng)),
                _ => Node::Comment(nasty_text(rng)),
            };
            el.children.push(node);
        }
    }
    el
}

#[test]
fn built_documents_write_valid_xml() {
    let start = first_seed();
    for seed in start..start + iterations(5_000) {
        let mut rng = Rng(seed | 1);
        let doc = random_element(&mut rng, 0);
        check_round_trip(&doc, &|| format!("seed {seed}: {doc:?}"));
    }
}

#[test]
fn values_survive_a_round_trip() {
    let mut rng = Rng(7);
    for _ in 0..iterations(5_000) {
        let value = nasty_text(&mut rng);
        let el = Element::new("a").with_attr("v", value.clone()).with_text(value.clone());
        let back = parse(&el.to_xml()).expect("parses");
        assert_eq!(back.attr("v"), Some(value.as_str()), "attribute {value:?}");
        assert_eq!(back.text(), value.trim(), "text {value:?}");

        // CDATA keeps its exact content, also next to child elements.
        let mut el = Element::new("a").with_child(Element::new("k"));
        el.children.push(Node::CData(value.clone()));
        let back = parse(&el.to_xml()).expect("parses");
        let cdata: String =
            back.children.iter().filter_map(|n| if let Node::CData(t) = n { Some(t.as_str()) } else { None }).collect();
        assert_eq!(cdata, value, "CDATA {value:?}");

        // Comments keep their content; "--" (not allowed in comments) is
        // written as "- -", and a trailing '-' gets a space.
        // Regression: "----->" was written as "<!--- -- -->-->", ending the
        // comment early.
        let mut el = Element::new("a").with_child(Element::new("k"));
        el.children.push(Node::Comment(value.clone()));
        let back = parse(&el.to_xml()).expect("parses");
        let comment = back.children.iter().find_map(|n| if let Node::Comment(t) = n { Some(t.clone()) } else { None });
        let mut expected = value.clone();
        while expected.contains("--") {
            expected = expected.replace("--", "- -");
        }
        if expected.ends_with('-') {
            expected.push(' ');
        }
        assert_eq!(comment.as_deref(), Some(expected.as_str()), "comment {value:?}");
    }
}
