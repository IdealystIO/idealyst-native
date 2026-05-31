//! Scene-commands → PNG bridge for headless screenshots, plus the
//! Robot-bridge `"screenshot"` verb registration.
//!
//! Factored into its own leaf crate so both `mock-backend` (tests) and
//! `dev-server` (the live runtime-server sidecar) can use it without a
//! cycle — it depends on neither. The flow:
//!
//!   wire `Vec<Command>` → dev_client::WireBackend → headless WgpuBackend
//!     → render-wgpu offscreen render → PNG
//!
//! See `render_wgpu::headless` for the renderer details (hardware →
//! software adapter fallback, etc.).

use dev_client::WireBackend;
use wire::{Command, DevToApp};

pub use render_wgpu::headless::Screenshotter;

/// Replay a recorded wire command stream into a headless GPU backend
/// and capture a PNG. `commands` is exactly what a client receives over
/// the wire (e.g. `WireRecordingBackend::snapshot()` or a
/// `DevToApp::Commands` payload). Round-trips through the real
/// `wire::codec` first, so a serialization regression fails here too.
///
/// Returns `Err` if no wgpu adapter (hardware or software) is available
/// — callers on GPU-less CI should treat that as "skip", not "fail".
pub fn screenshot_commands(
    width: u32,
    height: u32,
    commands: Vec<Command>,
) -> Result<Vec<u8>, String> {
    let mut shot = Screenshotter::new(width, height)?;
    let (tx, _rx) = std::sync::mpsc::channel();
    // Share the screenshotter's backend so the replay builds the very
    // tree it rasterizes.
    let mut client = WireBackend::new_with_shared(shot.backend(), tx);

    // Mirror the real transport: encode → decode through the codec.
    let bytes = wire::codec::encode(&DevToApp::Commands(commands)).map_err(|e| e.to_string())?;
    let decoded: DevToApp = wire::codec::decode(&bytes).map_err(|e| e.to_string())?;
    let cmds = match decoded {
        DevToApp::Commands(c) => c,
        other => return Err(format!("expected Commands, got {other:?}")),
    };
    client
        .apply_batch(cmds)
        .map_err(|e| format!("replay into WgpuBackend failed: {e:?}"))?;

    shot.capture_png()
}

/// Register a `"screenshot"` verb on the Robot bridge. `snapshot`
/// returns the current scene as wire commands each time the verb fires
/// (so the screenshot reflects the live tree); `default_size` is used
/// when the request omits `width`/`height`.
///
/// This is the hookup that lets Robot / the MCP server screenshot the
/// app even when it's only mocked: the dev-server session (which owns
/// the recorder) calls this once, on the bridge-poll thread, and any
/// external client can then send `{"cmd":"screenshot","args":{...}}`.
///
/// Request args (optional): `width`, `height` (u32). Response payload
/// (JSON): `{"png_base64": "...", "width": W, "height": H}`.
///
/// Must run on the thread that polls the bridge (the registry is
/// thread-local — see [`runtime_core::robot::bridge::register_command`]).
pub fn register_screenshot_command<F>(default_size: (u32, u32), snapshot: F)
where
    F: Fn() -> Vec<Command> + 'static,
{
    use base64::Engine as _;
    runtime_core::robot::bridge::register_command("screenshot", move |args| {
        let w = args
            .get("width")
            .and_then(|v| v.as_u64())
            .unwrap_or(default_size.0 as u64) as u32;
        let h = args
            .get("height")
            .and_then(|v| v.as_u64())
            .unwrap_or(default_size.1 as u64) as u32;

        let commands = snapshot();
        let png = screenshot_commands(w, h, commands)?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);

        serde_json::to_string(&serde_json::json!({
            "png_base64": b64,
            "width": w,
            "height": h,
        }))
        .map_err(|e| format!("screenshot response encode failed: {e}"))
    });
}
