//! Editor UI for the fiddle (v2 — multi-file project + syntax
//! highlighting).
//!
//! Three-column layout, top to bottom:
//!
//! - **File tree** (left, ~220 px). Recursive `view` over the
//!   project's path → contents map; folders expand/collapse;
//!   clicking a file sets the active path and loads its contents
//!   into the editor buffer.
//! - **Editor** (center, flex-grow). A `text_area` bound to the
//!   active file's contents, with a `code_block` overlay behind
//!   it carrying the Rust-tokenized colored runs. The textarea is
//!   `color: transparent; background: transparent`, so the
//!   colored layer shows through; the caret stays visible via
//!   the user-agent default `caret-color`. Mode toggle + Run +
//!   status pane sit below.
//! - **Preview** (right, 360×720). `WebView` URL driven by a
//!   signal pointing at `/compiled/<hash>/`.
//!
//! Styles are declared via one `stylesheet!` block (each rule emits
//! a thread-local cached `*_style()` function the framework can
//! diff cheaply). Call sites pass the style by name to
//! `.with_style(...)`.

mod fetch;
mod highlight;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use runtime_core::primitives::text_area::{text_area, TextAreaHandle};
use runtime_core::stylesheet;
use runtime_core::{
    button, signal, switch, text, text_fmt, ui, AlignItems, Color, FlexDirection,
    FontWeight, JustifyContent, KeyEvent, KeyOutcome, Length, Overflow, Position, Element, Ref,
    Signal,
};
use codeblock::code_block;
use idea_ui::{install_idea_theme, light_theme};

// The `stylesheet!` macro takes a `<Theme>` generic for syntactic
// reasons; the theme type isn't actually referenced at runtime
// (closures take `&VariantSet`, not a theme). Use any in-scope
// type — `IdeaThemeRef` is the idea-ui convention.
#[allow(unused_imports)]
use idea_ui::theme::IdeaThemeRef;
use wasm_bindgen::prelude::*;

use crate::fetch::Mode;
use crate::highlight::highlight_rust;

#[global_allocator]
static ALLOCATOR: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
    unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) };

thread_local! {
    static OWNER: RefCell<Option<runtime_core::Owner>> = const { RefCell::new(None) };
}

// =============================================================================
// Stylesheets — one block, one `*_style()` per row. The framework
// caches each `Rc<StyleSheet>` in a thread-local, so call sites pay
// nothing for repeated `<name>_style()` references.
// =============================================================================

// Editor font + line metrics. Hard-coded values shared between the
// textarea and the colored overlay — a single px difference makes
// glyphs drift apart row by row, so anchor them in one place.
//
// `line-height` is emitted by the web style layer as `<n>px`, so we
// pass the resolved pixel value rather than the conventional
// unitless multiplier. 20 px lands at ~1.54× the 13 px font.
const EDITOR_FONT_FAMILY: &str = "ui-monospace, SFMono-Regular, Menlo, monospace";
const EDITOR_FONT_SIZE: f32 = 13.0;
const EDITOR_LINE_HEIGHT: f32 = 20.0;
const EDITOR_PADDING: f32 = 12.0;

stylesheet! {
    pub Row<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Stretch,
            gap: Length::Px(12.0),
            padding: Length::Px(16.0),
            height: Length::Percent(100.0),
            background: Color("#f5f5f7".into()),
        }
    }
}

stylesheet! {
    pub TreePanel<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            width: Length::Px(220.0),
            background: Color("#ffffff".into()),
            padding: Length::Px(12.0),
            border_radius: Length::Px(8.0),
        }
    }
}

stylesheet! {
    pub TreeHeader<IdeaThemeRef> {
        base(_t) {
            font_size: Length::Px(13.0),
            color: Color("#57606a".into()),
            padding_bottom: Length::Px(8.0),
        }
    }
}

stylesheet! {
    pub TreeList<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
        }
    }
}

stylesheet! {
    pub TreeHint<IdeaThemeRef> {
        base(_t) {
            margin_top: Length::Px(12.0),
            padding_top: Length::Px(8.0),
            font_size: Length::Px(11.0),
            line_height: 16.0,
            color: Color("#57606a".into()),
        }
    }
}

stylesheet! {
    pub Center<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Stretch,
            flex_grow: 1.0,
            gap: Length::Px(12.0),
        }
    }
}

stylesheet! {
    pub EditorStack<IdeaThemeRef> {
        base(_t) {
            position: Position::Relative,
            flex_grow: 1.0,
            height: Length::Px(500.0),
            border_radius: Length::Px(8.0),
            overflow: Overflow::Hidden,
            background: Color("#ffffff".into()),
        }
    }
}

stylesheet! {
    pub Textarea<IdeaThemeRef> {
        base(_t) {
            position: Position::Absolute,
            top: Length::Px(0.0),
            right: Length::Px(0.0),
            bottom: Length::Px(0.0),
            left: Length::Px(0.0),
            padding: Length::Px(EDITOR_PADDING),
            font_family: EDITOR_FONT_FAMILY,
            font_size: Length::Px(EDITOR_FONT_SIZE),
            line_height: EDITOR_LINE_HEIGHT,
            // Textarea's own glyphs are invisible — the colored
            // overlay carries the visible text. The caret would also
            // vanish here (its default `caret-color: auto` follows
            // `color`); `caret_color` pins it to the editor fg hex
            // so the cursor stays visible. The framework's
            // `StyleRules::caret_color` field maps to CSS
            // `caret-color` on web, `tintColor` on UIKit, and
            // `setTextCursorDrawable` on Android (API 29+).
            color: Color("transparent".into()),
            caret_color: Color("#24292f".into()),
            background: Color("transparent".into()),
        }
    }
}

stylesheet! {
    pub CodeOverlay<IdeaThemeRef> {
        base(_t) {
            position: Position::Absolute,
            top: Length::Px(0.0),
            right: Length::Px(0.0),
            bottom: Length::Px(0.0),
            left: Length::Px(0.0),
            padding: Length::Px(EDITOR_PADDING),
            font_family: EDITOR_FONT_FAMILY,
            font_size: Length::Px(EDITOR_FONT_SIZE),
            line_height: EDITOR_LINE_HEIGHT,
            overflow: Overflow::Hidden,
            background: Color("#ffffff".into()),
        }
    }
}

stylesheet! {
    pub ModeRow<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            gap: Length::Px(8.0),
        }
    }
}

stylesheet! {
    pub StatusPane<IdeaThemeRef> {
        base(_t) {
            height: Length::Px(140.0),
            padding: Length::Px(8.0),
            background: Color("#0d1117".into()),
            color: Color("#c9d1d9".into()),
            font_family: EDITOR_FONT_FAMILY,
            font_size: Length::Px(12.0),
            border_radius: Length::Px(8.0),
        }
    }
}

// Simulator aspect (iPhone-portrait 390 × 844 logical, matches
// `variant-phone`). The iframe is sized to this ratio so neither
// mode (simulator canvas in sim mode, snippet DOM in web mode)
// shows internal scrollbars around the preview content. Pick a
// width that fits the editor column comfortably and derive the
// height; the simulator template's wrapper inside is 100% × 100%
// so the canvas matches whatever we hand it here.
const PREVIEW_WIDTH_PX: f32 = 360.0;
const PREVIEW_HEIGHT_PX: f32 = PREVIEW_WIDTH_PX * 844.0 / 390.0;

stylesheet! {
    pub Preview<IdeaThemeRef> {
        base(_t) {
            width: Length::Px(PREVIEW_WIDTH_PX),
            height: Length::Px(PREVIEW_HEIGHT_PX),
            background: Color("#ffffff".into()),
            border_radius: Length::Px(8.0),
            overflow: Overflow::Hidden,
        }
    }
}

stylesheet! {
    pub PreviewColumn<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
        }
    }
}

stylesheet! {
    pub ControlsCol<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: Length::Px(8.0),
        }
    }
}

stylesheet! {
    pub TreeRow<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::FlexStart,
            padding_top: Length::Px(4.0),
            padding_right: Length::Px(8.0),
            padding_bottom: Length::Px(4.0),
            padding_left: Length::Px(8.0),
            font_size: Length::Px(13.0),
            color: Color("#24292f".into()),
            background: Color("transparent".into()),
        }
        // `state` variant axis — `idle` for folder rows + inactive
        // files, `active` highlights the currently-edited file.
        variant state {
            #[default]
            idle(_t) {}
            active(_t) {
                background: Color("#dbe9ff".into()),
                color: Color("#0550ae".into()),
                font_weight: FontWeight::Medium,
            }
        }
        // `padding_left` is the per-row indent. Each tree depth
        // adds 12 px to the base 8 px gutter; passing it as an
        // override (continuous value) instead of a variant (finite
        // enum) is exactly what the macro's override mechanism is
        // for.
        override padding_left: Length
    }
}

// =============================================================================
// Starter project — two-file demo bundled via `include_str!` so the
// editor has something to render on first load.
// =============================================================================

const STARTER_LIB_RS: &str = include_str!("starter/lib.rs");
const STARTER_WIDGETS_RS: &str = include_str!("starter/widgets.rs");
const STARTER_ENTRY: &str = "lib.rs";

fn starter_files() -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert(STARTER_ENTRY.to_string(), STARTER_LIB_RS.to_string());
    m.insert("widgets.rs".to_string(), STARTER_WIDGETS_RS.to_string());
    m
}

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    backend_web::install_scheduler();
    backend_web::install_async_executor();
    backend_web::install_render_loop();
    install_idea_theme(light_theme());

    let backend = Rc::new(RefCell::new(backend_web::WebBackend::new("#app")));
    // Register third-party SDK primitives. The fiddle uses
    // `codeblock::code_block` for the syntax-highlight overlay
    // behind the textarea — see `editor_panel()`. `webview::register`
    // installs the `WebView` SDK so `preview_panel()`'s iframe shows
    // up; without it the framework's "external not supported"
    // placeholder renders instead.
    {
        let mut b = backend.borrow_mut();
        codeblock::register(&mut b);
        webview::register(&mut b);
    }
    let owner = runtime_core::render(backend, app());
    OWNER.with(|slot| *slot.borrow_mut() = Some(owner));
}

fn app() -> Element {
    // Root signals — owned at the app level so they survive every
    // re-render the framework triggers on signal updates.
    let files: Signal<BTreeMap<String, String>> = signal!(starter_files());
    let active: Signal<String> = signal!(STARTER_ENTRY.to_string());
    // Buffer holds the active file's contents. Initialized to the
    // starter entry's text; the tree's click handler refreshes it
    // when the user picks a different file. Kept separate from
    // `files` so per-keystroke updates don't churn the whole map.
    let buffer: Signal<String> = signal!(STARTER_LIB_RS.to_string());
    let iframe_url: Signal<String> = signal!("about:blank".to_string());
    let status: Signal<String> = signal!("Press Run to compile".to_string());
    let is_compiling: Signal<bool> = signal!(false);
    let mode_sim: Signal<bool> = signal!(true);
    let expanded: Signal<BTreeSet<String>> = signal!(BTreeSet::new());

    let tree = file_tree_panel(files, active, expanded, buffer);
    let editor = editor_panel(files, active, buffer);
    let controls = controls_panel(files, mode_sim, is_compiling, status, iframe_url);
    let preview = preview_panel(iframe_url);

    ui! {
        view(style = row_style()) {
            tree
            view(style = center_style()) {
                editor
                controls
            }
            view(style = preview_column_style()) {
                preview
            }
        }
    }
}

// =============================================================================
// File tree
// =============================================================================

fn file_tree_panel(
    files: Signal<BTreeMap<String, String>>,
    active: Signal<String>,
    expanded: Signal<BTreeSet<String>>,
    buffer: Signal<String>,
) -> Element {
    // The tree subtree rebuilds when any of three things change:
    // the set of file paths, the expanded folders, or the active
    // path (so the highlight on the active row stays current).
    // The dep tuple's `PartialEq` is what gates the rebuild.
    let dep_files = files;
    let dep_expanded = expanded;
    let dep_active = active;
    let build_active = active;
    let build_expanded = expanded;
    let build_files = files;
    let build_buffer = buffer;
    let body = switch(
        move || {
            let paths: Vec<String> = dep_files.get().keys().cloned().collect();
            (paths, dep_expanded.get(), dep_active.get())
        },
        move |_| {
            let project = build_files.get();
            let exp = build_expanded.get();
            let tree = build_tree(project.keys());
            render_tree_node(
                &tree,
                0,
                &exp,
                build_active,
                build_expanded,
                build_files,
                build_buffer,
            )
        },
    );

    ui! {
        view(style = tree_panel_style()) {
            text(style = tree_header_style()) { "Files" }
            scroll_view { body }
            text(style = tree_hint_style()) {
                "Tip: idea-ui components (Stack, Heading, Body, Card, …) \
                 are in scope alongside framework primitives. Reach for \
                 idea-ui when you want styled, opinionated shapes."
            }
        }
    }
}

struct TreeNode {
    name: String,
    full_path: String,
    is_dir: bool,
    children: Vec<TreeNode>,
}

fn build_tree<'a, I: Iterator<Item = &'a String>>(paths: I) -> TreeNode {
    let mut root = TreeNode {
        name: String::new(),
        full_path: String::new(),
        is_dir: true,
        children: Vec::new(),
    };
    for path in paths {
        insert_path(&mut root, path);
    }
    sort_tree(&mut root);
    root
}

fn insert_path(node: &mut TreeNode, path: &str) {
    let mut cur = node;
    let segments: Vec<&str> = path.split('/').collect();
    for (i, seg) in segments.iter().enumerate() {
        let is_last = i + 1 == segments.len();
        let full_path = if cur.full_path.is_empty() {
            (*seg).to_string()
        } else {
            format!("{}/{}", cur.full_path, seg)
        };
        let existing = cur.children.iter().position(|c| c.name == *seg);
        let idx = match existing {
            Some(i) => i,
            None => {
                cur.children.push(TreeNode {
                    name: (*seg).to_string(),
                    full_path,
                    is_dir: !is_last,
                    children: Vec::new(),
                });
                cur.children.len() - 1
            }
        };
        cur = &mut cur.children[idx];
    }
}

fn sort_tree(node: &mut TreeNode) {
    // Folders first, then files, alphabetical within each group —
    // matches VSCode's default ordering.
    node.children
        .sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        });
    for child in &mut node.children {
        sort_tree(child);
    }
}

fn render_tree_node(
    node: &TreeNode,
    depth: usize,
    expanded: &BTreeSet<String>,
    active: Signal<String>,
    expanded_signal: Signal<BTreeSet<String>>,
    files: Signal<BTreeMap<String, String>>,
    buffer: Signal<String>,
) -> Element {
    if node.full_path.is_empty() {
        // Virtual root — render its children flat.
        let kids: Vec<Element> = node
            .children
            .iter()
            .map(|c| {
                render_tree_node(
                    c,
                    depth,
                    expanded,
                    active,
                    expanded_signal,
                    files,
                    buffer,
                )
            })
            .collect();
        return ui! { view(style = tree_list_style()) { kids } };
    }
    if node.is_dir {
        let is_open = expanded.contains(&node.full_path);
        let chevron = if is_open { "▾ " } else { "▸ " };
        let name = node.name.clone();
        let path_for_click = node.full_path.clone();
        let header_style = TreeRow()
            .state(TreeRowState::Idle)
            .padding_left(row_indent(depth));
        let mut nodes: Vec<Element> = vec![ui! {
            button(
                label = text_fmt!("{}{}", chevron, name),
                on_click = move || {
                    let path = path_for_click.clone();
                    expanded_signal.update(|set| {
                        if !set.insert(path.clone()) {
                            set.remove(&path);
                        }
                    });
                },
                style = header_style,
            )
        }];
        if is_open {
            for child in &node.children {
                nodes.push(render_tree_node(
                    child,
                    depth + 1,
                    expanded,
                    active,
                    expanded_signal,
                    files,
                    buffer,
                ));
            }
        }
        return ui! { view(style = tree_list_style()) { nodes } };
    }
    // File leaf. The on_click handler does *both* writes
    // synchronously: set the active path, and load that file's
    // contents into the editor buffer. We can't lean on a
    // reactive Effect here because `app()` runs before the
    // framework sets up its render scope, so any `Effect::new(...)`
    // here would drop the moment its handle goes out of scope.
    let is_active = active.get() == node.full_path;
    let path_for_click = node.full_path.clone();
    let row_style = TreeRow()
        .state(if is_active {
            TreeRowState::Active
        } else {
            TreeRowState::Idle
        })
        .padding_left(row_indent(depth + 1));
    ui! {
        button(
            label = node.name.clone(),
            on_click = move || {
                let path = path_for_click.clone();
                let contents = files.get().get(&path).cloned().unwrap_or_default();
                active.set(path);
                buffer.set(contents);
            },
            style = row_style,
        )
    }
}

/// Tree-row indent: an 8 px gutter plus 12 px per nesting level.
/// Plugs into [`TreeRow`]'s `override padding_left` slot — kept as a
/// standalone helper so the magic numbers live in one place.
fn row_indent(depth: usize) -> Length {
    Length::Px(8.0 + (depth as f32) * 12.0)
}

// =============================================================================
// Editor (text_area + code_block overlay)
// =============================================================================

fn editor_panel(
    files: Signal<BTreeMap<String, String>>,
    active: Signal<String>,
    buffer: Signal<String>,
) -> Element {
    // Textarea -> buffer + files: every keystroke updates BOTH so
    // the highlight overlay refreshes this frame and `/compile`
    // sees the latest contents.
    let on_change = move |new_value: String| {
        buffer.set(new_value.clone());
        let path = active.get();
        files.update(|map| {
            map.insert(path.clone(), new_value);
        });
    };

    // Ref into the textarea so the on_key_down handler can call
    // `insert_text("    ")` for unmodified Tab. The handler captures
    // the Ref by value (it's Copy); the actual handle is filled at
    // mount time and remains usable for the lifetime of the textarea.
    let textarea_ref: Ref<TextAreaHandle> = Ref::new();
    let on_key_down = move |ev: &KeyEvent| {
        // Plain Tab → insert four spaces. With any modifier we let the
        // browser do its default (focus traversal, shift-tab back, …)
        // so power-users can still escape the textarea.
        if ev.key == "Tab" && !ev.shift && !ev.ctrl && !ev.alt && !ev.meta {
            if let Some(h) = textarea_ref.get() {
                h.insert_text("    ");
            }
            KeyOutcome::PreventDefault
        } else {
            KeyOutcome::Default
        }
    };
    let textarea = text_area(buffer, on_change)
        // Code-editor shape: no soft-wrap (long lines scroll
        // horizontally, matching the `code_block` overlay), and a fixed
        // height from the absolutely-positioned `textarea_style` rather
        // than growing to the file length.
        .code_mode()
        .on_key_down(on_key_down)
        .bind(textarea_ref)
        .with_style(textarea_style());

    // Highlight overlay — re-renders only when `buffer` changes.
    // `switch`'s `PartialEq` diff on the inner `String` gates the
    // rebuild; tokenizing 10 k chars is sub-millisecond so
    // per-keystroke rebuilds stay cheap.
    let dep_buffer = buffer;
    let build_buffer = buffer;
    let highlight_layer = switch(
        move || dep_buffer.get(),
        move |_| {
            let src = build_buffer.get();
            let spans = highlight_rust(&src);
            code_block(spans).with_style(code_overlay_style()).into()
        },
    );

    ui! {
        view(style = editor_stack_style()) {
            highlight_layer
            textarea
        }
    }
}

// =============================================================================
// Controls — mode toggle + Run + status pane.
// =============================================================================

fn controls_panel(
    files: Signal<BTreeMap<String, String>>,
    mode_sim: Signal<bool>,
    is_compiling: Signal<bool>,
    status: Signal<String>,
    iframe_url: Signal<String>,
) -> Element {
    let sim_button = button("Simulator", move || mode_sim.set(true))
        .disabled(move || mode_sim.get());
    let web_button = button("Web", move || mode_sim.set(false))
        .disabled(move || !mode_sim.get());
    let mode_row = ui! {
        view(style = mode_row_style()) {
            sim_button
            web_button
        }
    };

    // Reactive `text(closure)` is the canonical pattern for a
    // Text whose body reads from a signal — the framework wraps
    // the closure in an Effect at walk time, which fires
    // `update_text` on every signal change.
    let status_label = text(move || status.get());
    let status_pane = ui! {
        scroll_view(style = status_pane_style()) {
            status_label
        }
    };

    let on_run = move || {
        let project = files.get();
        let picked = if mode_sim.get() { Mode::Simulator } else { Mode::Web };
        is_compiling.set(true);
        status.set(match picked {
            Mode::Simulator => "Compiling for simulator…".to_string(),
            Mode::Web => "Compiling for web…".to_string(),
        });
        wasm_bindgen_futures::spawn_local(async move {
            match fetch::compile(&project, picked).await {
                Ok(hash) => {
                    iframe_url.set(format!(
                        "/compiled/{hash}/?t={}",
                        js_sys::Date::now() as u64
                    ));
                    status.set(format!("Built {hash}"));
                }
                Err(err) => status.set(err),
            }
            is_compiling.set(false);
        });
    };
    let run_button = button("Run", on_run).disabled(move || is_compiling.get());

    ui! {
        view(style = controls_col_style()) {
            mode_row
            run_button
            status_pane
        }
    }
}

// =============================================================================
// Preview — `WebView` driven by the `iframe_url` signal.
// =============================================================================

fn preview_panel(iframe_url: Signal<String>) -> Element {
    webview::web_view(webview::WebViewProps {
        url: webview::url(move || iframe_url.get()),
        ..Default::default()
    })
    .with_style(preview_style())
    .into()
}
