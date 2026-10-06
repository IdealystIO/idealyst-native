//! `idealyst ota` — over-the-air releases of an app's remote-component
//! bundles.
//!
//! - `init` — write the app's settings (`[package.metadata.idealyst.ota]`:
//!   where apps read releases, where publishing writes them) and create its
//!   signing key, whose public half goes into the settings so the app only
//!   runs bundles you signed.
//! - `publish` — build the bundles (`idealyst build --remote`), sign them,
//!   and publish them: each bundle's file, then the index apps read. A
//!   release that requires something its current release didn't is shown
//!   first: apps lacking it keep the release they have.
//! - `rollback` — take back a bundle's newest release.
//! - `status` — what is published, and what each registered app build runs.
//! - `manifest` — capture this app build's manifest (what it offers
//!   bundles) and register it, so the release location answers for it
//!   (`resolved/<id>.json`) and the console and `publish` can name it.
//! - `resolve` — rewrite every registered build's answer from the index.
//!
//! The app's side is the `ota` crate: `ota::start(ota::config!(), …)`.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use ota_index::{ManifestSource, Registered};
use ota_publish::{Planned, Target, Upload};
use remote_bundle::SigningKey;

/// The signing key's file, in the app's directory, when no other is given.
const KEY_FILE: &str = "ota-signing.key";
/// Overrides `bucket` (CI publishing to staging, say).
const BUCKET_ENV: &str = "IDEALYST_OTA_BUCKET";

#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(clap::Subcommand, Debug)]
pub enum Command {
    /// Set up over-the-air releases: write the settings into Cargo.toml and
    /// create the signing key.
    Init {
        /// Where apps read releases (`https://…`, the CDN in front of the bucket).
        #[arg(long)]
        url: Option<String>,
        /// Where publishing writes them (`s3://bucket/prefix`, or a directory).
        #[arg(long)]
        bucket: Option<String>,
        /// The app's directory.
        #[arg(default_value = ".")]
        dir: PathBuf,
    },
    /// Build, sign and publish the app's bundles.
    Publish {
        /// Publish only these bundles (repeatable); all by default.
        #[arg(long = "bundle", value_name = "NAME")]
        bundles: Vec<String>,
        /// The private signing key; `$IDEALYST_REMOTE_SIGNING_KEY`, or
        /// `ota-signing.key` in the app's directory, by default.
        #[arg(long, value_name = "FILE")]
        sign_key: Option<PathBuf>,
        /// Publish without asking, even when a release needs something
        /// older apps lack.
        #[arg(long, short)]
        yes: bool,
        #[arg(default_value = ".")]
        dir: PathBuf,
    },
    /// Take back a bundle's newest release: apps return to the one before.
    Rollback {
        bundle: String,
        #[arg(default_value = ".")]
        dir: PathBuf,
    },
    /// Show what is published, and what each registered app build runs.
    Status {
        #[arg(default_value = ".")]
        dir: PathBuf,
    },
    /// Capture this app build's manifest (what it offers bundles) and
    /// register it at the release location. Run it for each app release.
    Manifest {
        /// What to call this build (the console, `status`, `publish`);
        /// `<app> <version>` by default.
        #[arg(long)]
        label: Option<String>,
        /// Only write the manifest to a file; don't register it.
        #[arg(long)]
        no_register: bool,
        /// Where to write the manifest as well.
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
        #[arg(default_value = ".")]
        dir: PathBuf,
    },
    /// Rewrite every registered build's answer from the index (after a
    /// change that couldn't finish writing them).
    Resolve {
        #[arg(default_value = ".")]
        dir: PathBuf,
    },
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        Command::Init { url, bucket, dir } => init(&dir, url.as_deref(), bucket.as_deref()),
        Command::Publish { bundles, sign_key, yes, dir } => publish(&dir, &bundles, sign_key.as_deref(), yes),
        Command::Rollback { bundle, dir } => {
            let target = settings(&dir)?.target()?;
            match ota_publish::rollback(&target, &bundle, &actor())? {
                Some(r) => println!("`{bundle}`: apps go back to {} at their next check", r.version),
                None => println!("`{bundle}`: no release left"),
            }
            Ok(())
        }
        Command::Status { dir } => status(&dir),
        Command::Manifest { label, no_register, out, dir } => manifest(&dir, label, !no_register, out.as_deref()),
        Command::Resolve { dir } => {
            let r = ota_publish::resolve_registered(&settings(&dir)?.target()?)?;
            println!("{} answer(s) written, {} already current", r.written, r.current);
            for (id, e) in &r.failed {
                eprintln!("  {}: {e}", &id[..12]);
            }
            if !r.failed.is_empty() {
                bail!("{} answer(s) not written", r.failed.len());
            }
            Ok(())
        }
    }
}

/// `idealyst ota manifest`.
fn manifest(dir: &Path, label: Option<String>, register: bool, out: Option<&Path>) -> Result<()> {
    let settings = settings(dir)?;
    let host_fns = settings.host_fns.as_deref().ok_or_else(|| {
        anyhow!(
            "declare the app's host functions in [package.metadata.idealyst.ota] — `host_fns = \"app::host_fns\"`, a path from the library's root to a public `fn() -> Vec<HostFnDef>` — and leave `Options::host_fns` empty: the app and its manifest then use the same list"
        )
    })?;
    let captured = super::ota_capture::capture(dir, host_fns)?;
    let app = build_ios::parse_manifest(&std::fs::canonicalize(dir)?)?;
    let label = label.unwrap_or_else(|| format!("{} {}", app.name, app.app.version));
    if let Some(out) = out {
        std::fs::write(out, captured.to_json()).with_context(|| format!("write {}", out.display()))?;
    }
    let p = &captured.provides;
    println!(
        "{label}: manifest {}  ({} components, {} host functions, {} remote components, {} context types)",
        captured.id,
        p.components.len(),
        p.host_fns.len(),
        p.remote.len(),
        p.contexts.len()
    );
    if !register {
        return Ok(());
    }
    let target = settings.target()?;
    let outcome = ota_publish::register(&target, &captured, ManifestSource::Build, Some(label), None)?;
    println!(
        "{}",
        match outcome {
            ota_publish::Registration::Added => "registered: the location now answers for this build",
            ota_publish::Registration::Updated => "registered: known from the field until now; its answer is now precomputed",
            ota_publish::Registration::Known => "already registered",
        }
    );
    Ok(())
}

/// Who is acting, for the release location's audit log.
fn actor() -> String {
    format!("cli:{}", std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_else(|_| "unknown".into()))
}

/// `[package.metadata.idealyst.ota]`.
#[derive(Debug, Default, PartialEq)]
struct Settings {
    url: Option<String>,
    bucket: Option<String>,
    public_keys: Vec<String>,
    /// A path from the library's root to the app's host functions.
    host_fns: Option<String>,
}

impl Settings {
    fn target(&self) -> Result<Target> {
        let bucket = std::env::var(BUCKET_ENV).ok().or_else(|| self.bucket.clone()).ok_or_else(|| {
            anyhow!("no `bucket` in [package.metadata.idealyst.ota] (or ${BUCKET_ENV}): where to publish. `idealyst ota init --bucket s3://…` sets it")
        })?;
        Target::parse(&bucket)
    }
}

fn parse_settings(manifest: &str) -> Result<Option<Settings>> {
    let doc: toml::Value = manifest.parse().context("parse Cargo.toml")?;
    let Some(ota) = doc.get("package").and_then(|p| p.get("metadata")).and_then(|m| m.get("idealyst")).and_then(|m| m.get("ota")) else {
        return Ok(None);
    };
    let text = |k: &str| ota.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let public_keys = ota
        .get("public_keys")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|k| k.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    Ok(Some(Settings { url: text("url"), bucket: text("bucket"), public_keys, host_fns: text("host_fns") }))
}

fn settings(dir: &Path) -> Result<Settings> {
    let path = dir.join("Cargo.toml");
    let text = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    parse_settings(&text)?.ok_or_else(|| anyhow!("{}: no [package.metadata.idealyst.ota] — run `idealyst ota init`", path.display()))
}

/// Merge `url` / `bucket` / `public_key` into the manifest's settings,
/// keeping everything else in it as written.
fn write_settings(manifest: &str, url: Option<&str>, bucket: Option<&str>, public_key: Option<&str>) -> Result<String> {
    let mut doc: toml_edit::DocumentMut = manifest.parse().context("parse Cargo.toml")?;
    let package = doc.get_mut("package").and_then(|p| p.as_table_mut()).ok_or_else(|| anyhow!("Cargo.toml has no [package]"))?;
    let table = |t: &mut toml_edit::Table, k: &str| -> Result<()> {
        if !t.contains_key(k) {
            let mut new = toml_edit::Table::new();
            new.set_implicit(true);
            t.insert(k, toml_edit::Item::Table(new));
        }
        Ok(())
    };
    table(package, "metadata")?;
    let metadata = package["metadata"].as_table_mut().ok_or_else(|| anyhow!("[package.metadata] isn't a table"))?;
    table(metadata, "idealyst")?;
    let idealyst = metadata["idealyst"].as_table_mut().ok_or_else(|| anyhow!("[package.metadata.idealyst] isn't a table"))?;
    if !idealyst.contains_key("ota") {
        idealyst.insert("ota", toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let ota = idealyst["ota"].as_table_mut().ok_or_else(|| anyhow!("[package.metadata.idealyst.ota] isn't a table"))?;
    if let Some(url) = url {
        ota["url"] = toml_edit::value(url);
    }
    if let Some(bucket) = bucket {
        ota["bucket"] = toml_edit::value(bucket);
    }
    if let Some(key) = public_key {
        if !ota.contains_key("public_keys") {
            ota["public_keys"] = toml_edit::value(toml_edit::Array::new());
        }
        let keys = ota["public_keys"].as_array_mut().ok_or_else(|| anyhow!("`public_keys` isn't a list"))?;
        if !keys.iter().any(|k| k.as_str() == Some(key)) {
            keys.push(key);
        }
    }
    Ok(doc.to_string())
}

fn init(dir: &Path, url: Option<&str>, bucket: Option<&str>) -> Result<()> {
    let manifest_path = dir.join("Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path).with_context(|| format!("read {}", manifest_path.display()))?;
    let existing = parse_settings(&manifest)?.unwrap_or_default();
    if url.is_none() && existing.url.is_none() {
        bail!("pass --url: where apps read releases (the CDN in front of the bucket, https://…)");
    }
    if bucket.is_none() && existing.bucket.is_none() {
        bail!("pass --bucket: where `idealyst ota publish` writes releases (s3://bucket/prefix, or a directory)");
    }
    if let Some(b) = bucket {
        Target::parse(b)?;
    }
    let key_path = dir.join(KEY_FILE);
    let key = if key_path.exists() {
        let key = SigningKey::from_hex(&std::fs::read_to_string(&key_path)?).map_err(|e| anyhow!("{}: {e}", key_path.display()))?;
        println!("signing key  {} (existing)", key_path.display());
        key
    } else {
        let key = SigningKey::generate().map_err(|e| anyhow!(e))?;
        super::remote::write_private(&key_path, &format!("{}\n", key.to_hex()))?;
        println!("signing key  {} (new — keep it secret, and out of version control)", key_path.display());
        key
    };
    ignore(dir, KEY_FILE)?;
    let public = key.public().to_hex();
    std::fs::write(&manifest_path, write_settings(&manifest, url, bucket, Some(&public))?)?;
    println!("settings     {} [package.metadata.idealyst.ota]", manifest_path.display());
    println!();
    println!("In the app:");
    println!("  let ota = ota::start(ota::config!(), ota::Options::default())?;");
    println!("with its host functions declared in the settings (a path from the library's root):");
    println!("  host_fns = \"app::host_fns\"");
    println!("Publish:");
    println!("  idealyst ota publish");
    println!("For each app release, register the build, so the location answers for it:");
    println!("  idealyst ota manifest");
    println!("In CI, put the key's contents in ${} instead of the file.", build_remote::SIGNING_KEY_ENV);
    Ok(())
}

/// Add `name` to the directory's .gitignore, if it isn't there.
fn ignore(dir: &Path, name: &str) -> Result<()> {
    let path = dir.join(".gitignore");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    if text.lines().any(|l| l.trim() == name || l.trim() == format!("/{name}")) {
        return Ok(());
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    if !text.is_empty() && !text.ends_with('\n') {
        writeln!(f)?;
    }
    writeln!(f, "{name}")?;
    println!("ignored      {name} in {}", path.display());
    Ok(())
}

fn signing_key(dir: &Path, file: Option<&Path>, settings: &Settings) -> Result<Option<SigningKey>> {
    let default = dir.join(KEY_FILE);
    let file = file.map(Path::to_path_buf).or_else(|| (std::env::var(build_remote::SIGNING_KEY_ENV).is_err() && default.exists()).then_some(default));
    let key = build_remote::signing_key(file.as_deref())?;
    match (&key, settings.public_keys.is_empty()) {
        (None, false) => bail!(
            "the app only runs signed bundles, and there is no signing key: pass --sign-key FILE, set ${}, or put it in {KEY_FILE}",
            build_remote::SIGNING_KEY_ENV
        ),
        (Some(k), false) if !settings.public_keys.contains(&k.public().to_hex()) => bail!(
            "signing key {} isn't one the app trusts (`public_keys`): apps would refuse these bundles",
            k.public().id()
        ),
        (None, true) => eprintln!("[ota] bundles are unsigned: the app runs any bundle at its URL (`idealyst ota init` sets up signing)"),
        _ => {}
    }
    Ok(key)
}

fn publish(dir: &Path, only: &[String], key_file: Option<&Path>, yes: bool) -> Result<()> {
    let settings = settings(dir)?;
    let target = settings.target()?;
    let sign = signing_key(dir, key_file, &settings)?;
    let built = build_remote::build(dir, &build_remote::Options { out_dir: None, sign, only: only.to_vec() })?;
    let uploads: Vec<Upload> = built
        .iter()
        .map(|b| Ok(Upload { name: b.manifest.name.clone(), wasm: std::fs::read(&b.wasm)? }))
        .collect::<Result<_>>()?;
    let index = ota_publish::read_index(&target)?;
    let planned = ota_publish::plan(&index, &uploads)?;
    print!("{}", describe(&planned));
    if planned.iter().all(|p| p.unchanged) {
        println!("nothing to publish");
        return Ok(());
    }
    let unreached = unreached(&target, &uploads, &planned)?;
    print!("{unreached}");
    if (planned.iter().any(|p| !p.new_requirements.is_empty()) || !unreached.is_empty()) && !yes && !confirm()? {
        bail!("not published");
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    ota_publish::publish(&target, &uploads, now, &actor())?;
    let place = settings.url.as_deref().unwrap_or("its URL");
    println!("published: apps reading {place} pick it up at their next check");
    Ok(())
}

/// The registered app builds that won't run a new release, and why (the
/// first reason, and how many more).
fn unreached(target: &Target, uploads: &[Upload], planned: &[Planned]) -> Result<String> {
    let registry = ota_publish::read_registry(target)?;
    let mut builds = Vec::new();
    for entry in &registry.manifests {
        if let Some(m) = ota_publish::read_manifest(target, &entry.id)? {
            builds.push((entry, m.provides));
        }
    }
    let mut out = String::new();
    for (u, p) in uploads.iter().zip(planned).filter(|(_, p)| !p.unchanged) {
        let release = ota_publish::release_of(u, 0)?;
        let mut lines = Vec::new();
        for (entry, provides) in &builds {
            let errors: Vec<_> = remote_bundle::check(&release.requires, release.codec, provides).into_iter().filter(|p| p.is_error()).collect();
            if let Some(first) = errors.first() {
                let more = if errors.len() > 1 { format!(", and {} more", errors.len() - 1) } else { String::new() };
                lines.push(format!("      - {}: {first}{more}\n", build_name(entry)));
            }
        }
        if !lines.is_empty() {
            out.push_str(&format!("    registered app builds that won't run {} {} (they keep what they run):\n", p.bundle, p.version));
            out.extend(lines);
        }
    }
    Ok(out)
}

/// A registered build, for people: its label (or id), and where it came from.
fn build_name(entry: &Registered) -> String {
    let id = &entry.id[..12];
    let from = match entry.source {
        ManifestSource::Build => "",
        ManifestSource::Reported => ", seen in the field",
    };
    match &entry.label {
        Some(label) => format!("{label} ({id}{from})"),
        None => format!("{id}{from}"),
    }
}

/// What a publish will do, for the publisher.
fn describe(planned: &[Planned]) -> String {
    let mut out = String::new();
    for p in planned {
        if p.unchanged {
            out.push_str(&format!("  {} {}: unchanged\n", p.bundle, p.version));
            continue;
        }
        out.push_str(&format!("  {} {}: new release\n", p.bundle, p.version));
        if !p.new_requirements.is_empty() {
            out.push_str("    it requires what the current release didn't — apps that lack it keep the current release:\n");
            for r in &p.new_requirements {
                out.push_str(&format!("      - {r}\n"));
            }
        }
    }
    out
}

fn confirm() -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!("a release requires what older apps may lack; pass --yes to publish it anyway");
    }
    print!("Publish? [y/N] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes"))
}

fn status(dir: &Path) -> Result<()> {
    let target = settings(dir)?.target()?;
    let index = ota_publish::read_index(&target)?;
    if index.bundles.is_empty() {
        println!("nothing published");
    }
    for (name, b) in &index.bundles {
        match b.releases.first() {
            Some(r) => println!(
                "{name}  {}  {:.1} KB  {}  published {} UTC  ({} release(s), {} withdrawn)",
                r.version,
                r.size as f64 / 1024.0,
                r.signed_by.as_deref().map_or("unsigned".to_string(), |k| format!("signed by {k}")),
                utc(r.published),
                b.releases.len(),
                b.withdrawn.len(),
            ),
            None => println!("{name}  (no release)"),
        }
        if let Some(r) = b.releases.first() {
            for c in r.components() {
                println!("  provides {c}");
            }
        }
    }
    let registry = ota_publish::read_registry(&target)?;
    if !registry.manifests.is_empty() {
        println!();
        println!("app builds:");
    }
    for entry in &registry.manifests {
        let Some(m) = ota_publish::read_manifest(&target, &entry.id)? else {
            println!("  {}: its manifest is missing", build_name(entry));
            continue;
        };
        let answer = ota_index::resolve(&index, &m.provides);
        let runs: Vec<String> = answer
            .bundles
            .iter()
            .map(|(name, r)| match (&r.release, r.needs_app_update) {
                (Some(rel), false) => format!("{name} {}", rel.version),
                (Some(rel), true) => format!("{name} {} (newer needs an app update)", rel.version),
                (None, _) => format!("{name}: nothing it can run"),
            })
            .collect();
        println!("  {}: {}", build_name(entry), runs.join(", "));
    }
    Ok(())
}

/// `secs` since the Unix epoch as `YYYY-MM-DD HH:MM` (UTC).
fn utc(secs: u64) -> String {
    // Howard Hinnant's days-to-civil.
    let days = (secs / 86_400) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let doe = days.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let rem = secs % 86_400;
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", rem / 3_600, rem % 3_600 / 60)
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates_are_utc() {
        assert_eq!(super::utc(0), "1970-01-01 00:00");
        assert_eq!(super::utc(951_782_400), "2000-02-29 00:00");
        assert_eq!(super::utc(1_791_236_129), "2026-10-05 21:35");
    }

    use super::*;

    const MANIFEST: &str = "# The app.\n[package]\nname = \"shop\" # its name\nversion = \"0.1.0\"\n\n[package.metadata.idealyst.app]\nname = \"Shop\"\n";

    #[test]
    fn settings_are_merged_into_the_manifest_keeping_what_is_there() {
        let once = write_settings(MANIFEST, Some("https://ota.example.com/shop"), Some("s3://ota/shop"), Some("ab")).unwrap();
        assert!(once.starts_with("# The app.\n[package]\nname = \"shop\" # its name"), "{once}");
        assert!(once.contains("[package.metadata.idealyst.app]\nname = \"Shop\""), "{once}");
        assert_eq!(
            parse_settings(&once).unwrap(),
            Some(Settings {
                url: Some("https://ota.example.com/shop".into()),
                bucket: Some("s3://ota/shop".into()),
                public_keys: vec!["ab".into()],
                host_fns: None
            })
        );
        // Again, with a second key: added once, the rest kept.
        let twice = write_settings(&write_settings(&once, None, None, Some("cd")).unwrap(), None, None, Some("cd")).unwrap();
        assert_eq!(parse_settings(&twice).unwrap().unwrap().public_keys, ["ab", "cd"]);
        assert_eq!(parse_settings(&twice).unwrap().unwrap().url.as_deref(), Some("https://ota.example.com/shop"));
    }

    #[test]
    fn a_release_needing_more_says_what() {
        let planned = vec![
            Planned { bundle: "shop".into(), version: "1.2.0".into(), unchanged: false, new_requirements: vec!["prop `ui::Card.glow`".into()] },
            Planned { bundle: "home".into(), version: "1.0.0".into(), unchanged: true, new_requirements: vec![] },
        ];
        assert_eq!(
            describe(&planned),
            "  shop 1.2.0: new release\n    it requires what the current release didn't — apps that lack it keep the current release:\n      - prop `ui::Card.glow`\n  home 1.0.0: unchanged\n"
        );
    }

    /// Publishing names each registered build a new release won't reach,
    /// with the reason; builds it reaches aren't listed.
    #[test]
    fn publishing_names_the_registered_builds_a_release_wont_reach() {
        let dir = tempfile::tempdir().unwrap();
        let target = Target::Dir(dir.path().into());
        let app = |props: &[&str]| {
            let mut p = remote_bundle::Provides { codec: 2, ..Default::default() };
            p.components.insert("ui::Card".into(), props.iter().map(|k| (k.to_string(), "u8".to_string())).collect());
            ota_index::Manifest::new(p)
        };
        ota_publish::register(&target, &app(&[]), ManifestSource::Build, Some("shop 1.0".into()), None).unwrap();
        ota_publish::register(&target, &app(&["glow"]), ManifestSource::Reported, None, None).unwrap();
        let meta = remote_bundle::Metadata { name: "shop".into(), package: "shop".into(), version: "1.2.0".into(), codec: 2 };
        let wasm = remote_bundle::with_metadata(b"\0asm\x01\0\0\0", &meta).unwrap();
        let mut requires = remote_bundle::Requires::default();
        requires.components.insert("ui::Card".into(), [("glow".to_string(), "u8".to_string())].into());
        let uploads = [Upload { name: "shop".into(), wasm: remote_bundle::with_requires(&wasm, &requires).unwrap() }];
        let planned = ota_publish::plan(&ota_publish::read_index(&target).unwrap(), &uploads).unwrap();
        let id = app(&[]).id;
        assert_eq!(
            unreached(&target, &uploads, &planned).unwrap(),
            format!(
                "    registered app builds that won't run shop 1.2.0 (they keep what they run):\n      - shop 1.0 ({}): `ui::Card` has no prop `glow` in the app\n",
                &id[..12]
            )
        );
    }

    #[test]
    fn publishing_requires_a_key_the_app_trusts() {
        let dir = tempfile::tempdir().unwrap();
        let trusted = SigningKey::generate().unwrap();
        let settings = Settings { public_keys: vec![trusted.public().to_hex()], ..Default::default() };
        assert!(signing_key(dir.path(), None, &settings).unwrap_err().to_string().contains("no signing key"));
        let other = SigningKey::generate().unwrap();
        std::fs::write(dir.path().join("other.key"), other.to_hex()).unwrap();
        assert!(signing_key(dir.path(), Some(&dir.path().join("other.key")), &settings).unwrap_err().to_string().contains("isn't one the app trusts"));
        std::fs::write(dir.path().join(KEY_FILE), trusted.to_hex()).unwrap();
        assert_eq!(signing_key(dir.path(), None, &settings).unwrap().unwrap().public(), trusted.public());
    }
}
