//! Sweep: mount the docs app and visit EVERY sidebar route, asserting
//! each page really renders on the GTK backend.
#![cfg(target_os = "linux")]
use backend_linux::{gtk4, newcore, LinuxBackend};
use gtk4::prelude::*;
use runtime_vocabulary::caps::IntrospectionOps;
use std::cell::RefCell;
use std::rc::Rc;

type Backend = Rc<RefCell<LinuxBackend>>;

fn pump(backend: &Backend, ms: u64) {
    let ctx = gtk4::glib::MainContext::default();
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_millis(ms) {
        if let Ok(b) = backend.try_borrow() { b.pump(); }
        ctx.iteration(false);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// Iterative on purpose: a recursive walk cannot tell "very deep tree"
/// from "cycle in the widget graph" — it just overflows the stack. This
/// tracks visited pointers and reports depth so a cycle is diagnosable.
fn walk(w: &gtk4::Widget, out: &mut Vec<gtk4::Widget>) {
    use std::collections::HashSet;
    let mut seen: HashSet<usize> = HashSet::new();
    let mut stack = vec![(w.clone(), 0usize)];
    let mut max_depth = 0usize;
    while let Some((cur, depth)) = stack.pop() {
        let key = cur.as_ptr() as usize;
        if !seen.insert(key) {
            eprintln!("    CYCLE: revisited {} at depth {depth}", cur.type_().name());
            continue;
        }
        max_depth = max_depth.max(depth);
        out.push(cur.clone());
        if out.len() > 200_000 {
            eprintln!("    RUNAWAY: widget walk exceeded 200k nodes");
            return;
        }
        let mut c = cur.first_child();
        while let Some(ch) = c {
            stack.push((ch.clone(), depth + 1));
            c = ch.next_sibling();
        }
    }
    if max_depth > 200 {
        eprintln!("    DEEP: widget tree depth {max_depth}");
    }
}

fn click_gesture(w: &gtk4::Widget) -> Option<gtk4::GestureClick> {
    w.observe_controllers().into_iter()
        .find_map(|c| c.ok().and_then(|c| c.downcast::<gtk4::GestureClick>().ok()))
}

fn texts(w: &gtk4::Widget) -> Vec<String> {
    let mut all = Vec::new();
    walk(w, &mut all);
    all.iter().filter_map(|w| w.downcast_ref::<gtk4::Label>())
        .map(|l| l.text().to_string()).filter(|t| !t.trim().is_empty()).collect()
}

/// Sidebar rows: (label, widget). Identified by the row geometry the
/// sidebar gives every entry (x = 16, width = 220) so page content of the
/// same name can't be mistaken for a nav row.
fn sidebar_rows(root: &gtk4::Widget) -> Vec<(String, gtk4::Widget)> {
    let mut all = Vec::new();
    walk(root, &mut all);
    let mut out = Vec::new();
    for w in &all {
        if click_gesture(w).is_none() { continue; }
        let Some(b) = w.compute_bounds(root) else { continue };
        if (b.width() - 220.0).abs() > 1.0 || (b.x() - 16.0).abs() > 1.0 { continue; }
        let t = texts(w);
        if t.len() != 1 { continue; }
        if t[0] == "Search components…" { continue; }
        if out.iter().any(|(n, _): &(String, gtk4::Widget)| n == &t[0]) { continue; }
        out.push((t[0].clone(), w.clone()));
    }
    // Sort by vertical position so the sweep runs in catalog order and its
    // output is readable; `walk`'s child order is not a stable API.
    out.sort_by(|a, b| {
        let y = |w: &gtk4::Widget| w.compute_bounds(root).map(|b| b.y()).unwrap_or(f32::MAX);
        y(&a.1).partial_cmp(&y(&b.1)).unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

#[test]
fn every_docs_page_renders_on_gtk() {
    if gtk4::init().is_err() { eprintln!("SKIP: no display"); return; }

    // Install the scheduler the real host installs. Without it,
    // `after_ms` runs SYNCHRONOUSLY on native, so any self-rescheduling
    // animation (`Progress(mode = Simulated)`, `Skeleton`'s shimmer)
    // recurses into itself and overflows the stack — the test would be
    // exercising a runtime configuration production never uses.
    host_gtk::install_scheduler();

    // Capture GLib/GTK log output so a page that provokes a GTK critical
    // is attributed to that page instead of vanishing into stderr.
    let logs: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    {
        let logs = logs.clone();
        gtk4::glib::log_set_default_handler(move |domain, level, msg| {
            use gtk4::glib::LogLevel;
            if matches!(level, LogLevel::Critical | LogLevel::Warning | LogLevel::Error) {
                logs.lock().unwrap().push(format!("[{}] {}", domain.unwrap_or("?"), msg));
            }
        });
    }

    let window = gtk4::Window::new();
    window.set_default_size(1280, 860);
    runtime_shared::set_viewport_size(runtime_shared::ViewportSize { width: 1280.0, height: 860.0 });
    let backend: Backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone().upcast())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));
    let app = newcore::start(backend.clone(), idea_ui_docs::register_scene_extensions, idea_ui_docs::app);
    window.present();
    pump(&backend, 2000);
    if !window.is_mapped() { eprintln!("SKIP: window never mapped"); return; }
    let root = window.child().expect("realized root");

    let rows = sidebar_rows(&root);
    eprintln!("sidebar routes discovered: {}", rows.len());
    let mut failures: Vec<String> = Vec::new();

    let mut seen_content: std::collections::HashMap<String, String> = Default::default();
    for (idx, (name, row)) in rows.iter().enumerate() {
        logs.lock().unwrap().clear();
        let g = click_gesture(row).expect("row gesture");
        let b = row.compute_bounds(&root).expect("row bounds");
        let (cx, cy) = ((b.x() + b.width()/2.0) as f64, (b.y() + b.height()/2.0) as f64);
        g.emit_by_name::<()>("pressed", &[&1i32, &cx, &cy]);
        g.emit_by_name::<()>("released", &[&1i32, &cx, &cy]);
        pump(&backend, 700);

        let mut all = Vec::new();
        walk(&root, &mut all);
        let placeholders: Vec<String> = all.iter()
            .filter(|w| w.has_css_class("idealyst-placeholder"))
            .filter_map(|w| w.downcast_ref::<gtk4::Label>().map(|l| l.text().to_string()))
            .collect();
        // Content pane = the region right of the 252px sidebar.
        let content_widgets = all.iter().filter(|w| {
            w.compute_bounds(&root).map(|b| b.x() >= 252.0).unwrap_or(false)
        }).count();
        let page_texts: Vec<String> = all.iter()
            .filter(|w| w.compute_bounds(&root).map(|b| b.x() >= 252.0).unwrap_or(false))
            .filter_map(|w| w.downcast_ref::<gtk4::Label>())
            .map(|l| l.text().to_string()).filter(|t| !t.trim().is_empty()).collect();
        let gtk_logs = logs.lock().unwrap().clone();

        // Every text leaf must carry Pango attributes. A GtkLabel without
        // them renders in the user's SYSTEM GTK theme colour, so text whose
        // author style sets no colour was invisible on a dark desktop (the
        // Calendar page's day numbers) and fine on a light one — the same
        // app looking different on two Linux machines. `create_text` seeds
        // the framework default paint; this is the app-level net.
        let bare: Vec<String> = all.iter()
            .filter_map(|w| w.downcast_ref::<gtk4::Label>())
            .filter(|l| l.attributes().is_none() && !l.text().trim().is_empty())
            .map(|l| l.text().chars().take(30).collect::<String>())
            .collect();
        if !bare.is_empty() {
            failures.push(format!(
                "{name}: {} label(s) carry no Pango attributes, so they paint in \
                 the system theme colour: {:?}",
                bare.len(),
                &bare[..bare.len().min(5)]
            ));
        }

        let title_shown = page_texts.iter().any(|t| t == name);
        let content_key = {
            let mut joined = page_texts.join("\u{1}");
            joined.truncate(4096);
            joined
        };
        eprintln!(
            "{name:<18} widgets={content_widgets:<4} labels={:<4} title={} placeholders={} logs={}",
            page_texts.len(), title_shown, placeholders.len(), gtk_logs.len()
        );
        if !placeholders.is_empty() {
            eprintln!("    PLACEHOLDERS: {placeholders:?}");
            failures.push(format!("{name}: placeholders {placeholders:?}"));
        }
        // The DEFAULT route (first sidebar row) is a bespoke hero page: its
        // H1 is the product name, not the nav label, so it has no
        // `page_frame_content` title to match. Every catalog page does.
        if !title_shown && idx != 0 {
            failures.push(format!("{name}: page title never rendered in the content pane"));
        }
        if let Some(prev) = seen_content.get(&content_key) {
            failures.push(format!(
                "{name}: content pane is identical to {prev} — the route did not \
                 actually swap (a dead nav link renders the previous screen)"
            ));
        } else {
            seen_content.insert(content_key, name.clone());
        }
        if content_widgets < 20 {
            failures.push(format!("{name}: only {content_widgets} widgets in the content pane"));
        }
        for l in &gtk_logs {
            eprintln!("    LOG: {l}");
        }

        // With `IDEALYST_TEST_SHOT_DIR` set, dump each page through the
        // backend's own `screenshot` verb. The sweep then doubles as the
        // capture pass for eyeballing / visual diffing a Linux build,
        // which is otherwise the one thing automation could not see.
        if let Some(dir) = std::env::var_os("IDEALYST_TEST_SHOT_DIR") {
            let out: Rc<RefCell<Option<Result<runtime_shared::Screenshot, String>>>> =
                Rc::new(RefCell::new(None));
            {
                let b = backend.borrow();
                let sink = out.clone();
                b.capture_screenshot(Box::new(move |r| *sink.borrow_mut() = Some(r)));
            }
            // Frame-deferred on GTK — pump until the callback lands.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while out.borrow().is_none() && std::time::Instant::now() < deadline {
                pump(&backend, 50);
            }
            let captured = out.borrow_mut().take();
            match captured {
                Some(Ok(shot)) => {
                    let safe: String = name.chars()
                        .map(|c| if c.is_alphanumeric() { c } else { '_' })
                        .collect();
                    let path = std::path::Path::new(&dir).join(format!("page_{idx:02}_{safe}.png"));
                    if let Err(e) = std::fs::write(&path, &shot.png) {
                        eprintln!("    SHOT write failed: {e}");
                    }
                }
                Some(Err(e)) => failures.push(format!("{name}: screenshot failed: {e}")),
                None => failures.push(format!("{name}: screenshot verb never called back")),
            }
        }
    }

    app.stop();
    assert!(failures.is_empty(), "docs pages failed on GTK:\n  {}", failures.join("\n  "));
}
