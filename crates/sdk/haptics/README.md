# `haptics`

Cross-platform **tactile feedback** — fire-and-forget device haptics. Three
tiny synchronous functions trigger the platform's haptic engine, with one
identical API on every target: `impact(style)` for a physical tap,
`notify(feedback)` for a success/warning/error pattern, and `selection()` for
a light value-changed tick.

Haptics are *non-essential* feedback — a nicety, never load-bearing. So every
function is **best-effort and infallible**: where a platform can't deliver an
exact analog (the web has no notion of an "impact style"; a Mac without a
Force Touch trackpad has no haptics at all) the call maps to the nearest
effect or is a silent no-op. There's deliberately no `Result` and no error
type — a caller can't meaningfully recover from "the phone didn't buzz".

```rust
use haptics::{impact, notify, selection, ImpactStyle, NotificationFeedback};

impact(ImpactStyle::Medium);            // a tap when a control engages
selection();                            // a tick as a picker value changes
notify(NotificationFeedback::Success);  // the "it worked" pattern
```

## What you get

Three free functions plus a support predicate — no handle, no state, no async:

- `impact(style: ImpactStyle)` — a physical-impact tap. `ImpactStyle` is one
  of `Light`, `Medium`, `Heavy`, `Soft`, `Rigid` (mirrors iOS's impact
  weights).
- `notify(feedback: NotificationFeedback)` — a `Success` / `Warning` /
  `Error` pattern.
- `selection()` — a light "selection changed" tick.
- `is_supported() -> bool` — whether this device/target can produce haptics
  at all, so an app can present an honest "vibrate on tap" setting. The effect
  functions are always safe to call regardless.

Every backend delivers the **same shape** — the platforms diverge in
mechanism, not in the functions you call.

## Per-platform mechanism

| Target | Mechanism |
| --- | --- |
| iOS | `UIImpactFeedbackGenerator` / `UINotificationFeedbackGenerator` / `UISelectionFeedbackGenerator` (`prepare()` then fire), via objc2 — **compile-checked only ⚠️** |
| macOS | `NSHapticFeedbackManager.defaultPerformer performFeedbackPattern:` (Force Touch trackpad); impact styles collapse to the nearest of the three AppKit patterns — **compile-checked only ⚠️** |
| Android | `Vibrator` (API < 31) / `VibratorManager.getDefaultVibrator()` (API 31+) with predefined / one-shot `VibrationEffect`s (API 26+), via JNI — **compile-checked only ⚠️** |
| web (wasm32) | `navigator.vibrate(ms)` — duration-only; styles/feedback are **approximated** as short ms pulse patterns. Runnable where the browser supports the Vibration API (primarily Android Chrome) |
| Windows / Linux / other native | no-op; `is_supported()` returns `false` |

Where the platform has no impact-weight concept (macOS, web), the five
`ImpactStyle`s map onto the nearest available effect — the divergence is
*not* exposed in the API. The native paths build and link but have not been
verified on a device/trackpad from this crate (marked **compile-checked
only**); the macOS path additionally runs through `cargo test` on the host.

## Permissions

- **Android** — requires `<uses-permission
  android:name="android.permission.VIBRATE"/>`. That's a normal (install-time)
  permission, no runtime prompt. This crate declares `capabilities =
  ["haptics"]`; the CLI injects the manifest entry. Without it the system
  silently ignores the calls (still a safe no-op).
- **iOS / macOS / web** — none. Triggering haptics needs no permission.

## Scope

Predefined feedback patterns only — the unopinionated raw capability. Custom
waveform / amplitude composition (Core Haptics `CHHapticEngine` on iOS,
amplitude curves on Android) is deliberately left to a higher-level SDK rather
than baked in here, as is any reactive / per-event ergonomic wrapper.

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet (see the
verification note above). Tick each item as you exercise it.

**Automated**
- [ ] `cargo test -p haptics` — calls-never-panic across every variant, `is_supported()` answers, enums are value types
- [ ] `cargo build -p haptics --features catalog` — recipes/docs compile
- [ ] `cargo build -p haptics --target wasm32-unknown-unknown` — web target

**Behavior**
- [ ] **Web** — on a device with the Vibration API (primarily Android Chrome), `impact`/`notify`/`selection` each produce a short `navigator.vibrate` pulse; on a browser without it the calls are silent no-ops and `is_supported()` is `false`.
- [ ] **iOS** — on a PHYSICAL device (Simulator has no haptics), each `impact` weight, each `notify` pattern, and `selection()` produces a distinct tap.
- [ ] **macOS** — on a Force Touch trackpad the three `NSHapticFeedbackManager` patterns fire; `is_supported()` is `false` on a Mac without one.
- [ ] **Android** — the device vibrates for each call; confirm `VIBRATE` is in the merged manifest (without it the system silently ignores the calls — still a safe no-op).
