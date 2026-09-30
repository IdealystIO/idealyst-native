//! Run the own-bindings passes (`glue::extract` then
//! `command_exports::unwrap_command_exports`) over one linked module and
//! report their cost — the phase-1 scaling measurement of
//! docs/proposals/own-web-bindings.md. Run under `/usr/bin/time -l` for
//! peak RSS:
//!
//! ```text
//! cargo run --release -p wasm-carve --example glue_pass -- <in.wasm> <out.wasm>
//! ```

use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("usage: glue_pass <in.wasm> <out.wasm>");
    let output = args.next().expect("usage: glue_pass <in.wasm> <out.wasm>");
    let t = Instant::now();
    let bytes = std::fs::read(&input)?;
    let read = t.elapsed();
    let t = Instant::now();
    let glue = wasm_carve::glue::extract(&bytes)?;
    let extract = t.elapsed();
    let t = Instant::now();
    let unwrapped = wasm_carve::command_exports::unwrap_command_exports(&glue.wasm)?;
    let unwrap = t.elapsed();
    let out = unwrapped.as_ref().map_or(&glue.wasm, |(m, _)| m);
    let t = Instant::now();
    std::fs::write(&output, out)?;
    let write = t.elapsed();
    println!(
        "{} bytes in, {} out; glue imports {}, command exports unwrapped {}\n\
         read {read:?}  extract {extract:?}  unwrap {unwrap:?}  write {write:?}",
        bytes.len(),
        out.len(),
        glue.imports.len(),
        unwrapped.as_ref().map_or(0, |(_, n)| *n),
    );
    Ok(())
}
