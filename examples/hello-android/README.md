# hello-android

Runs the shared `hello::app()` tree (from `examples/hello`) on Android by
driving real `LinearLayout` / `TextView` / `Button` views through the
`backend-android` JNI backend.

## Project layout

```
hello-android/
├── Cargo.toml            # Rust cdylib crate. Builds into libhello_android.so.
├── src/lib.rs            # JNI exports: NativeBridge.attach / detach.
└── android/              # Standard Android Studio Gradle project.
    ├── settings.gradle.kts
    ├── build.gradle.kts
    └── app/
        ├── build.gradle.kts            # Invokes cargo-ndk via tasks.
        ├── src/main/AndroidManifest.xml
        ├── src/main/res/values/strings.xml
        └── src/main/java/com/idealyst/
            ├── hello/MainActivity.kt   # Activity + setContentView.
            ├── hello/NativeBridge.kt   # Kotlin facade for JNI entry points.
            └── runtime/RustClickListener.kt  # Click-event trampoline.
```

## Prerequisites (host machine)

The devcontainer can `cargo check` the Rust side, but the actual APK build
runs on your host so it can link against the NDK and install via `adb`.

1. **Android Studio** (any recent version) — for the SDK, NDK, and
   emulator/device tooling. Open the SDK Manager and install:
   - Android SDK Platform 34
   - Android SDK Build-Tools 34.0.0+
   - NDK (Side by side) — version `26.1.10909125` (matches
     `ndkVersion` in `app/build.gradle.kts`; bump there to use a
     different one).
   - Android SDK Platform-Tools (`adb`).
2. **Rust + Android targets** (host toolchain):
   ```bash
   rustup target add aarch64-linux-android x86_64-linux-android
   ```
3. **cargo-ndk** for cross-compiling and dropping the `.so` files into
   the locations Gradle expects:
   ```bash
   cargo install cargo-ndk
   ```
4. Environment:
   ```bash
   export ANDROID_HOME="$HOME/Android/Sdk"               # or wherever
   export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/26.1.10909125"
   export PATH="$ANDROID_HOME/platform-tools:$PATH"      # for adb
   ```

## Option A — Android Studio (recommended)

1. `File → Open` and pick `examples/hello-android/android/` (the folder
   that contains `settings.gradle.kts`, **not** the workspace root).
2. On first sync AS uses its bundled Gradle to read
   `gradle-wrapper.properties`, downloads Gradle 8.9, and materializes
   `gradle-wrapper.jar` + the `gradlew` scripts automatically. You do
   **not** need a system `gradle`.
3. It'll prompt to install AGP 8.5, Kotlin 1.9, SDK 34, and NDK
   26.1.10909125 if any are missing — accept the prompts.
4. Plug in a device or start an emulator, pick it in the run-target
   dropdown, hit ▶. AS runs `installDebug`, which triggers the
   `cargoBuildDebug` Gradle task, which calls `cargo ndk` to build the
   `.so` into `app/src/main/jniLibs/<abi>/`, then packs the APK and
   installs it.

## Option B — command line

Once the wrapper has been materialized (either by Android Studio on
first sync, or manually via `gradle wrapper --gradle-version 8.9` if
you have a system Gradle):

```bash
cd examples/hello-android/android
./gradlew installDebug
```

That single command will:

1. Run the `cargoBuildDebug` task, which calls
   `cargo ndk -t arm64-v8a -t x86_64 -o app/src/main/jniLibs build -p hello-android`
   from the workspace root.
2. Drop `libhello_android.so` under `app/src/main/jniLibs/<abi>/`.
3. Build the APK with those `.so` files embedded.
4. Install via `adb` to the attached device/emulator.

Launch the app from the device's launcher (or `adb shell am start -n com.idealyst.hello/.MainActivity`).

## Iterating

- Pure Rust changes: `./gradlew installDebug` — Gradle reruns the
  cargo task.
- Pure Kotlin changes: same — Gradle's incremental compile skips Rust.
- View `log::info!` output from Rust:
  ```bash
  adb logcat -s idealyst:I
  ```

## What's exercised vs. what's missing

The `hello::app()` tree (shared with the web example) exercises:

- Text + Button primitives, reactive text via `Signal`.
- Click handlers wired through the Kotlin `RustClickListener` trampoline.
- Theme switching (light/dark) — re-fires every styled effect.
- Stylesheets with variants, overrides, and the `Card` component's
  per-instance reactive padding.
- Refs: `Ref<ButtonHandle>` (programmatic click via `performClick`) and
  `Ref<CounterHandle>` (user-component method).
- The reactive `When` conditional (the "Login" → "Welcome back!" branch).

Known limits in this first pass:

- Layout is plain vertical `LinearLayout`. Flex `direction = Row`,
  `justify_content`, `align_items`, `gap`, etc. are ignored — the
  backend doesn't translate them to Android `LayoutParams` yet. The
  demo only uses vertical stacking, so it renders correctly.
- Click callbacks leak when a `When` conditional drops a button
  subtree. Bounded for this demo (the Login button is the only one
  inside a `When`).
- `setOrientation` is always vertical. Horizontal containers would
  need to read `flex_direction` from `StyleRules`.

## Troubleshooting

- **`UnsatisfiedLinkError: ... libhello_android.so not found`**:
  the Rust build failed silently. Run
  `./gradlew cargoBuildDebug --info` from `examples/hello-android/android`
  to see the cargo output, or run the cargo command manually from the
  workspace root.
- **`Java_com_idealyst_..._attach` undefined**: the cdylib was built
  but doesn't export the symbol. Check that `examples/hello-android/src/lib.rs`
  still has the `#[no_mangle] pub extern "system" fn Java_...` block
  and that you're not stripping symbols in debug builds.
- **App shows a blank screen**: check `adb logcat -s idealyst:I` for
  panic messages from the catch_unwind in `attach`.
