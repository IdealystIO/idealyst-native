import org.gradle.api.tasks.Exec

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "io.idealyst.hello"
    compileSdk = 34
    ndkVersion = "26.1.10909125"

    defaultConfig {
        applicationId = "io.idealyst.hello"
        minSdk = 24
        targetSdk = 34
        versionCode = 1
        versionName = "0.0.1"

        ndk {
            // Real devices (modern phones) are arm64; emulators are
            // typically x86_64. Add others if your target diverges.
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    // The Rust crate is built outside Gradle via `cargo-ndk` (configured
    // below). We point `jniLibs.srcDirs` at the cargo-ndk output so the
    // resulting `.so`s land in the APK at the expected
    // `lib/<abi>/lib<name>.so` paths.
    //
    // The Kotlin runtime for backend-android (RustClickListener +
    // Animators) lives next to the Rust crate at
    // `crates/backend-android/runtime/kotlin`. It's pulled in here as
    // an extra source root so the JVM-side glue compiles into this
    // app — the Rust backend calls into these classes via JNI by
    // name, so they must end up in the APK's classpath.
    sourceSets {
        getByName("main") {
            jniLibs.srcDirs("src/main/jniLibs")
            java.srcDirs(
                "src/main/java",
                "../../../../crates/backend-android/runtime/kotlin",
            )
        }
    }

    buildFeatures {
        viewBinding = false
    }
}

dependencies {
    implementation("androidx.appcompat:appcompat:1.7.0")
    implementation("androidx.core:core-ktx:1.13.1")
    // RecyclerView powers the framework's Virtualizer primitive on
    // Android. The backend's Kotlin glue (RustListAdapter,
    // RustLinearLayoutManager) lives under
    // `crates/backend-android/runtime/kotlin/` and is bundled into the
    // app via the sourceSets configuration above.
    implementation("androidx.recyclerview:recyclerview:1.3.2")
    // DrawerLayout powers the framework's DrawerNavigator on Android.
    // Provides the slide-in animation, scrim, and edge-swipe gesture
    // for free.
    implementation("androidx.drawerlayout:drawerlayout:1.2.0")
}

// ---------------------------------------------------------------------------
// Rust build integration.
//
// `cargo ndk` compiles the `hello-android` crate for every requested ABI
// and copies the resulting `.so` files into the location Gradle expects.
// Run before the Android build's `mergeJniLibs` task so the libraries
// are present when the APK is assembled.
//
// Requires `cargo-ndk` installed on the host: `cargo install cargo-ndk`.
// ---------------------------------------------------------------------------

val cargoTargetDir = file("../../../../target")
val workspaceManifest = file("../../../../Cargo.toml")
val jniLibsDir = file("src/main/jniLibs")

abstract class CargoBuildTask : Exec() {
    init {
        group = "build"
        description = "Build the hello-android cdylib for every configured ABI via cargo-ndk."
    }
}

tasks.register<CargoBuildTask>("cargoBuildDebug") {
    workingDir = workspaceManifest.parentFile
    commandLine = listOf(
        "cargo", "ndk",
        "-t", "arm64-v8a",
        "-t", "x86_64",
        "-o", jniLibsDir.absolutePath,
        "build",
        "-p", "hello-android",
    )
}

tasks.register<CargoBuildTask>("cargoBuildRelease") {
    workingDir = workspaceManifest.parentFile
    commandLine = listOf(
        "cargo", "ndk",
        "-t", "arm64-v8a",
        "-t", "x86_64",
        "-o", jniLibsDir.absolutePath,
        "build",
        "--release",
        "-p", "hello-android",
    )
}

// Hook the Rust build into the right Android build step. `mergeJniLibs`
// is the first task that consumes `jniLibs.srcDirs`, so we run cargo
// just before it.
tasks.whenTaskAdded {
    when (name) {
        "mergeDebugJniLibFolders" -> dependsOn("cargoBuildDebug")
        "mergeReleaseJniLibFolders" -> dependsOn("cargoBuildRelease")
    }
}
