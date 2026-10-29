//! The app's log file, for bug reports: failed operations (the same as the
//! error toasts), GLib/GTK warnings and crashes, in
//! `~/.local/state/virt-manager-nextgen/app.log`. It stays small (at
//! [`MAX_SIZE`] the file moves to `app.log.old`), only its owner can read
//! it, and About → Troubleshooting shows its end with version details.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use gtk::glib;

use crate::config;

const MAX_SIZE: u64 = 512 << 10;
/// Lines from the end of the log shown under About → Troubleshooting.
const SHOWN_LINES: usize = 300;

static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

fn path() -> Option<&'static Path> {
    PATH.get_or_init(|| {
        // Screenshot runs leave the person's log alone unless asked.
        #[cfg(debug_assertions)]
        if std::env::var_os("VMNG_SCREENSHOT").is_some() && std::env::var_os("VMNG_DEV_LOG").is_none() {
            return None;
        }
        // $XDG_STATE_HOME, as the XDG base directory spec defines it.
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| glib::home_dir().join(".local/state"));
        Some(state.join(config::CONFIG_DIR).join("app.log"))
    })
    .as_deref()
}

/// One log entry: a time stamp, its kind and the text, with continuation
/// lines indented.
fn entry(time: &str, kind: &str, text: &str) -> String {
    format!("{time} {kind}: {}\n", text.trim_end().replace('\n', "\n    "))
}

/// Appends an entry. A log that can't be written never gets in the way.
pub fn write(kind: &str, text: &str) {
    let Some(path) = path() else { return };
    let time = glib::DateTime::now_local().ok().and_then(|t| t.format("%F %T").ok()).unwrap_or_default();
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_SIZE) {
        let _ = std::fs::rename(path, path.with_extension("log.old"));
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(path);
    if let Ok(mut f) = file {
        let _ = f.write_all(entry(&time, kind, text).as_bytes());
    }
}

/// Starts logging: crashes and GLib/GTK warnings go to the log as well as
/// to the terminal.
pub fn init() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Release builds are stripped, so a backtrace only helps in
        // development; the message names the file and line either way.
        #[cfg(debug_assertions)]
        write("crash", &format!("{info}\n{}", std::backtrace::Backtrace::force_capture()));
        #[cfg(not(debug_assertions))]
        write("crash", &info.to_string());
        default(info);
    }));
    glib::log_set_writer_func(|level, fields| {
        let kind = match level {
            glib::LogLevel::Error => Some("fatal"),
            glib::LogLevel::Critical => Some("critical"),
            glib::LogLevel::Warning => Some("warning"),
            _ => None,
        };
        if let Some(kind) = kind {
            let field = |k: &str| fields.iter().find(|f| f.key() == k).and_then(|f| f.value_str()).unwrap_or("");
            write(kind, &format!("{}: {}", field("GLIB_DOMAIN"), field("MESSAGE")));
        }
        glib::log_writer_default(level, fields)
    });
    write("start", &format!("{} {}", config::APP_NAME, config::VERSION));
}

/// The last `n` lines of `text`.
fn tail(text: &str, n: usize) -> &str {
    let start = text.rmatch_indices('\n').nth(n).map_or(0, |(i, _)| i + 1);
    &text[start..]
}

/// Version details and the end of the log, for About → Troubleshooting.
pub fn report() -> String {
    let mut out = format!(
        "{} {}\nGTK {}.{}.{}, libadwaita {}.{}.{}\n",
        config::APP_NAME,
        config::VERSION,
        gtk::major_version(),
        gtk::minor_version(),
        gtk::micro_version(),
        adw::major_version(),
        adw::minor_version(),
        adw::micro_version(),
    );
    if let Some(path) = path() {
        out.push_str(&format!("\nLog ({}):\n", path.display()));
        match std::fs::read_to_string(path) {
            Ok(log) => out.push_str(tail(&log, SHOWN_LINES)),
            Err(_) => out.push_str("(empty)\n"),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_one_block() {
        assert_eq!(entry("T", "error", "Couldn't start x: no\nmore\n"), "T error: Couldn't start x: no\n    more\n");
    }

    #[test]
    fn tail_keeps_the_last_lines() {
        assert_eq!(tail("a\nb\nc\n", 2), "b\nc\n");
        assert_eq!(tail("a\nb\nc\n", 10), "a\nb\nc\n");
        assert_eq!(tail("", 3), "");
    }
}
