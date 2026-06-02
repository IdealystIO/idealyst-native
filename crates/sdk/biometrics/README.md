# `biometrics`

Cross-platform **biometric authentication** — the raw "prove the device
owner is present" capability (Face ID, Touch ID, Android fingerprint/face,
Windows Hello, WebAuthn). This is the *auth gate*; it's deliberately
unopinionated about what the gate guards. Pair it with the
[`credentials`](../credentials) crate when you want a biometric-protected
**secret** (a Keychain/Keystore ACL).

Its defining principle, shared with `credentials`: **it is backed only by
each platform's real biometric facility, and it refuses — loudly — where no
such facility exists, rather than faking a gate.**

| Platform | Backend | Modality | Testing state |
| --- | --- | --- | --- |
| iOS | LocalAuthentication (`LAContext`) | Face ID / Touch ID | 🔴 **UNTESTED** — compiles (`aarch64-apple-ios`); prompt not run on a device |
| macOS | LocalAuthentication (`LAContext`) | Touch ID | 🟡 **UNTESTED at runtime** — compiles + links on host; prompt not exercised |
| Android | framework `BiometricPrompt` + `BiometricManager` (no androidx) | fingerprint / face / iris | 🔴 **UNTESTED** — compiles (`aarch64-linux-android`); JNI signatures + `nativeResult` export not run on a device |
| Windows | Windows Hello (`UserConsentVerifier`) | face / fingerprint / PIN | 🔴 **UNTESTED** — compiles (`x86_64-pc-windows-gnu`); Hello modal not run on a host |
| web | WebAuthn (`navigator.credentials.get`) | platform / roaming passkey | 🔴 **UNTESTED** — compiles (`wasm32-unknown-unknown`); ceremony not run in a browser |
| Linux / other | **errors** — no standard API | n/a | 🟢 covered — `Unsupported` shim unit-tested |

> **Testing status: every real backend is COMPILE-CHECKED ONLY, not yet
> runtime/device-verified.** The portable core (trait, types, `AuthRequest`
> builder, `Unsupported` shim, factory) is unit-tested on the host; the
> per-platform prompts require an enrolled device/browser to exercise and
> have **not** been run. See [Status / verification](#status--verification)
> for the per-target detail and the open items to close.

```rust
use biometrics::{platform_biometrics, AuthRequest, Biometry, BioError};

# async fn demo() -> Result<(), BioError> {
let bio = platform_biometrics();                  // Arc<dyn BiometricAuthenticator>

match bio.availability() {
    Biometry::None => { /* no usable biometric — show a password screen */ }
    _ => {
        bio.authenticate(AuthRequest::new("Unlock your vault")).await?;
        // Authenticated. On native there's nothing more to do.
    }
}
# Ok(())
# }
```

Two operations, intentionally minimal:

- `availability() -> Biometry` — what biometric, if any, is usable on this
  device *right now* (hardware present **and** enrolled). Synchronous and
  cheap, so you can call it while building UI.
- `authenticate(AuthRequest) -> Future<Result<Authentication, BioError>>` —
  present the OS prompt and resolve success or a typed failure. Async
  because every native biometric API shows a prompt and resolves later; the
  platform completion callback/block is bridged to the future over a
  `futures-channel` oneshot.

`BioError` distinguishes `Cancelled` (user dismissed), `Failed` (didn't
match), `Lockout` (too many attempts), `Unavailable` (no enrolled
biometric), `Unsupported` (no API on this target), and `Backend` (a raw
platform error).

## Why web is different — WebAuthn, not a local gate

A browser has **no local "is the owner present" API.** The only biometric
path on the web is **WebAuthn**: the platform authenticator signs a
server-issued challenge with a passkey, and the resulting *assertion* is
meaningful only when a **relying-party server verifies the signature.** A
browser-side "success" with nothing checking the signature is trivially
spoofable, so this crate does not pretend otherwise:

- On web, `AuthRequest` **must** carry a `WebAuthnRequest` (the server's
  challenge + relying-party id). Without one, web `authenticate` returns
  `BioError::Unsupported` with guidance.
- On success, web returns the `WebAuthnAssertion` in
  `Authentication::assertion`. **Send it to your server and verify it
  there** — that verification *is* the authentication.
- On native, the OS verifies locally; `Authentication::assertion` is `None`.

```rust
use biometrics::{platform_biometrics, AuthRequest, WebAuthnRequest};

# async fn web_demo(challenge_from_server: Vec<u8>) -> Result<(), biometrics::BioError> {
let bio = platform_biometrics();
let auth = bio
    .authenticate(AuthRequest::new("Sign in").web_authn(WebAuthnRequest {
        rp_id: Some("example.com".into()),
        challenge: challenge_from_server,
        allow_credentials: vec![],          // any discoverable passkey for the rp
        timeout_ms: Some(60_000),
    }))
    .await?;

// POST `auth.assertion` to the relying-party server to complete sign-in.
# Ok(())
# }
```

The same `web_authn(..)` field is **ignored on native**, so one call site
works on every platform: pass it always (harmless on iOS/Android/Windows),
or only build it when targeting web.

## Platform notes

### iOS / macOS

Uses `LAContext.evaluatePolicy:localizedReason:reply:`. Pass
`AuthRequest::allow_device_credential(true)` to fall back to the device
passcode (`LAPolicy.deviceOwnerAuthentication`); the default is
biometrics-only. `reason` is the prompt's localized message (**required** by
the OS — don't pass an empty string). iOS apps must declare
`NSFaceIDUsageDescription` in `Info.plist` or Face ID is denied at runtime.

### Android

Uses the **framework** `android.hardware.biometrics.BiometricPrompt` (API
28+) — no androidx dependency. The abstract `AuthenticationCallback` is
supplied by a Kotlin shim (`RustBiometricPrompt.kt`) shipped from this crate
via `[package.metadata.idealyst.android].runtime_kotlin`; `idealyst run
android` discovers and compiles it automatically. Add
`<uses-permission android:name="android.permission.USE_BIOMETRIC"/>` to your
manifest. `allow_device_credential(true)` maps to `DEVICE_CREDENTIAL`
(API 30+) and drops the negative button.

The framework doesn't report *which* biometric is enrolled, so
`availability()` refines the modality best-effort from `PackageManager`
hardware features (fingerprint → face → iris → `Unknown`).

### Windows

Uses Windows Hello via the WinRT `UserConsentVerifier`. Win32 desktop apps
have no `CoreWindow`, so it goes through
`IUserConsentVerifierInterop::RequestVerificationForWindowAsync` parented to
the foreground window (`GetForegroundWindow`, falling back to
`GetActiveWindow`). The modal blocks, so the call runs on a worker thread
and bridges its result back to the future. Hello abstracts the modality, so
`availability()` reports `Unknown` when a verifier is configured.

## Status / verification

**Summary: compile-checked everywhere, runtime-tested nowhere.** Nothing in
the table below has been run against a real biometric prompt yet. Each
backend below lists exactly what is and isn't verified, and the open items
to close before it should be considered done.

### ✅ Done

- **Portable core** — trait, `Biometry`, `BioError`, `AuthRequest` builder,
  `WebAuthnRequest`/`Authentication`/`WebAuthnAssertion`, the `Unsupported`
  shim, and the `platform_biometrics()` factory. Unit-tested on the host
  (`cargo test -p biometrics`).
- **Compilation, all targets** — `aarch64-apple-ios`,
  `aarch64-linux-android`, `x86_64-pc-windows-gnu`,
  `wasm32-unknown-unknown`, plus the macOS host and the Linux unsupported
  path. Zero warnings.

### 🔴 Untested — needs a real device/browser

The author will return to these. Each is implemented and compiles, but the
prompt has **not** been exercised.

- **iOS** — run the Face ID / Touch ID prompt on a device (and the
  Simulator's *Features → Face ID → Enrolled* path). Confirm
  `NSFaceIDUsageDescription` gating, the cancel/lockout/`Unavailable`
  error mappings, and that the `LAContext` lifetime invariant holds (no
  use-after-free when the reply fires off-thread).
- **macOS** — same as iOS on a Touch ID Mac; the framework links on host
  but the prompt is unexercised.
- **Android** — the highest-risk backend. Verify on a device that: (1) the
  Kotlin shim `RustBiometricPrompt.kt` is discovered + compiled by
  `idealyst run android`; (2) the `nativeResult` JNI symbol actually
  resolves from the app `cdylib` (the `#[used]` pin is a belief, not a
  proven fact for an SDK-rlib export — confirm with `nm`/at runtime); (3)
  the JNI method signatures resolve; (4) the prompt builds on the main
  looper and the success/error/negative-button codes map correctly;
  (5) the manifest carries `USE_BIOMETRIC`.
- **Windows** — run Windows Hello on a host with an enrolled verifier.
  Confirm the `IUserConsentVerifierInterop` factory + the foreground-HWND
  parenting work for a Win32/winit window, and the worker-thread bridge
  delivers the result.
- **web** — run the WebAuthn ceremony in a browser against a registered
  passkey: a real server-issued challenge, `userVerification:"required"`,
  the returned assertion decoded into `WebAuthnAssertion`, and the
  `NotAllowedError`/`AbortError` → `Cancelled` mapping.

Failures on every backend are surfaced as typed `BioError`s carrying the
platform message, to keep that on-device diagnosis quick.
