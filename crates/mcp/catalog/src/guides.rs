//! Generated guide registrations. The body is produced by `build.rs`
//! scanning `guides/*.md` and emitting one `inventory::submit!` per
//! markdown file. Editing the markdown files (or adding new ones) is
//! the only authoring surface — this module is a one-line
//! `include!`.

include!(concat!(env!("OUT_DIR"), "/guides_generated.rs"));
