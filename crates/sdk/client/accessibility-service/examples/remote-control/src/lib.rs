//! `remote-control` — exercises the `accessibility-service` SDK end to end
//! on a real device, driven from the host over a localhost command port.
//!
//! ```text
//! idealyst build --android
//! adb -s <serial> install -r dist/android/remote-control-0.1.0.apk
//! adb -s <serial> shell settings put secure enabled_accessibility_services \
//!     io.idealyst.remotecontrol/io.idealyst.accessibilityservice.RustAccessibilityService
//! adb -s <serial> shell am start -n io.idealyst.remotecontrol/.MainActivity
//! adb -s <serial> forward tcp:7878 tcp:7878
//! ```
//!
//! Then send one command per line to `127.0.0.1:7878` (`nc 127.0.0.1 7878`).
//! The app keeps running in the background (`keep_alive`), so another app
//! can be in front while it captures and taps.
//!
//! | command | reply |
//! |---|---|
//! | `enabled` | `OK true` / `OK false` |
//! | `shot rgba` / `shot jpeg <quality>` | `OK <w> <h> <rotation> <bytes> <ms>`, then the bytes |
//! | `burst <n>` | `n` back-to-back JPEG screenshots, waiting out `TooSoon`: `OK <n> <fps>` |
//! | `soon` | two screenshots with no wait: `OK <second's error>` |
//! | `tap <x> <y>` | `OK <ms>` when the gesture completed |
//! | `long <x> <y> <ms>` | `OK <ms>` |
//! | `swipe <x1> <y1> <x2> <y2> <ms>` | `OK <ms>` |
//! | `global back\|home\|recents\|notifications\|quick_settings\|lock_screen` | `OK` |
//! | `text <string>` | `OK` (into the focused field) |
//!
//! A failure replies `ERR <error>`. The screen shows whether the service is
//! on, kept current by `on_enabled_change`.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use accessibility_service::{Error, GlobalAction, ImageFormat};
use runtime_core::{on_scope_drop, signal, text, ui, Element, Signal};

const PORT: u16 = 7878;

static SERVER_STARTED: AtomicBool = AtomicBool::new(false);

fn parse_f32(word: Option<&str>) -> Result<f32, String> {
    word.ok_or("missing number")?.parse().map_err(|e| format!("{e}"))
}

fn parse_u32(word: Option<&str>) -> Result<u32, String> {
    word.ok_or("missing number")?.parse().map_err(|e| format!("{e}"))
}

fn global(name: &str) -> Result<GlobalAction, String> {
    Ok(match name {
        "back" => GlobalAction::Back,
        "home" => GlobalAction::Home,
        "recents" => GlobalAction::Recents,
        "notifications" => GlobalAction::Notifications,
        "quick_settings" => GlobalAction::QuickSettings,
        "lock_screen" => GlobalAction::LockScreen,
        other => return Err(format!("unknown global action {other}")),
    })
}

/// Run one command; `Ok((reply line, trailing bytes))`.
fn run(line: &str) -> Result<(String, Vec<u8>), String> {
    let err = |e: Error| format!("{e:?}");
    let mut words = line.split_whitespace();
    let started = Instant::now();
    let ms = || started.elapsed().as_millis();
    match words.next().unwrap_or("") {
        "enabled" => Ok((format!("OK {}", accessibility_service::is_enabled()), Vec::new())),
        "shot" => {
            let format = match words.next() {
                Some("rgba") => ImageFormat::Rgba,
                Some("jpeg") => ImageFormat::Jpeg {
                    quality: parse_u32(words.next())? as u8,
                },
                other => return Err(format!("shot rgba|jpeg <q>, not {other:?}")),
            };
            let shot = pollster::block_on(accessibility_service::screenshot(format)).map_err(err)?;
            Ok((
                format!(
                    "OK {} {} {:?} {} {}",
                    shot.width,
                    shot.height,
                    shot.rotation,
                    shot.data.len(),
                    ms()
                ),
                shot.data,
            ))
        }
        "burst" => {
            let n = parse_u32(words.next())?;
            let mut taken = 0;
            while taken < n {
                match pollster::block_on(accessibility_service::screenshot(ImageFormat::Jpeg {
                    quality: 70,
                })) {
                    Ok(_) => taken += 1,
                    Err(Error::TooSoon { retry_after_ms }) => {
                        std::thread::sleep(Duration::from_millis(u64::from(retry_after_ms)))
                    }
                    Err(e) => return Err(err(e)),
                }
            }
            let fps = f64::from(n) / started.elapsed().as_secs_f64();
            Ok((format!("OK {n} {fps:.2}"), Vec::new()))
        }
        "soon" => {
            pollster::block_on(accessibility_service::screenshot(ImageFormat::Rgba)).map_err(err)?;
            let second = pollster::block_on(accessibility_service::screenshot(ImageFormat::Rgba));
            Ok((format!("OK {:?}", second.map(|s| s.width)), Vec::new()))
        }
        "tap" => {
            let (x, y) = (parse_f32(words.next())?, parse_f32(words.next())?);
            pollster::block_on(accessibility_service::tap(x, y)).map_err(err)?;
            Ok((format!("OK {}", ms()), Vec::new()))
        }
        "long" => {
            let (x, y) = (parse_f32(words.next())?, parse_f32(words.next())?);
            let duration = parse_u32(words.next())?;
            pollster::block_on(accessibility_service::long_press(x, y, duration)).map_err(err)?;
            Ok((format!("OK {}", ms()), Vec::new()))
        }
        "swipe" => {
            let from = (parse_f32(words.next())?, parse_f32(words.next())?);
            let to = (parse_f32(words.next())?, parse_f32(words.next())?);
            let duration = parse_u32(words.next())?;
            pollster::block_on(accessibility_service::swipe(from, to, duration)).map_err(err)?;
            Ok((format!("OK {}", ms()), Vec::new()))
        }
        "global" => {
            let action = global(words.next().unwrap_or(""))?;
            accessibility_service::global_action(action).map_err(err)?;
            Ok(("OK".to_string(), Vec::new()))
        }
        "text" => {
            let text = line.trim_start().strip_prefix("text").unwrap_or("").trim_start();
            accessibility_service::set_focused_text(text).map_err(err)?;
            Ok(("OK".to_string(), Vec::new()))
        }
        other => Err(format!("unknown command {other:?}")),
    }
}

fn serve(stream: TcpStream) -> std::io::Result<()> {
    let mut out = stream.try_clone()?;
    for line in BufReader::new(stream).lines() {
        let line = line?;
        match run(&line) {
            Ok((reply, bytes)) => {
                writeln!(out, "{reply}")?;
                out.write_all(&bytes)?;
            }
            Err(e) => writeln!(out, "ERR {e}")?,
        }
        out.flush()?;
    }
    Ok(())
}

/// The command port, on its own thread so a blocking screenshot or gesture
/// never stalls the UI thread. Started once per process.
fn start_server() {
    if SERVER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        let listener = match TcpListener::bind(("127.0.0.1", PORT)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("remote-control: cannot listen on {PORT}: {e}");
                return;
            }
        };
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let _ = serve(stream);
            });
        }
    });
}

pub fn app() -> Element {
    start_server();
    let enabled: Signal<bool> = signal(accessibility_service::is_enabled());
    let subscription = accessibility_service::on_enabled_change(move |on| enabled.set(on));
    on_scope_drop(move || drop(subscription));

    let open = move || {
        let _ = accessibility_service::open_settings();
    };

    ui! {
        view {
            text(move || if enabled.get() {
                "Accessibility service: on".to_string()
            } else {
                "Accessibility service: off".to_string()
            })
            text(format!("Command port: 127.0.0.1:{PORT}"))
            button(label = "Open accessibility settings".to_string(), on_click = open)
        }
    }
}

/// Android entry: the generated wrapper's `attach` mounts this.
pub fn scene_app() -> Element {
    app()
}

/// SDK-handler registration seam. `accessibility-service` is a capability
/// crate, not a rendered primitive, so there is nothing to register.
pub fn register_scene_extensions<H: runtime_scene::Host>(
    _registry: &mut runtime_scene::Registry<H>,
) {
}
