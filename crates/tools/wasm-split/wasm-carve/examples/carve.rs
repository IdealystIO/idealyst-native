//! `carve <new|old> <rustc.wasm> <bindgened.wasm> <out-dir>`: split with
//! wasm-carve or with wasm-split-cli, write every output, report time and
//! peak RSS. The bindgened input must already be neutralized.
use std::time::Instant;

fn peak_mb() -> u64 {
    unsafe {
        let mut u: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut u);
        (u.ru_maxrss as u64) >> 20
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a[1] == "neutralize" {
        let bytes = std::fs::read(&a[2]).unwrap();
        std::fs::write(&a[3], wasm_split_cli::neutralize_command_export_wrappers(&bytes).unwrap()).unwrap();
        return;
    }
    let (which, orig, bg, out) = (&a[1], &a[2], &a[3], std::path::Path::new(&a[4]));
    let original = std::fs::read(orig).unwrap();
    let bindgened = std::fs::read(bg).unwrap();
    let _ = std::fs::remove_dir_all(out);
    std::fs::create_dir_all(out).unwrap();
    let base = peak_mb();
    let t = Instant::now();
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    if which == "new" {
        let o = wasm_carve::split(&original, &bindgened).unwrap();
        files.push(("main.wasm".into(), o.main.bytes));
        for (i, c) in o.chunks.into_iter().enumerate() { files.push((format!("chunk{i}.wasm"), c.bytes)); }
        for (i, m) in o.modules.into_iter().enumerate() { files.push((format!("mod{i}.wasm"), m.bytes)); }
    } else {
        let s = wasm_split_cli::Splitter::new(&original, &bindgened).unwrap().with_emit_workers(Some(4));
        let o = s.emit().unwrap();
        files.push(("main.wasm".into(), o.main.bytes));
        for (i, c) in o.chunks.into_iter().enumerate() { files.push((format!("chunk{i}.wasm"), c.bytes)); }
        for (i, m) in o.modules.into_iter().enumerate() { files.push((format!("mod{i}.wasm"), m.bytes)); }
    }
    let took = t.elapsed();
    for (name, bytes) in &files { std::fs::write(out.join(name), bytes).unwrap(); }
    eprintln!("{which}: {:.2}s, peak RSS {} MB (inputs {} MB before), {} files", took.as_secs_f64(), peak_mb(), base, files.len());
}
