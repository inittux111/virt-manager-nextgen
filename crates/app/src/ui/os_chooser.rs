//! A searchable list of operating systems from osinfo-db.

use std::rc::Rc;

use adw::prelude::*;
use vmng_osinfo::OsInfo;

use super::widgets::OsLogo;
use crate::osdb;

/// Most rows shown at once; searching narrows the list.
const MAX_ROWS: usize = 150;

fn matches(os: &OsInfo, words: &[String]) -> bool {
    let hay = format!("{} {} {}", os.name, os.short_id, os.vendor.as_deref().unwrap_or_default()).to_lowercase();
    words.iter().all(|w| hay.contains(w.as_str()))
}

/// Newest first; unknown release dates last.
fn sorted_oses() -> Vec<&'static OsInfo> {
    let mut all: Vec<&OsInfo> = osdb::all().collect();
    all.sort_by(|a, b| b.release_date.cmp(&a.release_date).then_with(|| a.name.cmp(&b.name)));
    all
}

/// Opens the chooser; `chosen` receives the picked OS.
pub fn choose_os(parent: &impl IsA<gtk::Widget>, initial_query: &str, chosen: impl Fn(&OsInfo) + 'static) {
    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search, e.g. “ubuntu 24.04” or “windows 11”")
        .hexpand(true)
        .build();
    search.set_text(initial_query);
    let list = gtk::ListBox::builder().css_classes(["boxed-list"]).selection_mode(gtk::SelectionMode::None).build();
    let empty = adw::StatusPage::builder()
        .icon_name("system-search-symbolic")
        .title("No matches")
        .description("Try a shorter search, or pick a generic entry such as “Generic Linux”.")
        .build();
    empty.add_css_class("compact");
    let stack = gtk::Stack::new();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build();
    content.append(&list);
    stack.add_named(
        &gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&content).build(),
        Some("list"),
    );
    stack.add_named(&empty, Some("empty"));

    let header = adw::HeaderBar::new();
    let search_box = adw::Clamp::builder()
        .maximum_size(520)
        .child(&search)
        .margin_start(24)
        .margin_end(24)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    let toolbar = adw::ToolbarView::builder().content(&stack).build();
    toolbar.add_top_bar(&header);
    toolbar.add_top_bar(&search_box);
    let dialog = adw::Dialog::builder()
        .title("Choose Operating System")
        .content_width(560)
        .content_height(620)
        .child(&toolbar)
        .build();

    let all = Rc::new(sorted_oses());
    let chosen = Rc::new(chosen);
    let fill = {
        let (list, stack, dialog, all, chosen) =
            (list.clone(), stack.clone(), dialog.clone(), all.clone(), chosen.clone());
        move |query: &str| {
            list.remove_all();
            let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
            let mut shown = 0;
            for os in all.iter().filter(|os| matches(os, &words)).take(MAX_ROWS) {
                let logo = OsLogo::new(32);
                logo.set_logo(osdb::logo_key(&os.id));
                let mut sub = Vec::new();
                if let Some(v) = &os.vendor {
                    sub.push(v.clone());
                }
                if let Some(d) = &os.release_date {
                    sub.push(format!("released {}", &d[..4.min(d.len())]));
                }
                let row = adw::ActionRow::builder().title(&os.name).subtitle(sub.join(" · ")).activatable(true).build();
                row.add_prefix(&logo);
                let (dialog, chosen, os) = (dialog.clone(), chosen.clone(), (*os).clone());
                row.connect_activated(move |_| {
                    chosen(&os);
                    dialog.close();
                });
                list.append(&row);
                shown += 1;
            }
            stack.set_visible_child_name(if shown == 0 { "empty" } else { "list" });
        }
    };
    fill(initial_query);
    search.connect_search_changed(move |s| fill(&s.text()));
    dialog.present(Some(parent));
    search.grab_focus();
}
