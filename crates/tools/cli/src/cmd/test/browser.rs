//! The headless browser `idealyst test --web` drives, and the DevTools
//! protocol session that sizes it.
//!
//! The page's viewport comes from `Emulation.setDeviceMetricsOverride`, not
//! from `--window-size`: headless Chrome will not make a window narrower than
//! 500 px, so a 390 px window still reports `innerWidth` 500. The override
//! sets the layout viewport exactly, and for a device preset also sets the
//! device pixel ratio, touch input and the user agent.
//!
//! Every override ends when its DevTools session detaches, so the session is
//! held open for the whole run ([`HeadlessBrowser`] owns it) and closed with
//! the browser.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Chromium-family browsers the runner will drive headlessly, in preference
/// order. `IDEALYST_CHROME` overrides the search entirely.
///
/// Covers macOS and Linux installs, so the web half of a parity run can
/// start off a mac. Every candidate below takes the same `--headless=new`
/// flags and speaks the DevTools protocol.
const CHROME_CANDIDATES: &[&str] = &[
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/usr/bin/brave-browser",
    "/usr/bin/brave",
    "/usr/bin/microsoft-edge",
];

/// The headless browser to drive: `IDEALYST_CHROME` if set (honored even when
/// it names a bare command on `PATH`), else the first existing
/// [`CHROME_CANDIDATES`] entry.
pub(super) fn headless_browser() -> Option<String> {
    if let Ok(explicit) = std::env::var("IDEALYST_CHROME") {
        // A path must exist; a bare command name is left to the OS to resolve,
        // so `IDEALYST_CHROME=chromium` works too.
        if !explicit.contains('/') || Path::new(&explicit).exists() {
            return Some(explicit);
        }
        eprintln!("[test] IDEALYST_CHROME={explicit} does not exist — falling back to a search");
    }
    CHROME_CANDIDATES
        .iter()
        .find(|c| Path::new(c).exists())
        .map(|c| c.to_string())
}

/// What the page is told about the screen it runs on.
#[derive(Debug, Clone, PartialEq)]
pub struct Emulation {
    /// Layout viewport, CSS px.
    pub width: u32,
    pub height: u32,
    /// `devicePixelRatio`.
    pub device_scale_factor: f64,
    /// Chrome's mobile mode: a meta-viewport-aware layout viewport, overlay
    /// scrollbars, and `(hover: none)` with touch.
    pub mobile: bool,
    /// Touch input: `navigator.maxTouchPoints` > 0, `(pointer: coarse)`, and
    /// `ontouchstart` on `window`.
    pub touch: bool,
    /// User agent and `navigator.platform`; `None` keeps the browser's own.
    pub user_agent: Option<(&'static str, &'static str)>,
}

impl Emulation {
    /// A desktop page at exactly `width` x `height`: 1x pixels, a mouse.
    pub fn desktop(width: u32, height: u32) -> Emulation {
        Emulation { width, height, device_scale_factor: 1.0, mobile: false, touch: false, user_agent: None }
    }

    /// `--mobile`: a phone-like page at `width` x `height` — mobile mode,
    /// touch, [`MOBILE_DEVICE_SCALE_FACTOR`], and the generic phone user agent.
    pub fn mobile(width: u32, height: u32) -> Emulation {
        Emulation {
            width,
            height,
            device_scale_factor: MOBILE_DEVICE_SCALE_FACTOR,
            mobile: true,
            touch: true,
            user_agent: Some((IPHONE_UA, "iPhone")),
        }
    }

    /// One-line description for the run's per-size label.
    pub fn describe(&self) -> String {
        let mut s = format!("{}x{}", self.width, self.height);
        if self.device_scale_factor != 1.0 {
            s.push_str(&format!(" @{}x", self.device_scale_factor));
        }
        if self.touch {
            s.push_str(", touch");
        }
        s
    }
}

/// `devicePixelRatio` `--mobile` emulates: current phones are 3x.
pub const MOBILE_DEVICE_SCALE_FACTOR: f64 = 3.0;
/// Touch points a touch-emulated page reports (`navigator.maxTouchPoints`),
/// the value iOS Safari reports.
pub const EMULATED_TOUCH_POINTS: u32 = 5;

const IPHONE_UA: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 \
     (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";
const IPAD_UA: &str = "Mozilla/5.0 (iPad; CPU OS 17_0 like Mac OS X) AppleWebKit/605.1.15 \
     (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";
const ANDROID_UA: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 7) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36";

/// A named `--device`: the CSS-px viewport, pixel ratio and user agent of a
/// real device, in portrait.
#[derive(Debug)]
pub struct DevicePreset {
    pub name: &'static str,
    pub width: u32,
    pub height: u32,
    pub device_scale_factor: f64,
    pub user_agent: &'static str,
    pub platform: &'static str,
}

pub const DEVICE_PRESETS: &[DevicePreset] = &[
    DevicePreset { name: "iphone-se", width: 375, height: 667, device_scale_factor: 2.0, user_agent: IPHONE_UA, platform: "iPhone" },
    DevicePreset { name: "iphone-14", width: 390, height: 844, device_scale_factor: 3.0, user_agent: IPHONE_UA, platform: "iPhone" },
    DevicePreset { name: "iphone-14-pro-max", width: 430, height: 932, device_scale_factor: 3.0, user_agent: IPHONE_UA, platform: "iPhone" },
    DevicePreset { name: "iphone-15", width: 393, height: 852, device_scale_factor: 3.0, user_agent: IPHONE_UA, platform: "iPhone" },
    DevicePreset { name: "pixel-7", width: 412, height: 915, device_scale_factor: 2.625, user_agent: ANDROID_UA, platform: "Linux armv8l" },
    DevicePreset { name: "galaxy-s23", width: 360, height: 780, device_scale_factor: 3.0, user_agent: ANDROID_UA, platform: "Linux armv8l" },
    DevicePreset { name: "ipad-mini", width: 768, height: 1024, device_scale_factor: 2.0, user_agent: IPAD_UA, platform: "iPad" },
    DevicePreset { name: "ipad-air", width: 820, height: 1180, device_scale_factor: 2.0, user_agent: IPAD_UA, platform: "iPad" },
    DevicePreset { name: "ipad-pro-12", width: 1024, height: 1366, device_scale_factor: 2.0, user_agent: IPAD_UA, platform: "iPad" },
];

/// The preset called `name` (case-insensitive), or an error listing them all.
pub fn device_preset(name: &str) -> Result<&'static DevicePreset> {
    let wanted = name.trim().to_ascii_lowercase();
    DEVICE_PRESETS.iter().find(|d| d.name == wanted).with_context(|| {
        let names: Vec<&str> = DEVICE_PRESETS.iter().map(|d| d.name).collect();
        format!("unknown --device {name:?} (known: {})", names.join(", "))
    })
}

impl DevicePreset {
    pub fn emulation(&self) -> Emulation {
        Emulation {
            width: self.width,
            height: self.height,
            device_scale_factor: self.device_scale_factor,
            mobile: true,
            touch: true,
            user_agent: Some((self.user_agent, self.platform)),
        }
    }
}

/// The DevTools commands that put a page into `e`. Every field is set
/// explicitly — touch off, the browser's own user agent back — so moving from
/// a phone preset to a desktop size undoes the phone.
pub fn emulation_commands(e: &Emulation, default_user_agent: &str) -> Vec<(&'static str, Value)> {
    let (ua, platform) = match e.user_agent {
        Some((ua, platform)) => (ua.to_string(), platform.to_string()),
        None => (default_user_agent.to_string(), String::new()),
    };
    let mut ua_params = json!({ "userAgent": ua });
    if !platform.is_empty() {
        ua_params["platform"] = json!(platform);
    }
    let touch = if e.touch {
        json!({ "enabled": true, "maxTouchPoints": EMULATED_TOUCH_POINTS })
    } else {
        json!({ "enabled": false })
    };
    vec![
        (
            "Emulation.setDeviceMetricsOverride",
            json!({
                "width": e.width,
                "height": e.height,
                "deviceScaleFactor": e.device_scale_factor,
                "mobile": e.mobile,
                "screenWidth": e.width,
                "screenHeight": e.height,
            }),
        ),
        ("Emulation.setTouchEmulationEnabled", touch),
        ("Emulation.setUserAgentOverride", ua_params),
    ]
}

/// A headless browser with one DevTools session on its page, held for the
/// browser's lifetime. Dropping it closes the session and kills the browser
/// (with its helper processes: it leads its own process group).
pub struct HeadlessBrowser {
    child: Child,
    cdp: Cdp,
    url: String,
    default_user_agent: String,
    /// Chrome's own stderr — the evidence when the browser is what failed.
    pub log: PathBuf,
}

impl Drop for HeadlessBrowser {
    fn drop(&mut self) {
        super::kill_tree(&mut self.child);
    }
}

impl HeadlessBrowser {
    /// Start the browser on a blank page and attach to it. Nothing is loaded
    /// until [`Self::show`] — so the app's first boot already sees the
    /// emulated screen.
    pub fn launch(url: &str) -> Result<HeadlessBrowser> {
        let chrome = headless_browser().context(
            "no headless browser found — install Chrome/Chromium or set IDEALYST_CHROME",
        )?;
        // A profile directory private to THIS runner process, wiped on the way in.
        //
        // Chrome refuses to start a second instance against a profile whose
        // `SingletonLock` still exists, and a run that was killed (Ctrl-C, a
        // timeout, a stray `pkill`) leaves that lock pointing at a dead pid. The
        // next launch then hands its URL to a singleton that isn't there and exits
        // immediately — the app never dials, and the runner blames its own 240s
        // readiness wait. One killed run silently poisoned every run after it.
        let profile = std::env::temp_dir().join(format!("idealyst-test-chrome-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&profile);
        let log = profile.with_extension("log");
        let errlog = std::fs::File::create(&log).map(Stdio::from).unwrap_or_else(|_| Stdio::null());
        let mut cmd = Command::new(&chrome);
        cmd.args([
            "--headless=new",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            // No scrollbar eating width; 1:1 device pixels until an emulation
            // says otherwise (introspection reports logical px).
            "--hide-scrollbars",
            "--force-device-scale-factor=1",
            // Port 0: Chrome picks one and writes it to `DevToolsActivePort`
            // in the profile, so parallel runs never collide.
            "--remote-debugging-port=0",
        ])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("about:blank")
        .stdout(Stdio::null())
        .stderr(errlog);
        let child = super::own_group(&mut cmd)
            .spawn()
            .with_context(|| format!("starting the headless browser {chrome}"))?;
        // Owned from here, so an early return below still kills it.
        let mut launching = Launching(Some(child));

        let port = wait_for_devtools_port(&profile, launching.child(), Duration::from_secs(20))
            .with_context(|| format!("the headless browser never opened DevTools — see {}", log.display()))?;
        let mut cdp = Cdp::attach_page(port, Duration::from_secs(20))
            .with_context(|| format!("attaching to the headless browser's page — see {}", log.display()))?;
        let default_user_agent = cdp
            .evaluate("navigator.userAgent")?
            .as_str()
            .unwrap_or_default()
            .to_string();
        Ok(HeadlessBrowser {
            child: launching.0.take().expect("set above"),
            cdp,
            url: url.to_string(),
            default_user_agent,
            log,
        })
    }

    /// Put the page into `e` and (re)load the app under it, returning once
    /// the page's `load` fired — the previous page, and its connection to the
    /// relay, is gone by then.
    ///
    /// Reloading rather than resizing in place gives the app the screen from
    /// its first frame (touch support and the user agent are read at boot)
    /// and gives each size a fresh app, as a device would.
    pub fn show(&mut self, e: &Emulation) -> Result<()> {
        for (method, params) in emulation_commands(e, &self.default_user_agent) {
            self.cdp.call(method, params)?;
        }
        self.cdp.call("Page.enable", json!({}))?;
        self.cdp.events.clear();
        self.cdp.call("Page.navigate", json!({ "url": self.url }))?;
        let loaded = self.cdp.wait_event("Page.loadEventFired", Duration::from_secs(120));
        self.cdp.call("Page.disable", json!({}))?;
        loaded.with_context(|| format!("loading {} in the headless browser", self.url))?;
        Ok(())
    }

    /// Evaluate `expr` in the page and return its JSON value.
    #[cfg(test)]
    pub fn evaluate(&mut self, expr: &str) -> Result<Value> {
        self.cdp.evaluate(expr)
    }
}

/// Kills a browser that has not finished launching.
struct Launching(Option<Child>);
impl Launching {
    fn child(&mut self) -> &mut Child {
        self.0.as_mut().expect("present until taken")
    }
}
impl Drop for Launching {
    fn drop(&mut self) {
        if let Some(c) = &mut self.0 {
            super::kill_tree(c);
        }
    }
}

/// The DevTools port Chrome bound for `--remote-debugging-port=0`: the first
/// line of `<profile>/DevToolsActivePort`. Fails fast when the browser exits.
fn wait_for_devtools_port(profile: &Path, child: &mut Child, timeout: Duration) -> Result<u16> {
    let file = profile.join("DevToolsActivePort");
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(port) = std::fs::read_to_string(&file)
            .ok()
            .and_then(|s| s.lines().next()?.trim().parse::<u16>().ok())
        {
            return Ok(port);
        }
        if let Ok(Some(status)) = child.try_wait() {
            anyhow::bail!("the headless browser exited ({status})");
        }
        anyhow::ensure!(Instant::now() < deadline, "no {} within {timeout:?}", file.display());
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A minimal GET. Chrome's DevTools HTTP endpoint keeps the connection open
/// after the response whatever the request says, so reading to EOF hangs;
/// read exactly `Content-Length` bytes instead, under a timeout.
fn http_get(port: u16, path: &str) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").ok()?;
    let mut reader = BufReader::new(stream);
    let mut length = None;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                length = v.trim().parse::<usize>().ok();
            }
        }
    }
    let mut body = vec![0u8; length?];
    reader.read_exact(&mut body).ok()?;
    String::from_utf8(body).ok()
}

/// One DevTools-protocol session (a websocket to a page target).
struct Cdp {
    ws: tungstenite::WebSocket<TcpStream>,
    next_id: u64,
    /// Events read while waiting for a command's reply, kept for
    /// [`Self::wait_event`].
    events: VecDeque<Value>,
}

impl Cdp {
    fn attach_page(port: u16, timeout: Duration) -> Result<Cdp> {
        let deadline = Instant::now() + timeout;
        let ws_url = loop {
            // The PAGE target: Chrome lists its own UI surfaces too
            // (`"type": "browser_ui"`), and those never host the app.
            let page = http_get(port, "/json/list")
                .and_then(|body| serde_json::from_str::<Value>(&body).ok())
                .and_then(|list| {
                    list.as_array()?.iter().find_map(|t| {
                        (t["type"] == "page").then(|| t["webSocketDebuggerUrl"].as_str().map(str::to_string))?
                    })
                });
            if let Some(url) = page {
                break url;
            }
            anyhow::ensure!(Instant::now() < deadline, "no page target within {timeout:?}");
            std::thread::sleep(Duration::from_millis(100));
        };
        let stream = TcpStream::connect(("127.0.0.1", port)).context("connecting to DevTools")?;
        // Bounded reads, so a wait can time out instead of blocking forever.
        stream.set_read_timeout(Some(Duration::from_millis(500)))?;
        let (ws, _) = tungstenite::client(ws_url.as_str(), stream)
            .map_err(|e| anyhow::anyhow!("DevTools websocket handshake: {e}"))?;
        Ok(Cdp { ws, next_id: 1, events: VecDeque::new() })
    }

    /// Read one message, or `None` when the read timed out.
    fn read(&mut self) -> Result<Option<Value>> {
        match self.ws.read() {
            Ok(tungstenite::Message::Text(text)) => Ok(Some(serde_json::from_str(&text)?)),
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
            {
                Ok(None)
            }
            Err(e) => Err(anyhow::anyhow!("DevTools connection: {e}")),
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({ "id": id, "method": method, "params": params });
        self.ws
            .send(tungstenite::Message::text(msg.to_string()))
            .map_err(|e| anyhow::anyhow!("DevTools send {method}: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match self.read()? {
                Some(v) if v["id"].as_u64() == Some(id) => {
                    if let Some(err) = v.get("error") {
                        anyhow::bail!("DevTools {method} failed: {err}");
                    }
                    return Ok(v["result"].clone());
                }
                Some(v) if v.get("method").is_some() => self.events.push_back(v),
                _ => {}
            }
            anyhow::ensure!(Instant::now() < deadline, "DevTools {method}: no reply");
        }
    }

    fn wait_event(&mut self, method: &str, timeout: Duration) -> Result<Value> {
        if let Some(i) = self.events.iter().position(|e| e["method"] == method) {
            return Ok(self.events.remove(i).expect("index found"));
        }
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(v) = self.read()? {
                if v["method"] == method {
                    return Ok(v);
                }
            }
            anyhow::ensure!(Instant::now() < deadline, "no {method} within {timeout:?}");
        }
    }

    fn evaluate(&mut self, expr: &str) -> Result<Value> {
        let r = self.call(
            "Runtime.evaluate",
            json!({ "expression": expr, "returnByValue": true, "awaitPromise": true }),
        )?;
        Ok(r["result"]["value"].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by_method<'a>(cmds: &'a [(&'static str, Value)], m: &str) -> &'a Value {
        &cmds.iter().find(|(k, _)| *k == m).unwrap_or_else(|| panic!("no {m}")).1
    }

    /// A plain `--viewport 390x844` must give the page `innerWidth` 390:
    /// headless Chrome's window bottoms out at 500 px, so the size has to be
    /// a metrics override, at 1x with no touch.
    #[test]
    fn regression_a_narrow_viewport_is_a_metrics_override_not_a_window_size() {
        let cmds = emulation_commands(&Emulation::desktop(390, 844), "HeadlessChrome UA");
        let m = by_method(&cmds, "Emulation.setDeviceMetricsOverride");
        assert_eq!((m["width"].as_u64(), m["height"].as_u64()), (Some(390), Some(844)));
        assert_eq!(m["deviceScaleFactor"], 1.0);
        assert_eq!(m["mobile"], false);
        assert_eq!(by_method(&cmds, "Emulation.setTouchEmulationEnabled")["enabled"], false);
        // The browser's own UA is put back, undoing an earlier preset.
        assert_eq!(by_method(&cmds, "Emulation.setUserAgentOverride")["userAgent"], "HeadlessChrome UA");
    }

    #[test]
    fn a_device_preset_emulates_the_phone() {
        let e = device_preset("iPhone-14").unwrap().emulation();
        let cmds = emulation_commands(&e, "default");
        let m = by_method(&cmds, "Emulation.setDeviceMetricsOverride");
        assert_eq!((m["width"].as_u64(), m["height"].as_u64()), (Some(390), Some(844)));
        assert_eq!(m["deviceScaleFactor"], 3.0);
        assert_eq!(m["mobile"], true);
        let t = by_method(&cmds, "Emulation.setTouchEmulationEnabled");
        assert_eq!(t["enabled"], true);
        assert!(t["maxTouchPoints"].as_u64().unwrap() >= 1);
        let ua = by_method(&cmds, "Emulation.setUserAgentOverride");
        assert!(ua["userAgent"].as_str().unwrap().contains("iPhone"));
        assert_eq!(ua["platform"], "iPhone");
        assert_eq!(e.describe(), "390x844 @3x, touch");
    }

    #[test]
    fn an_unknown_device_lists_the_known_ones() {
        let err = device_preset("nokia-3310").unwrap_err().to_string();
        assert!(err.contains("iphone-14") && err.contains("pixel-7"), "{err}");
    }

    /// Real headless Chrome: the emulated page reports exactly what the run
    /// asked for. Skips when no browser is installed (CI without Chrome).
    #[test]
    fn headless_chrome_reports_the_emulated_screen() {
        if headless_browser().is_none() {
            eprintln!("skipping: no headless browser (set IDEALYST_CHROME)");
            return;
        }
        let page = "data:text/html,<meta name=viewport content='width=device-width'><body>hi</body>";
        let mut b = HeadlessBrowser::launch(page).expect("launch");
        let probe = "JSON.stringify([innerWidth, innerHeight, devicePixelRatio, navigator.maxTouchPoints, \
                     matchMedia('(pointer: coarse)').matches, navigator.userAgent])";
        let read = |b: &mut HeadlessBrowser| -> Value {
            serde_json::from_str(b.evaluate(probe).unwrap().as_str().unwrap()).unwrap()
        };

        b.show(&device_preset("iphone-14").unwrap().emulation()).unwrap();
        let v = read(&mut b);
        assert_eq!(v[0], 390, "{v}");
        assert_eq!(v[1], 844, "{v}");
        assert_eq!(v[2], 3.0, "{v}");
        assert!(v[3].as_u64().unwrap() >= 1, "{v}");
        assert_eq!(v[4], true, "{v}");
        assert!(v[5].as_str().unwrap().contains("iPhone"), "{v}");

        // Back to a plain narrow viewport: still exactly 390 wide (below
        // headless Chrome's 500 px window floor), and the phone is undone.
        b.show(&Emulation::desktop(390, 844)).unwrap();
        let v = read(&mut b);
        assert_eq!(v[0], 390, "{v}");
        assert_eq!(v[2], 1.0, "{v}");
        assert_eq!(v[3], 0, "{v}");
        assert!(!v[5].as_str().unwrap().contains("iPhone"), "{v}");
    }
}
