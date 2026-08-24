//! Print the watch set `idealyst dev` resolves for a project — the
//! answer to "why didn't my save rebuild?".
//!
//!     cargo run -p dev-reload --example watch_roots -- path/to/Cargo.toml
//!
//! The dev session prints the same list on startup (`[dev-reload]
//! watching …`); this is for checking it without starting one.

fn main() {
    let Some(arg) = std::env::args().nth(1) else {
        eprintln!("usage: watch_roots <path/to/Cargo.toml>");
        std::process::exit(2);
    };
    for path in dev_reload::watch_roots(std::path::Path::new(&arg)) {
        println!("{}", path.display());
    }
}
