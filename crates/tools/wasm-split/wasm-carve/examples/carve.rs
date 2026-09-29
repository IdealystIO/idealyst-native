//! `carve <rustc.wasm> <bindgened.wasm> <out-dir>`: split, write every
//! output, report time and peak RSS. For measuring the splitter on a real
//! app's modules outside the build.
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
    let (orig, bg, out) = (&a[1], &a[2], std::path::Path::new(&a[3]));
    let original = std::fs::read(orig).unwrap();
    let bindgened = std::fs::read(bg).unwrap();
    let _ = std::fs::remove_dir_all(out);
    std::fs::create_dir_all(out).unwrap();
    let t = Instant::now();
    let bindgened = wasm_carve::neutralize::neutralize_command_export_wrappers(&bindgened).unwrap();
    let o = wasm_carve::split(&original, &bindgened, &Default::default()).unwrap();
    let took = t.elapsed();
    std::fs::write(out.join("main.wasm"), &o.main.bytes).unwrap();
    for (i, c) in o.chunks.iter().enumerate() {
        std::fs::write(out.join(format!("chunk{i}.wasm")), &c.bytes).unwrap();
    }
    for (i, m) in o.modules.iter().enumerate() {
        std::fs::write(out.join(format!("mod{i}.wasm")), &m.bytes).unwrap();
    }
    eprintln!("{:.2}s, peak RSS {} MB", took.as_secs_f64(), peak_mb());
}
