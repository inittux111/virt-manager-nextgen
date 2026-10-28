//! Prints the operating system libosinfo detects on install media.
//!
//! Usage: `identify <iso> [<iso>…]`

fn main() {
    let db = match vmng_osinfo::OsDb::load() {
        Ok(db) => db,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    for path in std::env::args().skip(1) {
        match db.identify_media(&path) {
            Ok(Some(os)) => println!("{path}: {} ({})", os.name, os.id),
            Ok(None) => println!("{path}: not recognised"),
            Err(e) => println!("{path}: error: {e}"),
        }
    }
}
