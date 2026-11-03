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
//! The first-party files are read from whichever `backend-android-mobile`
//! the app's wrapper actually resolves (found through `cargo metadata`,
//! like the third-party sources below): its `runtime/kotlin/` tree, which
//! every published version of the crate carries. The Kotlin therefore
//! always matches the JNI code it is compiled against — embedding it in
//! the CLI would tie it to the commit the CLI was built from rather than
//! to the app's backend, and need a hand-kept file list.
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
//!     ["androidx.media3", "media3-exoplayer", "1.1.1"],
//!     ["androidx.fragment", "fragment"],  # pinned by the framework: no version
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
//! SDK), then read the files off disk at build time.
//!
//! # AndroidX resolution
//!
//! Every build links exactly [`PINNED_ANDROIDX`]: the libraries the Kotlin
//! runtime (`io.idealyst.runtime.*`) and the wrapper's `FragmentActivity`
//! are compiled and tested against, as a complete closure — each entry an
//! exact version, file type and SHA-1. A clean machine therefore links the
//! same bytes as a developer's machine, whatever Google has published
//! since. Each file is taken from the gradle cache or the idealyst cache at
//! its exact `<version>/<sha1>/` path, else downloaded once and refused
//! unless its SHA-1 matches the pin (`crate::maven::fetch_pinned`).
//!
//! An SDK's `androidx` entries are Maven coordinates (any group, not only
//! `androidx.*`):
//!
//! - `[group, artifact, version]` — that exact version. Its transitive
//!   runtime closure is resolved from the published metadata
//!   (`crate::maven::resolve_graph`): Gradle module metadata where the POM
//!   defers to it — so a Kotlin-multiplatform umbrella resolves to its
//!   `-android`/`-jvm` variant and that variant's dependencies — else the
//!   POM. Where two requests disagree the higher version wins, as in Gradle.
//! - `[group, artifact]` — only for a library the framework pins; it states
//!   the SDK relies on it and resolves to the pin. Anything else needs a
//!   version: picking "the newest" would make the build depend on the day
//!   it ran.
//!
//! A dependency on a pinned library always gets the pin; one asking for a
//! higher version is reported on stderr. Declaring a pinned library at a
//! different version fails the build. An AAR without a `classes.jar` (a
//! resources-only library) contributes its resources and no classes.
//!
//! `IDEALYST_MAVEN_CACHE` moves the idealyst cache (default
//! `~/.idealyst/maven`); `IDEALYST_NO_GRADLE_CACHE=1` ignores the gradle
//! cache, so a build behaves like one on a machine without Android Studio.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::maven::{Coord, Pin};

/// The crate whose `runtime/kotlin/` tree is the framework's own Kotlin
/// runtime (`RustNavigator`, `RustDrawerLayout`, the listener shims…).
const BACKEND_PACKAGE: &str = "backend-android-mobile";

/// Where that tree lives, relative to the backend's manifest directory.
/// Files under it are staged at their path relative to it, which is
/// their Java package path (`io/idealyst/runtime/RustNavigator.kt`).
const BACKEND_KOTLIN_DIR: &str = "runtime/kotlin";

/// The AndroidX closure every APK links, pinned: the libraries the runtime
/// `.kt` files import (drawerlayout, fragment, recyclerview), the wrapper's
/// `FragmentActivity` base, and every compile/runtime dependency their
/// published metadata declares, at the versions that metadata resolves to.
///
/// Why this line: it is the set the Kotlin runtime has been built, run and
/// device-tested against (byte-identical to what Android Studio's gradle
/// cache holds on the development machines — the SHA-1s are those cache
/// directories), and it predates the Kotlin-multiplatform split of
/// lifecycle (2.8) / savedstate (1.3) and `androidx.navigationevent`
/// (activity 1.12), so the closure is 29 plain libraries. Only `collection`,
/// `annotation` and `kotlinx-coroutines-core` were already multiplatform;
/// their `-jvm` variants are pinned, which [`crate::maven::Pin::answers`]
/// matches against requests for the umbrella name.
///
/// Left out on purpose (`crate::maven::is_provided`): the Kotlin stdlib (the
/// Kotlin distribution's is dexed), `org.jetbrains:annotations`, and
/// `androidx.profileinstaller` (reached only through its own manifest
/// entries, which the APK doesn't merge).
///
/// Moving a version means moving the whole closure: re-run
/// `pinned_closure_is_complete_against_published_metadata` (network,
/// `--ignored`), which checks every pinned library's runtime dependencies
/// are pinned at that version or higher, and test an app on a device.
pub const PINNED_ANDROIDX: &[Pin] = &[
    // Directly imported by the runtime .kt files.
    pin("androidx.drawerlayout", "drawerlayout", "1.2.0", "aar", "7da3dd40844ad410dd3d75842ef2b00187829c77"),
    pin("androidx.fragment", "fragment", "1.6.2", "aar", "ae9b6401f983c8479327acc1a5dfe48da7066a4e"),
    pin("androidx.recyclerview", "recyclerview", "1.3.2", "aar", "e1d76099dbbd88ee0ee7c39cfa96ed0ea01bc8ad"),
    // Fragment's host stack: FragmentActivity extends ComponentActivity.
    pin("androidx.activity", "activity", "1.8.2", "aar", "bce54c84024b17c34f9d4cdeaf1ce39d3e750c9e"),
    pin("androidx.core", "core", "1.12.0", "aar", "5aa3088ed93ba8ebaea2b8a83befdfa829339e7a"),
    // RecyclerView's constructor calls `PoolingContainer`, which walks views
    // with the `androidx.core.view.ViewGroupKt`/`ViewKt` extensions from
    // core-ktx. Lockstep with core: core-ktx 1.13+ is an empty jar whose
    // extensions moved into core 1.13.
    pin("androidx.customview", "customview-poolingcontainer", "1.1.0", "aar", "95994b41b2e6e8c98b56f8cf96269495cc2855b1"),
    pin("androidx.core", "core-ktx", "1.12.0", "aar", "1b80a40d6f34eb52c8de87c186feb57aa2a43ada"),
    pin("androidx.customview", "customview", "1.1.0", "aar", "0a030aaf64c5419a89f7d91a33755c4629a43471"),
    pin("androidx.lifecycle", "lifecycle-common", "2.6.2", "jar", "10f354fdb64868baecd67128560c5a0d6312c495"),
    pin("androidx.lifecycle", "lifecycle-runtime", "2.6.2", "aar", "f0ef27053faa89b3ec06c4f118cea0a15edde35b"),
    pin("androidx.lifecycle", "lifecycle-viewmodel", "2.6.2", "aar", "fa35bfdbbe0e7df1573e377cbdd15de35176141d"),
    pin("androidx.lifecycle", "lifecycle-viewmodel-savedstate", "2.6.2", "aar", "483115c35b2926c3780dfdaf3f5d0342855e73b1"),
    // Fragment holds its view-lifecycle owner in a `MutableLiveData`
    // (`-core`); `MediatorLiveData` is in the full `lifecycle-livedata`.
    pin("androidx.lifecycle", "lifecycle-livedata-core", "2.6.2", "aar", "23f66885a260f0a2cd6540927ea8795484afd2da"),
    pin("androidx.lifecycle", "lifecycle-livedata", "2.6.2", "aar", "e7f6d9e232bf035af69c030917c1ccda8e840b57"),
    pin("androidx.savedstate", "savedstate", "1.2.1", "aar", "e364fc9dec32aa569d855f85864e50250318a093"),
    // `LifecycleRegistry` (every ComponentActivity/Fragment) instantiates
    // `FastSafeIterableMap` from core-runtime; its base `SafeIterableMap`
    // lives in core-common.
    pin("androidx.arch.core", "core-runtime", "2.2.0", "aar", "39705982680d78556b679ef9d5400d4f17509b0b"),
    pin("androidx.arch.core", "core-common", "2.2.0", "jar", "5e1b8b81dfd5f52c56a8d53b18ca759c19a301f3"),
    // Kotlin-multiplatform already at these versions: the classes live in
    // the `-jvm` variant (`collection` 1.4's `SimpleArrayMap` has the
    // `(int,int,DefaultConstructorMarker)` ctor ComponentActivity calls).
    pin("androidx.collection", "collection-jvm", "1.4.0", "jar", "e209fb7bd1183032f55a0408121c6251a81acb49"),
    pin("androidx.annotation", "annotation-jvm", "1.8.0", "jar", "b8a16fe526014b7941c1debaccaf9c5153692dbb"),
    pin("androidx.annotation", "annotation-experimental", "1.3.0", "aar", "5087c6f545117dcd474e69e1a93cacec9d7334af"),
    pin("androidx.versionedparcelable", "versionedparcelable", "1.1.1", "aar", "18e18071e7715679e0593c570217edeb5ca48b9e"),
    pin("androidx.interpolator", "interpolator", "1.0.0", "aar", "8a01fa254a23b9388571eb6334b03707c7d122d7"),
    pin("androidx.loader", "loader", "1.1.0", "aar", "49591a915750c6483e1791daad039e0af71cb91c"),
    pin("androidx.viewpager", "viewpager", "1.0.0", "aar", "1f90e13820f96c2fb868f9674079a551678d68b2"),
    // `ComponentActivity.reportFullyDrawn` calls `androidx.tracing.Trace`.
    pin("androidx.tracing", "tracing", "1.0.0", "aar", "69cef34711dffbc517e7a0a419ec4a98f84f496a"),
    // core's `ResolvableFuture`/`ListenableFuture` users.
    pin("androidx.concurrent", "concurrent-futures", "1.0.0", "jar", "c1e77e3ee6f4643b77496a1ddf7a2eef1aefdaa1"),
    pin("com.google.guava", "listenablefuture", "1.0", "jar", "c949a840a6acbc5268d088e47b04177bf90b3cad"),
    // Lifecycle 2.6 exposes its state as a `StateFlow` and drives
    // `lifecycleScope` on `Dispatchers.Main.immediate`.
    // `kotlinx-coroutines-android` registers that `Main` dispatcher through a
    // `MainDispatcherFactory` ServiceLoader entry. The pair moves together.
    pin("org.jetbrains.kotlinx", "kotlinx-coroutines-core-jvm", "1.7.3", "jar", "2b09627576f0989a436a00a4a54b55fa5026fb86"),
    pin("org.jetbrains.kotlinx", "kotlinx-coroutines-android", "1.7.3", "jar", "38d9cad3a0b03a10453b56577984bdeb48edeed5"),
];

const fn pin(
    group: &'static str,
    artifact: &'static str,
    version: &'static str,
    ext: &'static str,
    sha1: &'static str,
) -> Pin {
    Pin { group, artifact, version, ext, sha1 }
}

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
    /// `<group>.<artifact>` — unique across the closure, so it names the
    /// artifact's extraction directories (`core` exists in several groups).
    label: String,
    source: PathBuf,
    /// Path the d8 step consumes. For `.jar` artifacts this equals
    /// `source`; for `.aar`, it's the extracted `classes.jar`, or `None`
    /// when the AAR ships no classes.
    classes_jar: Option<PathBuf>,
}

/// One `androidx` entry of an SDK's `[package.metadata.idealyst.android]`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AndroidxDecl {
    /// The declaring crate, for diagnostics.
    package: String,
    group: String,
    artifact: String,
    /// `None` for the two-element form, valid only for a pinned library.
    version: Option<String>,
}

/// One JVM source file contributed by a third-party SDK. Same shape
/// for both Kotlin (`.kt`) and Java (`.java`) — only the staging root
/// + compile step differ. The framework's own runtime Kotlin files are
/// staged the same way; they come from [`BACKEND_PACKAGE`]'s
/// [`BACKEND_KOTLIN_DIR`] rather than from a metadata declaration.
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
    /// The framework's own Kotlin runtime, from the resolved
    /// [`BACKEND_PACKAGE`]. Empty only when the wrapper doesn't depend on
    /// the backend, which `build_runtime` refuses.
    backend_kotlin: Vec<ExtensionSource>,
    kotlin: Vec<ExtensionSource>,
    java: Vec<ExtensionSource>,
    /// SDK-declared Maven coordinates, resolved on top of
    /// [`PINNED_ANDROIDX`] (see [`declared_roots`]).
    androidx: Vec<AndroidxDecl>,
}

/// Run `cargo metadata` on the wrapper: the dependency graph every
/// `[package.metadata.idealyst.android]` block is read from (Kotlin/Java
/// sources and AndroidX here, manifest fragments and resources in
/// `sdk_manifest`). Run once per build and shared.
///
/// The wrapper is generated under `target/idealyst/.../<project>/android/
/// wrapper/` and depends on the user's project crate, which transitively
/// pulls in every third-party SDK. So `cargo metadata` on the wrapper
/// gives us the full set of SDKs the app will mount at runtime — the
/// exact same scope that `cargo build` will compile.
pub fn cargo_metadata(wrapper_manifest: &Path) -> Result<Value> {
    if !wrapper_manifest.is_file() {
        anyhow::bail!(
            "no wrapper crate at {} — the Android build reads its dependency graph from it",
            wrapper_manifest.display(),
        );
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
    serde_json::from_slice(&output.stdout).with_context(|| "parse cargo metadata JSON")
}

/// Everything the Kotlin/Java/AndroidX side collects from
/// [`cargo_metadata`]'s JSON.
fn extensions_from_metadata(json: &Value) -> Result<DiscoveredExtensions> {
    let mut out = DiscoveredExtensions::default();
    let Some(packages) = json.get("packages").and_then(|v| v.as_array()) else {
        return Ok(out);
    };

    for pkg in packages {
        if pkg.get("name").and_then(|v| v.as_str()) == Some(BACKEND_PACKAGE) {
            let manifest_path = pkg
                .get("manifest_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("{BACKEND_PACKAGE} has no manifest_path"))?;
            let dir = Path::new(manifest_path)
                .parent()
                .ok_or_else(|| anyhow::anyhow!("manifest_path {manifest_path} has no parent"))?;
            if !out.backend_kotlin.is_empty() {
                anyhow::bail!(
                    "the wrapper resolves {BACKEND_PACKAGE} twice; its Kotlin runtime would be ambiguous"
                );
            }
            collect_backend_kotlin(&dir.join(BACKEND_KOTLIN_DIR), &mut out.backend_kotlin)?;
        }
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
                out.androidx.push(parse_androidx_entry(pkg_name, entry)?);
            }
        }
    }
    Ok(out)
}

/// One `androidx` entry: `[group, artifact, version]`, or `[group,
/// artifact]` (checked against the pins by [`declared_roots`]).
fn parse_androidx_entry(pkg_name: &str, entry: &Value) -> Result<AndroidxDecl> {
    let parts: Option<Vec<&str>> =
        entry.as_array().and_then(|a| a.iter().map(|v| v.as_str()).collect());
    match parts.as_deref() {
        Some([group, artifact]) => Ok(AndroidxDecl {
            package: pkg_name.to_string(),
            group: group.to_string(),
            artifact: artifact.to_string(),
            version: None,
        }),
        Some([group, artifact, version]) => Ok(AndroidxDecl {
            package: pkg_name.to_string(),
            group: group.to_string(),
            artifact: artifact.to_string(),
            version: Some(version.to_string()),
        }),
        _ => anyhow::bail!(
            "{pkg_name}: metadata.idealyst.android.androidx entries must be \
             [\"group\", \"artifact\", \"version\"] string arrays (got {entry})"
        ),
    }
}

/// The coordinates to resolve on top of `pins`: every declaration of an
/// unpinned library. A declaration of a pinned library must not carry
/// another version — the framework's Kotlin runtime is tested only against
/// the pin — and an unpinned one must carry a version, so the build never
/// depends on what a repository published last.
fn declared_roots(decls: &[AndroidxDecl], pins: &[Pin]) -> Result<Vec<Coord>> {
    let mut roots: Vec<Coord> = Vec::new();
    for d in decls {
        if let Some(pin) = crate::maven::pin_for(pins, &d.group, &d.artifact) {
            if let Some(v) = &d.version {
                if v != pin.version {
                    anyhow::bail!(
                        "{} declares {}:{}:{v}, but the framework pins {} — its Kotlin runtime \
                         is built and tested against that version. Declare [\"{}\", \"{}\"] \
                         (or the pinned version) instead",
                        d.package,
                        d.group,
                        d.artifact,
                        pin.coord(),
                        d.group,
                        d.artifact,
                    );
                }
            }
            continue;
        }
        let Some(version) = &d.version else {
            anyhow::bail!(
                "{} declares [\"{}\", \"{}\"] without a version. Only libraries the framework \
                 pins may omit it; add the version the SDK is tested against: \
                 [\"{}\", \"{}\", \"<version>\"]",
                d.package,
                d.group,
                d.artifact,
                d.group,
                d.artifact,
            );
        };
        let c = Coord::new(&d.group, &d.artifact, version);
        if !roots.contains(&c) {
            roots.push(c);
        }
    }
    roots.sort();
    Ok(roots)
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

/// Every `.kt` file under the backend's Kotlin tree, staged at its path
/// relative to the tree. Sorted, so the staged set is stable run to run.
fn collect_backend_kotlin(root: &Path, sink: &mut Vec<ExtensionSource>) -> Result<()> {
    fn walk(root: &Path, dir: &Path, sink: &mut Vec<ExtensionSource>) -> Result<()> {
        for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
            let path = entry?.path();
            if path.is_dir() {
                walk(root, &path, sink)?;
            } else if path.extension().is_some_and(|e| e == "kt") {
                sink.push(ExtensionSource {
                    package: BACKEND_PACKAGE.to_string(),
                    staged_relpath: path.strip_prefix(root).expect("walked under root").to_path_buf(),
                    source_path: path,
                });
            }
        }
        Ok(())
    }
    let start = sink.len();
    walk(root, root, sink)?;
    sink[start..].sort_by(|a, b| a.staged_relpath.cmp(&b.staged_relpath));
    if sink.len() == start {
        anyhow::bail!("{BACKEND_PACKAGE} has no Kotlin runtime under {}", root.display());
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
/// `metadata` is [`cargo_metadata`] of the generated wrapper crate at
/// `wrapper_manifest`: the graph third-party SDK Kotlin sources and
/// additional AndroidX requirements are discovered from.
pub fn build_runtime(
    build_dir: &Path,
    android_jar: &Path,
    build_tools: &Path,
    wrapper_manifest: &Path,
    metadata: &Value,
) -> Result<RuntimeArtifacts> {
    let kotlin_src_root = build_dir.join("kotlin");
    fs::create_dir_all(&kotlin_src_root)
        .with_context(|| format!("create {}", kotlin_src_root.display()))?;

    // Discover + stage the framework's runtime (from the backend the
    // wrapper resolves) and third-party runtime files, in one pass so a
    // third-party file that collides with a framework one is reported.
    // Kotlin sources go under `kotlin_src_root`; Java sources under
    // `extension-java/` (kept separate from the user's `java/` tree so we
    // can mix-and-match per call without disturbing the user's source
    // layout).
    let extensions = extensions_from_metadata(metadata)?;
    if extensions.backend_kotlin.is_empty() {
        anyhow::bail!(
            "{} does not depend on {BACKEND_PACKAGE}, so there is no Kotlin runtime to compile",
            wrapper_manifest.display(),
        );
    }
    let kotlin: Vec<&ExtensionSource> =
        extensions.backend_kotlin.iter().chain(&extensions.kotlin).collect();
    stage_extension_sources(&kotlin, &kotlin_src_root, "kotlin")?;

    let extension_java_dir = if extensions.java.is_empty() {
        None
    } else {
        let dir = build_dir.join("extension-java");
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        fs::create_dir_all(&dir)?;
        stage_extension_sources(&extensions.java.iter().collect::<Vec<_>>(), &dir, "java")?;
        Some(dir)
    };

    // The pinned closure plus whatever SDKs declare beyond it.
    let declared = declared_roots(&extensions.androidx, PINNED_ANDROIDX)?;

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
    let resolved = resolve_androidx(&aar_cache, &declared)?;
    let androidx_jars: Vec<PathBuf> = resolved.iter().filter_map(|r| r.classes_jar.clone()).collect();

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
    sources: &[&ExtensionSource],
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
        // The framework's own runtime is every build's baseline; only a
        // third-party contribution is worth a line.
        if ext.package != BACKEND_PACKAGE {
            eprintln!(
                "[run-android] {} runtime extension from {} → {}",
                label,
                ext.package,
                ext.staged_relpath.display()
            );
        }
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

/// Resolve [`PINNED_ANDROIDX`] plus the closure of `declared`, and extract
/// each `.aar`'s `classes.jar` under `out_dir`. Pinned libraries come first,
/// in list order, then the declared closure in coordinate order — a stable
/// classpath. Where each file comes from is `crate::maven`'s business: the
/// gradle cache, the idealyst cache, else a verified download.
fn resolve_androidx(out_dir: &Path, declared: &[Coord]) -> Result<Vec<ResolvedArtifact>> {
    let caches = crate::maven::Caches {
        gradle: crate::maven::gradle_cache_dir(),
        idealyst: crate::maven::idealyst_maven_cache()?,
    };
    let sources = locate_androidx(&crate::maven::Curl, crate::maven::REPOSITORIES, &caches, PINNED_ANDROIDX, declared)?;
    let mut out = Vec::with_capacity(sources.len());
    for (coord, source) in sources {
        let label = format!("{}.{}", coord.group, coord.artifact);
        let classes_jar = extract_classes(&source, out_dir, &label)?;
        out.push(ResolvedArtifact { label, source, classes_jar });
    }
    Ok(out)
}

/// The `.aar`/`.jar` of every pin, then of every library `declared` needs
/// beyond the pins, each with the coordinate it was taken from.
fn locate_androidx(
    fetcher: &dyn crate::maven::Fetch,
    repos: &[&str],
    caches: &crate::maven::Caches,
    pins: &[Pin],
    declared: &[Coord],
) -> Result<Vec<(Coord, PathBuf)>> {
    let mut out = Vec::new();
    for pin in pins {
        let p = crate::maven::fetch_pinned(fetcher, repos, caches, pin).with_context(|| {
            format!(
                "{} is in neither the gradle cache nor {}, and downloading it failed. \
                 Connect to the network for one build (it is cached afterwards)",
                pin.coord(),
                caches.idealyst.display()
            )
        })?;
        out.push((pin.coord(), p));
    }
    if !declared.is_empty() {
        let resolver = crate::maven::Resolver::new(fetcher, repos, caches);
        let resolution = crate::maven::resolve_graph(&resolver, pins, declared)
            .context("resolving the Android libraries SDKs declare")?;
        for note in &resolution.pin_overrides {
            eprintln!("[run-android] {note}; using the pinned version");
        }
        for node in resolution.nodes {
            if let Some(binary) = node.binary {
                out.push((node.coord, binary));
            }
        }
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
        let aar_res_dir = res_root.join(&r.label);
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

        let flat_path = res_root.join(format!("{}.flata", r.label));
        eprintln!(
            "[run-android] aapt2 compile (AAR {}) → {}",
            r.label,
            flat_path.display()
        );
        let status = Command::new(build_tools.join("aapt2"))
            .arg("compile")
            .arg("--dir")
            .arg(&aar_res_dir)
            .arg("-o")
            .arg(&flat_path)
            .status()
            .with_context(|| format!("spawn aapt2 compile for {}", r.label))?;
        if !status.success() {
            anyhow::bail!("aapt2 compile {} exited with {status}", r.label);
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

/// AARs are zips with a `classes.jar` inside; bare .jar files are
/// already the artifact. Either way, this returns a path the d8 step
/// can consume directly — or `None` for an AAR with no `classes.jar`.
///
/// A library may legitimately ship no classes: a resources-only AAR, or a
/// Kotlin-multiplatform umbrella (`lifecycle-runtime` 2.8+), whose AAR holds
/// only an `AndroidManifest.xml` because the classes live in its `-android`
/// variant. Such an AAR contributes its resources (if any) and nothing to
/// the classpath.
fn extract_classes(aar_or_jar: &Path, out_dir: &Path, label: &str) -> Result<Option<PathBuf>> {
    let ext = aar_or_jar.extension().and_then(|s| s.to_str()).unwrap_or("");
    if ext == "jar" {
        return Ok(Some(aar_or_jar.to_path_buf()));
    }
    let listing = Command::new("unzip")
        .arg("-Z1")
        .arg(aar_or_jar)
        .output()
        .with_context(|| format!("spawn unzip -Z1 for {}", aar_or_jar.display()))?;
    if !listing.status.success() {
        anyhow::bail!(
            "{} is not a readable AAR: {}",
            aar_or_jar.display(),
            String::from_utf8_lossy(&listing.stderr).trim()
        );
    }
    if !zip_listing_has_classes(&String::from_utf8_lossy(&listing.stdout)) {
        return Ok(None);
    }
    // Unzip to find classes.jar, then move it to a stable name under out_dir.
    let staging = out_dir.join(format!("{label}-staging"));
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
    let final_path = out_dir.join(format!("{label}-classes.jar"));
    if final_path.exists() {
        fs::remove_file(&final_path)?;
    }
    fs::rename(staging.join("classes.jar"), &final_path)?;
    let _ = fs::remove_dir_all(&staging);
    Ok(Some(final_path))
}

/// Whether an `unzip -Z1` listing (one entry per line) has a top-level
/// `classes.jar`.
fn zip_listing_has_classes(listing: &str) -> bool {
    listing.lines().any(|l| l.trim_end() == "classes.jar")
}

#[cfg(test)]
mod tests {
    use super::{
        declared_roots, extensions_from_metadata, extract_classes, locate_androidx,
        parse_androidx_entry, stage_extension_sources, zip_listing_has_classes, AndroidxDecl,
        ExtensionSource, BACKEND_KOTLIN_DIR, BACKEND_PACKAGE, PINNED_ANDROIDX,
    };
    use crate::maven::tests::{publish_kmp, FakeRepo, NoNetwork, REPO};
    use crate::maven::{Caches, Coord, Pin};
    use std::path::{Path, PathBuf};

    /// `cargo metadata` JSON naming one package, the way the wrapper's
    /// graph reports `backend-android-mobile`.
    fn metadata_with_backend(manifest: &Path) -> serde_json::Value {
        serde_json::json!({ "packages": [
            { "name": "user-app", "manifest_path": "/app/Cargo.toml", "metadata": null },
            { "name": BACKEND_PACKAGE, "manifest_path": manifest, "metadata": null },
        ]})
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("run-android-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn staged(root: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push(p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
                }
            }
        }
        out.sort();
        out
    }

    /// Regression: the backend's Kotlin runtime used to be a hand-kept
    /// `include_str!` list in this crate, and `RustUnderlineSpan.kt` —
    /// which `imp/primitives/text.rs` reaches with `find_class` — was never
    /// added to it, so no APK carried the class. Every `.kt` file in the
    /// backend's tree must now be staged, with nothing to keep in sync.
    #[test]
    fn regression_every_backend_kotlin_file_is_staged() {
        let backend = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../backend/android/mobile");
        let Ok(backend) = backend.canonicalize() else {
            return; // a packaged crate, outside the workspace
        };
        let found = extensions_from_metadata(&metadata_with_backend(&backend.join("Cargo.toml"))).unwrap();
        let out = scratch("every-kt");
        stage_extension_sources(&found.backend_kotlin.iter().collect::<Vec<_>>(), &out, "kotlin").unwrap();

        let tree = backend.join(BACKEND_KOTLIN_DIR).join("io/idealyst/runtime");
        let mut expected: Vec<String> = std::fs::read_dir(&tree)
            .unwrap()
            .map(|e| format!("io/idealyst/runtime/{}", e.unwrap().file_name().to_string_lossy()))
            .filter(|n| n.ends_with(".kt"))
            .collect();
        expected.sort();
        assert!(expected.contains(&"io/idealyst/runtime/RustUnderlineSpan.kt".to_string()));
        assert_eq!(staged(&out), expected);
        std::fs::remove_dir_all(&out).unwrap();
    }

    /// The runtime comes from the backend the wrapper RESOLVES, not from a
    /// copy fixed when the CLI was built: a CLI released on its own schedule
    /// must still compile the Kotlin that matches the app's JNI code.
    #[test]
    fn the_kotlin_runtime_comes_from_the_resolved_backend() {
        let fake = scratch("fake-backend");
        std::fs::create_dir_all(fake.join("runtime/kotlin/io/idealyst/runtime")).unwrap();
        std::fs::write(fake.join("runtime/kotlin/io/idealyst/runtime/OnlyThis.kt"), "class OnlyThis").unwrap();
        std::fs::write(fake.join("runtime/kotlin/io/idealyst/runtime/notes.txt"), "not kotlin").unwrap();

        let found = extensions_from_metadata(&metadata_with_backend(&fake.join("Cargo.toml"))).unwrap();
        let names: Vec<_> = found.backend_kotlin.iter().map(|e| e.staged_relpath.clone()).collect();
        assert_eq!(names, [PathBuf::from("io/idealyst/runtime/OnlyThis.kt")]);
        std::fs::remove_dir_all(&fake).unwrap();
    }

    #[test]
    fn a_graph_without_the_backend_yields_no_runtime() {
        let json = serde_json::json!({ "packages": [
            { "name": "user-app", "manifest_path": "/app/Cargo.toml", "metadata": null },
        ]});
        assert!(extensions_from_metadata(&json).unwrap().backend_kotlin.is_empty());
    }

    /// A third-party SDK shipping a file at a framework runtime path would
    /// silently replace the framework's class; staging both in one pass
    /// turns that into an error naming both packages.
    #[test]
    fn a_third_party_file_colliding_with_the_runtime_is_an_error() {
        let src = scratch("collide-src");
        std::fs::write(src.join("A.kt"), "class A").unwrap();
        let at = |package: &str| ExtensionSource {
            package: package.to_string(),
            source_path: src.join("A.kt"),
            staged_relpath: PathBuf::from("io/idealyst/runtime/RustNavigator.kt"),
        };
        let (framework, sdk) = (at(BACKEND_PACKAGE), at("some-sdk"));
        let out = scratch("collide-out");
        let err = stage_extension_sources(&[&framework, &sdk], &out, "kotlin").unwrap_err();
        assert!(err.to_string().contains("some-sdk"), "{err}");
        std::fs::remove_dir_all(&src).unwrap();
        std::fs::remove_dir_all(&out).unwrap();
    }

    fn lists(group: &str, artifact: &str) -> bool {
        PINNED_ANDROIDX.iter().any(|p| p.group == group && p.artifact == artifact)
    }

    /// Regression guard for the navigator-host transitive closure.
    ///
    /// The whiteboard (and any navigator app) hosts each screen as a Fragment
    /// under a `FragmentActivity`. Standing that up pulls a chain of androidx
    /// modules that are NOT imported by the runtime `.kt` files and so are
    /// invisible to a casual reading of the list — each one surfaced, in turn,
    /// as a `ClassNotFoundException` at app launch on a real device:
    ///   - `androidx.arch.core:core-runtime` → `FastSafeIterableMap`
    ///     (LifecycleRegistry's observer map)
    ///   - `androidx.lifecycle:lifecycle-livedata-core` → `MutableLiveData`
    ///     (Fragment's view-lifecycle owner)
    ///   - `org.jetbrains.kotlinx:kotlinx-coroutines-core` → `StateFlowKt`
    ///     (modern lifecycle exposes state as a StateFlow)
    /// If any is dropped, navigator apps crash on launch.
    #[test]
    fn fragment_activity_transitive_closure_is_dexed() {
        for (group, artifact) in [
            ("androidx.arch.core", "core-runtime"),
            ("androidx.arch.core", "core-common"),
            ("androidx.lifecycle", "lifecycle-livedata-core"),
            ("org.jetbrains.kotlinx", "kotlinx-coroutines-core-jvm"),
            ("org.jetbrains.kotlinx", "kotlinx-coroutines-android"),
        ] {
            assert!(
                lists(group, artifact),
                "PINNED_ANDROIDX must include {group}:{artifact} — without it a \
                 FragmentActivity navigator host crashes at launch with a \
                 ClassNotFoundException for one of its classes",
            );
        }
    }

    /// Regression: every `flat_list` on Android crashed at `RecyclerView.<init>`
    /// with `NoClassDefFoundError: androidx/customview/poolingcontainer/
    /// PoolingContainer` because RecyclerView's runtime-scope dependency was
    /// never dexed. The lists below are the dependencies RecyclerView 1.3.2's
    /// POM and customview-poolingcontainer 1.1.0's POM declare (KMP bases
    /// mapped to their pinned `-jvm` variant; `kotlin-stdlib` comes from the
    /// Kotlin distribution, not this list).
    #[test]
    fn regression_flat_list_recyclerview_runtime_closure_is_dexed() {
        for (group, artifact) in [
            // androidx.recyclerview:recyclerview:1.3.2
            ("androidx.annotation", "annotation-jvm"),
            ("androidx.collection", "collection-jvm"),
            ("androidx.core", "core"),
            ("androidx.customview", "customview"),
            ("androidx.customview", "customview-poolingcontainer"),
            // androidx.customview:customview-poolingcontainer:1.1.0
            ("androidx.core", "core-ktx"),
        ] {
            assert!(
                lists(group, artifact),
                "PINNED_ANDROIDX must include {group}:{artifact} — RecyclerView \
                 (every flat_list) needs it at runtime",
            );
        }
    }

    /// The pinned set is what makes a clean machine link the same libraries
    /// as a developer's: every entry an exact numeric version with a file
    /// type and SHA-1, one entry per library, and lockstep groups at one
    /// version (`core`/`core-ktx`: core-ktx 1.13+ is empty because its
    /// extensions moved into core).
    #[test]
    fn pinned_closure_is_exact_and_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for p in PINNED_ANDROIDX {
            assert!(
                !p.version.is_empty() && p.version.split('.').all(|n| n.parse::<u64>().is_ok()),
                "{}: pins must be exact stable versions",
                p.coord()
            );
            assert!(matches!(p.ext, "aar" | "jar"), "{}: ext {}", p.coord(), p.ext);
            assert!(
                p.sha1.len() == 40 && p.sha1.chars().all(|c| c.is_ascii_hexdigit()),
                "{}: sha1 {}",
                p.coord(),
                p.sha1
            );
            assert!(
                seen.insert((p.group, crate::maven::library_name(p.artifact))),
                "{} is pinned twice",
                p.coord()
            );
        }
        let version = |g: &str, a: &str| {
            PINNED_ANDROIDX.iter().find(|p| p.group == g && p.artifact == a).unwrap().version
        };
        for (g, a, b) in [
            ("androidx.core", "core", "core-ktx"),
            ("androidx.lifecycle", "lifecycle-runtime", "lifecycle-livedata"),
            ("androidx.lifecycle", "lifecycle-runtime", "lifecycle-viewmodel-savedstate"),
            ("androidx.arch.core", "core-runtime", "core-common"),
            ("org.jetbrains.kotlinx", "kotlinx-coroutines-core-jvm", "kotlinx-coroutines-android"),
        ] {
            assert_eq!(version(g, a), version(g, b), "{g}:{a} and {g}:{b} must move together");
        }
    }

    /// Regression (CrewForge, clean Ubuntu 24.04): the KMP split of lifecycle
    /// 2.8 / savedstate 1.3 and `androidx.navigationevent` (activity 1.12)
    /// must never reach the framework's own closure — that is what an empty
    /// `~/.idealyst/maven` picked when resolution meant "newest stable".
    #[test]
    fn regression_pins_predate_the_kmp_umbrellas() {
        for p in PINNED_ANDROIDX {
            assert_ne!(p.group, "androidx.navigationevent");
            let below = |g: &str, a: &str, max: &[u64]| {
                if p.group == g && p.artifact.starts_with(a) {
                    let v: Vec<u64> = p.version.split('.').map(|n| n.parse().unwrap()).collect();
                    assert!(v.as_slice() < max, "{} is past the KMP split", p.coord());
                }
            };
            below("androidx.lifecycle", "lifecycle", &[2, 8]);
            below("androidx.savedstate", "savedstate", &[1, 3]);
            below("androidx.activity", "activity", &[1, 12]);
        }
    }

    /// A pinned library resolves to exactly its pinned file. With both caches
    /// empty, only the pinned files are requested — never a version listing
    /// (`maven-metadata.xml`), so nothing is chosen by "newest".
    #[test]
    fn every_pin_is_fetched_at_its_exact_coordinate_and_nothing_else() {
        let cache = tempfile::tempdir().unwrap();
        let mut repo = FakeRepo::default();
        // Pins whose bytes hash to their sha1: fake files, so the test pins
        // are derived from the real list with the fake digests.
        let fakes: Vec<(Pin, Vec<u8>)> = PINNED_ANDROIDX
            .iter()
            .map(|p| {
                let body = format!("bytes of {}", p.coord()).into_bytes();
                let sha = crate::maven::tests::sha1_hex(&body);
                (Pin { sha1: Box::leak(sha.into_boxed_str()), ..*p }, body)
            })
            .collect();
        for (pin, body) in &fakes {
            let url = format!(
                "{REPO}/{}/{}/{}/{}-{}.{}",
                pin.group.replace('.', "/"), pin.artifact, pin.version, pin.artifact, pin.version, pin.ext
            );
            repo.serve(url, body);
        }
        let pins: Vec<Pin> = fakes.iter().map(|(p, _)| *p).collect();
        let caches = Caches { gradle: None, idealyst: cache.path().to_path_buf() };
        let got = locate_androidx(&repo, &[REPO], &caches, &pins, &[]).unwrap();
        assert_eq!(got.len(), PINNED_ANDROIDX.len());
        for ((coord, path), pin) in got.iter().zip(PINNED_ANDROIDX) {
            assert_eq!(*coord, pin.coord());
            assert!(path.ends_with(format!("{}-{}.{}", pin.artifact, pin.version, pin.ext)));
        }
        let requested = repo.requested.borrow();
        assert!(requested.iter().all(|u| !u.contains("maven-metadata") && !u.ends_with(".pom") && !u.ends_with(".module")));
        drop(requested);
        // Second build: offline.
        locate_androidx(&NoNetwork, &[REPO], &caches, &pins, &[]).unwrap();
    }

    /// A gradle cache holding other versions of a pinned library (newer or
    /// older) is ignored for it; one holding the pinned file is used.
    #[test]
    fn the_gradle_cache_serves_only_the_pinned_file() {
        let gradle = tempfile::tempdir().unwrap();
        let idealyst = tempfile::tempdir().unwrap();
        let pin = PINNED_ANDROIDX.iter().find(|p| p.artifact == "core").unwrap();
        let at = |version: &str, hash: &str| {
            let dir = gradle.path().join(pin.group).join(pin.artifact).join(version).join(hash);
            std::fs::create_dir_all(&dir).unwrap();
            let f = dir.join(format!("{}-{version}.{}", pin.artifact, pin.ext));
            std::fs::write(&f, b"bin").unwrap();
            f
        };
        at("1.19.1", "ffff");
        let caches = Caches { gradle: Some(gradle.path().to_path_buf()), idealyst: idealyst.path().to_path_buf() };
        let err = locate_androidx(&FakeRepo::default(), &[REPO], &caches, std::slice::from_ref(pin), &[]).unwrap_err();
        assert!(format!("{err:#}").contains("androidx.core:core:1.12.0"), "{err:#}");
        let hit = at(pin.version, pin.sha1);
        let got = locate_androidx(&NoNetwork, &[REPO], &caches, std::slice::from_ref(pin), &[]).unwrap();
        assert_eq!(got[0].1, hit);
    }

    fn decl(entry: serde_json::Value) -> anyhow::Result<AndroidxDecl> {
        parse_androidx_entry("some-sdk", &entry)
    }

    #[test]
    fn sdk_androidx_entries_take_an_optional_version() {
        let d = decl(serde_json::json!(["androidx.media3", "media3-exoplayer", "1.1.1"])).unwrap();
        assert_eq!(d.version.as_deref(), Some("1.1.1"));
        assert_eq!(decl(serde_json::json!(["androidx.fragment", "fragment"])).unwrap().version, None);
        for bad in [serde_json::json!(["only"]), serde_json::json!("a:b:c"), serde_json::json!(["a", 1])] {
            assert!(decl(bad).unwrap_err().to_string().contains("some-sdk"));
        }
    }

    #[test]
    fn sdk_declarations_are_deterministic() {
        let ok = [
            decl(serde_json::json!(["androidx.fragment", "fragment"])).unwrap(),
            decl(serde_json::json!(["androidx.fragment", "fragment", "1.6.2"])).unwrap(),
            // The umbrella name of a pinned `-jvm` library is the pin.
            decl(serde_json::json!(["androidx.collection", "collection"])).unwrap(),
            decl(serde_json::json!(["androidx.media3", "media3-exoplayer", "1.1.1"])).unwrap(),
            decl(serde_json::json!(["androidx.media3", "media3-exoplayer", "1.1.1"])).unwrap(),
        ];
        assert_eq!(
            declared_roots(&ok, PINNED_ANDROIDX).unwrap(),
            [Coord::new("androidx.media3", "media3-exoplayer", "1.1.1")]
        );
        // Unpinned without a version: "newest" would depend on the day.
        let err = declared_roots(&[decl(serde_json::json!(["androidx.media3", "media3-common"])).unwrap()], PINNED_ANDROIDX)
            .unwrap_err()
            .to_string();
        assert!(err.contains("some-sdk") && err.contains("without a version"), "{err}");
        // Pinned at another version: the runtime isn't tested against it.
        let err = declared_roots(&[decl(serde_json::json!(["androidx.core", "core", "1.13.1"])).unwrap()], PINNED_ANDROIDX)
            .unwrap_err()
            .to_string();
        assert!(err.contains("androidx.core:core:1.12.0"), "{err}");
    }

    /// Regression (CrewForge, clean Ubuntu): an SDK-declared library that is
    /// a KMP umbrella resolves through its `.module` to the `-android`
    /// variant and that variant's dependencies, next to the pinned set.
    #[test]
    fn regression_a_declared_kmp_umbrella_brings_its_variant_closure() {
        let cache = tempfile::tempdir().unwrap();
        let mut repo = FakeRepo::default();
        let root = Coord::new("com.example", "player", "2.0.0");
        publish_kmp(&mut repo, &root, &[
            ("androidx.navigationevent", "navigationevent", "1.0.0"),
            // Pinned: answered by the pin, never fetched.
            ("androidx.lifecycle", "lifecycle-runtime", "2.6.1"),
        ]);
        publish_kmp(&mut repo, &Coord::new("androidx.navigationevent", "navigationevent", "1.0.0"), &[]);
        publish_kmp(&mut repo, &Coord::new("androidx.lifecycle", "lifecycle-runtime", "2.6.1"), &[]);
        let caches = Caches { gradle: None, idealyst: cache.path().to_path_buf() };
        let got = locate_androidx(&repo, &[REPO], &caches, &[], &[root]).unwrap();
        let coords: Vec<String> = got.iter().map(|(c, _)| c.to_string()).collect();
        assert_eq!(
            coords,
            ["androidx.lifecycle:lifecycle-runtime-android:2.6.1",
             "androidx.navigationevent:navigationevent-android:1.0.0",
             "com.example:player-android:2.0.0"],
            "with no pins every dependency is walked"
        );
        let lifecycle = PINNED_ANDROIDX.iter().filter(|p| p.group == "androidx.lifecycle").copied().collect::<Vec<_>>();
        let cache2 = tempfile::tempdir().unwrap();
        let caches2 = Caches { gradle: None, idealyst: cache2.path().to_path_buf() };
        // Pins present (served from the cache), so only the SDK closure is walked.
        for p in &lifecycle {
            let dir = cache2.path().join(p.group).join(p.artifact).join(p.version).join(p.sha1);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{}-{}.{}", p.artifact, p.version, p.ext)), b"pinned").unwrap();
        }
        let got = locate_androidx(&repo, &[REPO], &caches2, &lifecycle, &[Coord::new("com.example", "player", "2.0.0")]).unwrap();
        let coords: Vec<String> = got.iter().skip(lifecycle.len()).map(|(c, _)| c.to_string()).collect();
        assert_eq!(coords, ["androidx.navigationevent:navigationevent-android:1.0.0", "com.example:player-android:2.0.0"]);
    }

    #[test]
    fn zip_listing_detects_classes_jar_at_the_top_level_only() {
        assert!(zip_listing_has_classes("AndroidManifest.xml\nclasses.jar\nR.txt\n"));
        assert!(!zip_listing_has_classes("AndroidManifest.xml\nMETA-INF/LICENSE.txt\n"));
        assert!(!zip_listing_has_classes("libs/classes.jar\n"));
    }

    /// Regression (CrewForge, clean Ubuntu): `unzip … lifecycle-runtime-2.11.0.aar
    /// classes.jar` failed with exit status 11 because that umbrella AAR holds
    /// only `AndroidManifest.xml` and a LICENSE, and the build stopped. An AAR
    /// with no `classes.jar` contributes no classes; it is not an error.
    #[test]
    fn regression_umbrella_aar_without_classes_jar_contributes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("META-INF")).unwrap();
        std::fs::write(src.join("AndroidManifest.xml"), "<manifest package=\"androidx.lifecycle\"/>").unwrap();
        std::fs::write(src.join("META-INF/LICENSE.txt"), "license").unwrap();
        let zip = |name: &str, entries: &[&str]| {
            let aar = dir.path().join(name);
            let status = std::process::Command::new("zip")
                .arg("-q").arg(&aar).args(entries).current_dir(&src).status().unwrap();
            assert!(status.success());
            aar
        };
        let out = dir.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        let umbrella = zip("lifecycle-runtime-2.11.0.aar", &["AndroidManifest.xml", "META-INF/LICENSE.txt"]);
        assert_eq!(extract_classes(&umbrella, &out, "androidx.lifecycle.lifecycle-runtime").unwrap(), None);

        std::fs::write(src.join("classes.jar"), b"jar").unwrap();
        let real = zip("lifecycle-runtime-android-2.11.0.aar", &["AndroidManifest.xml", "classes.jar"]);
        let got = extract_classes(&real, &out, "androidx.lifecycle.lifecycle-runtime-android").unwrap().unwrap();
        assert_eq!(std::fs::read(got).unwrap(), b"jar");
    }

    /// Checks the pinned set is a closed closure against the real
    /// repositories: every pinned library's Android runtime dependencies
    /// (read from its `.module`/POM exactly as the resolver reads them) are
    /// pinned at that version or higher, or provided. Run with `--ignored`
    /// after changing a pin.
    #[test]
    #[ignore = "network: reads metadata from dl.google.com and repo1.maven.org"]
    fn pinned_closure_is_complete_against_published_metadata() {
        let cache = tempfile::tempdir().unwrap();
        let caches = Caches { gradle: None, idealyst: cache.path().to_path_buf() };
        let resolver = crate::maven::Resolver::new(&crate::maven::Curl, crate::maven::REPOSITORIES, &caches);
        let mut missing = Vec::new();
        for pin in PINNED_ANDROIDX {
            let node = resolver.node(&pin.coord()).unwrap();
            assert_eq!(node.coord, pin.coord(), "a pin must name the variant that holds the classes");
            let bin = node.binary.expect("pinned library publishes a binary");
            assert_eq!(
                sha1_smol::Sha1::from(std::fs::read(&bin).unwrap()).digest().to_string(),
                pin.sha1,
                "{}",
                pin.coord()
            );
            for dep in node.dependencies {
                if crate::maven::is_provided(&dep.group, &dep.artifact) {
                    continue;
                }
                match crate::maven::pin_for(PINNED_ANDROIDX, &dep.group, &dep.artifact) {
                    Some(p) if crate::maven::compare_versions(p.version, &dep.version)
                        != std::cmp::Ordering::Less => {}
                    other => missing.push(format!(
                        "{} needs {}:{}:{} (pinned: {:?})",
                        pin.coord(), dep.group, dep.artifact, dep.version, other.map(|p| p.coord().to_string())
                    )),
                }
            }
        }
        assert!(missing.is_empty(), "the pinned closure is not closed:\n{}", missing.join("\n"));
    }
}
