//! `Divider` — thin separator line, horizontal or vertical.

use runtime_core::{component, component_styles, ui, Element, IdealystSchema, Length};

use crate::theme::IdeaThemeRef;

component_styles! {
    /// The styled sections of [`Divider`].
    pub Divider<IdeaThemeRef> {
        slot root {
            base(t) {
                background: t.color.border(),
                height: 1.0,
                width: Length::pct(100.0),
            }
            variant axis {
                #[default]
                horizontal(_t) {
                    height: 1.0,
                    width: Length::pct(100.0),
                }
                // Vertical dividers fill their parent's cross axis via
                // `align_self: stretch` (so a vertical divider inside a
                // flex-row container stretches to the row's height).
                // `min_height` provides a sensible fallback when the
                // parent doesn't have a definite height — without it,
                // `height: 100%` resolves to 0 and the divider becomes
                // invisible.
                vertical(_t) {
                    width: 1.0,
                    height: Length::pct(100.0),
                    min_height: 24.0,
                    align_self: runtime_core::AlignSelf::Stretch,
                }
            }
            transitions {
                background: 250ms EaseInOut,
            }
        }
    }
}

// Reactive-by-default: `#[props]` wraps `axis` → `Reactive<DividerAxis>`. The
// derived `Default` still works (`Reactive::default()` = `Static(Default)`).
// `axis` routes into the style sink, read `.get()` INSIDE so the apply-style
// Effect subscribes when it's live; a bare value stays the static fast path.
#[runtime_core::props(styles = DividerStyles)]
#[derive(Default, IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct DividerProps {
    /// Orientation of the separator line. Horizontal spans the parent's
    /// width (1px tall); Vertical spans its height (1px wide).
    pub axis: DividerAxis,
}

/// Thin themed separator line. Renders as a 1px rule along the chosen
/// `axis`, picking up the theme's border color.
#[component]
pub fn Divider(props: &DividerProps) -> Element {
    // A static `axis` takes the build-time fast path; a live one re-resolves
    // the axis in place.
    let style = DividerStyles::root().set(props.axis.clone()).overridden(props.styles.root.as_ref());
    ui! { view(style = style) {} }
}
