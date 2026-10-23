//! Editor UI for the fiddle (v2 — multi-file project + syntax
//! highlighting).
//!
//! Three-column layout, top to bottom:
//!
//! - **File tree** (left, ~220 px). Recursive `view` over the
//!   project's path → contents map; folders expand/collapse;
//!   clicking a file sets the active path and loads its contents
//!   into the editor buffer.
//! - **Editor** (center, flex-grow). One `code_editor` bound to the
//!   active file's contents, decorated by `codeblock::rust`. The
//!   primitive owns both of its layers and the metrics they share; this
//!   file only supplies the ranges and the box styling. Run + status pane
//!   sit below.
//! - **Preview** (right, 360×780). `WebView` URL driven by a
//!   signal pointing at `/compiled/<hash>/` — a real web build of the
//!   project (`idealyst build --web` on the server).
//!
//! Styles are declared via one `stylesheet!` block (each rule emits
//! a thread-local cached `*_style()` function the framework can
//! diff cheaply). Call sites pass the style by name to
//! `.with_style(...)`.

mod fetch;

use std::collections::{BTreeMap, BTreeSet};

use runtime_core::primitives::text_area::TextAreaHandle;
use runtime_core::stylesheet;
use runtime_core::{
    button, signal, switch, text, ui, AlignItems, AlignSelf, Color, FlexDirection,
    FontWeight, JustifyContent, KeyEvent, KeyOutcome, Length, Overflow, Element, Ref,
    ScopeAlive, Signal, spawn_then_in,
};
use codeblock::code_editor;
use idea_ui::{install_idea_theme, light_theme};

// The `stylesheet!` macro takes a `<Theme>` generic for syntactic
// reasons; the theme type isn't actually referenced at runtime
// (closures take `&VariantSet`, not a theme). Use any in-scope
// type — `IdeaThemeRef` is the idea-ui convention.
#[allow(unused_imports)]
use idea_ui::theme::IdeaThemeRef;

use crate::fetch::Built;


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
/// Glyph color for source the tokenizer leaves undecorated — the same
/// ink `codeblock::rust::decorations` leaves undecorated.
const EDITOR_INK: &str = "#1f2328";

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
            flex_grow: 1.0,
            height: Length::Px(500.0),
            border_radius: Length::Px(8.0),
            background: Color("#ffffff".into()),
        }
    }
}

stylesheet! {
    pub EditorSurface<IdeaThemeRef> {
        base(_t) {
            // Box styling only. Font family / size / line height /
            // padding are METRICS, owned by the `code_editor` primitive
            // and written to both of its layers — putting them here is
            // exactly the drift this primitive exists to prevent.
            background: Color("#ffffff".into()),
            // The decorated layer measures to the full file, so the
            // surface must be allowed to exceed the scroller's box in
            // both axes rather than being squeezed to fit it.
            flex_shrink: 0.0,
            align_self: AlignSelf::FlexStart,
            min_width: Length::Percent(100.0),
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

// A phone-portrait preview (390 × 844 logical, the `variant-phone`
// aspect), scaled to fit beside the editor.
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

const STARTER_APP_RS: &str = include_str!("starter/app.rs");
const STARTER_WIDGETS_RS: &str = include_str!("starter/widgets.rs");
/// The entry file the server requires (`examples/fiddle/src/compile.rs`).
const STARTER_ENTRY: &str = "app.rs";

fn starter_files() -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert(STARTER_ENTRY.to_string(), STARTER_APP_RS.to_string());
    m.insert("widgets.rs".to_string(), STARTER_WIDGETS_RS.to_string());
    m
}

/// The scene registration seam `idealyst::entry!` calls. Registration is
/// mandatory: an unregistered payload panics at realize. `codeblock` draws
/// the editor, `webview` the preview frame.
pub fn register_scene_extensions<H>(registry: &mut runtime_scene::Registry<H>)
where
    H: runtime_scene::Host
        + runtime_vocabulary::caps::TextOps
        + runtime_vocabulary::caps::ExternalOps
        + runtime_vocabulary::style_attach::StyleServices
        + 'static,
{
    codeblock::register(registry);
    webview::register(registry);
}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    // Root signals — owned at the app level so they survive every
    // re-render the framework triggers on signal updates.
    let files: Signal<BTreeMap<String, String>> = signal(starter_files());
    let active: Signal<String> = signal(STARTER_ENTRY.to_string());
    // Buffer holds the active file's contents. Initialized to the
    // starter entry's text; the tree's click handler refreshes it
    // when the user picks a different file. Kept separate from
    // `files` so per-keystroke updates don't churn the whole map.
    let buffer: Signal<String> = signal(STARTER_APP_RS.to_string());
    let iframe_url: Signal<String> = signal("about:blank".to_string());
    let status: Signal<String> = signal("Press Run to compile".to_string());
    let is_compiling: Signal<bool> = signal(false);
    let expanded: Signal<BTreeSet<String>> = signal(BTreeSet::new());

    let tree = file_tree_panel(files, active, expanded, buffer);
    let editor = editor_panel(files, active, buffer);
    let controls = controls_panel(files, is_compiling, status, iframe_url);
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
    // idealyst-lint-disable-next-line prefer-ui-control-flow -- keyed rebuild of one shape: the file tree per (paths, expanded, active)
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
                "app.rs must define pub fn app() -> Element. Every file \
                 imports what it uses, exactly as in a project made by \
                 idealyst new; runtime_core and idea_ui are available."
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
                label = format!("{chevron}{name}"),
                on_click = move || {
                    let path = path_for_click.clone();
                    // `Signal::update` is staged and value-returning
                    // (`&T -> T`), so the toggle clones, edits, returns.
                    expanded_signal.update(|set| {
                        let mut next = set.clone();
                        if !next.insert(path.clone()) {
                            next.remove(&path);
                        }
                        next
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
    // framework sets up its render scope, so any `effect!`
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
            let mut next = map.clone();
            next.insert(path.clone(), new_value.clone());
            next
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
    // One `code_editor`: the primitive owns the decorated layer, the
    // editing layer, and the metrics both of them must agree on. The
    // tokenizer is passed in as a plain `&str -> Vec<Decoration>`
    // function — the primitive never parses anything itself, which is
    // why the same call works for any language (or for diagnostics that
    // aren't a language at all).
    //
    // This replaces the hand-rolled version of the same idea: a
    // transparent `text_area` absolutely positioned over a `code_block`,
    // with the font metrics duplicated across two stylesheets and the
    // two layers' scroll offsets silently diverging the moment a line
    // ran past the panel's width.
    let editor = code_editor(buffer, on_change)
        .decorate(|src| codeblock::rust::decorations(src, &codeblock::rust::Palette::LIGHT))
        .on_key_down(on_key_down)
        .bind(textarea_ref)
        .font(EDITOR_FONT_FAMILY, EDITOR_FONT_SIZE)
        .line_height(EDITOR_LINE_HEIGHT)
        .padding(EDITOR_PADDING)
        .text_color(EDITOR_INK)
        .with_style(editor_surface_style());

    // The editor grows to its content; the scroller is the ancestor, so
    // both of its layers move together and can never drift apart.
    let editor: Element = editor.into();
    ui! {
        scroll_view(style = editor_stack_style()) {
            editor
        }
    }
}

// =============================================================================
// Controls — Run + status pane.
// =============================================================================

fn controls_panel(
    files: Signal<BTreeMap<String, String>>,
    is_compiling: Signal<bool>,
    status: Signal<String>,
    iframe_url: Signal<String>,
) -> Element {
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

    // Taken in the panel's body: the reply lands only while the panel is
    // mounted, and `is_compiling` rebuilding the Run button must not cancel it.
    let alive = ScopeAlive::current();
    let on_run = move || {
        let project = files.get();
        is_compiling.set(true);
        status.set("Compiling…".to_string());
        spawn_then_in(&alive, fetch::compile(project), move |reply| {
            match reply {
                Ok(Built::Ok { url, ms, warnings }) => {
                    iframe_url.set(url);
                    let took = if ms == 0 { "cached".to_string() } else { format!("{ms} ms") };
                    status.set(if warnings.is_empty() {
                        format!("Built ({took})")
                    } else {
                        format!("Built ({took}) with warnings:\n{warnings}")
                    });
                }
                Ok(Built::Failed(report)) => status.set(report),
                Err(err) => status.set(err),
            }
            is_compiling.set(false);
        });
    };
    let run_button = button("Run", on_run).disabled(move || is_compiling.get());

    ui! {
        view(style = controls_col_style()) {
            run_button
            status_pane
        }
    }
}

// =============================================================================
// Preview — `WebView` driven by the `iframe_url` signal.
// =============================================================================

fn preview_panel(iframe_url: Signal<String>) -> Element {
    webview::WebView(webview::WebViewProps {
        url: webview::url(move || iframe_url.get()),
        ..Default::default()
    })
    .with_style(preview_style())
    .into()
}
