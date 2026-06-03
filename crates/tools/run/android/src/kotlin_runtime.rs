//! Compile the backend's Kotlin runtime classes (`RustNavigator`,
//! `RustDrawerLayout`, etc.) plus any third-party Kotlin runtime
//! sources contributed by SDK crates into the APK.
//!
//! `backend-android-mobile` ships a small JVM-side runtime — listener
//! shims, fragment hosts, layout subclasses — that the Rust code
//! reaches via `env.find_class(...)`. The classes live under
//! `crates/backend/android/mobile/runtime/kotlin/io/idealyst/runtime/`
//! and are owned by the backend, but the `run-android` build pipeline
//! is what actually compiles + bundles them into the user's APK.
//!
//! The first-party files are embedded into the CLI binary via
//! `include_str!`, so the CLI carries its own copy and external
//! `cargo install` builds still see them.
//!
//! # Third-party SDK contributions
//!
//! Third-party SDKs (e.g. a `webview-android` leaf that ships a
//! `RustWebViewClient.kt`) declare their Kotlin sources in their own
//! `Cargo.toml` under `[package.metadata.idealyst.android]`:
//!
//! ```toml
//! [package.metadata.idealyst.android]
//! runtime_kotlin = ["runtime/kotlin/io/foo/RustFoo.kt"]
//! runtime_java   = ["runtime/java/io/foo/FooBridge.java"]
//! androidx = [
//!     ["androidx.media3", "media3-exoplayer"],
//!     ["androidx.media3", "media3-common"],
//! ]
//! ```
//!
//! `runtime_kotlin` files are staged under the Kotlin source root and
//! compiled by `kotlinc`; `runtime_java` files are staged under a
//! parallel Java extension root and join the user's project Java in
//! the `javac` invocation. Both convention-strip a leading
//! `runtime/kotlin/` or `runtime/java/` (or just `runtime/`) so the
//! staged tree mirrors the Java package hierarchy directly.
//!
//! Paths are resolved relative to the declaring package's manifest
//! directory. We discover them by running `cargo metadata` against
//! the user's wrapper crate (which transitively depends on every
//! SDK), then read the files off disk at build time. Unlike the
//! first-party runtime — which the CLI bakes in via `include_str!` —
//! third-party files must exist on disk during `idealyst run`.
//!
//! # AndroidX resolution
//!
//! The runtime imports a handful of `androidx.*` classes
//! (drawerlayout, fragment, recyclerview). We resolve those from the
//! user's local gradle cache rather than fetching from Maven — every
//! Android Studio installation populates the cache by default, so
//! there is no extra setup. Missing artifacts fail loudly with a
//! pointer at the file we expected. Third-party AndroidX requirements
//! get merged into the same resolution pass (deduped by
//! (group, artifact)). SDKs must declare the full transitive closure
//! they need — we don't read AAR `.pom` files to chase dependencies.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde_json::Value;

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
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/Animators.kt"
        ),
    ),
    (
        "RustActionBarHelper.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustActionBarHelper.kt"
        ),
    ),
    (
        "RustActivityResult.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustActivityResult.kt"
        ),
    ),
    (
        "RustBorderDrawable.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustBorderDrawable.kt"
        ),
    ),
    (
        "RustClickListener.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustClickListener.kt"
        ),
    ),
    (
        "RustCodeBlock.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustCodeBlock.kt"
        ),
    ),
    (
        "RustDrawerLayout.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustDrawerLayout.kt"
        ),
    ),
    (
        "RustFrameCallback.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustFrameCallback.kt"
        ),
    ),
    (
        "RustGraphicsCallback.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustGraphicsCallback.kt"
        ),
    ),
    (
        "RustTextureListener.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustTextureListener.kt"
        ),
    ),
    (
        "RustHostFragment.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustHostFragment.kt"
        ),
    ),
    (
        "RustListAdapter.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustListAdapter.kt"
        ),
    ),
    (
        "RustNavigator.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustNavigator.kt"
        ),
    ),
    (
        "RustKeyListener.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustKeyListener.kt"
        ),
    ),
    (
        "RustOverlayDismissListener.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustOverlayDismissListener.kt"
        ),
    ),
    (
        "RustOverlayKeyListener.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustOverlayKeyListener.kt"
        ),
    ),
    (
        "RustPopupDismissListener.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustPopupDismissListener.kt"
        ),
    ),
    (
        "RustSliderListener.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustSliderListener.kt"
        ),
    ),
    (
        "RustStateListener.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustStateListener.kt"
        ),
    ),
    (
        "RustStickyScrollListener.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustStickyScrollListener.kt"
        ),
    ),
    (
        "RustTextWatcher.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustTextWatcher.kt"
        ),
    ),
    (
        "RustToggleListener.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustToggleListener.kt"
        ),
    ),
    (
        "RustTouchListener.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustTouchListener.kt"
        ),
    ),
    (
        "RustScheduledRunnable.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustScheduledRunnable.kt"
        ),
    ),
    (
        "RustAsyncPoll.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustAsyncPoll.kt"
        ),
    ),
    (
        "RustActivityResult.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustActivityResult.kt"
        ),
    ),
    (
        "RustOverlayPassthrough.kt",
        include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustOverlayPassthrough.kt"
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
    /// (`io/idealyst/runtime/*.class` plus any third-party Kotlin
    /// classes). Hand to d8 alongside the javac output.
    pub kotlin_class_dir: PathBuf,
    /// Directory containing third-party Java sources contributed by
    /// SDKs via `[package.metadata.idealyst.android].runtime_java`.
    /// `None` if no SDK contributed any. When `Some`, pass it as an
    /// additional input dir to `javac` alongside the user's `java/`
    /// tree and the AAR-generated `r_java_dir`.
    pub extension_java_dir: Option<PathBuf>,
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

/// One JVM source file contributed by a third-party SDK. Same shape
/// for both Kotlin (`.kt`) and Java (`.java`) — only the staging root
/// + compile step differ. The framework's own runtime Kotlin files are
/// staged the same way but originate from [`RUNTIME_KOTLIN_FILES`]
/// (embedded via `include_str!`).
struct ExtensionSource {
    /// The crate that declared this file — used only for diagnostics
    /// (conflict messages, "where did this come from?" errors).
    package: String,
    /// Absolute path to the .kt / .java file on disk. The CLI doesn't
    /// embed third-party sources; they have to be present at
    /// `idealyst run` time.
    source_path: PathBuf,
    /// Relative subpath under the language-specific source root where
    /// the file should be staged. Mirrors the directory layout the SDK
    /// author chose. e.g. `io/foo/RustFoo.kt` or `io/foo/FooBridge.java`.
    staged_relpath: PathBuf,
}

/// Bundle of everything `cargo metadata` discovered across the user's
/// transitive dep tree.
#[derive(Default)]
struct DiscoveredExtensions {
    kotlin: Vec<ExtensionSource>,
    java: Vec<ExtensionSource>,
    /// Additional (group, artifact) pairs to feed into AndroidX
    /// resolution. Deduped against [`REQUIRED_ANDROIDX`] and against
    /// itself before resolution runs.
    androidx: Vec<(String, String)>,
}

/// Walk the wrapper's dep tree via `cargo metadata` and collect every
/// package's `[package.metadata.idealyst.android]` block.
///
/// The wrapper is generated under `target/idealyst/.../<project>/android/
/// wrapper/` and depends on the user's project crate, which transitively
/// pulls in every third-party SDK. So `cargo metadata` on the wrapper
/// gives us the full set of SDKs the app will mount at runtime — the
/// exact same scope that `cargo build` will compile.
fn discover_extensions(wrapper_manifest: &Path) -> Result<DiscoveredExtensions> {
    if !wrapper_manifest.is_file() {
        // No wrapper yet — caller hasn't generated one. The framework's
        // own runtime still works without third-party extensions.
        return Ok(DiscoveredExtensions::default());
    }

    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .arg("--manifest-path")
        .arg(wrapper_manifest)
        .output()
        .with_context(|| {
            format!(
                "spawn cargo metadata --manifest-path {}",
                wrapper_manifest.display()
            )
        })?;
    if !output.status.success() {
        anyhow::bail!(
            "cargo metadata failed for {}: {}",
            wrapper_manifest.display(),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    let json: Value = serde_json::from_slice(&output.stdout)
        .with_context(|| "parse cargo metadata JSON")?;

    let mut out = DiscoveredExtensions::default();
    let Some(packages) = json.get("packages").and_then(|v| v.as_array()) else {
        return Ok(out);
    };

    for pkg in packages {
        let Some(android) = pkg
            .pointer("/metadata/idealyst/android")
            .and_then(|v| v.as_object())
        else {
            continue;
        };
        let pkg_name = pkg
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("<unknown>");
        let manifest_path = pkg
            .get("manifest_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("package {} missing manifest_path", pkg_name))?;
        let pkg_dir = Path::new(manifest_path)
            .parent()
            .ok_or_else(|| anyhow::anyhow!("manifest_path {} has no parent", manifest_path))?
            .to_path_buf();

        collect_jvm_sources(android, "runtime_kotlin", pkg_name, &pkg_dir, &mut out.kotlin)?;
        collect_jvm_sources(android, "runtime_java", pkg_name, &pkg_dir, &mut out.java)?;

        if let Some(deps) = android.get("androidx").and_then(|v| v.as_array()) {
            for entry in deps {
                let pair = entry.as_array().ok_or_else(|| {
                    anyhow::anyhow!(
                        "{}: metadata.idealyst.android.androidx entries must be [group, artifact] arrays",
                        pkg_name
                    )
                })?;
                if pair.len() != 2 {
                    anyhow::bail!(
                        "{}: metadata.idealyst.android.androidx entry must have exactly 2 elements (group, artifact)",
                        pkg_name
                    );
                }
                let group = pair[0].as_str().ok_or_else(|| {
                    anyhow::anyhow!("{}: androidx group must be a string", pkg_name)
                })?;
                let artifact = pair[1].as_str().ok_or_else(|| {
                    anyhow::anyhow!("{}: androidx artifact must be a string", pkg_name)
                })?;
                out.androidx.push((group.to_string(), artifact.to_string()));
            }
        }
    }
    Ok(out)
}

/// Pull every entry for one `runtime_*` key (e.g. `"runtime_kotlin"`
/// or `"runtime_java"`) out of an SDK's
/// `[package.metadata.idealyst.android]` block and append them to
/// `sink`. Validates each entry is a string + the referenced file
/// exists on disk; fails loudly with the declaring package name if not.
fn collect_jvm_sources(
    android: &serde_json::Map<String, Value>,
    key: &'static str,
    pkg_name: &str,
    pkg_dir: &Path,
    sink: &mut Vec<ExtensionSource>,
) -> Result<()> {
    let Some(files) = android.get(key).and_then(|v| v.as_array()) else {
        return Ok(());
    };
    for entry in files {
        let rel = entry.as_str().ok_or_else(|| {
            anyhow::anyhow!(
                "{}: metadata.idealyst.android.{} entries must be strings",
                pkg_name,
                key,
            )
        })?;
        let source_path = pkg_dir.join(rel);
        if !source_path.is_file() {
            anyhow::bail!(
                "{} declares runtime source {} (via {}) but the file does not exist on disk",
                pkg_name,
                source_path.display(),
                key,
            );
        }
        sink.push(ExtensionSource {
            package: pkg_name.to_string(),
            source_path,
            staged_relpath: staged_relpath_from(rel)?,
        });
    }
    Ok(())
}

/// Given a `runtime_kotlin`/`runtime_java` entry relative to the SDK's
/// manifest dir, produce the path the file should be staged at under
/// the language-specific source root. Entries are conventionally
/// `runtime/{kotlin,java}/<java/pkg/path>/<File>.{kt,java}`; we strip
/// the `runtime/` (and optional language-name) prefix so the staged
/// tree reflects the Java package hierarchy directly.
fn staged_relpath_from(declared: &str) -> Result<PathBuf> {
    // Reject absolute paths or anything escaping the package dir.
    let p = PathBuf::from(declared);
    if p.is_absolute() {
        anyhow::bail!(
            "runtime source entries must be relative to the package manifest dir (got {})",
            declared,
        );
    }
    for comp in p.components() {
        if matches!(comp, std::path::Component::ParentDir) {
            anyhow::bail!(
                "runtime source entries must not escape the package dir with `..` (got {})",
                declared,
            );
        }
    }
    // Convention: SDKs put files under
    // `runtime/{kotlin,java}/<java/package>/<File>.{kt,java}`. Strip
    // the language-specific prefix first (most specific), then the
    // bare `runtime/` fallback. If neither matches, stage exactly as
    // declared.
    let stripped = p
        .strip_prefix("runtime/kotlin")
        .or_else(|_| p.strip_prefix("runtime/java"))
        .or_else(|_| p.strip_prefix("runtime"))
        .unwrap_or(&p);
    Ok(stripped.to_path_buf())
}

/// Stage the runtime .kt files into `build_dir`, locate kotlinc + the
/// required androidx AARs, compile to `build_dir/kotlin-classes/`,
/// extract every AAR's `res/` and compile it to a `.flata` for the
/// later aapt2 link, and return the inputs the rest of the pipeline
/// needs.
///
/// `wrapper_manifest` is the Cargo.toml of the generated wrapper crate
/// for this build. `cargo metadata` runs against it to discover any
/// third-party SDK Kotlin sources and additional AndroidX requirements.
pub fn build_runtime(
    build_dir: &Path,
    android_jar: &Path,
    build_tools: &Path,
    wrapper_manifest: &Path,
) -> Result<RuntimeArtifacts> {
    let kotlin_src_root = build_dir.join("kotlin");
    fs::create_dir_all(&kotlin_src_root)
        .with_context(|| format!("create {}", kotlin_src_root.display()))?;

    // Stage first-party (framework) runtime files under
    // `io/idealyst/runtime/`.
    let idealyst_runtime_dir = kotlin_src_root.join("io/idealyst/runtime");
    fs::create_dir_all(&idealyst_runtime_dir)
        .with_context(|| format!("create {}", idealyst_runtime_dir.display()))?;
    for (name, body) in RUNTIME_KOTLIN_FILES {
        fs::write(idealyst_runtime_dir.join(name), body)
            .with_context(|| format!("write runtime kotlin {}", name))?;
    }

    // Discover + stage third-party runtime files. Kotlin sources go
    // under `kotlin_src_root`; Java sources under `extension-java/`
    // (kept separate from the user's `java/` tree so we can mix-and-
    // -match per call without disturbing the user's source layout).
    let extensions = discover_extensions(wrapper_manifest)?;
    stage_extension_sources(&extensions.kotlin, &kotlin_src_root, "kotlin")?;

    let extension_java_dir = if extensions.java.is_empty() {
        None
    } else {
        let dir = build_dir.join("extension-java");
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        fs::create_dir_all(&dir)?;
        stage_extension_sources(&extensions.java, &dir, "java")?;
        Some(dir)
    };

    // Build the merged AndroidX list: built-in pairs first (preserves
    // existing resolution order) + any third-party pairs not already
    // present. Dedup is by exact (group, artifact) match.
    let mut androidx_pairs: Vec<(String, String)> = REQUIRED_ANDROIDX
        .iter()
        .map(|(g, a)| ((*g).to_string(), (*a).to_string()))
        .collect();
    for (g, a) in &extensions.androidx {
        if !androidx_pairs.iter().any(|(eg, ea)| eg == g && ea == a) {
            androidx_pairs.push((g.clone(), a.clone()));
        }
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
    let resolved = resolve_androidx(&aar_cache, &androidx_pairs)?;
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

    // kotlinc accepts a directory and recursively finds every .kt file
    // beneath it, so we pass the staged source root (which contains
    // first-party files under `io/idealyst/runtime/` plus any
    // third-party files under their own package directories).
    let kotlin_src_arg = kotlin_src_root.display().to_string();
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
        .arg(&kotlin_src_arg)
        .status()
        .with_context(|| "spawn java -jar kotlin-compiler.jar — is JDK 11+ on your PATH?")?;
    if !status.success() {
        anyhow::bail!("kotlinc exited with {status}");
    }

    Ok(RuntimeArtifacts {
        kotlin_class_dir,
        extension_java_dir,
        androidx_jars,
        r_java_dir,
        aar_resource_flats,
        aar_extra_packages,
        kotlin_stdlib_jar,
    })
}

/// Copy every `sources` entry into `dest_root` at its `staged_relpath`,
/// creating parent dirs as needed. Detects conflicts between SDKs
/// staging to the same target path — silently overwriting would produce
/// confusing class-loading failures at runtime, so we surface them as
/// build errors with both declaring package names.
///
/// `label` is a human-readable language tag ("kotlin" or "java") used
/// only in eprintln + error messages.
fn stage_extension_sources(
    sources: &[ExtensionSource],
    dest_root: &Path,
    label: &str,
) -> Result<()> {
    let mut staged_paths: std::collections::HashMap<PathBuf, String> =
        std::collections::HashMap::new();
    for ext in sources {
        let dest = dest_root.join(&ext.staged_relpath);
        if let Some(prior) = staged_paths.get(&dest) {
            anyhow::bail!(
                "{} runtime path conflict at {}: both `{}` and `{}` declare this file. \
                 SDKs must use distinct Java packages.",
                label,
                dest.display(),
                prior,
                ext.package,
            );
        }
        staged_paths.insert(dest.clone(), ext.package.clone());

        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        fs::copy(&ext.source_path, &dest).with_context(|| {
            format!(
                "stage extension {} source {} → {}",
                label,
                ext.source_path.display(),
                dest.display(),
            )
        })?;
        eprintln!(
            "[run-android] {} runtime extension from {} → {}",
            label,
            ext.package,
            ext.staged_relpath.display()
        );
    }
    Ok(())
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
///
/// `pairs` is the merged (first-party + third-party) list of
/// (group, artifact) tuples. Order matters only for deterministic
/// classpath assembly; resolution itself doesn't depend on it.
fn resolve_androidx(
    out_dir: &Path,
    pairs: &[(String, String)],
) -> Result<Vec<ResolvedArtifact>> {
    let cache = gradle_cache_dir()?;
    let mut out = Vec::with_capacity(pairs.len());
    for (group, artifact) in pairs {
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
            artifact: artifact.clone(),
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
