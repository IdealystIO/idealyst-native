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
//! | `text <string>` | `OK` (replaces the focused field's text) |
//! | `type <string>` | `OK` (typed at the focused field's cursor) |
//! | `key backspace\|delete\|enter\|tab\|left\|right\|up\|down\|home\|end\|escape` | `OK` |
//! | `find <query>` | `OK <n> <ms>`, then one line per element: `<i> <node>` |
//! | `wait <ms> <query>` | as `find`, once something matches |
//! | `click <i>` | `OK <ms>`: clicks element `i` of the last `find` / `wait` |
//! | `windows` | `OK <n>`, then one line per window |
//! | `events` | `OK <n>`, then the window events heard since the last `events` |
//!
//! A `<query>` is `key=value` words, `+` standing for a space in a value:
//! `id=`, `text=`, `text~=` (contains), `desc=`, `desc~=`, `pkg=`,
//! `class=`. For example `find pkg=com.android.systemui text~=start+now`.
//!
//! A failure replies `ERR <error>`. The screen shows whether the service is
//! on, kept current by `on_enabled_change`, and the app listens for window
//! changes from launch.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use accessibility_service::{Error, GlobalAction, ImageFormat, Key, Node, Query};
use runtime_core::{on_scope_drop, signal, text, ui, Element, Signal};

const PORT: u16 = 7878;

static SERVER_STARTED: AtomicBool = AtomicBool::new(false);
/// The last `find` / `wait` result, for `click <i>`.
static LAST_FOUND: Mutex<Vec<Node>> = Mutex::new(Vec::new());
/// Window events heard on the UI thread, drained by `events`.
static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn parse_query<'a>(words: impl Iterator<Item = &'a str>) -> Result<Query, String> {
    let mut q = Query::new();
    for word in words {
        let (key, value) = word.split_once('=').ok_or(format!("{word:?} is not key=value"))?;
        let value = value.replace('+', " ");
        q = match key {
            "id" => q.view_id(value),
            "text" => q.text(value),
            "text~" => q.text_contains(value),
            "desc" => q.description(value),
            "desc~" => q.description_contains(value),
            "pkg" => q.package(value),
            "class" => q.class_name(value),
            other => return Err(format!("unknown query key {other:?}")),
        };
    }
    Ok(q)
}

fn found_reply(nodes: Vec<Node>, ms: u128) -> (String, Vec<u8>) {
    let mut reply = format!("OK {} {ms}", nodes.len());
    for (i, n) in nodes.iter().enumerate() {
        reply.push_str(&format!("\n{i} {n:?}"));
    }
    *LAST_FOUND.lock().unwrap() = nodes;
    (reply, Vec::new())
}

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

fn key(name: &str) -> Result<Key, String> {
    Ok(match name {
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "enter" => Key::Enter,
        "tab" => Key::Tab,
        "left" => Key::Left,
        "right" => Key::Right,
        "up" => Key::Up,
        "down" => Key::Down,
        "home" => Key::Home,
        "end" => Key::End,
        "escape" => Key::Escape,
        other => return Err(format!("unknown key {other}")),
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
        "type" => {
            let text = line.trim_start().strip_prefix("type").unwrap_or("").trim_start();
            accessibility_service::type_text(text).map_err(err)?;
            Ok(("OK".to_string(), Vec::new()))
        }
        "key" => {
            accessibility_service::press_key(key(words.next().unwrap_or(""))?).map_err(err)?;
            Ok(("OK".to_string(), Vec::new()))
        }
        "find" => {
            let q = parse_query(words)?;
            let nodes = pollster::block_on(accessibility_service::find(&q)).map_err(err)?;
            Ok(found_reply(nodes, ms()))
        }
        "wait" => {
            let timeout = Duration::from_millis(u64::from(parse_u32(words.next())?));
            let q = parse_query(words)?;
            let nodes =
                pollster::block_on(accessibility_service::wait_for(&q, timeout)).map_err(err)?;
            Ok(found_reply(nodes, ms()))
        }
        "click" => {
            let i = parse_u32(words.next())? as usize;
            let node = LAST_FOUND.lock().unwrap().get(i).cloned().ok_or(format!("no element {i}"))?;
            pollster::block_on(accessibility_service::click(&node)).map_err(err)?;
            Ok((format!("OK {}", ms()), Vec::new()))
        }
        "windows" => {
            let windows = pollster::block_on(accessibility_service::windows()).map_err(err)?;
            let mut reply = format!("OK {}", windows.len());
            for w in windows {
                reply.push_str(&format!("\n{w:?}"));
            }
            Ok((reply, Vec::new()))
        }
        "events" => {
            let events = std::mem::take(&mut *EVENTS.lock().unwrap());
            let mut reply = format!("OK {}", events.len());
            for e in events {
                reply.push('\n');
                reply.push_str(&e);
            }
            Ok((reply, Vec::new()))
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
    let windows = accessibility_service::on_window_change(|e| {
        EVENTS.lock().unwrap().push(format!("{e:?}"));
    });
    on_scope_drop(move || drop(windows));

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
