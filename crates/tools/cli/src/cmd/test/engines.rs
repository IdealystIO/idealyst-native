//! The browser engines a web suite runs in, behind one interface
//! ([`Browser`]): Chromium over the DevTools protocol ([`HeadlessBrowser`]),
//! Firefox over WebDriver BiDi ([`Firefox`]), and WebKit as Safari over
//! classic WebDriver ([`Safari`]).
//!
//! The runner owns each browser's one control session. For Chromium the
//! robot relay also reaches the tab itself over DevTools (several clients
//! may attach), but a WebDriver session has exactly one client, so for
//! Firefox and Safari the runner answers the browser verbs the app can't —
//! `load_url`, the cookie verbs, `open_page`, `screenshot` — through its
//! browser control socket (`robot_test::browser_control`).
//!
//! What each engine can emulate differs, and the run says so rather than
//! pretending:
//!
//! - Chromium: everything ([`Emulation`]): viewport, pixel ratio, touch,
//!   mobile layout, user agent, in place.
//! - Firefox: viewport and pixel ratio in place; touch, the coarse pointer
//!   media query and the user agent are preferences fixed at launch, so a size that changes them relaunches Firefox (the app's
//!   dev server stays up; only the browser restarts).
//! - Safari: the viewport only (its window is sized until the page's inner
//!   size matches). No touch, user agent or pixel ratio; not headless (a
//!   real Safari window, which `safaridriver` drives); one cookie jar, so no
//!   isolated `open_page`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use base64::Engine as _;
use serde_json::{json, Value};

use super::browser::{Emulation, HeadlessBrowser};

/// A web engine a test runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Chromium,
    Firefox,
    Webkit,
}

impl Engine {
    pub fn name(self) -> &'static str {
        match self {
            Engine::Chromium => "chromium",
            Engine::Firefox => "firefox",
            Engine::Webkit => "webkit",
        }
    }

    pub fn parse(name: &str) -> Option<Engine> {
        Some(match name {
            "chromium" | "chrome" => Engine::Chromium,
            "firefox" => Engine::Firefox,
            "webkit" | "safari" => Engine::Webkit,
            _ => return None,
        })
    }
}

/// One browser the runner drives.
pub trait Browser: Send {
    fn engine(&self) -> Engine;
    /// Put the page into `e` and (re)load the app under it.
    fn show(&mut self, e: &Emulation) -> Result<()>;
    /// Resize the page in place (the robot's `set_viewport`).
    fn resize(&mut self, width: u32, height: u32) -> Result<()>;
    /// The browser's own output, when it fails.
    fn log(&self) -> &Path;
    /// `load_url {url}`: navigate to `url` resolved against the page; the
    /// address loaded.
    fn navigate(&mut self, _url: &str) -> Result<String> {
        anyhow::bail!("the robot relay serves load_url for {}", self.engine().name())
    }
    /// `screenshot`: the page's PNG.
    fn screenshot(&mut self) -> Result<Vec<u8>> {
        anyhow::bail!("the robot relay serves screenshots for {}", self.engine().name())
    }
    /// `set_cookie` (the relay's argument shape).
    fn set_cookie(&mut self, _args: &Value) -> Result<()> {
        anyhow::bail!("the robot relay serves cookies for {}", self.engine().name())
    }
    /// `get_cookies`: the page's cookies (the relay's shape).
    fn get_cookies(&mut self) -> Result<Value> {
        anyhow::bail!("the robot relay serves cookies for {}", self.engine().name())
    }
    /// `clear_cookies {name?}`.
    fn clear_cookies(&mut self, _name: Option<&str>) -> Result<()> {
        anyhow::bail!("the robot relay serves cookies for {}", self.engine().name())
    }
    /// `open_page {url, isolated}`: open another tab at `url`.
    fn open_page(&mut self, _url: &str, _isolated: bool) -> Result<()> {
        anyhow::bail!("the robot relay serves open_page for {}", self.engine().name())
    }
}

/// Start `engine` on a blank page (Chromium, Firefox), or attach Safari.
pub fn launch(engine: Engine, url: &str, profiles_dir: &Path) -> Result<Box<dyn Browser>> {
    Ok(match engine {
        Engine::Chromium => Box::new(HeadlessBrowser::launch(url, profiles_dir)?),
        Engine::Firefox => Box::new(Firefox::launch(url, profiles_dir)?),
        Engine::Webkit => Box::new(Safari::launch(url)?),
    })
}

impl Browser for HeadlessBrowser {
    fn engine(&self) -> Engine {
        Engine::Chromium
    }
    fn show(&mut self, e: &Emulation) -> Result<()> {
        HeadlessBrowser::show(self, e)
    }
    fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        HeadlessBrowser::resize(self, width, height)
    }
    fn log(&self) -> &Path {
        &self.log
    }
}

// ---------------------------------------------------------------------------
// Finding the browsers
// ---------------------------------------------------------------------------

/// Firefox: `IDEALYST_FIREFOX`, else the usual install places.
pub fn firefox_binary() -> Option<String> {
    if let Ok(p) = std::env::var("IDEALYST_FIREFOX") {
        return Some(p);
    }
    [
        "/Applications/Firefox.app/Contents/MacOS/firefox",
        "/usr/bin/firefox",
        "/usr/lib/firefox/firefox",
        "/snap/bin/firefox",
        "C:\\Program Files\\Mozilla Firefox\\firefox.exe",
    ]
    .iter()
    .find(|p| Path::new(p).exists())
    .map(|p| p.to_string())
}

/// `safaridriver`, which ships with macOS.
pub fn safaridriver() -> Option<&'static str> {
    Some("/usr/bin/safaridriver").filter(|p| Path::new(p).exists())
}

// ---------------------------------------------------------------------------
// Firefox (WebDriver BiDi)
// ---------------------------------------------------------------------------

/// The Firefox preferences an emulation needs at launch: what Firefox
/// can't change on a running page.
pub fn firefox_prefs(e: &Emulation) -> Vec<(&'static str, String)> {
    let mut prefs = vec![
        // The page acts as the focused one: a headless window never is, and
        // `focus` / `blur` events would not fire (Chromium's
        // `setFocusEmulationEnabled`).
        ("focusmanager.testmode", "true".to_string()),
        ("browser.shell.checkDefaultBrowser", "false".to_string()),
        ("datareporting.policy.dataSubmissionEnabled", "false".to_string()),
        ("toolkit.telemetry.reportingpolicy.firstRun", "false".to_string()),
        ("browser.startup.homepage_override.mstone", "\"ignore\"".to_string()),
        // A debug bundle's wasm (hundreds of MB) stays cached between the
        // suite's page loads, as Chromium's `DISK_CACHE_SIZE_ARG` does
        // there. By default Firefox stores no entry over 50 MB, so every
        // load fetched and compiled the wasm again (measured, Firefox 160:
        // a 240 MB response is reused only with these). In KiB.
        ("browser.cache.disk.smart_size.enabled", "false".to_string()),
        ("browser.cache.disk.capacity", "2097152".to_string()),
        ("browser.cache.disk.max_entry_size", "1048576".to_string()),
        // `(pointer: coarse)` and `(hover: none)` for touch; a mouse with
        // hover otherwise (bits: 1 coarse, 2 fine, 4 hover).
        ("ui.primaryPointerCapabilities", if e.touch { "1" } else { "6" }.to_string()),
        ("ui.allPointerCapabilities", if e.touch { "1" } else { "6" }.to_string()),
        ("dom.w3c_touch_events.enabled", if e.touch { "1" } else { "0" }.to_string()),
        // Not `dom.meta-viewport.enabled`: with it on, headless Firefox
        // never answers `browsingContext.setViewport` (Firefox 150). It isn't
        // needed: `setViewport` makes the layout viewport the device's CSS
        // width, which is what a `width=device-width` meta viewport gives.
    ];
    if let Some((ua, _)) = e.user_agent {
        prefs.push(("general.useragent.override", format!("{ua:?}")));
    }
    prefs
}

/// What a Firefox launch fixed, to tell when a size needs a relaunch.
fn launch_key(e: &Emulation) -> (bool, Option<&'static str>) {
    (e.touch, e.user_agent.map(|(ua, _)| ua))
}

/// Firefox, headless, over WebDriver BiDi.
pub struct Firefox {
    child: Option<Child>,
    ws: Option<tungstenite::WebSocket<TcpStream>>,
    next_id: u64,
    context: String,
    url: String,
    profiles_dir: PathBuf,
    profile: PathBuf,
    log: PathBuf,
    launched: Option<(bool, Option<&'static str>)>,
    current: Option<Emulation>,
}

impl Drop for Firefox {
    fn drop(&mut self) {
        self.stop();
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

impl Firefox {
    /// Prepared, not yet started: [`Browser::show`] starts it with the
    /// preferences the first size needs.
    pub fn launch(url: &str, profiles_dir: &Path) -> Result<Firefox> {
        firefox_binary().context("Firefox isn't installed (or set IDEALYST_FIREFOX)")?;
        let profile = profiles_dir.join(format!("test-firefox-{}", std::process::id()));
        Ok(Firefox {
            child: None,
            ws: None,
            next_id: 1,
            context: String::new(),
            url: url.to_string(),
            profiles_dir: profiles_dir.to_path_buf(),
            log: profile.join("firefox-stderr.log"),
            profile,
            launched: None,
            current: None,
        })
    }

    fn stop(&mut self) {
        if let Some(ws) = &mut self.ws {
            let _ = ws.close(None);
        }
        self.ws = None;
        if let Some(c) = &mut self.child {
            super::kill_tree(c);
        }
        self.child = None;
    }

    /// (Re)start Firefox with `e`'s launch preferences, on `about:blank`.
    fn start(&mut self, e: &Emulation) -> Result<()> {
        self.stop();
        let _ = std::fs::remove_dir_all(&self.profile);
        std::fs::create_dir_all(&self.profile)
            .with_context(|| format!("creating the Firefox profile {}", self.profile.display()))?;
        let user_js: String =
            firefox_prefs(e).iter().map(|(k, v)| format!("user_pref({k:?}, {v});\n")).collect();
        std::fs::write(self.profile.join("user.js"), user_js)?;
        let binary = firefox_binary().context("Firefox isn't installed (or set IDEALYST_FIREFOX)")?;
        let err = std::fs::File::create(&self.log)?;
        let mut cmd = Command::new(&binary);
        cmd.args(["--headless", "--no-remote", "--remote-debugging-port", "0", "--profile"])
            .arg(&self.profile)
            .arg("about:blank")
            .stdout(Stdio::null())
            .stderr(err);
        let child = super::own_group(&mut cmd).spawn().with_context(|| format!("starting Firefox ({binary})"))?;
        self.child = Some(child);
        // Firefox prints `WebDriver BiDi listening on ws://127.0.0.1:<port>`.
        let deadline = Instant::now() + Duration::from_secs(30);
        let ws_url = loop {
            let text = std::fs::read_to_string(&self.log).unwrap_or_default();
            if let Some(at) = text.find("WebDriver BiDi listening on ") {
                let rest = &text[at + "WebDriver BiDi listening on ".len()..];
                break format!("{}/session", rest.lines().next().unwrap_or("").trim());
            }
            if let Some(Ok(Some(status))) = self.child.as_mut().map(|c| c.try_wait()) {
                anyhow::bail!("Firefox exited ({status}) — see {}", self.log.display());
            }
            anyhow::ensure!(Instant::now() < deadline, "Firefox never opened WebDriver BiDi — see {}", self.log.display());
            std::thread::sleep(Duration::from_millis(100));
        };
        let host = ws_url.trim_start_matches("ws://").split('/').next().unwrap_or("").to_string();
        let stream = TcpStream::connect(&host).with_context(|| format!("connecting to Firefox at {host}"))?;
        stream.set_read_timeout(Some(Duration::from_secs(120)))?;
        let (ws, _) = tungstenite::client(ws_url.as_str(), stream).map_err(|e| anyhow::anyhow!("BiDi handshake: {e}"))?;
        self.ws = Some(ws);
        self.call("session.new", json!({ "capabilities": {} }))?;
        let tree = self.call("browsingContext.getTree", json!({}))?;
        self.context = tree["contexts"][0]["context"].as_str().context("Firefox has no browsing context")?.to_string();
        self.launched = Some(launch_key(e));
        Ok(())
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let ws = self.ws.as_mut().context("Firefox isn't running")?;
        let id = self.next_id;
        self.next_id += 1;
        ws.send(tungstenite::Message::text(json!({ "id": id, "method": method, "params": params }).to_string()))
            .map_err(|e| anyhow::anyhow!("BiDi send {method}: {e}"))?;
        loop {
            let msg = ws.read().map_err(|e| anyhow::anyhow!("BiDi {method}: {e}"))?;
            let tungstenite::Message::Text(text) = msg else { continue };
            let v: Value = serde_json::from_str(&text)?;
            if v["id"].as_u64() != Some(id) {
                continue; // an event
            }
            if v["type"] == "error" {
                anyhow::bail!("Firefox {method}: {} ({})", v["message"].as_str().unwrap_or(""), v["error"].as_str().unwrap_or(""));
            }
            return Ok(v["result"].clone());
        }
    }

    fn set_viewport(&mut self, context: &str, e: &Emulation) -> Result<()> {
        self.call(
            "browsingContext.setViewport",
            json!({
                "context": context,
                "viewport": { "width": e.width, "height": e.height },
                "devicePixelRatio": e.device_scale_factor,
            }),
        )?;
        Ok(())
    }

    fn evaluate(&mut self, expr: &str) -> Result<Value> {
        let context = self.context.clone();
        let r = self.call(
            "script.evaluate",
            json!({ "expression": expr, "target": { "context": context }, "awaitPromise": true }),
        )?;
        anyhow::ensure!(r["type"] == "success", "Firefox evaluate {expr}: {}", r["exceptionDetails"]["text"]);
        Ok(bidi_value(&r["result"]))
    }

    fn resolve(&mut self, url: &str) -> Result<String> {
        let quoted = serde_json::to_string(url)?;
        self.evaluate(&format!("new URL({quoted}, location.href).href"))?
            .as_str()
            .map(str::to_string)
            .with_context(|| format!("{url:?} is not a URL"))
    }
}

/// A BiDi remote value as plain JSON.
pub fn bidi_value(v: &Value) -> Value {
    match v["type"].as_str() {
        Some("string") | Some("boolean") => v["value"].clone(),
        Some("number") => match &v["value"] {
            Value::String(s) => json!(s), // NaN, Infinity, -0
            n => n.clone(),
        },
        Some("array") => Value::Array(v["value"].as_array().into_iter().flatten().map(bidi_value).collect()),
        Some("object") => Value::Object(
            v["value"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|pair| Some((pair[0].as_str()?.to_string(), bidi_value(&pair[1]))))
                .collect(),
        ),
        _ => Value::Null,
    }
}

impl Browser for Firefox {
    fn engine(&self) -> Engine {
        Engine::Firefox
    }

    fn show(&mut self, e: &Emulation) -> Result<()> {
        if self.launched != Some(launch_key(e)) || self.ws.is_none() {
            let profiles = self.profiles_dir.clone();
            std::fs::create_dir_all(&profiles).ok();
            self.start(e)?;
        }
        let context = self.context.clone();
        self.set_viewport(&context, e)?;
        let url = self.url.clone();
        self.call("browsingContext.navigate", json!({ "context": context, "url": url, "wait": "complete" }))
            .with_context(|| format!("loading {url} in Firefox — see {}", self.log.display()))?;
        self.current = Some(e.clone());
        Ok(())
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        let mut e = self.current.clone().unwrap_or_else(|| Emulation::desktop(width, height));
        e.width = width;
        e.height = height;
        let context = self.context.clone();
        self.set_viewport(&context, &e)?;
        self.current = Some(e);
        Ok(())
    }

    fn log(&self) -> &Path {
        &self.log
    }

    fn navigate(&mut self, url: &str) -> Result<String> {
        let target = self.resolve(url)?;
        let context = self.context.clone();
        // `none`: the new page's app is what the robot waits for.
        self.call("browsingContext.navigate", json!({ "context": context, "url": target, "wait": "none" }))?;
        Ok(target)
    }

    fn screenshot(&mut self) -> Result<Vec<u8>> {
        let context = self.context.clone();
        let r = self.call("browsingContext.captureScreenshot", json!({ "context": context }))?;
        let data = r["data"].as_str().context("Firefox returned no screenshot")?;
        Ok(base64::engine::general_purpose::STANDARD.decode(data)?)
    }

    fn set_cookie(&mut self, args: &Value) -> Result<()> {
        let name = args["name"].as_str().context("set_cookie needs a string 'name'")?;
        let domain = match args["domain"].as_str() {
            Some(d) => d.to_string(),
            None => match args["url"].as_str() {
                Some(u) => self.evaluate(&format!("new URL({}).hostname", serde_json::to_string(u)?))?,
                None => self.evaluate("location.hostname")?,
            }
            .as_str()
            .unwrap_or_default()
            .to_string(),
        };
        let mut cookie = json!({
            "name": name,
            "value": { "type": "string", "value": args["value"].as_str().unwrap_or("") },
            "domain": domain,
            "path": args["path"].as_str().unwrap_or("/"),
        });
        if let Some(b) = args["http_only"].as_bool() {
            cookie["httpOnly"] = json!(b);
        }
        if let Some(b) = args["secure"].as_bool() {
            cookie["secure"] = json!(b);
        }
        if let Some(s) = args["same_site"].as_str() {
            cookie["sameSite"] = json!(s.to_ascii_lowercase());
        }
        if let Some(e) = args["expires"].as_f64() {
            cookie["expiry"] = json!(e as u64);
        }
        self.call("storage.setCookie", json!({ "cookie": cookie }))?;
        Ok(())
    }

    fn get_cookies(&mut self) -> Result<Value> {
        let host = self.evaluate("location.hostname")?;
        let r = self.call("storage.getCookies", json!({}))?;
        let host = host.as_str().unwrap_or_default().to_string();
        Ok(Value::Array(
            r["cookies"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|c| {
                    let d = c["domain"].as_str().unwrap_or("").trim_start_matches('.');
                    host == d || host.ends_with(&format!(".{d}"))
                })
                .map(|c| {
                    json!({
                        "name": c["name"], "value": c["value"]["value"], "domain": c["domain"], "path": c["path"],
                        "http_only": c["httpOnly"], "secure": c["secure"], "same_site": c["sameSite"],
                        "expires": c["expiry"],
                    })
                })
                .collect(),
        ))
    }

    fn clear_cookies(&mut self, name: Option<&str>) -> Result<()> {
        let filter = match name {
            Some(n) => json!({ "filter": { "name": n } }),
            None => json!({}),
        };
        self.call("storage.deleteCookies", filter)?;
        Ok(())
    }

    fn open_page(&mut self, url: &str, isolated: bool) -> Result<()> {
        let target = self.resolve(url)?;
        let mut params = json!({ "type": "tab" });
        if isolated {
            let ctx = self.call("browser.createUserContext", json!({}))?;
            params["userContext"] = ctx["userContext"].clone();
        }
        let tab = self.call("browsingContext.create", params)?;
        let tab = tab["context"].as_str().context("Firefox opened no tab")?.to_string();
        if let Some(e) = self.current.clone() {
            self.set_viewport(&tab, &e)?;
        }
        self.call("browsingContext.navigate", json!({ "context": tab, "url": target, "wait": "none" }))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Safari (classic WebDriver)
// ---------------------------------------------------------------------------

/// Safari, driven by `safaridriver`: a real (not headless) window.
pub struct Safari {
    driver: Option<Child>,
    port: u16,
    session: String,
    url: String,
    /// The tab the app runs in (`open_page` switches away and back).
    main: String,
    log: PathBuf,
}

impl Drop for Safari {
    fn drop(&mut self) {
        let _ = http_json(self.port, "DELETE", &format!("/session/{}", self.session), None);
        if let Some(c) = &mut self.driver {
            super::kill_tree(c);
        }
    }
}

impl Safari {
    pub fn launch(url: &str) -> Result<Safari> {
        let driver = safaridriver().context("safaridriver isn't installed (it ships with macOS)")?;
        let port = super::free_port()?;
        let log = std::env::temp_dir().join(format!("idealyst-test-safaridriver-{}.log", std::process::id()));
        let err = std::fs::File::create(&log)?;
        let child = Command::new(driver)
            .args(["-p", &port.to_string()])
            .stdout(Stdio::null())
            .stderr(err)
            .spawn()
            .context("starting safaridriver")?;
        let mut safari = Safari { driver: Some(child), port, session: String::new(), url: url.to_string(), main: String::new(), log };
        let deadline = Instant::now() + Duration::from_secs(20);
        while TcpStream::connect(("127.0.0.1", port)).is_err() {
            anyhow::ensure!(Instant::now() < deadline, "safaridriver never listened on {port}");
            std::thread::sleep(Duration::from_millis(100));
        }
        let r = http_json(
            port,
            "POST",
            "/session",
            Some(json!({ "capabilities": { "alwaysMatch": { "browserName": "safari" } } })),
        )?;
        if let Some(e) = r["value"]["error"].as_str() {
            let message = r["value"]["message"].as_str().unwrap_or("");
            anyhow::bail!(
                "Safari refused a WebDriver session ({e}: {message}). Turn on remote automation once: \
                 `safaridriver --enable` (it asks for your password), or Safari > Settings > Advanced > \
                 Show features for web developers, then Develop > Allow Remote Automation"
            );
        }
        safari.session = r["value"]["sessionId"].as_str().context("safaridriver returned no session")?.to_string();
        safari.main = safari.cmd("GET", "/window", None)?.as_str().unwrap_or_default().to_string();
        Ok(safari)
    }

    fn cmd(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let r = http_json(self.port, method, &format!("/session/{}{path}", self.session), body)?;
        if let Some(e) = r["value"]["error"].as_str() {
            anyhow::bail!("Safari {method} {path}: {e}: {}", r["value"]["message"].as_str().unwrap_or(""));
        }
        Ok(r["value"].clone())
    }

    fn evaluate(&mut self, expr: &str) -> Result<Value> {
        self.cmd("POST", "/execute/sync", Some(json!({ "script": format!("return ({expr});"), "args": [] })))
    }

    /// Size the window so the page's inner size is `width` x `height`: the
    /// window includes Safari's toolbar, measured and added back.
    fn fit(&mut self, width: u32, height: u32) -> Result<()> {
        let (mut w, mut h) = (width as i64, height as i64);
        for _ in 0..3 {
            self.cmd("POST", "/window/rect", Some(json!({ "width": w, "height": h })))?;
            let inner = self.evaluate("[innerWidth, innerHeight]")?;
            let (iw, ih) = (inner[0].as_i64().unwrap_or(0), inner[1].as_i64().unwrap_or(0));
            if iw == width as i64 && ih == height as i64 {
                return Ok(());
            }
            w += width as i64 - iw;
            h += height as i64 - ih;
        }
        Ok(())
    }
}

impl Browser for Safari {
    fn engine(&self) -> Engine {
        Engine::Webkit
    }

    fn show(&mut self, e: &Emulation) -> Result<()> {
        let url = self.url.clone();
        self.cmd("POST", "/url", Some(json!({ "url": url })))?;
        self.fit(e.width, e.height)
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        self.fit(width, height)
    }

    fn log(&self) -> &Path {
        &self.log
    }

    fn navigate(&mut self, url: &str) -> Result<String> {
        let target = self
            .evaluate(&format!("new URL({}, location.href).href", serde_json::to_string(url)?))?
            .as_str()
            .map(str::to_string)
            .with_context(|| format!("{url:?} is not a URL"))?;
        self.cmd("POST", "/url", Some(json!({ "url": target })))?;
        Ok(target)
    }

    fn screenshot(&mut self) -> Result<Vec<u8>> {
        let data = self.cmd("GET", "/screenshot", None)?;
        Ok(base64::engine::general_purpose::STANDARD.decode(data.as_str().context("no screenshot")?)?)
    }

    fn set_cookie(&mut self, args: &Value) -> Result<()> {
        let name = args["name"].as_str().context("set_cookie needs a string 'name'")?;
        let mut cookie = json!({ "name": name, "value": args["value"].as_str().unwrap_or(""), "path": args["path"].as_str().unwrap_or("/") });
        for (from, to) in [("domain", "domain"), ("same_site", "sameSite")] {
            if let Some(v) = args[from].as_str() {
                cookie[to] = json!(v);
            }
        }
        for (from, to) in [("http_only", "httpOnly"), ("secure", "secure")] {
            if let Some(v) = args[from].as_bool() {
                cookie[to] = json!(v);
            }
        }
        if let Some(e) = args["expires"].as_f64() {
            cookie["expiry"] = json!(e as u64);
        }
        self.cmd("POST", "/cookie", Some(json!({ "cookie": cookie })))?;
        Ok(())
    }

    fn get_cookies(&mut self) -> Result<Value> {
        let all = self.cmd("GET", "/cookie", None)?;
        Ok(Value::Array(
            all.as_array()
                .into_iter()
                .flatten()
                .map(|c| {
                    json!({
                        "name": c["name"], "value": c["value"], "domain": c["domain"], "path": c["path"],
                        "http_only": c["httpOnly"], "secure": c["secure"], "same_site": c["sameSite"],
                        "expires": c["expiry"],
                    })
                })
                .collect(),
        ))
    }

    fn clear_cookies(&mut self, name: Option<&str>) -> Result<()> {
        match name {
            Some(n) => self.cmd("DELETE", &format!("/cookie/{n}"), None)?,
            None => self.cmd("DELETE", "/cookie", None)?,
        };
        Ok(())
    }

    fn open_page(&mut self, url: &str, isolated: bool) -> Result<()> {
        anyhow::ensure!(
            !isolated,
            "Safari's WebDriver has one cookie jar per session, so open_page can't give the new tab its own: \
             use open_page_shared on webkit"
        );
        let target = self
            .evaluate(&format!("new URL({}, location.href).href", serde_json::to_string(url)?))?
            .as_str()
            .map(str::to_string)
            .with_context(|| format!("{url:?} is not a URL"))?;
        let tab = self.cmd("POST", "/window/new", Some(json!({ "type": "tab" })))?;
        let handle = tab["handle"].as_str().context("Safari opened no tab")?.to_string();
        self.cmd("POST", "/window", Some(json!({ "handle": handle })))?;
        self.cmd("POST", "/url", Some(json!({ "url": target })))?;
        let main = self.main.clone();
        self.cmd("POST", "/window", Some(json!({ "handle": main })))?;
        Ok(())
    }
}

/// One JSON request to a local WebDriver server (`Connection: close`, read
/// to the end).
fn http_json(port: u16, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).with_context(|| format!("connecting to 127.0.0.1:{port}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(120)))?;
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    let mut reader = BufReader::new(stream);
    let mut length = None;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
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
    let mut buf = Vec::new();
    match length {
        Some(n) => {
            buf.resize(n, 0);
            reader.read_exact(&mut buf)?;
        }
        None => {
            reader.read_to_end(&mut buf)?;
        }
    }
    if buf.is_empty() {
        return Ok(json!({ "value": null }));
    }
    Ok(serde_json::from_slice(&buf).with_context(|| format!("{method} {path}: not JSON"))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engines_parse() {
        assert_eq!(Engine::parse("chrome"), Some(Engine::Chromium));
        assert_eq!(Engine::parse("safari"), Some(Engine::Webkit));
        assert_eq!(Engine::parse("lynx"), None);
    }

    /// CrewForge: a debug wasm (225 MB) was fetched and compiled again on
    /// every page load, because no browser cache kept an entry that large.
    /// Firefox stores none over 50 MB by default; the prefs must allow a
    /// 240 MB one (values in KiB). Checked here as prefs rather than through
    /// a real Firefox: driving one takes the runner's WebDriver session.
    /// That these prefs make headless Firefox reuse a 240 MB response was
    /// measured by hand; Chromium's flag has a real-browser test in
    /// `browser.rs`.
    #[test]
    fn regression_firefox_keeps_a_large_debug_wasm_cached() {
        let prefs: std::collections::HashMap<_, _> = firefox_prefs(&Emulation::desktop(1280, 800)).into_iter().collect();
        let kib = |k: &str| prefs[k].parse::<u64>().unwrap();
        assert_eq!(prefs["browser.cache.disk.smart_size.enabled"], "false", "smart sizing would override the capacity");
        assert!(kib("browser.cache.disk.max_entry_size") * 1024 >= 240 * 1024 * 1024);
        assert!(kib("browser.cache.disk.capacity") >= kib("browser.cache.disk.max_entry_size"));
    }

    /// Firefox can't switch touch, the pointer media query or the user agent
    /// on a running page: they are launch preferences, and a size that
    /// changes them relaunches.
    #[test]
    fn firefox_launch_prefs_follow_the_emulation() {
        let phone = Emulation::mobile(390, 844);
        let prefs: std::collections::HashMap<_, _> = firefox_prefs(&phone).into_iter().collect();
        assert_eq!(prefs["ui.primaryPointerCapabilities"], "1", "coarse");
        assert_eq!(prefs["dom.w3c_touch_events.enabled"], "1");
        assert!(!prefs.contains_key("dom.meta-viewport.enabled"), "it stalls setViewport in headless Firefox");
        assert!(prefs["general.useragent.override"].contains("iPhone"));
        assert_eq!(prefs["focusmanager.testmode"], "true");
        let desktop: std::collections::HashMap<_, _> = firefox_prefs(&Emulation::desktop(1280, 800)).into_iter().collect();
        assert_eq!(desktop["ui.primaryPointerCapabilities"], "6", "fine + hover");
        assert!(!desktop.contains_key("general.useragent.override"));
        assert_ne!(launch_key(&phone), launch_key(&Emulation::desktop(390, 844)));
        assert_eq!(launch_key(&Emulation::desktop(390, 844)), launch_key(&Emulation::desktop(1280, 800)), "a resize alone doesn't relaunch");
    }

    #[test]
    fn bidi_values_become_json() {
        let v = json!({ "type": "array", "value": [
            { "type": "number", "value": 390 },
            { "type": "string", "value": "x" },
            { "type": "object", "value": [["a", { "type": "boolean", "value": true }]] },
            { "type": "undefined" },
        ]});
        assert_eq!(bidi_value(&v), json!([390, "x", { "a": true }, null]));
    }

    /// The real Firefox: launched with a phone's preferences, the page sees
    /// the emulated viewport, a coarse pointer and the user agent; a desktop
    /// size relaunches it with a fine pointer. Skipped where Firefox isn't
    /// installed.
    #[test]
    fn firefox_emulates_the_screen_it_is_given() {
        if firefox_binary().is_none() {
            eprintln!("firefox_emulates_the_screen_it_is_given: no Firefox; skipped");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let page = dir.path().join("page.html");
        std::fs::write(&page, "<!doctype html><meta name=viewport content='width=device-width'><body>x</body>").unwrap();
        let url = format!("file://{}", page.display());
        let mut ff = Firefox::launch(&url, dir.path()).unwrap();
        ff.show(&Emulation::mobile(390, 844)).unwrap();
        let seen = ff.evaluate("[innerWidth, matchMedia('(pointer: coarse)').matches, navigator.userAgent, devicePixelRatio]").unwrap();
        assert_eq!(seen[0], 390, "{seen}");
        assert_eq!(seen[1], true, "coarse pointer: {seen}");
        assert!(seen[2].as_str().unwrap().contains("iPhone"), "{seen}");
        assert_eq!(seen[3], 3.0, "{seen}");
        ff.show(&Emulation::desktop(1280, 800)).unwrap();
        let seen = ff.evaluate("[innerWidth, matchMedia('(pointer: fine)').matches]").unwrap();
        assert_eq!(seen, json!([1280, true]));
        ff.resize(1000, 700).unwrap();
        assert_eq!(ff.evaluate("innerWidth").unwrap(), 1000);
        let png = ff.screenshot().unwrap();
        assert_eq!(&png[..4], b"\x89PNG");
    }
}
