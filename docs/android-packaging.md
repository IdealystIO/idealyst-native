# Android packaging

How the CLI turns an app crate into an APK: what the app declares, what an
SDK can add to the manifest, how the APK is signed and versioned, and the
device-owner provisioning helper. There is no Gradle project; the CLI drives
`aapt2`, `javac`/`kotlinc`, `d8`, `zipalign` and `apksigner` directly
(`crates/tools/run/android`).

| Command | Output |
|---|---|
| `idealyst run android` | Builds, installs and launches on an emulator or device. Cleartext HTTP allowed, so a dev build reaches an `http://` server on your machine. |
| `idealyst build --android [--release]` | A signed APK at `<project>/dist/android/<name>-<version>.apk`. Cleartext HTTP off. |
| `idealyst android provisioning-qr` | The QR code that makes a device-owner app the owner of a freshly reset device. |

## App settings: `[package.metadata.idealyst.app.android]`

```toml
[package.metadata.idealyst.app]
version = "1.4.2"

[package.metadata.idealyst.app.android]
version_code   = 42                # optional; see "Versioning"
lock_task_mode = "if_whitelisted"  # the main activity's android:lockTaskMode
home_launcher  = true              # add a HOME + DEFAULT intent filter

[package.metadata.idealyst.app.android.signing]
keystore = "keys/release.jks"      # relative to the app crate
alias    = "release"
store_password_env = "IDEALYST_ANDROID_STORE_PASSWORD"  # the default
key_password_env   = "IDEALYST_ANDROID_KEY_PASSWORD"    # default: the store password
```

`lock_task_mode` takes `normal`, `never`, `always` or `if_whitelisted`. A
kiosk app that a device owner allow-lists uses `if_whitelisted`: it enters
lock task whenever it launches. `home_launcher` lets the app be the home
screen, which a device owner needs before it can make the app the
persistent launcher (`device_admin::set_home_activity`). Both are the app's
choice rather than an SDK's, because the same SDK is often in both the
kiosk and the app that manages it.

### Versioning

Android refuses an update whose `versionCode` is not higher than the
installed one. With no `version_code`, it is derived from `app.version` as
`major × 1,000,000 + minor × 1,000 + patch` (`1.4.2` → `1004002`), which
keeps semver order while minor and patch stay below 1000. A pre-release
suffix is ignored, so `1.4.2-beta.1` and `1.4.2` share a code. Set
`version_code` explicitly to use a counter instead. `versionName` is
`app.version`.

### Signing

With a `signing` block, every build of the app is signed with that key,
`idealyst run` included. A device refuses an update signed with a different
certificate, so a debug-signed dev install and a release-signed build cannot
replace each other; uninstall to switch. The passwords are read from the
named environment variables by `apksigner` itself (`--ks-pass env:VAR`), so
they never appear in a process argument list. A missing variable or keystore
fails the build before anything compiles.

Without a `signing` block, builds use `~/.android/debug.keystore` (password
`android`), which Android Studio creates on first launch.

## SDK contributions: `[package.metadata.idealyst.android]`

An SDK crate declares what it adds to every app whose dependency graph
contains it. Paths are relative to the SDK's own directory.

```toml
[package.metadata.idealyst.android]
runtime_kotlin = ["runtime/kotlin/io/example/RustFoo.kt"]
runtime_java   = ["runtime/java/io/example/FooBridge.java"]
androidx       = [["androidx.media3", "media3-exoplayer"]]
manifest_application = ["manifest/application.xml"]  # inside <application>
manifest_root        = ["manifest/root.xml"]         # inside <manifest>
res                  = ["res/xml/foo.xml"]           # compiled into the APK
```

- **`manifest_application` / `manifest_root`**: files holding a sequence
  of manifest elements, with no root element and no XML declaration. The
  `android:` prefix is predeclared. Each file must be well-formed XML, and
  each top-level element must be legal where it is spliced. Inside
  `<application>` that means `activity`, `activity-alias`, `receiver`,
  `service`, `provider`, `meta-data`, `uses-library`,
  `uses-native-library`, `profileable` and `property`. Inside `<manifest>`
  it means `uses-permission`, `uses-permission-sdk-23`, `permission`,
  `permission-group`, `permission-tree`, `uses-feature`, `queries` and
  `supports-screens`. A violation fails the build and names the SDK.
  `${applicationId}` is replaced with the app's package name.
- **`res`**: each entry is `res/<type>/<file>` with a lowercase file name
  (`res/xml/device_admin.xml`). Two SDKs declaring the same resource is an
  error, because resource names share one namespace in the APK.
- Prefer `[package.metadata.idealyst] capabilities` for plain permissions:
  it also covers iOS and the app's reason strings.

Everything an SDK declares lands in every app that depends on it, so an SDK
should only declare components that stay inert until something enables
them.

## Device-owner provisioning

A device owner (see the `device-admin` SDK) can only be set on a freshly
reset device with no accounts. Two routes:

```sh
# Development: over adb.
adb shell dpm set-device-owner <package>/io.idealyst.deviceadmin.AdminReceiver

# Production: a QR code scanned from the setup wizard (tap the welcome
# screen six times). Build, host that exact APK over https, then:
idealyst build --android --release
idealyst android provisioning-qr --download-url https://example.com/agent.apk \
    --extras '{"server": "https://example.com", "enrolment_token": "…"}' \
    --svg provisioning.svg
```

`provisioning-qr` reads the package, the receiver guarded by
`android.permission.BIND_DEVICE_ADMIN`, and the signing certificate from
the APK (`--apk`, default `dist/android/<name>-<version>.apk`). It prints the
payload JSON and the QR code, and with `--svg` also writes the code to a
file. The `--extras` object reaches the app as
`device_admin::Event::ProvisioningComplete`. Its values must be strings,
numbers, booleans or nested objects, because they travel as an Android
`PersistableBundle`.

Provisioning removes the manufacturer's bundled (system) apps for the
user, which is what a kiosk wants. Pass `--leave-system-apps` to keep them
(`PROVISIONING_LEAVE_ALL_SYSTEM_APPS_ENABLED`). Either way the device owner
can change its mind later: `device_admin::enable_system_app` restores a
removed one, and `set_application_hidden` / `set_packages_suspended` switch
off one that was kept.
