//! The Inspector's layout sheets. Colours, radii and type sizes come from
//! idea-ui's theme tokens, so the installed theme drives the whole app;
//! only layout (widths, grid tracks) is local.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use idea_ui::IdeaThemeRef;
use runtime_core::stylesheet;
use runtime_core::{
    AlignItems, Color, Cursor, DisplayKind, FlexDirection, FontWeight, JustifyContent, Length,
    Overflow, StyleApplication, StyleRules, StyleSheet, TextTransform, Tokenized, TrackSize,
};

/// The monospace stack every value, id and path renders in.
pub const MONO: &str = "ui-monospace, SFMono-Regular, Menlo, monospace";

/// Sidebar width (px).
pub const SIDEBAR_W: f32 = 232.0;
/// Component-tree pane width (px).
pub const TREE_PANE_W: f32 = 440.0;
/// Tree indent per depth level (px).
pub const TREE_INDENT: f32 = 14.0;

// ---- Frame ------------------------------------------------------------------

stylesheet! {
    pub Root<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            height: Length::pct(100.0),
            background: t.color.background(),
        }
    }
}

stylesheet! {
    pub ShellRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            flex_grow: 1.0,
            min_height: Length::Px(0.0),
            width: Length::pct(100.0),
        }
    }
}

stylesheet! {
    pub Sidebar<IdeaThemeRef> {
        base(t) {
            width: Length::Px(SIDEBAR_W),
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            gap: 4.0,
            padding: 12.0,
            background: t.color.surface(),
            border_right_width: 1.0,
            border_right_color: t.color.border(),
        }
    }
}

stylesheet! {
    pub IdentityCard<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: 4.0,
            padding: 12.0,
            margin_bottom: 12.0,
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.md(),
            background: t.color.background(),
        }
    }
}

stylesheet! {
    pub NavItem<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 10.0,
            padding_vertical: 8.0,
            padding_horizontal: 10.0,
            border_radius: 8.0,
            background: Color("transparent".into()),
            cursor: Cursor::Pointer,
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                background: t.intent.primary.soft_bg(),
            }
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
    }
}

stylesheet! {
    pub NavItemText<IdeaThemeRef> {
        base(t) {
            font_size: 14.0,
            color: t.color.text_muted(),
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                color: t.color.text(),
                font_weight: FontWeight::SemiBold,
            }
        }
    }
}

stylesheet! {
    pub Spacer<()> {
        base(_t) { flex_grow: 1.0 }
    }
}

stylesheet! {
    pub Main<()> {
        base(_t) {
            flex_grow: 1.0,
            flex_basis: Length::Px(0.0),
            min_width: Length::Px(0.0),
            flex_direction: FlexDirection::Column,
        }
    }
}

stylesheet! {
    pub SplitRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            flex_grow: 1.0,
            min_height: Length::Px(0.0),
        }
    }
}

stylesheet! {
    pub Pane<IdeaThemeRef> {
        base(t) {
            width: Length::Px(TREE_PANE_W),
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            border_right_width: 1.0,
            border_right_color: t.color.border(),
        }
    }
}

stylesheet! {
    pub SidePane<IdeaThemeRef> {
        base(t) {
            width: Length::Px(420.0),
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            gap: 20.0,
            padding: 20.0,
            border_left_width: 1.0,
            border_left_color: t.color.border(),
        }
    }
}

stylesheet! {
    pub PaneHeader<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: 12.0,
            padding_vertical: 16.0,
            padding_horizontal: 20.0,
            border_bottom_width: 1.0,
            border_bottom_color: t.color.border(),
        }
    }
}

stylesheet! {
    pub PaneFooter<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::SpaceBetween,
            padding_vertical: 8.0,
            padding_horizontal: 20.0,
            border_top_width: 1.0,
            border_top_color: t.color.border(),
        }
    }
}

stylesheet! {
    // A scroll view that fills what's left of a bounded column. An author
    // style replaces `scroll_view`'s own grow seed, so it is restated.
    pub ScrollFill<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_basis: Length::Px(0.0),
            min_height: Length::Px(0.0),
        }
    }
}

stylesheet! {
    pub Padded<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 24.0,
            padding_vertical: 20.0,
            padding_horizontal: 24.0,
        }
    }
}

stylesheet! {
    pub TreeList<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            padding: 8.0,
        }
    }
}

stylesheet! {
    pub RowBetween<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            gap: 12.0,
        }
    }
}

stylesheet! {
    pub RowStart<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 8.0,
        }
    }
}

stylesheet! {
    pub Column<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 8.0,
        }
    }
}

// ---- Tree -------------------------------------------------------------------

stylesheet! {
    pub TreeRowBox<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            height: Length::Px(28.0),
            padding_horizontal: 6.0,
            border_radius: 6.0,
        }
        variant selected {
            #[default]
            off(_t) {}
            on(t) {
                background: t.intent.primary.soft_bg(),
            }
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
    }
}

stylesheet! {
    // The chevron's hit target: a solid, fixed box (an icon alone gives a
    // pressable no reliable hittable area on macOS).
    pub ChevronBox<()> {
        base(_t) {
            width: Length::Px(18.0),
            height: Length::Px(18.0),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            cursor: Cursor::Pointer,
            background: Color("transparent".into()),
        }
    }
}

stylesheet! {
    pub TreeLabel<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 6.0,
            flex_grow: 1.0,
            min_width: Length::Px(0.0),
            height: Length::pct(100.0),
            cursor: Cursor::Pointer,
        }
    }
}

thread_local! {
    static INDENTS: RefCell<HashMap<usize, Rc<StyleSheet>>> = RefCell::new(HashMap::new());
}

/// A fixed-width spacer for tree depth `depth` (sheets cached per depth).
pub fn indent(depth: usize) -> StyleApplication {
    let sheet = INDENTS.with(|m| {
        m.borrow_mut()
            .entry(depth)
            .or_insert_with(|| {
                Rc::new(StyleSheet::r#static(StyleRules {
                    width: Some(Tokenized::Literal(Length::Px(depth as f32 * TREE_INDENT))),
                    flex_shrink: Some(Tokenized::Literal(0.0)),
                    ..Default::default()
                }))
            })
            .clone()
    });
    StyleApplication::new(sheet)
}

// ---- Text -------------------------------------------------------------------

stylesheet! {
    pub ComponentName<IdeaThemeRef> {
        base(t) {
            font_size: 13.5,
            font_weight: FontWeight::Medium,
            color: t.color.text(),
        }
    }
}

stylesheet! {
    pub Mono<IdeaThemeRef> {
        base(t) {
            font_family: MONO,
            font_size: 12.5,
            color: t.color.text(),
        }
    }
}

stylesheet! {
    pub MonoMuted<IdeaThemeRef> {
        base(t) {
            font_family: MONO,
            font_size: 12.0,
            color: t.color.text_muted(),
        }
    }
}

stylesheet! {
    pub MonoLarge<IdeaThemeRef> {
        base(t) {
            font_family: MONO,
            font_size: 32.0,
            font_weight: FontWeight::Medium,
            color: t.color.text(),
        }
    }
}

stylesheet! {
    pub SectionTitle<IdeaThemeRef> {
        base(t) {
            font_size: 12.0,
            font_weight: FontWeight::SemiBold,
            letter_spacing: 0.8,
            text_transform: TextTransform::Uppercase,
            color: t.color.text_muted(),
        }
    }
}

stylesheet! {
    pub Caption<IdeaThemeRef> {
        base(t) {
            font_size: 12.5,
            color: t.color.text_muted(),
        }
    }
}

stylesheet! {
    pub ErrorText<IdeaThemeRef> {
        base(t) {
            font_size: 12.5,
            color: t.intent.danger.fg(),
        }
    }
}

stylesheet! {
    pub OkText<IdeaThemeRef> {
        base(t) {
            font_family: MONO,
            font_size: 12.0,
            color: t.intent.success.fg(),
        }
    }
}

stylesheet! {
    pub StatusDot<IdeaThemeRef> {
        base(t) {
            width: Length::Px(8.0),
            height: Length::Px(8.0),
            border_radius: 4.0,
            background: t.color.text_muted(),
        }
        variant live {
            #[default]
            off(_t) {}
            on(t) {
                background: t.intent.success.solid_bg(),
            }
        }
    }
}

// ---- Tables -----------------------------------------------------------------

stylesheet! {
    pub TableBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.md(),
            overflow: Overflow::Hidden,
        }
    }
}

/// A grid table row with fixed column tracks (`Fr(1)` = the flexible one).
/// Cached per track list; `header` rows get the table-header band.
pub fn grid_row(tracks: &'static [TrackSize], header: bool, selected: bool) -> StyleApplication {
    thread_local! {
        static ROWS: RefCell<HashMap<(usize, bool, bool), Rc<StyleSheet>>> = RefCell::new(HashMap::new());
    }
    let key = (tracks.as_ptr() as usize, header, selected);
    let sheet = ROWS.with(|m| {
        m.borrow_mut()
            .entry(key)
            .or_insert_with(|| {
                Rc::new(StyleSheet::new(move |_| {
                    let t = idea_ui::tokens();
                    StyleRules {
                        display: Some(DisplayKind::Grid),
                        grid_template_columns: Some(tracks.to_vec()),
                        column_gap: Some(Tokenized::Literal(Length::Px(12.0))),
                        align_items: Some(AlignItems::Center),
                        padding_top: Some(Tokenized::Literal(Length::Px(if header { 7.0 } else { 9.0 }))),
                        padding_bottom: Some(Tokenized::Literal(Length::Px(if header { 7.0 } else { 9.0 }))),
                        padding_left: Some(Tokenized::Literal(Length::Px(14.0))),
                        padding_right: Some(Tokenized::Literal(Length::Px(14.0))),
                        border_top_width: Some(Tokenized::Literal(if header { 0.0 } else { 1.0 })),
                        border_top_color: Some(t.color.border()),
                        background: if header {
                            Some(t.color.table_header())
                        } else if selected {
                            Some(t.intent.primary.soft_bg())
                        } else {
                            None
                        },
                        ..Default::default()
                    }
                }))
            })
            .clone()
    });
    StyleApplication::new(sheet)
}

stylesheet! {
    pub KvBox<IdeaThemeRef> {
        base(t) {
            display: DisplayKind::Grid,
            grid_template_columns: vec![TrackSize::Px(116.0), TrackSize::Minmax(Box::new(TrackSize::Px(0.0)), Box::new(TrackSize::Fr(1.0)))],
            row_gap: 8.0,
            column_gap: 12.0,
            padding: 14.0,
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.md(),
        }
    }
}

/// A small colour chip showing `color` (a `#rrggbb[aa]` string).
pub fn swatch(color: String) -> StyleApplication {
    StyleApplication::new(Rc::new(StyleSheet::new(move |_| StyleRules {
        width: Some(Tokenized::Literal(Length::Px(12.0))),
        height: Some(Tokenized::Literal(Length::Px(12.0))),
        background: Some(Tokenized::Literal(Color(color.clone()))),
        ..rounded(3.0, Some(idea_ui::tokens().color.border_strong()))
    })))
}

/// Rules with every corner rounded to `radius` px and, when `border` is
/// given, a 1 px border in it (`StyleRules` spells both per side).
fn rounded(radius: f32, border: Option<Tokenized<Color>>) -> StyleRules {
    let r = Some(Tokenized::Literal(Length::Px(radius)));
    let mut rules = StyleRules {
        border_top_left_radius: r.clone(),
        border_top_right_radius: r.clone(),
        border_bottom_left_radius: r.clone(),
        border_bottom_right_radius: r,
        ..Default::default()
    };
    if let Some(c) = border {
        let w = Some(Tokenized::Literal(1.0));
        rules.border_top_width = w.clone();
        rules.border_right_width = w.clone();
        rules.border_bottom_width = w.clone();
        rules.border_left_width = w;
        rules.border_top_color = Some(c.clone());
        rules.border_right_color = Some(c.clone());
        rules.border_bottom_color = Some(c.clone());
        rules.border_left_color = Some(c);
    }
    rules
}

stylesheet! {
    pub Clickable<()> {
        base(_t) {
            cursor: Cursor::Pointer,
        }
    }
}

// ---- Signals chart ----------------------------------------------------------

stylesheet! {
    pub ChartBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexEnd,
            gap: 2.0,
            height: Length::Px(88.0),
            padding: 8.0,
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.md(),
        }
    }
}

/// One bar of the history chart: `fraction` of the chart's height.
pub fn bar(fraction: f32) -> StyleApplication {
    let pct = (fraction.clamp(0.0, 1.0) * 100.0).round().max(4.0);
    thread_local! {
        static BARS: RefCell<HashMap<u32, Rc<StyleSheet>>> = RefCell::new(HashMap::new());
    }
    let sheet = BARS.with(|m| {
        m.borrow_mut()
            .entry(pct as u32)
            .or_insert_with(|| {
                Rc::new(StyleSheet::new(move |_| StyleRules {
                    flex_grow: Some(Tokenized::Literal(1.0)),
                    height: Some(Tokenized::Literal(Length::Percent(pct))),
                    background: Some(idea_ui::tokens().intent.success.solid_bg()),
                    ..rounded(2.0, None)
                }))
            })
            .clone()
    });
    StyleApplication::new(sheet)
}

// ---- Navigation -------------------------------------------------------------

stylesheet! {
    pub StackCards<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 10.0,
            flex_wrap: runtime_core::FlexWrap::Wrap,
        }
    }
}

stylesheet! {
    pub StackCard<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: 6.0,
            width: Length::Px(240.0),
            padding: 14.0,
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.md(),
            background: t.color.surface(),
        }
        variant current {
            #[default]
            off(_t) {}
            on(t) {
                border_width: 2.0,
                border_color: t.intent.primary.border(),
                background: t.intent.primary.soft_bg(),
            }
        }
    }
}

stylesheet! {
    pub ListButton<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 4.0,
            padding: 12.0,
            border_radius: 8.0,
            cursor: Cursor::Pointer,
        }
        variant selected {
            #[default]
            off(_t) {}
            on(t) {
                background: t.intent.primary.soft_bg(),
            }
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
    }
}

// ---- Connect ----------------------------------------------------------------

stylesheet! {
    pub ConnectColumn<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 28.0,
            width: Length::Px(760.0),
            max_width: Length::pct(100.0),
            padding_vertical: 96.0,
            padding_horizontal: 24.0,
            align_self: runtime_core::AlignSelf::Center,
        }
    }
}

stylesheet! {
    pub AppRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 16.0,
            padding_vertical: 16.0,
            padding_horizontal: 18.0,
            border_top_width: 1.0,
            border_top_color: t.color.border(),
        }
    }
}

stylesheet! {
    pub Grow<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 4.0,
            flex_grow: 1.0,
            min_width: Length::Px(0.0),
        }
    }
}
