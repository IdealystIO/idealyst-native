# esp32-ili9341 — backend-cpu on real (or simulated) hardware

ESP32-C3 firmware that drives the `backend-cpu` software rasterizer
against an **ILI9341 320×240 SPI display**. The exact same scene
the desktop preview shows — title bar, color swatches, body text,
translucent overlay — rendered on a real (or Wokwi-virtual) panel
over a 40 MHz SPI bus.

## Crate is outside the workspace

This crate lives at `examples/esp32-ili9341/` but is **not** a
member of the root workspace — its target triple
(`riscv32imc-esp-espidf`), profile settings, and build-script
linker wiring conflict with the host workspace. Build commands
below assume you've `cd`'d into this directory.

## Toolchain install (one-time)

You need three things on top of stock Rust: the ESP toolchain
manager, the IDF linker proxy, and the flasher.

```bash
# 1. Tool manager. Pulls down the matching rustc + LLVM bits.
cargo install espup
espup install

# Adds e.g. ~/export-esp.sh on macOS/Linux that puts IDF tools on PATH.
source ~/export-esp.sh

# 2. Linker proxy. esp-idf-sys uses this to thread its linker
#    args through cargo correctly.
cargo install ldproxy

# 3. Flasher (skip if you're only running in Wokwi).
cargo install espflash

# 4. (Skip — `riscv32imc-esp-espidf` is a tier-3 Rust target with
#    no pre-built std. We build std from source via `build-std`,
#    which only requires the `rust-src` component (already pulled
#    by `rust-toolchain.toml`). `rustup target add` would error
#    with "component 'rust-std' for target ... is unavailable".)
```

> **Note:** `espup` installs a custom Rust toolchain at
> `~/.rustup/toolchains/esp/` (the Xtensa-LLVM fork, needed only
> for ESP32 / S2 / S3). For the **C3** (RISC-V) we use upstream
> nightly — see `rust-toolchain.toml`. If you bump esp-idf-svc and
> the build complains about a missing rustc feature, bump the
> nightly pin in `rust-toolchain.toml` to a newer date.

## Build

From this directory:

```bash
cargo build --release
```

The first build pulls + compiles the entire ESP-IDF C SDK (one-time,
~5 min, ~3 GB of downloads), then your Rust code (~30 s). Subsequent
builds reuse the cached SDK and take seconds.

A debug build also works but exceeds the C3's 4 MB flash partition
unless you trim deps — release is the path of least resistance.

### Host-environment gotchas (Apple Silicon)

Got hit by these the first time around — if you see one of these
exact errors, here's the fix:

1. **`riscv32-esp-elf-gcc: error: unrecognized command-line option '--target=riscv32-none'`** —
   `embuild < 0.33` injects clang-only flags into CMAKE_C_FLAGS. Make
   sure you're on `esp-idf-svc = "0.51"+`, which pulls embuild 0.33.

2. **`clang: error: unknown argument: '--traditional-format'`** —
   GCC's driver is calling `as` from PATH, which on macOS is Apple's
   clang-based assembler. The fix is a one-line symlink so GCC finds
   its own bundled cross-`as`:
   ```bash
   ln -s riscv32-esp-elf-as \
       ~/.espressif/tools/riscv32-esp-elf/esp-13.2.0_20230928/riscv32-esp-elf/bin/as
   ```
   (Adjust the version dir if espup installed a different one.)

3. **`fatal error: cstdint: No such file or directory`** (or `memory`,
   `cassert`, `cxxabi.h`, …) — the toolchain's libstdc++ headers
   didn't extract, likely because of a partial download. Re-extract:
   ```bash
   TOOL=~/.espressif/tools/riscv32-esp-elf/esp-13.2.0_20230928
   rm -rf "$TOOL"
   mkdir -p "$TOOL"
   tar -C "$TOOL" -xf \
       ~/.espressif/dist/riscv32-esp-elf-13.2.0_20230928-aarch64-apple-darwin.tar.xz
   # Then re-add the `as` symlink from item 2.
   ```

4. **`Unable to find libclang: ... (mach-o file, but is an incompatible architecture (have 'arm64', need 'x86_64'))`** —
   `rustup` is using the x86_64 nightly even though your Mac is
   arm64. The `rust-toolchain.toml` here pins `nightly-2025-09-01`
   without a host suffix — rustup may choose either variant if you
   have both installed. Install the arm64 one explicitly and remove
   the x86_64:
   ```bash
   rustup install nightly-2025-09-01-aarch64-apple-darwin --component rust-src
   rustup uninstall nightly-2025-09-01-x86_64-apple-darwin
   ```

5. **`ldproxy: Cannot locate argument '--ldproxy-linker <linker>'`** —
   The binary's `build.rs` didn't propagate `esp-idf-sys`'s link
   args. That `build.rs` lives in this directory and calls
   `embuild::espidf::sysenv::output()` — don't delete it.

6. **`error adding symbols: file format not recognized`** for
   `libcompiler_builtins-*.rlib` — `lto = true` makes rustc emit
   LLVM IR bitcode in rlibs, which the ESP cross-linker can't read.
   Keep `lto = false` in `[profile.release]` (set in `Cargo.toml`
   here) unless you separately set up linker-plugin LTO.

## Run in Wokwi (no hardware needed)

[Wokwi](https://wokwi.com/) is a free in-browser ESP32 simulator
with a virtual ILI9341. The fastest path:

1. Install the [Wokwi for VS Code](https://marketplace.visualstudio.com/items?itemName=Wokwi.wokwi-vscode)
   extension. You'll need a free Wokwi account; the extension
   prompts on first run.
2. Open this directory in VS Code (`code .` from inside
   `examples/esp32-ili9341/`).
3. Build: `cargo build --release`.
4. Press **F1 → Wokwi: Start Simulator**. The extension reads
   `wokwi.toml` (firmware path) and `diagram.json` (wiring),
   spawns the simulator in a side panel, and you should see the
   demo scene a couple seconds after boot.

The Wokwi window streams the serial console too — `log::info!`
lines from `main` show up there.

## Run on real hardware

Wire an ILI9341 breakout to the ESP32-C3 per the table in
[src/main.rs](src/main.rs). Then:

```bash
cargo run --release
```

The `runner` defined in `.cargo/config.toml` would normally call
`espflash`, but we keep that off by default so Wokwi users aren't
prompted for a serial port that doesn't exist. To use the hardware
path, add to `.cargo/config.toml`:

```toml
[target.riscv32imc-esp-espidf]
runner = "espflash flash --monitor"
```

## What's in the box

- [src/main.rs](src/main.rs) — Surface impl over `mipidsi` (the
  one piece of hardware-specific code), scene builder, render loop.
- [diagram.json](diagram.json) — Wokwi wiring (ESP32-C3 + ILI9341).
- [wokwi.toml](wokwi.toml) — Wokwi extension launcher.
- [sdkconfig.defaults](sdkconfig.defaults) — IDF config overrides
  (main-task stack, Wi-Fi/BT disabled for size, faster SPI).
- [.cargo/config.toml](.cargo/config.toml) — cross-build target +
  linker + env-var setup.

## Adapting to other panels

The Surface impl is fully generic over the `mipidsi` model. To
target a different controller:

| Panel       | Constant to use                         | Notes                          |
|-------------|------------------------------------------|--------------------------------|
| ILI9341     | `mipidsi::models::ILI9341Rgb565` (default) | 320×240 landscape          |
| ST7789      | `mipidsi::models::ST7789`               | 240×240 / 240×320 portrait     |
| ST7735      | `mipidsi::models::ST7735s`              | 160×128 — fits tiny budgets    |
| GC9A01      | `mipidsi::models::GC9A01`               | 240×240 round                  |

Swap the import + the `Builder::new(...)` argument in `main`.
Pin assignments stay the same.

## Performance expectations

| Operation              | Cost on C3 + 40 MHz SPI                |
|------------------------|-----------------------------------------|
| Full-viewport clear    | ~30 ms (uses panel's RAMWR fast path)   |
| Per-pixel `draw_iter`  | ~24 µs per pixel (slow — avoid)         |
| Solid `fill_solid`     | ~10× faster than per-pixel              |
| Text glyph (8 px)      | ~1.5 ms (64 pixel updates)              |

Frame budget at 30 fps is 33 ms. With the demo scene that lands at
~12 ms / frame on the C3, leaving 20 ms of headroom. A full UI of
~20 nodes should hold 30 fps; busier scenes will want the
damage-rect tracking mentioned in
[../../crates/backend/cpu/README.md](../../crates/backend/cpu/README.md).
