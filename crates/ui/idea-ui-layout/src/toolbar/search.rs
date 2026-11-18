//! [`ToolbarSearch`] — a list's search: a glyph that opens its field,
//! or the field itself where it has the row's left end.

use std::rc::Rc;

use icons_lucide::{SEARCH, X};
use idea_ui::{tone, variant, Adornment, ElementAlign, ElementSide, Field, IconButton, Popover, PopoverSlotStyles};
use runtime_core::primitives::text_input::TextInputHandle;
use runtime_core::{
    component, on_key, rx, signal, stylesheet, ui, AnchorTarget, AppKeyEvent, Color, Element, FlexDirection,
    IdealystSchema, KeyEvent, KeyOutcome, Length, Position, PressableHandle, Ref, Signal, StyleRules, Tokenized,
};

use super::button::BadgeDot;

/// The inline field's widest: past it a search box is a line nobody
/// types across.
pub const SEARCH_FIELD_MAX_WIDTH: f32 = 360.0;

/// The opened field's width under its glyph.
pub const SEARCH_POPOVER_WIDTH: f32 = 320.0;

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct ToolbarSearchProps {
    /// The screen's search text. The control writes it on every edit.
    pub value: Signal<String>,
    /// What the field asks for: "Search by name or employee ID".
    #[prop(static)]
    pub placeholder: String,
    /// The field itself, for a toolbar with nothing else at its left
    /// end (a screen with no tabs): pass it as the toolbar's `start`.
    /// Otherwise a glyph that opens the field.
    #[prop(static)]
    pub inline: bool,
    /// Runs after each edit, for what a new term resets (the page a
    /// list is on).
    #[prop(static)]
    pub on_change: Option<Rc<dyn Fn(String)>>,
    /// The glyph form's field is open. Pass a signal to open or close it
    /// from outside (a command palette, a "search this list" link);
    /// without one the control keeps its own. The inline form ignores it.
    #[prop(static)]
    pub open: Option<Signal<bool>>,
    /// Ctrl-F (⌘F on a Mac) goes to the search while the control is
    /// mounted, in place of the browser's find. Turn it off for all but
    /// one where two searches are mounted at once.
    #[prop(static)]
    pub find_shortcut: bool,
    /// The field's robot/E2E id.
    #[prop(static)]
    pub test_id: Option<&'static str>,
}

impl Default for ToolbarSearchProps {
    fn default() -> Self {
        Self {
            value: signal(String::new()),
            placeholder: "Search".to_string(),
            inline: false,
            on_change: None,
            open: None,
            find_shortcut: true,
            test_id: None,
        }
    }
}

/// A list's search (rule `search-button`), the same at every width.
///
/// - As a glyph: the magnifier at the toolbar's right end opens the
///   field under it. A dot on the glyph says a term is in use while the
///   field is closed. Enter and Escape close the field and keep the
///   term.
/// - `inline`: the field stands in the row, where a screen has no tabs
///   to put at the left. On a phone it takes the row.
///
/// Either way the field has a ✕ that clears the term while there is
/// one, and Ctrl-F (⌘F on a Mac) goes to it: the glyph's field opens,
/// the inline field takes focus. The browser's own find cannot see the
/// rows a paged or virtualized list has not drawn.
///
/// ```ignore
/// Toolbar(end = Some(ui! { ToolbarSearch(value = search, placeholder = "Search workers".to_string()) }))
/// Toolbar(start = Some(ui! { ToolbarSearch(value = search, inline = true) }))
/// ```
#[component]
pub fn ToolbarSearch(props: &ToolbarSearchProps) -> Element {
    let value = props.value;
    let placeholder = props.placeholder.clone();
    let test_id = props.test_id.unwrap_or("toolbar-search");
    let after = props.on_change.clone();
    let write: Rc<dyn Fn(String)> = Rc::new(move |text: String| {
        value.set(text.clone());
        if let Some(after) = &after {
            after(text);
        }
    });
    let clear: Rc<dyn Fn()> = {
        let write = write.clone();
        Rc::new(move || write(String::new()))
    };
    let has_term: Rc<dyn Fn() -> bool> = Rc::new(move || !value.get().is_empty());
    let find_shortcut = props.find_shortcut;
    if props.inline {
        let field: Ref<TextInputHandle> = Ref::new();
        if find_shortcut {
            on_key(move |ev| {
                if !is_find(ev) {
                    return KeyOutcome::Default;
                }
                if let Some(input) = field.get() {
                    input.focus();
                }
                KeyOutcome::PreventDefault
            });
        }
        return ui! {
            view(style = SearchInlineSeat()) {
                Field(
                    field_ref = Some(field),
                    value = value,
                    on_change = write.clone(),
                    placeholder = Some(placeholder.clone()),
                    leading = Adornment::Icon(SEARCH),
                    trailing = Adornment::Button { icon: X, label: Some("Clear search".to_string()), on_press: clear.clone() },
                    trailing_when = Some(has_term.clone()),
                    test_id = Some(test_id),
                )
            }
        };
    }
    let open = props.open.unwrap_or_else(|| signal(false));
    if find_shortcut {
        on_key(move |ev| {
            if !is_find(ev) {
                return KeyOutcome::Default;
            }
            open.set(true);
            KeyOutcome::PreventDefault
        });
    }
    let anchor: Ref<PressableHandle> = Ref::new();
    let toggle: Rc<dyn Fn()> = Rc::new(move || open.set(!open.peek()));
    let close: Rc<dyn Fn()> = Rc::new(move || open.set(false));
    let on_key: Rc<dyn Fn(&KeyEvent) -> KeyOutcome> = Rc::new(move |ev: &KeyEvent| {
        if ev.key == "Enter" || ev.key == "Escape" {
            open.set(false);
            KeyOutcome::PreventDefault
        } else {
            KeyOutcome::Default
        }
    });
    ui! {
        view(style = SearchGlyphSeat()) {
            IconButton(
                icon = Some(SEARCH),
                variant = variant::Outlined,
                tone = tone::Neutral,
                on_click = toggle.clone(),
                bind_to = Some(anchor),
                label = Some("Search".to_string()),
                test_id = Some("toolbar-search-open"),
            )
            BadgeDot(on = rx!(!value.get().is_empty()))
            if open.get() {
                Popover(
                    target = Some(AnchorTarget::from(anchor)),
                    // Under the glyph, its right edge on the glyph's:
                    // the glyph sits at the toolbar's right end.
                    side = ElementSide::Below,
                    align = ElementAlign::End,
                    on_dismiss = Some(close.clone()),
                    // The popover IS the field: no panel round it, only
                    // the field's own box, lifted by the popover's
                    // shadow.
                    styles = PopoverSlotStyles::default().surface(bare_surface()),
                ) {
                    view(style = SearchPopoverBody()) {
                        Field(
                            value = value,
                            on_change = write.clone(),
                            placeholder = Some(placeholder.clone()),
                            leading = Adornment::Icon(SEARCH),
                            trailing = Adornment::Button { icon: X, label: Some("Clear search".to_string()), on_press: clear.clone() },
                            trailing_when = Some(has_term.clone()),
                            autofocus = true,
                            on_key_down = Some(on_key.clone()),
                            test_id = Some(test_id),
                        )
                    }
                }
            }
        }
    }
}

/// The find chord: Ctrl-F, or ⌘F on a Mac, pressed (not held, not
/// released). With Alt it is some other chord.
fn is_find(ev: &AppKeyEvent) -> bool {
    ev.is_press() && (ev.ctrl || ev.meta) && !ev.alt && ev.key.eq_ignore_ascii_case("f")
}

/// The popover's surface with its padding, border and fill taken away,
/// so the field is the only box drawn.
fn bare_surface() -> StyleRules {
    let zero = || Some(Tokenized::Literal(Length::Px(0.0)));
    StyleRules {
        padding_top: zero(),
        padding_right: zero(),
        padding_bottom: zero(),
        padding_left: zero(),
        border_top_width: Some(Tokenized::Literal(0.0)),
        border_right_width: Some(Tokenized::Literal(0.0)),
        border_bottom_width: Some(Tokenized::Literal(0.0)),
        border_left_width: Some(Tokenized::Literal(0.0)),
        background: Some(Tokenized::Literal(Color("transparent".into()))),
        ..Default::default()
    }
}

stylesheet! {
    // The inline field takes the room its slot gives it, up to a
    // search box's widest.
    SearchInlineSeat<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_width: Length::Px(0.0),
            max_width: Length::Px(SEARCH_FIELD_MAX_WIDTH),
        }
    }
}

stylesheet! {
    // The glyph's own box, so the dot has a corner to stand in.
    SearchGlyphSeat<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            position: Position::Relative,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    // Never wider than the window it opens in.
    SearchPopoverBody<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            width: Length::Px(SEARCH_POPOVER_WIDTH),
            max_width: Length::pct(100.0),
        }
    }
}
