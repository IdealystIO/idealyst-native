//! `Breadcrumbs` — a horizontal trail of navigational links ending in
//! the current page.
//!
//! ```ignore
//! ui! {
//!     Breadcrumbs(items = vec![
//!         Crumb::linked("Home", Rc::new(move || go_home())),
//!         Crumb::linked("Library", Rc::new(move || go_library())),
//!         Crumb::new("Settings"), // last = current, not clickable
//!     ])
//! }
//! ```
//!
//! The last crumb renders as the current page (bold, no link);
//! every earlier crumb with an `on_press` is clickable.

use std::rc::Rc;

use runtime_core::{
    component, component_styles, icon, ui, AlignItems, Color, Cursor, Element, FlexDirection, FontWeight,
    IconData, IdealystSchema, IntoElement, Length, Reactive, StyleRules, Tokenized,
};

use crate::theme::IdeaThemeRef;
use idea_theme::tokens;

component_styles! {
    /// The styled sections of [`Breadcrumbs`].
    pub Breadcrumbs<IdeaThemeRef> {
        slot row {
            base(t) {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                gap: t.spacing.xs(),
            }
        }

        slot crumb {
            base(t) {
                color: t.color.text_muted(),
                font_size: t.typography.body_sm_size(),
                background: Color("transparent".into()),
                padding_vertical: 0.0,
                padding_horizontal: t.spacing.xs(),
                border_radius: t.radius.sm(),
            }
            variant current {
                #[default]
                off(_t) {}
                on(t) {
                    color: t.color.text(),
                    font_weight: FontWeight::SemiBold,
                }
            }
            // A linked (non-current) crumb shows a pointer. Variant rather than a
            // computed layer so the rule premints — see `ListItemRow::interactive`.
            variant interactive {
                #[default]
                off(_t) {}
                on(_t) { cursor: Cursor::Pointer }
            }
            state hovered(t) { color: t.color.text() }
            // Focus mirrors hover (text brighten) — replaces the native macOS ring.
            state focused(t) { color: t.color.text() }
            transitions { color: 120ms EaseOut }
        }

        slot separator {
            base(t) {
                color: t.color.text_muted(),
                font_size: t.typography.body_sm_size(),
            }
        }
    }
}

/// Pixel size of an icon separator — matches the crumb text's `body-sm`
/// line so the glyph sits on the same baseline as the labels.
const SEPARATOR_ICON_PX: f32 = 14.0;

/// One crumb. `Crumb::new(label)` is a plain (non-clickable) crumb;
/// `Crumb::linked(label, on_press)` is clickable.
#[derive(Clone, IdealystSchema, runtime_core::Remote)]
pub struct Crumb {
    /// Crumb text. `Reactive<String>` — static or live (signal/`rx!`).
    pub label: Reactive<String>,
    /// When `Some` (and not the last crumb), the crumb is a clickable
    /// link firing this handler.
    pub on_press: Option<Rc<dyn Fn()>>,
}

impl Crumb {
    pub fn new(label: impl Into<Reactive<String>>) -> Self {
        Self { label: label.into(), on_press: None }
    }
    pub fn linked(label: impl Into<Reactive<String>>, on_press: Rc<dyn Fn()>) -> Self {
        Self { label: label.into(), on_press: Some(on_press) }
    }
}

// Reactive-by-default: `#[props]` wraps the scalar `separator`/`separator_icon`
// → `Reactive<…>`; `items` (a `Vec<Crumb>`) is auto-skipped (collections aren't
// sink-consumed — the trail drives the imperative child build). `Crumb.label`
// is already reactive and routes live to its `text()` sink. The separator props
// are snapshotted at build (they drive the imperative separator structure — see
// the TODO in the body).
#[runtime_core::props(styles = BreadcrumbsStyles)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct BreadcrumbsProps {
    /// The trail, root-first. The last item renders as the current page
    /// (emphasized, not clickable).
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub items: Vec<Crumb>,
    /// Separator glyph between crumbs. Default `/`. Ignored when
    /// [`separator_icon`](Self::separator_icon) is `Some`.
    pub separator: String,
    /// Optional icon separator (e.g. `icons_lucide::CHEVRON_RIGHT`). When
    /// `Some`, an icon is drawn between crumbs instead of the text glyph;
    /// `None` falls back to the [`separator`](Self::separator) string — so
    /// both forms are supported.
    pub separator_icon: Option<IconData>,
}

impl Default for BreadcrumbsProps {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            separator: Reactive::Static("/".to_string()),
            separator_icon: Reactive::Static(None),
            styles: Default::default(),
        }
    }
}


/// Renders a horizontal trail of crumbs joined by `separator`. Earlier
/// linked crumbs are clickable; the last crumb is the current page.
#[component]
pub fn Breadcrumbs(props: BreadcrumbsProps) -> Element {
    let n = props.items.len();
    // TODO(reactive-sweep): route `separator`/`separator_icon` reactively. They
    // drive the imperative separator STRUCTURE woven between crumbs (a text
    // glyph vs an icon element), which a live signal can't swap in place
    // without a `switch`/`when` per separator. Snapshotted at build; a live
    // separator won't update. (`Crumb.label` is already routed live to `text()`
    // below.)
    let sep = props.separator.get();
    let sep_icon = props.separator_icon.get();

    let mut kids: Vec<Element> = Vec::with_capacity(n * 2);
    for (i, crumb) in props.items.into_iter().enumerate() {
        let is_current = i + 1 == n;
        let item: Element = match crumb.on_press {
            // A clickable crumb: pressable + a pointer cursor so it reads as
            // interactive (the current/last crumb and plain crumbs stay
            // default-cursor text).
            Some(cb) if !is_current => runtime_core::pressable(
                vec![runtime_core::text(crumb.label).into_element()],
                move || (cb)(),
            )
            .with_style(
                BreadcrumbsStyles::crumb()
                    .set(BreadcrumbsInteractive::On)
                    .overridden(props.styles.crumb.as_ref()),
            )
            .into_element(),
            _ => runtime_core::text(crumb.label)
                .with_style(
                    BreadcrumbsStyles::crumb()
                        .set(BreadcrumbsCurrent::from(is_current))
                        .overridden(props.styles.crumb.as_ref()),
                )
                .into_element(),
        };
        kids.push(item);
        if !is_current {
            // Icon separator when provided, else the text glyph — both forms
            // are supported. The icon is tinted muted to match the glyph.
            let sep_el = match sep_icon {
                Some(data) => icon(data)
                    .with_style(BreadcrumbsStyles::separator().overridden(props.styles.separator.as_ref()).inline(
                        StyleRules {
                            width: Some(Tokenized::Literal(Length::Px(SEPARATOR_ICON_PX))),
                            height: Some(Tokenized::Literal(Length::Px(SEPARATOR_ICON_PX))),
                            ..Default::default()
                        },
                    ))
                    .color(|| {
                        tokens().color.text_muted().resolve()
                    })
                    .into_element(),
                None => runtime_core::text(sep.clone())
                    .with_style(BreadcrumbsStyles::separator().overridden(props.styles.separator.as_ref()))
                    .into_element(),
            };
            kids.push(sep_el);
        }
    }

    let row_style = BreadcrumbsStyles::row().overridden(props.styles.row.as_ref());
    ui! { view(style = row_style) { kids } }
}
