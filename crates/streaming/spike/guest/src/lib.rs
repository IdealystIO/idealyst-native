//! The spike's bundle: two components that exercise every crossing the
//! boundary has — host props in (read-only and two-way signals), guest
//! state, a guest effect, reactive text, events, and a host-component
//! import — plus two benchmark exports the measurement example calls.

// Only a bundle build has a host to call; elsewhere this crate is empty.
#![cfg(idealyst_stream_guest)]

use spike_camera::{battery_level, take_photo, PhotoOptions};
use stream_guest::*;

/// Host signal in (read-only), guest-local state, an effect deriving one
/// guest signal from another, and an imported host component. `step` is
/// optional (see `bundle!` below): the demo app never sends it.
fn counter(title: String, external: ReadSignal<i64>, step: i64) -> Node {
    let clicks = signal(0i64);
    let doubled = signal(0i64);
    effect(move || doubled.set(clicks.get() * 2));
    view(vec![
        text(title),
        text_dyn(move || format!("external: {}", external.get())),
        text_dyn(move || format!("clicks: {} doubled: {}", clicks.get(), doubled.get())),
        button(format!("add {step}"), move || clicks.update(move |c| c + step)),
        host("Badge", Props::new().string("streamed")),
    ])
}

/// A two-way host signal the guest writes back.
fn stepper(value: Signal<i64>) -> Node {
    view(vec![
        text_dyn(move || format!("value: {}", value.get())),
        button("+1", move || value.update(|v| v + 1)),
    ])
}

/// No events, so nothing outlives the mount in a host-side handler table —
/// the clean case for leak accounting.
fn label(external: ReadSignal<i64>) -> Node {
    let local = signal(1i64);
    effect(move || {
        let _ = local.get();
    });
    text_dyn(move || format!("{} + {}", external.get(), local.get()))
}

/// A camera screen: a NATIVE preview mounted by name, a sync host call
/// (`battery_level`) and an async one (`take_photo`) whose result lands in
/// guest state only if this component is still mounted.
fn camera(facing: String) -> Node {
    let status = signal(String::from("ready"));
    let shots = signal(0i64);
    view(vec![
        host("CameraPreview", Props::new().string(facing.clone())),
        // Integer math on purpose: `{:.0}` on an f64 links in float
        // formatting, which measured as most of a 37 KB jump in bundle size.
        text(format!("battery at mount: {}%", (battery_level() * 100.0) as i64)),
        button("Capture", move || {
            status.set("capturing…".into());
            let opts = PhotoOptions { camera: facing.clone() };
            spawn_then(take_photo(opts), move |result| match result {
                Ok(photo) => {
                    shots.update(|n| n + 1);
                    status.set(format!("photo #{}: {}×{} from the {} camera", photo.sequence, photo.width, photo.height, photo.camera));
                }
                Err(spike_camera::CameraError::NoSuchCamera(name)) => {
                    status.set(format!("camera error: no {name} camera"))
                }
            });
        }),
        text_dyn(move || status.get()),
        text_dyn(move || format!("photos this mount: {}", shots.get())),
    ])
}

stream_guest::bundle! {
    components: [
        "Counter" => counter(title: String, external: ReadSignal<i64>, step: i64 = 1),
        "Stepper" => stepper(value: Signal<i64>),
        "Label" => label(external: ReadSignal<i64>),
        "Camera" => camera(facing: String = "back".to_string()),
    ],
    imports: ["Badge", "CameraPreview"],
}

/// `n` tracked-free reads of a host signal: the per-crossing cost.
#[cfg(target_arch = "wasm32")]
#[no_mangle]
extern "C" fn bench_reads(handle: u32, n: u32) -> i64 {
    let s = ReadSignal::<i64>::from_raw(handle);
    let mut acc = 0i64;
    for _ in 0..n {
        acc = acc.wrapping_add(s.get());
    }
    acc
}

/// Pure guest compute with no crossings: the interpreter's raw speed.
/// Mirrored natively in the measurement example.
#[cfg(target_arch = "wasm32")]
#[no_mangle]
extern "C" fn bench_compute(n: u32, seed: u32) -> i64 {
    let mut x = seed as u64 | 1;
    let mut acc = 0u64;
    for _ in 0..n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        acc = acc.wrapping_add(x % 1000);
    }
    acc as i64
}

/// `n` raw crossings with no guest-side decoding.
#[cfg(target_arch = "wasm32")]
#[no_mangle]
extern "C" fn bench_raw_reads(handle: u32, n: u32) -> i64 {
    let mut buf = [0u8; 16];
    let mut acc = 0i64;
    for _ in 0..n {
        acc = acc.wrapping_add(stream_guest::__raw_signal_get(handle, &mut buf) as i64);
    }
    acc
}
