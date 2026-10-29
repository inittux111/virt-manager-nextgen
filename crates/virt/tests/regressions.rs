//! Regression tests: one per bug that was found and fixed, so it can't come
//! back unnoticed. Each test names the bug it guards against. Run by
//! `cargo test` (locally in ./test-pipeline.sh and on GitHub).
//!
//! Bugs that only show in the running app are guarded by checks in
//! tests/suite.sh whose names start with `regression-`.

use vmng_virt::{
    MemorySize, SnapshotLocation, SshTarget, VolumeOptions, check_network_xml, display_address, normalize_mac,
    set_memory, set_video_3d, validate_snapshot_name_for, video_3d, volume_xml,
};
use vmng_xml::{Element, Node};

fn xml(s: &str) -> Element {
    vmng_xml::parse(s).unwrap()
}

/// 0.3.0: a snapshot of a running machine with the suggested name failed
/// with "Invalid job ID": QEMU job ids can't contain spaces.
#[test]
fn running_internal_snapshot_names_without_spaces() {
    assert!(validate_snapshot_name_for("Before update", true, SnapshotLocation::Internal).is_err());
    assert!(validate_snapshot_name_for("Before-update", true, SnapshotLocation::Internal).is_ok());
    // Stopped machines and separate files have no such limit.
    assert!(validate_snapshot_name_for("Before update", false, SnapshotLocation::Internal).is_ok());
    assert!(validate_snapshot_name_for("Before update", true, SnapshotLocation::External).is_ok());
}

/// 0.3.0: turning 3D acceleration off left the extra display behind, and
/// editors bound to a device position jumped to another device.
#[test]
fn video_3d_keeps_device_positions() {
    let mut d = xml("<domain><devices><graphics type='spice'/><video><model type='virtio'/></video>\
                     <input type='tablet'/></devices></domain>");
    let pos = |d: &Element, name: &str| {
        d.child("devices").unwrap().children.iter().position(|n| matches!(n, Node::Element(e) if e.name == name))
    };
    let (video, input) = (pos(&d, "video").unwrap(), pos(&d, "input"));
    set_video_3d(&mut d, video, true);
    set_video_3d(&mut d, video, false);
    assert!(!video_3d(d.find("devices/video").unwrap()));
    assert!(!d.to_xml().contains("egl-headless"));
    assert_eq!(pos(&d, "input"), input);
}

/// 0.4.0: "Allocate the whole size now" did nothing for qcow2 images
/// (allocation must equal the size; the preallocation flag is set when
/// creating, see net_pool_check).
#[test]
fn fully_allocated_volume_asks_for_its_whole_size() {
    let v = volume_xml("a.qcow2", 1 << 30, "qcow2", true, None);
    assert_eq!(v.find_text("allocation").as_deref(), Some("1073741824"));
    assert!(!VolumeOptions::default().allocate);
}

/// Current memory above the maximum is refused by libvirt: the maximum
/// follows the current size up.
#[test]
fn memory_maximum_never_below_current() {
    let mut d = xml("<domain><memory unit='MiB'>1024</memory></domain>");
    set_memory(&mut d, MemorySize { current: 4 << 30, maximum: 2 << 30 });
    let m = MemorySize::of(&d);
    assert!(m.maximum >= m.current);
}

/// A group (multicast) MAC address makes the network card unusable.
#[test]
fn mac_address_must_be_a_card_address() {
    assert!(normalize_mac("01:00:5e:00:00:01").is_err());
    assert_eq!(normalize_mac("52-54-00-AA-BB-CC").unwrap(), "52:54:00:aa:bb:cc");
}

/// Edited network XML must not rename the network or change its UUID
/// (that would create a second network).
#[test]
fn network_xml_keeps_name_and_uuid() {
    let cur = xml("<network><name>n</name><uuid>u</uuid></network>");
    assert!(check_network_xml(&cur, &xml("<network><name>m</name><uuid>u</uuid></network>")).is_err());
    assert!(check_network_xml(&cur, &xml("<network><name>n</name><uuid>v</uuid></network>")).is_err());
}

/// A computer name starting with "-" would be read by ssh as an option.
#[test]
fn ssh_names_cant_become_options() {
    assert!(SshTarget::parse("qemu+ssh://-oProxyCommand=x/system").is_none());
    let t = SshTarget { host: "-oProxyCommand=x".into(), ..Default::default() };
    assert!(t.validate().is_err());
}

/// A display listening on every address is reached on 127.0.0.1 through
/// the SSH tunnel (not "0.0.0.0").
#[test]
fn tunnel_goes_to_a_reachable_address() {
    let d = xml("<domain><devices><graphics type='vnc' port='5900' listen='0.0.0.0'/></devices></domain>");
    assert_eq!(display_address(&d, 0).as_deref(), Some("127.0.0.1:5900"));
}

/// 1.0.0: a comment holding "----->" was written as "<!--- -- -->-->", so
/// it ended early and the rest became part of the document; a trailing '-'
/// gave "--->", which libvirt rejects. (Found by crates/xml/tests/fuzz.rs.)
/// The VNC/SPICE size overflow found in the same review is guarded in
/// vmng-display (`screen::tests::pixel_bytes_rejects_bad_sizes`).
#[test]
fn comments_are_written_as_valid_xml() {
    for text in ["----->", "a-", "x --><evil/><!--"] {
        let mut d = Element::new("domain").with_child(Element::new("name").with_text("n"));
        d.children.push(Node::Comment(text.into()));
        let written = d.to_xml();
        let body = written.split("<!--").nth(1).and_then(|c| c.split("-->").next()).unwrap();
        assert!(!body.contains("--") && !body.ends_with('-'), "{written}");
        assert_eq!(xml(&written).elements().count(), 1, "{written}");
    }
}
