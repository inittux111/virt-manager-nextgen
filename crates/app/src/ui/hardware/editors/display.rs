//! Editors for the display devices: graphics (SPICE/VNC) and video.

use super::*;

/// The graphical display (SPICE/VNC).
pub fn graphics(bind: &Bind) -> gtk::Box {
    let b = vbox();
    let g = group("Display");
    g.add(&combo(
        bind,
        "Protocol",
        &[("spice", "SPICE (recommended)"), ("vnc", "VNC")],
        |e| e.attr("type").unwrap_or_default().to_owned(),
        |e, v| {
            e.set_attr("type", v);
            // Protocol-specific settings don't carry over.
            e.remove_children(|c| {
                matches!(
                    c.name.as_str(),
                    "image" | "streaming" | "clipboard" | "mouse" | "filetransfer" | "gl" | "channel"
                )
            });
            e.remove_attr("tlsPort");
        },
    ));
    g.add(&combo(
        bind,
        "Reachable from",
        &[("127.0.0.1", "This computer only"), ("0.0.0.0", "Any network (less safe)"), ("none", "Local socket only")],
        |e| match e.find_attr("listen", "type") {
            Some("none") => "none".into(),
            _ => e.find_attr("listen", "address").or(e.attr("listen")).unwrap_or("127.0.0.1").to_owned(),
        },
        |e, v| {
            e.remove_children(|c| c.name == "listen");
            e.remove_attr("listen");
            let listen = if v == "none" {
                Element::new("listen").with_attr("type", "none")
            } else {
                e.set_attr("listen", v);
                Element::new("listen").with_attr("type", "address").with_attr("address", v)
            };
            e.push(listen);
        },
    ));
    g.add(&switch(
        bind,
        "Choose port automatically",
        "Recommended when several machines run",
        |e| e.attr("autoport") != Some("no"),
        |e, on| {
            e.set_attr("autoport", if on { "yes" } else { "no" });
            if on {
                e.remove_attr("port");
            } else if e.attr("port").is_none_or(|p| p == "-1") {
                e.set_attr("port", "5900");
            }
        },
    ));
    g.add(&spin(
        bind,
        "Port (when not automatic)",
        (5900.0, 65535.0, 1.0, 0),
        |e| e.attr("port").and_then(|p| p.parse().ok()).filter(|&p: &f64| p > 0.0).unwrap_or(5900.0),
        |e, v| {
            if e.attr("autoport") == Some("no") {
                e.set_attr("port", (v as u32).to_string());
            }
        },
    ));
    let vnc = bind.get(|e| e.attr("type") == Some("vnc"));
    let password = adw::PasswordEntryRow::builder()
        .title(if vnc { "Password (optional, up to 8 characters)" } else { "Password (optional)" })
        .text(bind.get(|e| e.attr("passwd").unwrap_or_default().to_owned()))
        .build();
    let bind2 = bind.clone();
    password.connect_changed(move |r| {
        let text = r.text().to_string();
        // VNC passwords are at most 8 characters; QEMU won't start with a
        // longer one, so it isn't saved until it fits.
        let too_long = bind2.get(|e| e.attr("type") == Some("vnc")) && text.chars().count() > VNC_PASSWORD_MAX;
        if too_long {
            r.add_css_class("error");
            return;
        }
        r.remove_css_class("error");
        bind2.set(|e| {
            if text.is_empty() {
                e.remove_attr("passwd");
            } else {
                e.set_attr("passwd", text);
            }
        });
    });
    g.add(&password);
    b.append(&g);
    b
}

/// The longest password VNC accepts.
pub(super) const VNC_PASSWORD_MAX: usize = 8;

pub fn video(bind: &Bind) -> gtk::Box {
    let b = vbox();
    let g = group("Video");
    let model = combo(
        bind,
        "Model",
        &[
            ("virtio", "VirtIO (recommended for Linux)"),
            ("qxl", "QXL"),
            ("vga", "VGA"),
            ("bochs", "Bochs"),
            ("ramfb", "ramfb"),
            ("cirrus", "Cirrus (very old systems)"),
            ("none", "None"),
        ],
        |e| e.find_attr("model", "type").unwrap_or_default().to_owned(),
        |e, v| {
            let m = e.child_or_insert("model");
            *m = Element::new("model").with_attr("type", v);
            drop_address(e);
        },
    );
    g.add(&model);
    let Target::Device(index) = bind.target else { return b };
    let accel = adw::SwitchRow::builder()
        .title("3D acceleration")
        .subtitle("Draws with the host's graphics card: smoother Linux desktops. VirtIO only")
        .active(bind.get(video_3d))
        .build();
    let is_virtio = |b: &Bind| b.get(|e| e.find_attr("model", "type") == Some("virtio"));
    accel.set_sensitive(is_virtio(bind));
    {
        let bind = bind.clone();
        accel.connect_active_notify(move |r| {
            let on = r.is_active();
            bind.set_domain(|d| set_video_3d(d, index, on));
        });
    }
    {
        // Another model has no 3D acceleration.
        let (bind, accel) = (bind.clone(), accel.clone());
        model.connect_selected_notify(move |_| {
            let virtio = is_virtio(&bind);
            accel.set_sensitive(virtio);
            if !virtio {
                accel.set_active(false);
                bind.set_domain(|d| set_video_3d(d, index, false));
            }
        });
    }
    g.add(&accel);
    b.append(&g);
    b
}
