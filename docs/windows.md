# Windows: renderers, Wine and testing on Windows

The Windows backend (`crates/backend/windows`, hosted by `crates/host/win32`)
draws an app's views, text, icons and images itself, into the one top-level
window. Five primitives are real Win32 controls: `button` (a `BUTTON`),
`text_input` (an `EDIT` control), `toggle` (a checkbox), `slider` (a
trackbar) and the activity indicator (a progress bar). Pressable views are
painted like any other view. Layout, input, hit testing and the native
controls work the same whichever renderer draws the rest.

Mouse input over painted views lands on the host window, which hit-tests
the scene (honouring `pointer_events`) and routes it the way every backend
does: deepest view first, then up through its ancestors. A view's
`on_touch` gets the mouse as one pointer. The button that pressed is
reported through `pointer_button()`, and the window holds mouse capture
until the release, so a drag that leaves the view or the window keeps
reporting to the view that took the press. `on_wheel` gets wheel and
Ctrl+wheel (zoom) ticks before any scroll view under it, and `on_hover`
gets enter and leave. A precision touchpad pinch arrives as Ctrl+wheel.
Graphics surfaces (a `canvas`, a 3D view) are DirectComposition visuals
above the painted scene. Painted content that comes after one in the tree
and overlaps it (a chart's tooltip, a modal over a canvas, a HUD over a 3D
view) is drawn into its own visual just above that surface, so it stacks in
tree order as on every other platform. Only content that actually overlaps
a surface is lifted; a window without graphics surfaces does none of this.
Source: `layers.rs`, `overlay.rs`.

The `canvas` SDK's default renderer (`canvas-native`) draws with Direct2D
on Windows. It needs Windows 10 or later, like the Direct2D scene renderer.

The GPU renderer (`canvas-vello`) takes over when an app registers it and
the machine has a real GPU. On a software adapter, which is what a machine
or VM without a GPU has (WARP), it steps aside and `canvas-native` keeps
the canvas: the WARP that ships with Windows 11 (10.0.26100) crashes in
vello's compute shaders. Microsoft's redistributable WARP
(`Microsoft.Direct3D.WARP` 1.0.20, `d3d10warp.dll` placed next to the
`.exe`) runs them correctly. Headless `canvas_vello::render_to_rgba` still
uses a software adapter when there is no GPU, so a GPU-less Windows server
needs that DLL. Source: `gpu-surface/src/config.rs` (`Requirements.hardware`).

Files dragged in from Explorer reach `on_file_drop` the same way: the window
registers one OLE drop target (when the first `on_file_drop` view mounts),
offers each drag position to views deepest first, and shows the copy cursor
over a view that accepts. Dropped files carry real paths. Source:
`view_input.rs`, `file_drop.rs`.

Timers, animation frames and async work all run on the UI thread, from the
host's message pump. A worker thread tracks deadlines and wakes the pump
with one posted message at a time, so a slow frame never queues wakes ahead
of input. `spawn_async` polls a future only up to its next `.await`; when
something wakes it, from any thread, the pump polls it again. A future that
waits on another thread (a network reply, an OS event) therefore never
blocks the window, and its continuation, which may write signals, runs on
the UI thread. Source: `host/win32/src/scheduler.rs`, `async_executor.rs`.

At startup the generated wrapper identifies the app to Windows
(`host_win32::set_app_identity`): the manifest's `bundle_id` becomes the
process's AppUserModelID, registered for the current user under
`HKCU\Software\Classes\AppUserModelId\<bundle_id>` with the app `name`
as its display name. Toast notifications are attributed to it (Windows only
shows toasts for an id it knows; an unpackaged `.exe` has none of its own),
and the taskbar groups the app's windows under it. Source:
`backend/windows/src/app_identity.rs`.

Some SDKs need a file beside the `.exe` at runtime. The webview SDK needs
Microsoft's `WebView2Loader.dll`: its static form links only with the MSVC
toolchain, and Windows apps here are built with the GNU one. An SDK declares
such a file in its manifest,

```toml
[package.metadata.idealyst.windows]
runtime-files = [{ package = "webview2-com-sys", path = "{arch}/WebView2Loader.dll" }]
```

and `idealyst build --windows` copies every declared file (`{arch}` = `x64`
/ `arm64` / `x86`) out of the named dependency package next to the `.exe`;
`run --windows` on a remote machine copies them with it. Ship the `.exe`'s
whole folder. The webview SDK loads the DLL at runtime rather than importing
it, so an app missing it still starts. Source:
`tools/build/windows/src/runtime_files.rs`.

## Client SDKs on Windows

Each SDK's Windows backend uses the OS's own stack; the SDK's README has
the details and what was verified.

| SDK | Windows mechanism |
| --- | --- |
| svg | the backend's SVG image path (resvg), via an `image()` node |
| clipboard | Win32 clipboard (`arboard`, shared with Linux) |
| connectivity | WinRT `NetworkInformation`; changes delivered on the UI thread |
| notifications | WinRT toasts for the app's AppUserModelID; OS-scheduled delivery |
| permissions | notification setting; location / camera / microphone privacy settings (`AppCapability`) |
| webview | WebView2 in a child window; `WebView2Loader.dll` ships beside the `.exe` |
| video | Media Foundation Media Engine → composition swapchain; painted controls |
| video-decode | Media Engine frame server; frames copied to the CPU; PCM from a second (source-reader) decoder of the audio track, delivered in step with the engine's clock |
| audio | Media Engine, audio-only |
| microphone | WASAPI via `cpal` |
| camera | Media Foundation capture (source reader → RGB32) |
| location | WinRT `Geolocator` |
| share | the share sheet (`DataTransferManager`) |
| screen-recorder | `Windows.Graphics.Capture` (video only); `PrivateLayer` content is a layered popup over the app's window (`backend_windows::create_private_layer_window`), excluded from captures with `WDA_EXCLUDEFROMCAPTURE` while a recording runs. Painted content only — native controls inside it are recorded |
| media-writer | Media Foundation Sink Writer, H.264 + AAC in MP4 |

`examples/sdk-gallery` has a panel for each, to check them by hand.

## The two renderers

| | GDI+ (default) | Direct2D |
|---|---|---|
| Select with | nothing, or `IDEALYST_WINDOWS_RENDERER=gdi` | `IDEALYST_WINDOWS_RENDERER=d2d` |
| Draws with | GDI+ on the CPU, into a back buffer copied to the window on `WM_PAINT` | Direct2D on the GPU, into a swapchain shown through DirectComposition |
| Text | measured with GDI, drawn with GDI+ | measured and drawn with DirectWrite |
| Needs | any Windows | Windows 10 or later (Direct3D 11 + DirectComposition) |
| Source | `scene.rs` | `d2d.rs`, `dwrite_text.rs` |

GDI+ redraws every node on the CPU on every frame. On scenes with a lot of
text or large gradients that costs tens of milliseconds a frame (measured on
the idea-ui docs app: about 140 ms per scroll frame under Wine, most of it
text). Direct2D draws the same scene on the GPU, skips nodes outside the
visible area, and keeps each line's text layout between frames.

The Direct2D renderer is opt-in while it reaches parity. Any value of
`IDEALYST_WINDOWS_RENDERER` other than `gdi`, `d2d` or empty stops the app with
an error, so a misspelt name can't silently measure the wrong renderer. Code
that builds a backend directly can pass the renderer instead:
`WindowsBackend::with_renderer(hwnd, RendererKind::D2d)`.

### Why text measurement moves with the renderer

Text has to be measured by the engine that draws it. GDI and DirectWrite give
slightly different advances, kerning and line heights, so text measured by one
and drawn by the other wraps in the wrong places. On Direct2D, every text
measurement (wrapping, `max_lines` truncation, button labels, code blocks)
uses DirectWrite. The word-wrapping logic itself (`wrap.rs`) is shared by
both renderers.

### Custom fonts

Fonts registered with `typeface!` are installed for GDI (which native controls
and the GDI+ renderer use) and loaded a second time into a DirectWrite font
collection for the Direct2D renderer. DirectWrite can't see fonts installed
through GDI's private-font calls, so without the second load a registered
family would fall back to a system font.

### Differences you may see between the renderers

- **Edges.** GDI+ centres pixels on whole coordinates, so a shape edge that
  falls on a whole pixel is drawn half-covered on both sides, a soft one-pixel
  seam. Direct2D draws the same edge crisp, as the web and Apple backends do.
- **Glyphs.** The two text engines rasterize glyphs differently. Line breaks
  and box sizes come from the engine that draws, so layout matches what you
  see on each.

## Running on a Mac with Wine

`idealyst run windows` and `idealyst dev --windows` cross-compile to
`x86_64-pc-windows-gnu` and, on a non-Windows host, launch the `.exe` under
Wine (`crates/tools/run/windows/src/wine.rs`). You need:

- the Rust target: `rustup target add x86_64-pc-windows-gnu`;
- the MinGW linker: `brew install mingw-w64`;
- **Wine 9 or newer.** Rust 1.78+ programs need `bcryptprimitives.dll`, which
  Wine 8 doesn't ship, and the launcher refuses older versions. Homebrew no
  longer offers the WineHQ builds; use the
  [Gcenx macOS builds](https://github.com/Gcenx/macOS_Wine_builds/releases).

Point `IDEALYST_WINE` at the `wine` binary. Use `$HOME`, not `~`, because the
shell does not expand `~` inside quotes:

```bash
export IDEALYST_WINE="$HOME/.local/wine-devel-11.18/Wine Devel.app/Contents/Resources/wine/bin/wine"
```

The launcher runs apps in their own Wine prefix, `~/.idealyst/wine-prefix`,
unless `WINEPREFIX` is set, so a newer Wine never upgrades a prefix another
program uses. Apps under Wine register in `~/.idealyst/apps-wine` instead of
`~/.idealyst/apps`, because Wine process ids aren't Mac process ids.

### Which Wine runs which renderer

| Renderer | Wine setup |
|---|---|
| GDI+ | any Wine 9+ (the devel build is fine) |
| Direct2D | **Wine-Staging** plus **DXVK-macOS** |

Plain Wine can't run the Direct2D renderer on macOS. Its DirectComposition is
a placeholder (`E_NOTIMPL`), and its Direct3D layer can't create the
Direct3D 10-level device Direct2D needs on Apple GPUs. Wine-Staging includes a
DirectComposition implementation, and DXVK gives Direct2D a working Direct3D
device on Metal (through MoltenVK). To set it up:

1. Download `wine-staging-<version>-osx64.tar.xz` from the Gcenx releases and
   extract it, for example to `~/.local/wine-staging-<version>`.
2. Download the `-builtin` archive from
   [Gcenx/DXVK-macOS](https://github.com/Gcenx/DXVK-macOS/releases) and copy its
   `x86_64-windows/d3d11.dll` and `d3d10core.dll` into the Staging install's
   `Contents/Resources/wine/lib/wine/x86_64-windows/`. Keep the originals if you
   may want them back.
3. Run with a separate prefix:

```bash
export IDEALYST_WINE="$HOME/.local/wine-staging-<version>/Wine Staging.app/Contents/Resources/wine/bin/wine"
export WINEPREFIX="$HOME/.idealyst/wine-prefix-staging"
IDEALYST_WINDOWS_RENDERER=d2d idealyst run windows
```

**Known gaps of this setup (Wine-Staging 11.18 + DXVK-macOS 1.10.3):**

- DirectComposition *surfaces* are not implemented. The renderer doesn't use
  them (it presents one composition swapchain), so this doesn't affect it.
- Direct2D layers (`PushLayer`) are stubs, so containers with rounded corners
  and `overflow: hidden` clip to their square box under Wine. They clip
  correctly on Windows.
- Apps crash intermittently inside DXVK's `d3d11.dll` (always at the same
  offset, on a DXVK worker thread) during the first frames; about a third of
  the idea-ui docs app's launches. The app uses Direct2D only from its UI
  thread, and a standalone program doing the same Direct2D work doesn't
  crash. On Windows 11 the same build started five times out of five with no
  crash, so this is a Wine/DXVK problem; still check real Windows before
  treating a crash under Wine as an app bug.
- An idle Direct2D app uses roughly a third of a CPU core under this setup
  while painting about once in 10 seconds: the load is in Wine/DXVK's own
  threads, not in repainting. On Windows 11 the same build idles at 0.5%
  (GDI+ at 0%).
- Wine's DirectWrite and Direct2D are not safe to use from several threads at
  once (Windows' are). The backend uses them from the UI thread only; its
  tests that touch them share a lock (`render_test_lock`) because the test
  harness runs tests on parallel threads.

### Wine gaps that affect both renderers

- There is no Segoe UI, so text falls back to Tahoma, and semibold (600)
  renders as bold (700).
- An unimplemented Windows function makes Wine start its debugger, which
  then hangs the app (alive, 0% CPU). An example was
  `SystemTimeToTzSpecificLocalTimeEx`, which `zone-offset` now looks up at
  run time.
- Screenshots of Wine windows come out black on macOS, which is why the
  robot screenshot reads the renderer's own frame instead of the screen.

### Running the backend's tests under Wine

Cargo splits a test runner path on spaces, so link the `wine` binary to a
path without spaces first:

```bash
ln -sf "$IDEALYST_WINE" ~/.local/bin/wine-runner
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER=~/.local/bin/wine-runner \
DXVK_STATE_CACHE=0 \
  cargo test -p backend-windows --target x86_64-pc-windows-gnu --tests
```

`DXVK_STATE_CACHE=0` stops DXVK writing `.dxvk-cache` files into the working
directory. Under a Wine without Direct2D, the Direct2D renderer's tests print
`SKIP` with the reason and pass. On Windows they always run.

## Running on a real Windows machine from the Mac (parity included)

Set `IDEALYST_WINDOWS_REMOTE` to an SSH destination and every Windows launch
path (`idealyst run windows`, `idealyst dev --windows`, and therefore
`idealyst test --parity …,windows`) runs the app on that machine instead of
under Wine. The build, the parity runner and the MCP tools stay on the Mac.

```bash
export IDEALYST_WINDOWS_REMOTE=you@192.168.65.2
export IDEALYST_WINDOWS_REMOTE_KEY=~/.ssh/idealyst_vm   # optional: a key just for this machine
idealyst test --parity web,windows websites/idea-ui-docs
```

This is the way to check whether a difference seen under Wine is Wine's or
the app's: run the same command with and without `IDEALYST_WINDOWS_REMOTE`.

How it works (`crates/tools/run/windows/src/remote.rs`):

- The `.exe` (and its `assets/` folder) is copied to `C:\idealyst\<app>` on
  the remote machine with `scp`.
- Programs started over SSH have no desktop, so the app is started by a
  scheduled task in the desktop session of the logged-in user. **Someone has
  to be logged in to the machine's desktop.**
- The app binds its robot bridge there on a port fixed by its name (in
  9718–10717), so two apps on one machine don't collide; an SSH forward exposes
  it on a free local port, and a registration in `~/.idealyst/apps` points
  at that port, so the parity runner and the MCP tools find the app as if it
  were local. Loopback URLs the app is given (a dev server, for example) are
  forwarded back to the Mac the same way.
- The local `ssh` session is the app's lifetime: stopping it (Ctrl-C, the
  parity runner's teardown) stops the app. Windows' OpenSSH leaves a
  disconnected session's shells running, so the launcher watches the
  session's own `sshd.exe` instead.
- `IDEALYST_WINDOW_SIZE` (the parity viewport) and `IDEALYST_WINDOWS_RENDERER`
  are passed through from the environment.
- The app's console output is in `C:\idealyst\<app>\app.log`.

The backend's own tests can run there too. `scripts/windows-vm-runner.sh` is a
cargo test runner that copies each test binary to the machine, runs it in the
desktop session, and returns its output and exit code:

```bash
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER=scripts/windows-vm-runner.sh \
  cargo test -p backend-windows -p host-win32 --target x86_64-pc-windows-gnu
```

Running the same command with the Wine runner shows whether a failure is
Wine's. Set `IDEALYST_WINDOWS_REMOTE_DIRECT=1` to run in the SSH session
instead of the desktop (faster, but there is no desktop for window tests).

### Setting up the Windows machine

1. Install and start OpenSSH Server (an administrator PowerShell):
   `Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0`, then
   `Start-Service sshd` and `Set-Service sshd -StartupType Automatic`.
2. Allow port 22 on every network type. The rule OpenSSH installs covers
   only Private networks, and VM networks are often classed Public:
   `New-NetFirewallRule -Name idealyst-ssh-any -DisplayName 'SSH' -Direction Inbound -Protocol TCP -LocalPort 22 -Action Allow -Profile Any`.
3. Add the Mac's public key. For an administrator account it goes in
   `C:\ProgramData\ssh\administrators_authorized_keys`, which sshd only reads
   when Administrators and SYSTEM are its only users:
   `icacls C:\ProgramData\ssh\administrators_authorized_keys /inheritance:r /grant *S-1-5-32-544:F /grant *S-1-5-18:F`.
   The launcher logs in non-interactively, so the key needs no passphrase or
   must be loaded in `ssh-agent`; a key made only for this machine keeps your
   main key out of it.
4. Optionally make PowerShell the SSH shell:
   `New-ItemProperty -Path HKLM:\SOFTWARE\OpenSSH -Name DefaultShell -Value C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe -PropertyType String -Force`.

A UTM or other VM without 3D acceleration runs the Direct2D renderer on
WARP, Windows' software rasterizer: right for checking output and crashes,
not for timing.
To try the vello GPU path there anyway, put the redistributable WARP DLL in
the app's folder on the VM (`C:\idealyst\<app>-windows\`; the remote launch
does not clear it) and lift `hardware` in `canvas-vello`'s
`VELLO_REQUIREMENTS` for that run.

## Testing on a Windows machine or VM by hand

The `.exe` built on the Mac runs on Windows unchanged. It depends only on
system DLLs (the C runtime is the UCRT built into Windows 10 and 11), so a
small VM that couldn't compile the app can still run it:

1. Build on the Mac: `idealyst build --windows --release` (in the app's
   directory).
2. Copy `target/idealyst/<app>/windows/target/x86_64-pc-windows-gnu/release/<app>-windows.exe`
   to the VM, together with the `assets/` folder next to it, if there is one.
3. Run it from a terminal so its log is visible. To pick the renderer:
   `set IDEALYST_WINDOWS_RENDERER=d2d` (cmd) or
   `$env:IDEALYST_WINDOWS_RENDERER="d2d"` (PowerShell).

On a Windows-on-Arm VM (VMware or Parallels on an Apple-silicon Mac), the x64
`.exe` runs under Windows' built-in emulation. That's fine for checking
correctness, but timings there don't represent x64 hardware.
