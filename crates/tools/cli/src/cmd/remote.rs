//! `idealyst remote` — signing keys and release bundles of remote components.
//!
//! Building is `idealyst build --remote` (`build_remote`). These are the
//! pieces around it:
//!
//! - `keygen` — a new Ed25519 signing key: the private half to a file (keep
//!   it out of version control; CI reads it from `IDEALYST_REMOTE_SIGNING_KEY`
//!   or `--sign-key`), the public half printed, for the app's `Trust`.
//! - `sign` — sign a bundle built elsewhere (or re-sign with another key).
//! - `verify` — check a bundle against public keys, as an app requiring a
//!   signature would.
//! - `inspect` — what a bundle file says it is, and what it requires of an
//!   app.

use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use remote_bundle::{PublicKey, SigningKey};

#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(clap::Subcommand, Debug)]
pub enum Command {
    /// Create a signing key: writes the private key (hex) to FILE and
    /// prints the public key an app trusts.
    Keygen {
        /// Where to write the private key. Refuses to overwrite one.
        #[arg(long, short, value_name = "FILE", default_value = "remote-signing.key")]
        out: PathBuf,
    },
    /// Sign a bundle (replacing any signature it has).
    Sign {
        /// The bundle (`.wasm`).
        bundle: PathBuf,
        /// The private key file (`idealyst remote keygen`). Without it,
        /// `IDEALYST_REMOTE_SIGNING_KEY`.
        #[arg(long, value_name = "FILE")]
        key: Option<PathBuf>,
        /// Write the signed bundle here instead of over the input.
        #[arg(long, short, value_name = "FILE")]
        out: Option<PathBuf>,
    },
    /// Check a bundle's signature against public keys (hex, repeatable).
    Verify {
        bundle: PathBuf,
        #[arg(long = "public-key", value_name = "HEX", required = true)]
        public_keys: Vec<String>,
    },
    /// Print a bundle's metadata, size, hash, signing key and what it
    /// requires of an app.
    Inspect { bundle: PathBuf },
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        Command::Keygen { out } => keygen(&out),
        Command::Sign { bundle, key, out } => {
            let key = build_remote::signing_key(key.as_deref())?
                .ok_or_else(|| anyhow!("no signing key: pass --key FILE or set {}", build_remote::SIGNING_KEY_ENV))?;
            let wasm = read(&bundle)?;
            let signed = remote_bundle::sign(&wasm, &key)?;
            let dest = out.unwrap_or(bundle);
            std::fs::write(&dest, &signed).with_context(|| format!("write {}", dest.display()))?;
            println!("signed {} with key {}", dest.display(), key.public().id());
            update_manifest(&dest, &signed, &key)?;
            Ok(())
        }
        Command::Verify { bundle, public_keys } => {
            let keys = public_keys.iter().map(|k| PublicKey::from_hex(k).map_err(|e| anyhow!("--public-key {k}: {e}"))).collect::<Result<Vec<_>>>()?;
            let wasm = read(&bundle)?;
            match remote_bundle::verify(&wasm, &keys) {
                Ok(id) => {
                    println!("{}: signed by trusted key {id}", bundle.display());
                    Ok(())
                }
                Err(e) => bail!("{}: {e}", bundle.display()),
            }
        }
        Command::Inspect { bundle } => {
            let wasm = read(&bundle)?;
            println!("{}", bundle.display());
            match remote_bundle::metadata(&wasm)? {
                Some(m) => println!("  bundle   {} ({} {}), codec {}", m.name, m.package, m.version, m.codec),
                None => println!("  bundle   (no metadata: not a release build)"),
            }
            println!("  size     {} bytes", wasm.len());
            println!("  sha256   {}", remote_bundle::content_hash(&wasm));
            match remote_bundle::signature(&wasm) {
                Ok(Some(s)) => println!("  signed   by key {}", s.key),
                Ok(None) => println!("  signed   no"),
                Err(e) => println!("  signed   {e}"),
            }
            match remote_bundle::requires(&wasm)? {
                Some(r) => print!("{}", requirements(&r)),
                None => println!("  requires (not recorded: not a release build)"),
            }
            Ok(())
        }
    }
}

/// What a bundle requires of an app, one item per line.
fn requirements(r: &remote_bundle::Requires) -> String {
    let mut out = String::new();
    for (name, props) in &r.components {
        let props: Vec<String> = props.iter().map(|(p, shape)| format!("{p}: {shape}")).collect();
        out.push_str(&format!("  uses     {name}({})\n", props.join(", ")));
    }
    for (import, shape) in &r.host_fns {
        let path = import.rsplit_once('#').map_or(import.as_str(), |(p, _)| p);
        // A generic host function's types are checked by kind, in the
        // signature fingerprint its import name carries.
        let shape = if shape == "?" { "generic" } else { shape.as_str() };
        out.push_str(&format!("  calls    {path}: {shape}\n"));
    }
    for (name, params) in &r.remote {
        let params: Vec<String> = params.iter().map(|p| format!("{}: {}", p.name, p.shape)).collect();
        out.push_str(&format!("  provides {name}({})\n", params.join(", ")));
    }
    out
}

/// The build writes `<name>.json` beside `<name>.wasm`; signing changes the
/// file's hash and signer, so a manifest there is brought up to date.
fn update_manifest(wasm_path: &std::path::Path, signed: &[u8], key: &SigningKey) -> Result<()> {
    let json = wasm_path.with_extension("json");
    let Ok(text) = std::fs::read(&json) else { return Ok(()) };
    let Ok(mut m) = serde_json::from_slice::<build_remote::Manifest>(&text) else { return Ok(()) };
    m.file = wasm_path.file_name().map_or(m.file, |f| f.to_string_lossy().into_owned());
    m.size = signed.len() as u64;
    m.sha256 = remote_bundle::content_hash(signed);
    m.signed_by = Some(key.public().id().to_string());
    std::fs::write(&json, serde_json::to_vec_pretty(&m)?).with_context(|| format!("write {}", json.display()))?;
    println!("updated {}", json.display());
    Ok(())
}

fn read(path: &std::path::Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("read {}", path.display()))
}

fn keygen(out: &std::path::Path) -> Result<()> {
    if out.exists() {
        bail!("{} already exists; not overwriting a key (pass --out for another file)", out.display());
    }
    let key = SigningKey::generate().map_err(|e| anyhow!(e))?;
    write_private(out, &format!("{}\n", key.to_hex()))?;
    let public = key.public();
    println!("private key  {} (keep it secret: whoever has it can sign bundles your app runs)", out.display());
    println!("key id       {}", public.id());
    println!("public key   {}", public.to_hex());
    println!();
    println!("In the app, trust it and require signed bundles:");
    println!("  Trust::default().key(PublicKey::from_hex(\"{}\")?).require_signature()", public.to_hex());
    Ok(())
}

/// Write a private key readable only by its owner where the OS has modes.
pub(crate) fn write_private(path: &std::path::Path, text: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("create {}", path.display()))?;
        f.write_all(text.as_bytes())?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, text).with_context(|| format!("write {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Signing a built bundle updates the manifest the build wrote beside
    /// it: a stale hash would make a cache or server reject the file.
    #[test]
    fn signing_updates_the_manifest_beside_the_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let wasm = dir.path().join("shop.wasm");
        std::fs::write(&wasm, b"\0asm\x01\0\0\0").unwrap();
        let manifest = build_remote::Manifest {
            name: "shop".into(),
            package: "shop-screens".into(),
            version: "1.0.0".into(),
            codec: 2,
            file: "shop.wasm".into(),
            size: 8,
            sha256: "old".into(),
            signed_by: None,
            requires: Default::default(),
        };
        std::fs::write(dir.path().join("shop.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
        let key = SigningKey::generate().unwrap();
        std::fs::write(dir.path().join("k"), key.to_hex()).unwrap();
        run(Args { command: Command::Sign { bundle: wasm.clone(), key: Some(dir.path().join("k")), out: None } }).unwrap();
        let signed = std::fs::read(&wasm).unwrap();
        let m: build_remote::Manifest = serde_json::from_slice(&std::fs::read(dir.path().join("shop.json")).unwrap()).unwrap();
        assert_eq!(m.sha256, remote_bundle::content_hash(&signed));
        assert_eq!(m.size, signed.len() as u64);
        assert_eq!(m.signed_by, Some(key.public().id().to_string()));
        assert_eq!(remote_bundle::verify(&signed, &[key.public()]), Ok(key.public().id()));
    }

    #[test]
    fn inspect_lists_what_a_bundle_requires() {
        let mut r = remote_bundle::Requires::default();
        r.components.insert("ui::Card".into(), [("title".to_string(), "reactive<str>".to_string())].into());
        r.host_fns.insert("app::sort#00000000000000aa".into(), "fn(list<u32>)->list<u32>".into());
        r.remote.insert(
            "shop::Offer".into(),
            vec![remote_bundle::manifest::Param { name: "id".into(), shape: "u64".into() }],
        );
        assert_eq!(
            requirements(&r),
            "  uses     ui::Card(title: reactive<str>)\n  calls    app::sort: fn(list<u32>)->list<u32>\n  provides shop::Offer(id: u64)\n"
        );
    }

    #[test]
    fn keygen_writes_a_private_key_only_its_owner_reads_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("k");
        keygen(&file).unwrap();
        let key = SigningKey::from_hex(&std::fs::read_to_string(&file).unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert!(keygen(&file).is_err(), "an existing key is never overwritten");
        assert_eq!(SigningKey::from_hex(&std::fs::read_to_string(&file).unwrap()).unwrap().public(), key.public());
    }
}
