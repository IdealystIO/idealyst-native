//! Compile the backend's Kotlin runtime classes (`RustNavigator`,
//! `RustDrawerLayout`, etc.) into the APK.
//!
//! `backend-android-mobile` ships a small JVM-side runtime — listener
//! shims, fragment hosts, layout subclasses — that the Rust code
//! reaches via `env.find_class(...)`. The classes live under
//! `crates/backend/android/mobile/runtime/kotlin/io/idealyst/runtime/`
//! and are owned by the backend, but the `run-android` build pipeline
//! is what actually compiles + bundles them into the user's APK.
//!
//! The files are embedded into the CLI binary via `include_str!`, so
//! the CLI carries its own copy and external `cargo install` builds
//! still see them.
//!
//! The runtime imports a handful of `androidx.*` classes
//! (drawerlayout, fragment, recyclerview). We resolve those from the
//! user's local gradle cache rather than fetching from Maven — every
//! Android Studio installation populates the cache by default, so
//! there is no extra setup. Missing artifacts fail loudly with a
//! pointer at the file we expected.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

/// Embedded copy of the backend's Kotlin runtime. Each entry is the
/// filename + raw source; we stage them into the per-build directory
/// before invoking kotlinc.
///
/// Updating this list: add or remove a file under
/// `crates/backend/android/mobile/runtime/kotlin/io/idealyst/runtime/`
/// and mirror the change here. There is no automatic discovery
/// because `include_str!` requires a literal path.
const RUNTIME_KOTLIN_FILES: &[(&str, &str)] = &[
    (
        "Animators.kt",
        include_str!(
            "../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/Animators.kt"
        ),
    ),
    (
        "RustActionBarHelper.kt",
        include_str!(
            "../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustActionBarHelper.kt"
        ),
    ),
    (
        "RustClickListener.kt",
        include_str!(
            "../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustClickListener.kt"
        ),
    ),
    (
        "RustDrawerLayout.kt",
        include_str!(
            "../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustDrawerLayout.kt"
        ),
    ),
    (
        "RustGraphicsCallback.kt",
        include_str!(
            "../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustGraphicsCallback.kt"
        ),
    ),
    (
        "RustHostFragment.kt",
        include_str!(
            "../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustHostFragment.kt"
        ),
    ),
    (
        "RustListAdapter.kt",
        include_str!(
            "../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustListAdapter.kt"
        ),
    ),
    (
        "RustNavigator.kt",
        include_str!(
            "../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustNavigator.kt"
        ),
    ),
    (
        "RustOverlayDismissListener.kt",
        include_str!(
            "../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustOverlayDismissListener.kt"
        ),
    ),
    (
        "RustPopupDismissListener.kt",
        include_str!(
            "../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustPopupDismissListener.kt"
        ),
    ),
    (
        "RustSliderListener.kt",
        include_str!(
            "../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustSliderListener.kt"
        ),
    ),
    (
        "RustStateListener.kt",
        include_str!(
            "../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustStateListener.kt"
        ),
    ),
    (
        "RustTextWatcher.kt",
        include_str!(
            "../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustTextWatcher.kt"
        ),
    ),
    (
        "RustToggleListener.kt",
        include_str!(
            "../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustToggleListener.kt"
        ),
    ),
];

/// AndroidX modules the runtime references directly or transitively.
///
/// Order matters only insofar as the search picks the highest version
/// available; the d8 step consumes the result as a set. Anything the
/// runtime imports via `androidx.*` plus the standard transitive
/// closure (Fragment depends on Activity/Lifecycle/SavedState, etc.).
///
/// Adding a new androidx import to the runtime usually means adding
/// the corresponding (group, artifact) pair here so kotlinc can
/// resolve the symbol and d8 has the class to dex.
const REQUIRED_ANDROIDX: &[(&str, &str)] = &[
    // Directly imported by the runtime .kt files.
    ("androidx.drawerlayout", "drawerlayout"),
    ("androidx.fragment", "fragment"),
    ("androidx.recyclerview", "recyclerview"),
    // Compile-time transitive closure. DrawerLayout extends a
    // CustomView; Fragment extends ComponentActivity's host stack;
    // RecyclerView pulls in core + customview.
    ("androidx.customview", "customview"),
    ("androidx.core", "core"),
    ("androidx.activity", "activity"),
    ("androidx.lifecycle", "lifecycle-common"),
    ("androidx.lifecycle", "lifecycle-runtime"),
    ("androidx.lifecycle", "lifecycle-viewmodel"),
    ("androidx.lifecycle", "lifecycle-viewmodel-savedstate"),
    ("androidx.savedstate", "savedstate"),
    ("androidx.collection", "collection"),
    ("androidx.annotation", "annotation"),
    ("androidx.versionedparcelable", "versionedparcelable"),
    ("androidx.interpolator", "interpolator"),
    ("androidx.loader", "loader"),
    ("androidx.viewpager", "viewpager"),
];

/// Inputs the rest of the build pipeline consumes once kotlin/AAR
/// processing finishes.
pub struct RuntimeArtifacts {
    /// Directory containing the kotlinc-compiled .class files
    /// (`io/idealyst/runtime/*.class`). Hand to d8 alongside the
    /// javac output.
    pub kotlin_class_dir: PathBuf,
    /// AAR `classes.jar` extracts (one per androidx artifact).
    /// d8-friendly inputs.
    pub androidx_jars: Vec<PathBuf>,
    /// Directory of aapt2-generated `R.java` files — one tree per AAR
    /// namespace (plus the main package). Compile alongside the user's
    /// java; the resulting `R$attr`/`R$styleable` classes have real
    /// resource IDs that match the linked APK's resource table.
    pub r_java_dir: PathBuf,
    /// Compiled resource flats from every AAR. Pass to `aapt2 link`
    /// (one `-R` per entry) so the final APK actually contains the
    /// resources the AAR bytecode references.
    pub aar_resource_flats: Vec<PathBuf>,
    /// Java package of each AAR that contributes resources. Pass to
    /// `aapt2 link --extra-packages` so aapt2 emits an `R.java` for
    /// each one in addition to the main package.
    pub aar_extra_packages: Vec<String>,
    /// Kotlin stdlib jar — also d8'd into the final dex so the runtime
    /// can use kotlin language features at runtime.
    pub kotlin_stdlib_jar: PathBuf,
}

/// One resolved androidx artifact. Tracks the original `.aar`/`.jar`
/// source path so the R-class stub generator can extract `R.txt` +
/// `AndroidManifest.xml` from it.
struct ResolvedArtifact {
    artifact: String,
    source: PathBuf,
    /// Path the d8 step consumes. For `.jar` artifacts this equals
    /// `source`; for `.aar`, it's the extracted `classes.jar`.
    classes_jar: PathBuf,
}

/// Stage the runtime .kt files into `build_dir`, locate kotlinc + the
/// required androidx AARs, compile to `build_dir/kotlin-classes/`,
/// extract every AAR's `res/` and compile it to a `.flata` for the
/// later aapt2 link, and return the inputs the rest of the pipeline
/// needs.
pub fn build_runtime(
    build_dir: &Path,
    android_jar: &Path,
    build_tools: &Path,
) -> Result<RuntimeArtifacts> {
    let runtime_src = build_dir.join("kotlin/io/idealyst/runtime");
    fs::create_dir_all(&runtime_src)
        .with_context(|| format!("create {}", runtime_src.display()))?;
    for (name, body) in RUNTIME_KOTLIN_FILES {
        fs::write(runtime_src.join(name), body)
            .with_context(|| format!("write runtime kotlin {}", name))?;
    }

    let kotlin_dist = find_kotlin_dist()?;
    let kotlin_compiler_jar = kotlin_dist.join("lib/kotlin-compiler.jar");
    if !kotlin_compiler_jar.is_file() {
        anyhow::bail!(
            "expected kotlin-compiler.jar at {} — kotlin distribution layout changed?",
            kotlin_compiler_jar.display()
        );
    }
    let kotlin_stdlib_jar = kotlin_dist.join("lib/kotlin-stdlib.jar");
    if !kotlin_stdlib_jar.is_file() {
        anyhow::bail!(
            "expected kotlin-stdlib.jar at {} — kotlin distribution layout changed?",
            kotlin_stdlib_jar.display()
        );
    }

    let aar_cache = build_dir.join("aar-cache");
    fs::create_dir_all(&aar_cache)?;
    let resolved = resolve_androidx(&aar_cache)?;
    let androidx_jars: Vec<PathBuf> = resolved.iter().map(|r| r.classes_jar.clone()).collect();

    // Resource pipeline: pull each AAR's `res/` out, compile it to a
    // `.flata` via aapt2. The final aapt2 link (in lib.rs) consumes
    // these and emits R.java with real resource IDs that match the
    // packed APK's resource table.
    let (aar_resource_flats, aar_extra_packages, r_java_dir) =
        process_aar_resources(&resolved, build_dir, build_tools)?;

    let kotlin_class_dir = build_dir.join("kotlin-classes");
    if kotlin_class_dir.exists() {
        fs::remove_dir_all(&kotlin_class_dir)?;
    }
    fs::create_dir_all(&kotlin_class_dir)?;

    // Classpath: android.jar + all androidx classes.jars + kotlin-stdlib
    // (lets the runtime reference Kotlin types).
    let mut classpath_parts: Vec<String> =
        vec![android_jar.display().to_string(), kotlin_stdlib_jar.display().to_string()];
    for j in &androidx_jars {
        classpath_parts.push(j.display().to_string());
    }
    let classpath = classpath_parts.join(":");

    let runtime_src_arg = runtime_src.display().to_string();
    let class_dir_arg = kotlin_class_dir.display().to_string();
    eprintln!("[run-android] kotlinc → {}", kotlin_class_dir.display());
    let status = Command::new("java")
        .arg("--enable-native-access=ALL-UNNAMED")
        .arg("-jar")
        .arg(&kotlin_compiler_jar)
        .arg("-classpath")
        .arg(&classpath)
        .arg("-d")
        .arg(&class_dir_arg)
        // JDK 8 bytecode matches what javac --release 8 emits and
        // keeps d8 happy across build-tools revisions.
        .arg("-jvm-target")
        .arg("1.8")
        .arg("-no-stdlib")
        .arg(&runtime_src_arg)
        .status()
        .with_context(|| "spawn java -jar kotlin-compiler.jar — is JDK 11+ on your PATH?")?;
    if !status.success() {
        anyhow::bail!("kotlinc exited with {status}");
    }

    Ok(RuntimeArtifacts {
        kotlin_class_dir,
        androidx_jars,
        r_java_dir,
        aar_resource_flats,
        aar_extra_packages,
        kotlin_stdlib_jar,
    })
}

/// Locate the Kotlin distribution we'll invoke. Tries (in order):
///
/// 1. `KOTLIN_HOME` env var (matches the upstream kotlinc convention).
/// 2. Android Studio's bundled distribution
///    (`/Applications/Android Studio.app/Contents/plugins/Kotlin/kotlinc/`).
/// 3. Common Homebrew install paths.
fn find_kotlin_dist() -> Result<PathBuf> {
    if let Ok(h) = std::env::var("KOTLIN_HOME") {
        let p = PathBuf::from(h);
        if p.is_dir() {
            return Ok(p);
        }
    }
    let candidates: [PathBuf; 4] = [
        PathBuf::from("/Applications/Android Studio.app/Contents/plugins/Kotlin/kotlinc"),
        PathBuf::from("/opt/homebrew/opt/kotlin/libexec"),
        PathBuf::from("/usr/local/opt/kotlin/libexec"),
        PathBuf::from("/usr/share/kotlin"),
    ];
    for c in &candidates {
        if c.join("lib/kotlin-compiler.jar").is_file() {
            return Ok(c.clone());
        }
    }
    anyhow::bail!(
        "couldn't find a Kotlin distribution. Install Kotlin (Homebrew: \
         `brew install kotlin`) or set $KOTLIN_HOME. \
         Android Studio also bundles one at \
         /Applications/Android Studio.app/Contents/plugins/Kotlin/kotlinc."
    )
}

/// Resolve every required androidx artifact's source (.aar or .jar)
/// and extract its `classes.jar` (for .aar) under `out_dir`. Uses
/// the user's local gradle cache (`~/.gradle/caches/modules-2/files-2.1/`)
/// as the source.
fn resolve_androidx(out_dir: &Path) -> Result<Vec<ResolvedArtifact>> {
    let cache = gradle_cache_dir()?;
    let mut out = Vec::with_capacity(REQUIRED_ANDROIDX.len());
    for (group, artifact) in REQUIRED_ANDROIDX {
        let source = find_artifact(&cache, group, artifact).with_context(|| {
            format!(
                "missing {}:{} in the gradle cache — \
                 open a project that depends on it in Android Studio once to \
                 populate {}",
                group,
                artifact,
                cache.display()
            )
        })?;
        let classes_jar = extract_classes(&source, out_dir, artifact)?;
        out.push(ResolvedArtifact {
            artifact: (*artifact).to_string(),
            source,
            classes_jar,
        });
    }
    Ok(out)
}

/// Pull each AAR's `res/` directory out, compile it with `aapt2 compile`
/// to a `.flata`, and gather the package names for `--extra-packages`.
/// Returns `(flats, extra_packages, r_java_out_dir)` — the latter is
/// the directory aapt2 will later be told to write generated `R.java`
/// files into when the caller runs `aapt2 link`.
fn process_aar_resources(
    resolved: &[ResolvedArtifact],
    build_dir: &Path,
    build_tools: &Path,
) -> Result<(Vec<PathBuf>, Vec<String>, PathBuf)> {
    let res_root = build_dir.join("aar-res");
    if res_root.exists() {
        fs::remove_dir_all(&res_root)?;
    }
    fs::create_dir_all(&res_root)?;
    let r_java_dir = build_dir.join("aar-r-java");
    if r_java_dir.exists() {
        fs::remove_dir_all(&r_java_dir)?;
    }
    fs::create_dir_all(&r_java_dir)?;

    let mut flats: Vec<PathBuf> = Vec::new();
    let mut extra_packages: Vec<String> = Vec::new();
    for r in resolved {
        // .jar artifacts have no resources; skip them.
        let is_aar = r.source.extension().and_then(|s| s.to_str()) == Some("aar");
        if !is_aar {
            continue;
        }
        // Each AAR gets its own `res/` extraction dir so aapt2 sees
        // a clean tree (it's strict about extraneous files).
        let aar_res_dir = res_root.join(&r.artifact);
        fs::create_dir_all(&aar_res_dir)?;
        // Skip extraction if the AAR has no resource files; aapt2
        // compile on an empty dir succeeds but emits no flat.
        let extracted = extract_aar_res(&r.source, &aar_res_dir)?;
        let Some(pkg) = read_aar_package(&r.source)? else {
            continue;
        };
        extra_packages.push(pkg);
        if !extracted {
            continue;
        }

        let flat_path = res_root.join(format!("{}.flata", r.artifact));
        eprintln!(
            "[run-android] aapt2 compile (AAR {}) → {}",
            r.artifact,
            flat_path.display()
        );
        let status = Command::new(build_tools.join("aapt2"))
            .arg("compile")
            .arg("--dir")
            .arg(&aar_res_dir)
            .arg("-o")
            .arg(&flat_path)
            .status()
            .with_context(|| format!("spawn aapt2 compile for {}", r.artifact))?;
        if !status.success() {
            anyhow::bail!("aapt2 compile {} exited with {status}", r.artifact);
        }
        flats.push(flat_path);
    }
    Ok((flats, extra_packages, r_java_dir))
}

/// Extract just the `res/` subtree from an AAR into `out_dir`. AARs
/// vary in what they ship: some include `res/values/values.xml`, some
/// (Kotlin-only artifacts like `lifecycle-common.jar`) have nothing at
/// all. Returns `true` if any resource file was extracted, `false` if
/// the AAR has no `res/` content (caller skips the aapt2 compile step
/// in that case).
fn extract_aar_res(aar: &Path, out_dir: &Path) -> Result<bool> {
    let staging = out_dir.parent().unwrap().join(format!(
        "{}-extract",
        out_dir.file_name().and_then(|s| s.to_str()).unwrap_or("aar")
    ));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;
    let output = Command::new("unzip")
        .arg("-q")
        .arg("-o")
        .arg(aar)
        .arg("res/*")
        .arg("-d")
        .arg(&staging)
        .output()
        .with_context(|| format!("unzip res/ from {}", aar.display()))?;
    // `unzip` exits 11 ("no matching files") when the AAR has nothing
    // under res/. That's expected — treat it as "no resources" rather
    // than a failure.
    let extracted_res = staging.join("res");
    if !extracted_res.is_dir() {
        let _ = fs::remove_dir_all(&staging);
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Real failure (not just "nothing matched") — surface it.
        if !output.status.success() && output.status.code() != Some(11) && !stderr.is_empty() {
            anyhow::bail!("unzip res/ from {} failed: {}", aar.display(), stderr);
        }
        return Ok(false);
    }
    // Move the extracted res/ tree into `out_dir`.
    if out_dir.exists() {
        fs::remove_dir_all(out_dir)?;
    }
    fs::rename(&extracted_res, out_dir).with_context(|| {
        format!(
            "move extracted res/ from {} to {}",
            staging.display(),
            out_dir.display()
        )
    })?;
    let _ = fs::remove_dir_all(&staging);
    Ok(true)
}

/// Pull an entry out of an AAR (a plain zip), if it exists.
fn read_aar_entry(aar: &Path, entry: &str) -> Result<Option<String>> {
    let output = Command::new("unzip")
        .arg("-p")
        .arg(aar)
        .arg(entry)
        .output()
        .with_context(|| format!("unzip -p {} {}", aar.display(), entry))?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    if text.is_empty() {
        Ok(None)
    } else {
        Ok(Some(text))
    }
}

fn read_aar_package(aar: &Path) -> Result<Option<String>> {
    let Some(manifest) = read_aar_entry(aar, "AndroidManifest.xml")? else {
        return Ok(None);
    };
    let needle = "package=\"";
    let Some(start) = manifest.find(needle) else {
        return Ok(None);
    };
    let rest = &manifest[start + needle.len()..];
    let end = rest
        .find('"')
        .ok_or_else(|| anyhow::anyhow!("unterminated package=\" in {}", aar.display()))?;
    Ok(Some(rest[..end].to_string()))
}

fn gradle_cache_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| anyhow::anyhow!("$HOME is not set"))?;
    let p = home.join(".gradle/caches/modules-2/files-2.1");
    if !p.is_dir() {
        anyhow::bail!(
            "no gradle cache at {}; install Android Studio and open any project once to populate it",
            p.display()
        );
    }
    Ok(p)
}

/// Walk `<cache>/<group>/<artifact>/<version>/<hash>/<artifact>-<version>.{aar|jar}`
/// and pick the highest version that actually has a binary artifact.
/// Skips versions whose hash dirs contain only `.pom`/`.module` metadata
/// — common for KMP-published modules like newer `androidx.collection`
/// where the JVM artifact lives under a sibling `<artifact>-jvm`.
///
/// If the base artifact is metadata-only across every version, falls
/// back to `<artifact>-jvm`. This handles every androidx KMP module
/// uniformly without callers having to know which artifacts are KMP.
fn find_artifact(cache: &Path, group: &str, artifact: &str) -> Result<PathBuf> {
    match find_artifact_in(cache, group, artifact) {
        Ok(p) => Ok(p),
        Err(base_err) => match find_artifact_in(cache, group, &format!("{}-jvm", artifact)) {
            Ok(p) => Ok(p),
            Err(jvm_err) => anyhow::bail!(
                "{}; fallback to {}-jvm also failed: {}",
                base_err,
                artifact,
                jvm_err
            ),
        },
    }
}

fn find_artifact_in(cache: &Path, group: &str, artifact: &str) -> Result<PathBuf> {
    let artifact_root = cache.join(group).join(artifact);
    if !artifact_root.is_dir() {
        anyhow::bail!("{} does not exist", artifact_root.display());
    }
    let mut versions: Vec<(Vec<u64>, PathBuf)> = Vec::new();
    for entry in fs::read_dir(&artifact_root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if let Some(parsed) = parse_version(&name) {
            versions.push((parsed, entry.path()));
        }
    }
    versions.sort_by(|a, b| a.0.cmp(&b.0));
    while let Some((_, version_dir)) = versions.pop() {
        if let Some(p) = find_binary_in_version(&version_dir)? {
            return Ok(p);
        }
    }
    anyhow::bail!(
        "no binary .aar/.jar under any version of {} — only metadata (.pom/.module) found",
        artifact_root.display()
    )
}

fn find_binary_in_version(version_dir: &Path) -> Result<Option<PathBuf>> {
    for hash_entry in fs::read_dir(version_dir)? {
        let hash_entry = hash_entry?;
        if !hash_entry.file_type()?.is_dir() {
            continue;
        }
        for file_entry in fs::read_dir(hash_entry.path())? {
            let file_entry = file_entry?;
            let p = file_entry.path();
            let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
            if ext == "aar" || ext == "jar" {
                return Ok(Some(p));
            }
        }
    }
    Ok(None)
}

fn parse_version(s: &str) -> Option<Vec<u64>> {
    let cleaned: String = s.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    if cleaned.is_empty() {
        return None;
    }
    let parts: Vec<u64> = cleaned
        .split('.')
        .map(|p| p.parse::<u64>().unwrap_or(0))
        .collect();
    if parts.is_empty() { None } else { Some(parts) }
}

/// AARs are zips with a `classes.jar` inside; bare .jar files are
/// already the artifact. Either way, this returns a path the d8 step
/// can consume directly.
fn extract_classes(
    aar_or_jar: &Path,
    out_dir: &Path,
    artifact_name: &str,
) -> Result<PathBuf> {
    let ext = aar_or_jar.extension().and_then(|s| s.to_str()).unwrap_or("");
    if ext == "jar" {
        return Ok(aar_or_jar.to_path_buf());
    }
    // .aar — unzip to find classes.jar, then move it to a stable name
    // under out_dir.
    let staging = out_dir.join(format!("{}-staging", artifact_name));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;
    let status = Command::new("unzip")
        .arg("-q")
        .arg("-o")
        .arg(aar_or_jar)
        .arg("classes.jar")
        .arg("-d")
        .arg(&staging)
        .status()
        .with_context(|| format!("spawn unzip for {}", aar_or_jar.display()))?;
    if !status.success() {
        anyhow::bail!("unzip {} failed with {status}", aar_or_jar.display());
    }
    let inner = staging.join("classes.jar");
    if !inner.is_file() {
        // Some AARs (api-only stubs) ship without classes — that's
        // fine, but log so missing-symbol errors at d8 are easier to
        // diagnose.
        anyhow::bail!(
            "{} has no classes.jar (api-only AAR?) — drop it from REQUIRED_ANDROIDX or vendor it differently",
            aar_or_jar.display(),
        );
    }
    let final_path = out_dir.join(format!("{}-classes.jar", artifact_name));
    if final_path.exists() {
        fs::remove_file(&final_path)?;
    }
    fs::rename(&inner, &final_path)?;
    Ok(final_path)
}
