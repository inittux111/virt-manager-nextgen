//! 3D acceleration for a VirtIO video card.
//!
//! The guest draws with the host's graphics card (virgl). QEMU needs a GPU
//! display for that: SPICE with OpenGL hands the image over as GPU
//! buffers, which a client can only show when it knows the buffer's memory
//! layout, and SPICE's client library doesn't pass it on. So this adds an
//! `egl-headless` display instead: QEMU renders on the GPU and sends the
//! finished image over the normal SPICE or VNC connection, which every
//! client shows.

use vmng_xml::{Element, Node};

/// Whether a video device has 3D acceleration on.
pub fn video_3d(video: &Element) -> bool {
    video.find_attr("model/acceleration", "accel3d") == Some("yes")
}

fn is_gl_display(e: &Element) -> bool {
    e.name == "graphics"
        && (e.attr("type") == Some("egl-headless") || e.children_named("gl").any(|g| g.attr("enable") == Some("yes")))
}

/// Turns 3D acceleration on or off for the video device at position
/// `video` of `<devices>`. Other devices keep their positions (a removed
/// display leaves an empty text node behind, which writes as nothing), so
/// editors bound to a position stay on their device.
pub fn set_video_3d(domain: &mut Element, video: usize, on: bool) {
    let Some(devices) = domain.child_mut("devices") else { return };
    let Some(Node::Element(v)) = devices.children.get_mut(video) else { return };
    let model = v.child_or_insert("model");
    model.remove_children(|c| c.name == "acceleration");
    if on {
        model.push(Element::new("acceleration").with_attr("accel3d", "yes"));
    }
    let others = devices.elements().filter(|e| e.name == "video").any(video_3d);
    let headless =
        devices.children.iter().position(|n| matches!(n, Node::Element(e) if e.attr("type") == Some("egl-headless")));
    let gl_display = devices.elements().any(is_gl_display);
    match (others, headless) {
        (true, None) if !gl_display => {
            devices.push(Element::new("graphics").with_attr("type", "egl-headless"));
        }
        (false, Some(i)) => devices.children[i] = Node::Text(String::new()),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOM: &str = "<domain><devices><graphics type='spice'/><video><model type='virtio'/></video>\
                       <input type='tablet'/></devices></domain>";

    fn position(d: &Element, name: &str) -> Option<usize> {
        d.child("devices")?.children.iter().position(|n| matches!(n, Node::Element(e) if e.name == name))
    }

    #[test]
    fn adds_and_removes_the_gpu_display() {
        let mut d = vmng_xml::parse(DOM).unwrap();
        let video = position(&d, "video").unwrap();
        let input = position(&d, "input").unwrap();
        set_video_3d(&mut d, video, true);
        assert!(video_3d(d.find("devices/video").unwrap()));
        let types: Vec<_> =
            d.child("devices").unwrap().children_named("graphics").filter_map(|g| g.attr("type")).collect();
        assert_eq!(types, ["spice", "egl-headless"]);
        assert_eq!(position(&d, "input"), Some(input), "positions stay");

        set_video_3d(&mut d, video, false);
        assert!(!video_3d(d.find("devices/video").unwrap()));
        assert_eq!(d.child("devices").unwrap().children_named("graphics").count(), 1);
        assert_eq!(position(&d, "input"), Some(input));
        assert!(!d.to_xml().contains("egl-headless"));
    }

    #[test]
    fn spice_opengl_needs_no_extra_display() {
        let mut d = vmng_xml::parse(
            "<domain><devices><graphics type='spice'><gl enable='yes'/></graphics>\
             <video><model type='virtio'/></video></devices></domain>",
        )
        .unwrap();
        let video = position(&d, "video").unwrap();
        set_video_3d(&mut d, video, true);
        assert!(!d.to_xml().contains("egl-headless"));
    }
}
