//! Prints the operating system libosinfo detects on install media, or in
//! a network install tree (an `http(s)://` address).
//!
//! Usage: `identify <iso or url> […]`

fn main() {
    let db = match vmng_osinfo::OsDb::load() {
        Ok(db) => db,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    for path in std::env::args().skip(1) {
        if path.starts_with("http://") || path.starts_with("https://") {
            match db.identify_tree(&path) {
                Ok(t) => println!(
                    "{path}: {:?} kernel={:?} initrd={:?} url-argument={:?}",
                    t.os.map(|o| o.id),
                    t.kernel,
                    t.initrd,
                    t.url_argument
                ),
                Err(e) => println!("{path}: error: {e}"),
            }
            continue;
        }
        match db.identify_media(&path) {
            Ok(Some(os)) => println!("{path}: {} ({})", os.name, os.id),
            Ok(None) => println!("{path}: not recognised"),
            Err(e) => println!("{path}: error: {e}"),
        }
    }
}
