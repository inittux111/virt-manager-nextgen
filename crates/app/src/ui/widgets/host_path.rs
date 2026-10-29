//! Choosing a file or folder on the computer the machines run on.

use adw::prelude::*;
use gtk::{gio, glib};

use super::FormDialog;

/// Lets the person pick a file (or a folder) where the machines run. On
/// this computer that is the file chooser; on another computer (`remote` is
/// its name) this computer's files aren't there, so the path is typed in.
pub fn pick_host_path(
    anchor: &gtk::Widget,
    remote: Option<&str>,
    folder: bool,
    title: &str,
    filter: Option<gtk::FileFilter>,
    done: impl Fn(String) + 'static,
) {
    if let Some(host) = remote {
        let form = FormDialog::new(title, "Choose");
        let group = adw::PreferencesGroup::builder()
            .description(format!("Enter the full path of the {} on {host}.", if folder { "folder" } else { "file" }))
            .build();
        let path = adw::EntryRow::builder().title("Path").activates_default(true).build();
        group.add(&path);
        form.add(&group);
        form.connect_primary(move |form| {
            let p = path.text().trim().to_owned();
            if !p.starts_with('/') {
                return form.set_error(Some("Enter a full path, starting with /."));
            }
            form.close();
            done(p);
        });
        form.present(anchor);
        return;
    }
    let dialog = gtk::FileDialog::builder().title(title).modal(true).build();
    if let Some(f) = filter {
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&f);
        dialog.set_filters(Some(&filters));
        dialog.set_default_filter(Some(&f));
    }
    let window = anchor.root().and_downcast::<gtk::Window>();
    glib::spawn_future_local(async move {
        let file = if folder {
            dialog.select_folder_future(window.as_ref()).await
        } else {
            dialog.open_future(window.as_ref()).await
        };
        if let Some(p) = file.ok().and_then(|f| f.path()) {
            done(p.to_string_lossy().into_owned());
        }
    });
}
