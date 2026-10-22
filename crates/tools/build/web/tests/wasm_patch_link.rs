//! The patch link takes as many objects as a patch has.
//!
//! The bug this guards: the link passed every object path on wasm-ld's
//! command line. An untrimmed patch of a crate thirty others depend on is
//! ~9,000 object paths, past macOS's ARG_MAX, and the exec failed with
//! `Argument list too long` (os error 7) — every patch of CrewForge's
//! `crewforge-core` fell back to a rebuild on a Mac. The objects now go in
//! a response file.
//!
//! This links 12,000 copies of one real wasm32 object (an empty `no_std`
//! crate: no symbols, so the copies do not collide) under a long
//! directory name — about 1.6 MB of paths, over macOS's 1 MB ARG_MAX — plus
//! one path with a space in it, which the response file must quote.
//!
//! `#[ignore]`d like its siblings: it needs the `wasm32-unknown-unknown`
//! target.
//!
//! ```text
//! cargo test -p build-web --test wasm_patch_link -- --ignored
//! ```

use std::path::PathBuf;
use std::process::Command;

fn sysroot() -> PathBuf {
    let out = Command::new("rustc").args(["--print", "sysroot"]).output().expect("run rustc");
    PathBuf::from(String::from_utf8(out.stdout).unwrap().trim())
}

fn wasm_ld() -> PathBuf {
    let host = String::from_utf8(Command::new("rustc").arg("-vV").output().unwrap().stdout)
        .unwrap()
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
        .unwrap();
    sysroot().join("lib/rustlib").join(host).join("bin/gcc-ld/wasm-ld")
}

#[test]
#[ignore = "compiles for wasm32 and links 12,000 objects; run with --ignored"]
fn regression_a_patch_of_thousands_of_objects_links() {
    let dir = std::env::temp_dir().join(format!("idealyst-patch-link-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let deep = dir.join("a-directory-name-long-enough-that-twelve-thousand-paths-under-it-pass-arg-max-xxxxxxxxxxxxxxxx");
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(dir.join("empty.rs"), "#![no_std]\n").unwrap();
    let status = Command::new(sysroot().join("bin/rustc"))
        .args(["--crate-type", "lib", "--crate-name", "empty", "--emit=obj", "--target", "wasm32-unknown-unknown"])
        .args(["-C", "relocation-model=pic", "-o"])
        .arg(dir.join("empty.o"))
        .arg(dir.join("empty.rs"))
        .status()
        .expect("run rustc");
    assert!(status.success(), "compiling the object failed (is the wasm32-unknown-unknown target installed?)");

    let one = std::fs::read(dir.join("empty.o")).unwrap();
    let mut objects: Vec<PathBuf> = (0..12_000)
        .map(|i| {
            let p = deep.join(format!("object-{i:05}.o"));
            std::fs::write(&p, &one).unwrap();
            p
        })
        .collect();
    let spaced = dir.join("with space");
    std::fs::create_dir_all(&spaced).unwrap();
    std::fs::write(spaced.join("empty.o"), &one).unwrap();
    objects.push(spaced.join("empty.o"));
    let bytes: usize = objects.iter().map(|p| p.as_os_str().len() + 1).sum();
    assert!(bytes > 1 << 20, "the paths ({bytes} bytes) must outgrow ARG_MAX for this to test anything");

    let out = dir.join("patch.wasm");
    build_web::hotpatch_build::link_patch(&wasm_ld(), &objects, &out)
        .unwrap_or_else(|e| panic!("the link failed:\n{e:#}"));
    assert!(out.is_file());
    assert!(!out.with_extension("objects.rsp").exists(), "the response file is left behind");
    let _ = std::fs::remove_dir_all(&dir);
}
