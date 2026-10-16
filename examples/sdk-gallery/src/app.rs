//! The gallery screen: one panel per SDK, each exercising the SDK's whole
//! author surface the way an app would.

use std::rc::Rc;
use std::time::Duration;

use idea_ui::{install_idea_theme, light_theme, tone, variant, Button};
use runtime_core::driver::spawn_async;
use runtime_core::{component, effect, rx, signal, ui, Element, IntoElement, Signal};
use video::VideoBind;
use webview::WebViewBind;

/// The gallery root.
pub fn app() -> Element {
    install_idea_theme(light_theme());
    ui! {
        scroll_view(style = styles::Scroll()) {
            view(style = styles::Root()) {
                text(style = styles::Title()) { "SDK gallery".to_string() }
                text(style = styles::Subtitle()) {
                    "One panel per client SDK — the same app on every platform.".to_string()
                }
                SvgPanel()
                ClipboardPanel()
                ConnectivityPanel()
                NotificationsPanel()
                WebViewPanel()
                VideoPanel()
                AudioPanel()
                LocationPanel()
                SharePanel()
                CameraPanel()
                RecordingPanel()
            }
        }
    }
}

/// A titled card holding one SDK's controls.
#[component]
pub fn Panel(title: String, children: Vec<Element>) -> Element {
    ui! {
        view(style = styles::Panel()) {
            text(style = styles::PanelTitle()) { title }
            children
        }
    }
}

/// A line of status text under a panel's controls.
#[component]
pub fn Status(line: String) -> Element {
    ui! {
        text(style = styles::Status()) { line }
    }
}

const LOGO: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="60" viewBox="0 0 120 60">
  <defs><linearGradient id="g" x1="0" x2="1"><stop offset="0" stop-color="#6366f1"/><stop offset="1" stop-color="#ec4899"/></linearGradient></defs>
  <rect x="2" y="2" width="116" height="56" rx="14" fill="url(#g)"/>
  <circle cx="30" cy="30" r="14" fill="#fff" fill-opacity="0.9"/>
  <path d="M54 22 h50 M54 30 h38 M54 38 h44" stroke="#fff" stroke-width="5" stroke-linecap="round"/>
</svg>"##;

/// `svg` — a vector logo from markup, plus its reported intrinsic size.
#[component]
pub fn SvgPanel() -> Element {
    let loaded = signal("parsing…".to_string());
    let on_load = Rc::new(move || loaded.set("on_load fired".to_string())) as Rc<dyn Fn()>;
    ui! {
        Panel(title = "svg".to_string()) {
            view(style = styles::SvgBox()) {
                { svg::Svg(svg::SvgProps {
                    markup: svg::markup(LOGO),
                    on_load: Some(on_load),
                    on_error: None,
                })
                .into_element() }
            }
            Status(line = rx!(loaded.get()))
        }
    }
}

/// `clipboard` — copy a string, paste it back.
#[component]
pub fn ClipboardPanel() -> Element {
    let status = signal("Copy, then paste anything (here or from another app).".to_string());
    let copy = Rc::new(move || {
        spawn_async(async move {
            let line = match clipboard::set_text("Copied from the SDK gallery ✓").await {
                Ok(()) => "Copied. Paste it anywhere, or press Paste.".to_string(),
                Err(e) => format!("Copy failed: {e}"),
            };
            status.set(line);
        });
    }) as Rc<dyn Fn()>;
    let paste = Rc::new(move || {
        spawn_async(async move {
            let line = match clipboard::text().await {
                Ok(Some(t)) => format!("Pasted: {t}"),
                Ok(None) => "The clipboard holds no text.".to_string(),
                Err(e) => format!("Paste failed: {e}"),
            };
            status.set(line);
        });
    }) as Rc<dyn Fn()>;
    ui! {
        Panel(title = "clipboard".to_string()) {
            view(style = styles::Row()) {
                Button(label = "Copy".to_string(), tone = tone::Primary, variant = variant::Filled, on_click = copy)
                Button(label = "Paste".to_string(), tone = tone::Neutral, variant = variant::Soft, on_click = paste)
            }
            Status(line = rx!(status.get()))
        }
    }
}

fn describe(net: connectivity::Connectivity) -> String {
    if net.online {
        format!("Online via {:?}", net.transport)
    } else {
        "Offline".to_string()
    }
}

/// `connectivity` — the current state, kept live by `watch`.
#[component]
pub fn ConnectivityPanel() -> Element {
    let status = signal(describe(connectivity::current()));
    let changes = signal(0u32);
    let sub = connectivity::watch(move |net| {
        status.set(describe(net));
        changes.set(changes.get() + 1);
    });
    // The scope owns the subscription: it unregisters when the panel goes.
    effect!({
        let _keep = &sub;
    });
    ui! {
        Panel(title = "connectivity".to_string()) {
            Status(line = rx!(status.get()))
            Status(line = rx!(format!("Changes seen: {} (toggle the network to test)", changes.get())))
        }
    }
}

/// `notifications` — post now, schedule, cancel.
#[component]
pub fn NotificationsPanel() -> Element {
    let status = signal("Not posted yet.".to_string());
    let post = |status: Signal<String>, after: Option<u64>| {
        Rc::new(move || {
            spawn_async(async move {
                let grant = notifications::authorize().await;
                if !grant.is_usable() {
                    status.set(format!("Not allowed ({grant:?}) — turn notifications on in Settings."));
                    return;
                }
                let n = notifications::Notification::new("SDK gallery", "Hello from the notifications SDK")
                    .subtitle(if after.is_some() { "Scheduled" } else { "Posted now" })
                    .id("gallery");
                let result = match after {
                    None => notifications::notify(n).await,
                    Some(s) => notifications::schedule(n, Duration::from_secs(s)).await,
                };
                status.set(match (result, after) {
                    (Ok(id), None) => format!("Posted as {id}."),
                    (Ok(id), Some(s)) => format!("Scheduled as {id}, in {s} s."),
                    (Err(e), _) => format!("Failed: {e}"),
                });
            });
        }) as Rc<dyn Fn()>
    };
    let cancel = Rc::new(move || {
        spawn_async(async move {
            notifications::cancel(&notifications::NotificationId::from("gallery")).await;
            status.set("Cancelled.".to_string());
        });
    }) as Rc<dyn Fn()>;
    ui! {
        Panel(title = "notifications".to_string()) {
            view(style = styles::Row()) {
                Button(label = "Notify now".to_string(), tone = tone::Primary, variant = variant::Filled, on_click = post(status, None))
                Button(label = "In 10 s".to_string(), tone = tone::Neutral, variant = variant::Soft, on_click = post(status, Some(10)))
                Button(label = "Cancel".to_string(), tone = tone::Neutral, variant = variant::Soft, on_click = cancel)
            }
            Status(line = rx!(status.get()))
        }
    }
}

/// An inline page that talks back: its button posts to the app, and it
/// shows what the app posts to it.
const PAGE: &str = r#"<!doctype html><html><body style="font-family:system-ui;margin:16px">
<h3 style="margin:0 0 8px">Inline page</h3>
<button onclick="window.postMessage('hello from the page')">Post to the app</button>
<p id="got">Nothing from the app yet.</p>
<script>window.addEventListener('message', function (e) {
  if (typeof e.data === 'string' && e.data !== 'hello from the page') document.getElementById('got').textContent = 'From the app: ' + e.data;
});</script></body></html>"#;

/// `webview` — a real page, an inline page, and messages both ways.
#[component]
pub fn WebViewPanel() -> Element {
    let source = signal("https://example.com".to_string());
    let status = signal("Loading…".to_string());
    let handle: runtime_core::Ref<webview::WebViewHandle> = runtime_core::Ref::new();
    let show_site = Rc::new(move || source.set("https://example.com".to_string())) as Rc<dyn Fn()>;
    let show_inline = Rc::new(move || source.set(PAGE.to_string())) as Rc<dyn Fn()>;
    let h = handle.clone();
    let send = Rc::new(move || {
        h.with(|w| w.post_message("hello from the app"));
    }) as Rc<dyn Fn()>;
    let h = handle.clone();
    let reload = Rc::new(move || {
        h.with(|w| w.reload());
    }) as Rc<dyn Fn()>;
    let on_load = Rc::new(move || status.set("on_load fired.".to_string())) as Rc<dyn Fn()>;
    let on_error = Rc::new(move || status.set("on_error fired.".to_string())) as Rc<dyn Fn()>;
    let on_message = Rc::new(move |m: String| status.set(format!("on_message: {m}"))) as Rc<dyn Fn(String)>;
    ui! {
        Panel(title = "webview".to_string()) {
            view(style = styles::Row()) {
                Button(label = "example.com".to_string(), tone = tone::Neutral, variant = variant::Soft, on_click = show_site)
                Button(label = "Inline page".to_string(), tone = tone::Neutral, variant = variant::Soft, on_click = show_inline)
                Button(label = "Post to page".to_string(), tone = tone::Primary, variant = variant::Filled, on_click = send)
                Button(label = "Reload".to_string(), tone = tone::Neutral, variant = variant::Soft, on_click = reload)
            }
            view(style = styles::WebBox()) {
                { webview::WebView(webview::WebViewProps {
                    url: webview::url(move || source.get()),
                    on_message: Some(on_message),
                    on_load: Some(on_load),
                    on_error: Some(on_error),
                })
                .with_style(styles::WebFill())
                .bind(handle.clone())
                .into_element() }
            }
            Status(line = rx!(status.get()))
        }
    }
}

/// A video source that shows whatever the signal holds (nothing while empty).
struct Live(Signal<Option<MediaContentSource>>);

/// What [`Live`] can show: a live stream or a file.
#[derive(Clone, PartialEq)]
pub enum MediaContentSource {
    /// A live capture.
    Stream(camera::MediaStream),
    /// A file on disk.
    File(String),
}

impl video::VideoSource for Live {
    fn resolve(&self) -> video::MediaContent {
        match self.0.get() {
            Some(MediaContentSource::Stream(s)) => video::MediaContent::Stream(s),
            Some(MediaContentSource::File(p)) => video::MediaContent::Url(p),
            None => video::MediaContent::None,
        }
    }
}

/// `camera` — open the camera and preview it.
#[component]
pub fn CameraPanel() -> Element {
    let status = signal("Camera closed.".to_string());
    let feed: Signal<Option<MediaContentSource>> = signal(None);
    let open = Rc::new(move || {
        status.set("Opening…".to_string());
        spawn_async(async move {
            match camera::Camera::new().open(camera::CameraConfig::new().with_resolution(640, 480)).await {
                Ok(stream) => {
                    feed.set(Some(MediaContentSource::Stream(stream)));
                    status.set("Camera open.".to_string());
                }
                Err(e) => status.set(format!("Failed: {e}")),
            }
        });
    }) as Rc<dyn Fn()>;
    let close = Rc::new(move || {
        feed.set(None);
        status.set("Camera closed.".to_string());
    }) as Rc<dyn Fn()>;
    ui! {
        Panel(title = "camera".to_string()) {
            view(style = styles::Row()) {
                Button(label = "Open camera".to_string(), tone = tone::Primary, variant = variant::Filled, on_click = open)
                Button(label = "Close".to_string(), tone = tone::Neutral, variant = variant::Soft, on_click = close)
            }
            view(style = styles::VideoBox()) {
                { video::Video(video::VideoProps { source: Box::new(Live(feed)), ..Default::default() })
                    .with_style(styles::WebFill())
                    .into_element() }
            }
            Status(line = rx!(status.get()))
        }
    }
}

/// `screen-recorder` + `media-writer` — record this window to an MP4, then
/// play the file back.
#[component]
pub fn RecordingPanel() -> Element {
    let status = signal("Not recording.".to_string());
    let recording = signal(false);
    let clip: Signal<Option<MediaContentSource>> = signal(None);
    let active: Rc<std::cell::RefCell<Option<(camera::MediaStream, media_writer::Recording)>>> = Rc::default();
    let a = active.clone();
    let takes = Rc::new(std::cell::Cell::new(0u32));
    let start = Rc::new(move || {
        if a.borrow().is_some() {
            return; // already recording
        }
        let a = a.clone();
        // Let go of the last recording (the player holds the file open) and
        // write each take to its own file.
        clip.set(None);
        takes.set(takes.get() + 1);
        let name = format!("gallery-recording-{}-{}.mp4", std::process::id(), takes.get());
        status.set("Starting…".to_string());
        spawn_async(async move {
            let stream = match screen_recorder::ScreenRecorder::new().start(screen_recorder::RecordingConfig::new()).await {
                Ok(s) => s,
                Err(e) => return status.set(format!("Capture failed: {e}")),
            };
            let store = match files::app_files("sdk-gallery") {
                Ok(s) => s,
                Err(e) => return status.set(format!("No file store: {e}")),
            };
            match media_writer::MediaWriter::new()
                .record(media_writer::MediaInputs::video(&stream), media_writer::RecordConfig::new(store, name))
                .await
            {
                Ok(rec) => {
                    *a.borrow_mut() = Some((stream, rec));
                    recording.set(true);
                    status.set("Recording this window — press Stop. The red REC badge is private: it is not in the clip.".to_string());
                }
                Err(e) => status.set(format!("Writer failed: {e}")),
            }
        });
    }) as Rc<dyn Fn()>;
    let a = active.clone();
    let stop = Rc::new(move || {
        let Some((_stream, rec)) = a.borrow_mut().take() else { return };
        recording.set(false);
        status.set("Finishing…".to_string());
        spawn_async(async move {
            match rec.stop().await {
                Ok(rel) => {
                    let path = files::app_files("sdk-gallery").ok().and_then(|s| s.local_path(&rel));
                    match path {
                        Some(p) => {
                            status.set(format!("Saved {}", p.display()));
                            clip.set(Some(MediaContentSource::File(p.to_string_lossy().into_owned())));
                        }
                        None => status.set(format!("Saved {rel}")),
                    }
                }
                Err(e) => status.set(format!("Finishing failed: {e}")),
            }
        });
    }) as Rc<dyn Fn()>;
    ui! {
        Panel(title = "screen recording".to_string()) {
            view(style = styles::Row()) {
                Button(label = "Record".to_string(), tone = tone::Primary, variant = variant::Filled, on_click = start)
                Button(label = "Stop".to_string(), tone = tone::Neutral, variant = variant::Soft, on_click = stop)
            }
            view(style = styles::VideoBox()) {
                { video::Video(video::VideoProps { source: Box::new(Live(clip)), controls: true, ..Default::default() })
                    .with_style(styles::WebFill())
                    .into_element() }
            }
            Status(line = rx!(status.get()))
            // A private layer: on screen while recording, left out of the
            // recording (its place in the tree doesn't matter — it shows in
            // its own window over the app).
            if recording.get() {
                { screen_recorder::PrivateLayer(vec![ui! {
                    view(style = styles::RecBadge()) {
                        text(style = styles::RecBadgeText()) { "● REC (private)".to_string() }
                    }
                }])
                .into_element() }
            }
        }
    }
}

/// `share` — the platform share sheet with text and a link.
#[component]
pub fn SharePanel() -> Element {
    let status = signal("Not shared yet.".to_string());
    let go = Rc::new(move || {
        spawn_async(async move {
            let content = share::ShareContent::text("Shared from the idealyst SDK gallery")
                .with_url("https://example.com")
                .with_title("SDK gallery");
            status.set(match share::share(content).await {
                Ok(outcome) => format!("{outcome:?}"),
                Err(e) => format!("Failed: {e}"),
            });
        });
    }) as Rc<dyn Fn()>;
    ui! {
        Panel(title = "share".to_string()) {
            view(style = styles::Row()) {
                Button(label = "Share".to_string(), tone = tone::Primary, variant = variant::Filled, on_click = go)
            }
            Status(line = rx!(status.get()))
        }
    }
}

/// `location` — one fix, and a live watch.
#[component]
pub fn LocationPanel() -> Element {
    let status = signal("No fix requested.".to_string());
    // Numbered so a repeat request that lands on the same position still
    // reads as a new answer.
    let fixes = Rc::new(std::cell::Cell::new(0u32));
    let locate = Rc::new(move || {
        status.set("Locating…".to_string());
        let fixes = fixes.clone();
        spawn_async(async move {
            status.set(match location::current().await {
                Ok(p) => {
                    fixes.set(fixes.get() + 1);
                    format!("Fix #{}: {:.5}, {:.5} (±{:.0} m)", fixes.get(), p.latitude, p.longitude, p.accuracy_m)
                }
                Err(e) => format!("No fix: {e}"),
            });
        });
    }) as Rc<dyn Fn()>;
    ui! {
        Panel(title = "location".to_string()) {
            view(style = styles::Row()) {
                Button(label = "Get location".to_string(), tone = tone::Primary, variant = variant::Filled, on_click = locate)
            }
            Status(line = rx!(status.get()))
        }
    }
}

/// A public MP3 (SoundHelix's example track).
const SONG: &str = "https://www.soundhelix.com/examples/mp3/SoundHelix-Song-1.mp3";

/// `audio` — load a streamed MP3, play / pause / stop it.
#[component]
pub fn AudioPanel() -> Element {
    let status = signal("Not loaded.".to_string());
    // The running playback; dropping it stops the sound.
    let playback: Rc<std::cell::RefCell<Option<audio::Playback>>> = Rc::default();
    let pb = playback.clone();
    let play = Rc::new(move || {
        let pb = pb.clone();
        status.set("Loading…".to_string());
        spawn_async(async move {
            match audio::load(audio::AudioSource::url(SONG)).await {
                Ok(sound) => {
                    *pb.borrow_mut() = Some(sound.play());
                    status.set("Playing.".to_string());
                }
                Err(e) => status.set(format!("Failed: {e}")),
            }
        });
    }) as Rc<dyn Fn()>;
    let pb = playback.clone();
    let pause = Rc::new(move || {
        if let Some(p) = pb.borrow().as_ref() {
            if p.is_playing() {
                p.pause();
                status.set("Paused.".to_string());
            } else {
                p.resume();
                status.set("Playing.".to_string());
            }
        }
    }) as Rc<dyn Fn()>;
    let pb = playback.clone();
    let stop = Rc::new(move || {
        pb.borrow_mut().take();
        status.set("Stopped.".to_string());
    }) as Rc<dyn Fn()>;
    ui! {
        Panel(title = "audio".to_string()) {
            view(style = styles::Row()) {
                Button(label = "Load + play".to_string(), tone = tone::Primary, variant = variant::Filled, on_click = play)
                Button(label = "Pause / resume".to_string(), tone = tone::Neutral, variant = variant::Soft, on_click = pause)
                Button(label = "Stop".to_string(), tone = tone::Neutral, variant = variant::Soft, on_click = stop)
            }
            Status(line = rx!(status.get()))
        }
    }
}

/// A public MP4 (the Sintel trailer, W3C's media test file).
const CLIP: &str = "https://media.w3.org/2010/05/sintel/trailer.mp4";

/// `video` — a URL with native-style controls, plus the imperative ops.
#[component]
pub fn VideoPanel() -> Element {
    let handle: runtime_core::Ref<video::VideoHandle> = runtime_core::Ref::new();
    let status = signal("Press play on the video, or use the buttons.".to_string());
    let h = handle.clone();
    let from_ten = Rc::new(move || {
        h.with(|v| {
            v.seek(10.0);
            v.play();
        });
    }) as Rc<dyn Fn()>;
    let h = handle.clone();
    let pause = Rc::new(move || {
        h.with(|v| v.pause());
    }) as Rc<dyn Fn()>;
    let h = handle.clone();
    let where_at = Rc::new(move || {
        let line = h
            .with(|v| format!("At {:.1} s of {:.1} s.", v.position(), v.duration()))
            .unwrap_or_default();
        status.set(line);
    }) as Rc<dyn Fn()>;
    ui! {
        Panel(title = "video".to_string()) {
            view(style = styles::Row()) {
                Button(label = "Play from 0:10".to_string(), tone = tone::Primary, variant = variant::Filled, on_click = from_ten)
                Button(label = "Pause".to_string(), tone = tone::Neutral, variant = variant::Soft, on_click = pause)
                Button(label = "Position".to_string(), tone = tone::Neutral, variant = variant::Soft, on_click = where_at)
            }
            view(style = styles::VideoBox()) {
                { video::Video(video::VideoProps {
                    source: video::url(CLIP),
                    controls: true,
                    ..Default::default()
                })
                .with_style(styles::WebFill())
                .bind(handle.clone())
                .into_element() }
            }
            Status(line = rx!(status.get()))
        }
    }
}

mod styles {
    use idea_ui::IdeaThemeRef;
    use runtime_core::{stylesheet, AlignItems, FlexDirection, FlexWrap, Length, Position};

    stylesheet! {
        pub Scroll<IdeaThemeRef> {
            base(t) {
                width: Length::pct(100.0),
                height: Length::pct(100.0),
                background: t.color.background(),
            }
        }
    }

    stylesheet! {
        pub Root<IdeaThemeRef> {
            base(t) {
                flex_direction: FlexDirection::Column,
                gap: t.spacing.md(),
                padding: t.spacing.lg(),
            }
        }
    }

    stylesheet! {
        pub Title<IdeaThemeRef> {
            base(t) {
                font_size: Length::Px(22.0),
                color: t.color.text(),
            }
        }
    }

    stylesheet! {
        pub Subtitle<IdeaThemeRef> {
            base(t) {
                font_size: Length::Px(13.0),
                color: t.color.text_muted(),
            }
        }
    }

    stylesheet! {
        pub Panel<IdeaThemeRef> {
            base(t) {
                flex_direction: FlexDirection::Column,
                gap: t.spacing.sm(),
                padding: t.spacing.md(),
                background: t.color.surface(),
                border_radius: t.radius.md(),
            }
        }
    }

    stylesheet! {
        pub PanelTitle<IdeaThemeRef> {
            base(t) {
                font_size: Length::Px(15.0),
                color: t.color.text(),
            }
        }
    }

    stylesheet! {
        pub Status<IdeaThemeRef> {
            base(t) {
                font_size: Length::Px(13.0),
                color: t.color.text_muted(),
            }
        }
    }

    stylesheet! {
        pub RecBadge<IdeaThemeRef> {
            base(t) {
                position: Position::Absolute,
                top: Length::Px(12.0),
                right: Length::Px(12.0),
                padding: t.spacing.xs(),
                background: runtime_core::Color::from("#dc2626"),
                border_radius: t.radius.md(),
            }
        }
    }

    stylesheet! {
        pub RecBadgeText<IdeaThemeRef> {
            base(_t) {
                font_size: Length::Px(13.0),
                color: runtime_core::Color::from("#ffffff"),
            }
        }
    }

    stylesheet! {
        pub Row<IdeaThemeRef> {
            base(t) {
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                align_items: AlignItems::Center,
                gap: t.spacing.xs(),
            }
        }
    }

    stylesheet! {
        pub SvgBox<IdeaThemeRef> {
            base(_t) {
                width: Length::Px(120.0),
                height: Length::Px(60.0),
            }
        }
    }

    stylesheet! {
        pub WebBox<IdeaThemeRef> {
            base(t) {
                height: Length::Px(320.0),
                width: Length::pct(100.0),
                border_radius: t.radius.sm(),
            }
        }
    }

    stylesheet! {
        pub VideoBox<IdeaThemeRef> {
            base(t) {
                height: Length::Px(300.0),
                width: Length::pct(100.0),
                background: t.color.text(),
                border_radius: t.radius.sm(),
            }
        }
    }

    stylesheet! {
        pub WebFill<IdeaThemeRef> {
            base(_t) {
                width: Length::pct(100.0),
                height: Length::pct(100.0),
            }
        }
    }
}
