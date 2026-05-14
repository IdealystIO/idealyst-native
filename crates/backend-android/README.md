# backend-android

Android backend for the framework. Drives the Android `View` hierarchy
via JNI from Rust.

## Architecture

Two pieces ship together, both inside this crate's directory:

- **`src/`** — the Rust crate. Implements the `Backend` trait by
  calling into the Android Java view system through JNI.
- **`runtime/kotlin/`** — JVM-side glue the Rust crate calls into.
  Two small classes:
  - `io.idealyst.runtime.RustClickListener` — `View.OnClickListener`
    that trampolines `onClick` to a Rust closure via a cached native
    pointer.
  - `io.idealyst.runtime.Animators` — companion-object helpers for
    `ValueAnimator`-based transitions on properties without an
    `ObjectAnimator`-friendly setter (per-side padding, stroke,
    corner radii).

Both pieces are required at runtime — the Rust crate `find_class`es
the Kotlin classes by name and crashes if they aren't on the
classpath.

## Why the Kotlin runtime exists

Some operations have to live JVM-side:

- `View.OnClickListener` is a Java interface. The Android framework
  invokes `onClick(View?)` by class — Rust can't implement a Java
  interface at compile time without dynamic class generation.
- `ValueAnimator.addUpdateListener { ... }` takes a Kotlin lambda
  invoked from the UI thread on each frame. Rust can't be that lambda
  directly; a JVM-side trampoline class is the conventional approach.

The Rust crate calls into these classes via `find_class` and
`call_static_method` (or constructs `RustClickListener` instances via
`new_object`), and the Kotlin glue calls back into Rust where needed
(`RustClickListener.onClick` → `nativeInvoke` → Rust closure).

## Using `backend-android` in an Android app

Two ingredients:

### 1. The Rust crate

Depend on `backend-android` as a normal Cargo crate. Build with
`cargo ndk` for the ABIs you target. The resulting `.so` files land
in your Android app's `jniLibs` directory.

### 2. The Kotlin runtime

Add `crates/backend-android/runtime/kotlin` as an additional Kotlin
source root in your app's `build.gradle.kts`. From the
`examples/hello-android` demo:

```kotlin
android {
    sourceSets {
        getByName("main") {
            java.srcDirs(
                "src/main/java",
                "../../../../crates/backend-android/runtime/kotlin",
            )
        }
    }
}
```

The path is relative to the app module — adjust for your own layout.
That's the entire integration: no separate AAR, no included Gradle
build, no `local.properties` dance.

If you'd rather depend on a published artifact instead of a source
include, package the Kotlin sources into your own AAR module and
depend on that. The shape of the file tree at `runtime/kotlin` is
deliberately mirror-compatible with a Gradle Android library project
(`src/main/java/...`).

## Versioning

The Kotlin runtime is co-versioned with the Rust crate. The JNI
surface between them is the contract — class names, method
signatures — and changes to it require updates on both sides.
