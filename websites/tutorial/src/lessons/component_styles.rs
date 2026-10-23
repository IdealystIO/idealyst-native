//! Component stylesheets — variants, interaction states, overrides and
//! compounds, `component_styles!` slots, and restyling or extending
//! idea-ui. Each step centers on a live fiddle whose starting code lives
//! in `crate::fiddles::component_styles`.

use idea_ui::{typography_kind, Typography};
use runtime_core::{ui, Element};

use crate::common::{Callout, DocsLink, LessonPage, workspace};
use crate::fiddle::Fiddle;
use crate::routes::{
    CS_OVERRIDES_ROUTE, CS_RESTYLE_ROUTE, CS_SLOTS_ROUTE, CS_STATES_ROUTE, CS_VARIANTS_ROUTE,
};

pub fn variants() -> Element {
    ui! {
        LessonPage(
            current = CS_VARIANTS_ROUTE.name(),
            title = "Variants".to_string(),
            lead = "Named versions of one stylesheet, picked where the style is used.".to_string(),
            workspace = workspace(|| ui! { Fiddle(source = include_str!("../fiddles/component_styles/variants.rs")) }),
            learn = vec![
                "Declare a variant axis with a default",
                "Pick a variant at the call site, or follow a signal",
                "Why variants cost a lookup, not a recompute",
            ],
        ) {
            Typography(
                content = "Most components come in a few versions: a badge that is neutral, \
                    primary or alarming; a button that is small or large. A variant axis declares \
                    those versions inside the stylesheet. You name the axis (tone), list its \
                    values, and give each value the rules it adds on top of base. #[default] marks \
                    the value a call site gets when it doesn't choose.".to_string()
            )
            Typography(
                content = "The macro turns each axis into an enum \u{2014} variant tone on Badge \
                    becomes BadgeTone, with BadgeTone::Neutral, ::Primary and ::Danger \u{2014} and \
                    adds a setter to the builder, so a call site writes \
                    Badge().tone(BadgeTone::Primary). Axes combine: tone and size in the example \
                    are independent choices on one sheet.".to_string()
            )

            Typography(content = "Reading the example".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Pill wraps the sheet in a component, and its tone prop picks the \
                    variant. That is the usual shape: the component owns the stylesheet, and the \
                    caller only says which version it wants. The unread counter shows the other \
                    common case, a variant that follows state. A Signal<BadgeTone> can go straight \
                    to the setter \u{2014} .tone(tone) \u{2014} and here, where the tone is computed \
                    from a count, derived(move || ...) hands it a closure instead; it re-runs \
                    whenever a signal it reads changes, so the badge turns red at three without any \
                    code that restyles it.".to_string()
            )
            Typography(
                content = "Try it: add a fourth tone, success, using t.intent.success.soft_bg() \
                    and soft_text(), and give one Pill that tone. Then make large the default size \
                    and watch every badge grow.".to_string()
            )

            Callout(label = "Why variants are cheap".to_string()) {
                Typography(
                    content = "Every value of every axis is known when the app compiles, so the \
                        framework works out each one's rules ahead of time and reuses them. On the \
                        web each value becomes one CSS class, and choosing a variant swaps a class \
                        name. Reach for a variant whenever the choices can be listed.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "The stylesheet! grammar, the generated enums and setters, and how sheets resolve.".to_string(),
                link_label = "Styling reference \u{2014} stylesheet!".to_string(),
                doc_file = "styling.md#the-stylesheet-macro".to_string(),
            )
        }
    }
}

pub fn states() -> Element {
    ui! {
        LessonPage(
            current = CS_STATES_ROUTE.name(),
            title = "Interaction states".to_string(),
            lead = "Hover, press, focus and disabled styling, declared once and animated.".to_string(),
            workspace = workspace(|| ui! { Fiddle(source = include_str!("../fiddles/component_styles/states.rs")) }),
            learn = vec![
                "Style hovered, pressed, focused and disabled",
                "Animate between states with transitions",
                "How web and native reach the same result",
            ],
        ) {
            Typography(
                content = "A control should answer the person using it: lighten under the pointer, \
                    sink when pressed, show a ring when the keyboard reaches it, and grey out when \
                    it can't be used. A state block declares each of those. There are four \u{2014} \
                    hovered, pressed, focused and disabled \u{2014} and each one's rules merge over \
                    the base for as long as that state is on.".to_string()
            )

            Typography(content = "Reading the example".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Nothing in app() mentions hover or press: the stylesheet says how each \
                    state looks, and the framework tracks which states are on. The transitions \
                    block animates the change, so the background fades and the press shrinks over \
                    a few frames instead of jumping. Flip the Disabled switch and the button stops \
                    responding, its look changes, and hovering it does nothing \u{2014} disabled \
                    switches the other three states off.".to_string()
            )
            Typography(
                content = "When several states are on at once they layer in a fixed order, lowest \
                    to highest: hovered, focused, pressed, disabled. A pressed button is always \
                    hovered too, so its press styling wins; a focus ring survives the pointer \
                    resting on it. The order you write the blocks in doesn't matter.".to_string()
            )
            Typography(
                content = "Try it: give state hovered a transform of \
                    vec![Transform::Scale(1.03)] so the button grows under the pointer, and add \
                    transform to the transitions block if it isn't there. Then click into the \
                    preview and press Tab to reach the button: that is the focused state, drawn by \
                    the border.".to_string()
            )

            Callout(label = "Same result, different machinery".to_string()) {
                Typography(
                    content = "On the web the states become CSS rules (:hover, :active, \
                        :focus-visible), so the browser applies them with no Rust code running. On \
                        iOS and Android the backend listens for touches and focus and re-applies \
                        the style. You write one stylesheet and the button behaves the same on \
                        both. Declaring state focused also replaces the platform's default focus \
                        ring with yours \u{2014} which is why the base reserves a transparent border \
                        of the same width, so focusing doesn't shift the layout.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "State precedence, how each backend activates states, and focus indicators.".to_string(),
                link_label = "Styling reference \u{2014} Interaction states".to_string(),
                doc_file = "styling.md#interaction-states".to_string(),
            )
        }
    }
}

pub fn overrides() -> Element {
    ui! {
        LessonPage(
            current = CS_OVERRIDES_ROUTE.name(),
            title = "Overrides & compounds".to_string(),
            lead = "Values a variant can't list, and styles for a combination of variants.".to_string(),
            workspace = workspace(|| ui! { Fiddle(source = include_str!("../fiddles/component_styles/overrides.rs")) }),
            learn = vec![
                "Declare overrides for values no variant can list",
                "Drive overrides and variants from signals",
                "Style a combination with a compound variant",
            ],
        ) {
            Typography(
                content = "Variants cover choices you can name in advance. Some values can't be: a \
                    padding the reader drags with a slider, a size computed from the screen. For \
                    those a stylesheet declares an override \u{2014} override padding: f32 \u{2014} \
                    and the builder gains a padding(...) setter that takes any number. An override \
                    merges in last, after the base and every variant, so it always wins.".to_string()
            )

            Typography(content = "Following state".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Every setter on the builder takes a signal as readily as a value: \
                    padding(padding) hands over the slider's signal itself, and the style re-applies \
                    each time it changes. The two switches are bools, not the variant enums, so they \
                    go through derived(move || ...), which turns any expression over signals into a \
                    source the setter follows.".to_string()
            )

            Typography(content = "Compound variants".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Two axes can set the same property. Here selected colors the border and \
                    so would urgent, and \"selected and urgent\" deserves its own color. A compound \
                    block names that combination outright \u{2014} compound (selected: on, urgent: \
                    yes) \u{2014} and applies only when every named axis holds its value, after all \
                    the single-axis rules. Turn both switches on to see the border go red. Naming an \
                    axis or value the sheet doesn't have is a compile error, so a typo can't fail \
                    silently.".to_string()
            )
            Typography(
                content = "Try it: add override font_size: f32 to the sheet and a third slider that \
                    drives it. Then add a compound for selected: on, urgent: no that sets a \
                    background.".to_string()
            )

            Callout(label = "Why overrides cost more than variants".to_string()) {
                Typography(
                    content = "A variant's rules are worked out once and reused; on the web each \
                        value is a CSS class. An override's value can be anything, so each distinct \
                        value is resolved on its own and can't be prepared ahead of time. Use an \
                        override when the value genuinely can't be a list of choices.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "Compound variants, and why the framework splits variants from overrides.".to_string(),
                link_label = "Styling reference \u{2014} Compound variants".to_string(),
                doc_file = "styling.md#compound-variants".to_string(),
            )
        }
    }
}

pub fn slots() -> Element {
    ui! {
        LessonPage(
            current = CS_SLOTS_ROUTE.name(),
            title = "Component styles".to_string(),
            lead = "Style a component as named parts, so the apps using it can change any part.".to_string(),
            workspace = workspace(|| ui! { Fiddle(source = include_str!("../fiddles/component_styles/slots.rs")) }),
            learn = vec![
                "Declare a component's parts with component_styles!",
                "Share an axis across slots",
                "Let callers override one slot on one instance",
            ],
        ) {
            Typography(
                content = "A real component has more than one styled part. A banner has its box and \
                    its message; a text field has the input, the label and the helper text. \
                    component_styles! declares those parts as named slots, each one a stylesheet, \
                    plus the axes they share. This is how idea-ui's components are written, and how \
                    you would write a component library of your own.".to_string()
            )

            Typography(content = "What the macro generates".to_string(), kind = typography_kind::H2)
            Typography(
                content = "From pub Banner it builds BannerStyles, with one function per slot \u{2014} \
                    BannerStyles::root() and ::message() \u{2014} returning that slot's style. Each \
                    axis becomes its own type, BannerEmphasis, with ::Quiet and ::Loud. And it \
                    builds BannerSlotStyles: one optional override per slot.".to_string()
            )
            Typography(
                content = "The component uses them in two steps per slot. set(...) passes the \
                    emphasis prop to the slot, and works whether the prop is a plain value or a \
                    signal. overridden(...) applies the caller's override for that slot, if there \
                    is one. #[props(styles = BannerStyles)] is what adds the styles prop to \
                    BannerProps, which is why this component declares its props as a struct rather \
                    than as function parameters.".to_string()
            )
            Typography(
                content = "Try it: add a third slot, icon, styled as a small circle, and render it \
                    before the message. Then give the last banner a message override too, through \
                    BannerSlotStyles::default().root(...).message(...).".to_string()
            )

            Callout(label = "Why an axis lives on the component".to_string()) {
                Typography(
                    content = "emphasis is declared once at the top, so every slot that styles it \
                        agrees on its values and default. Axis values are typed per component: \
                        BannerEmphasis::Loud cannot be passed to another component's slot, even one \
                        with an axis of the same name.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "component_styles!, slots, open axes, restyles and per-instance overrides.".to_string(),
                link_label = "Styling reference \u{2014} Component styles".to_string(),
                doc_file = "styling.md#component-styles-component_styles".to_string(),
            )
        }
    }
}

pub fn restyle() -> Element {
    ui! {
        LessonPage(
            current = CS_RESTYLE_ROUTE.name(),
            title = "Restyling & extending".to_string(),
            lead = "Change how a library's components look, and add versions they never shipped.".to_string(),
            workspace = workspace(|| ui! { Fiddle(source = include_str!("../fiddles/component_styles/restyle.rs")) }),
            learn = vec![
                "Restyle a library component app-wide at boot",
                "Add your own variant value with axis_values!",
                "Change a default, or restyle a single instance",
            ],
        ) {
            Typography(
                content = "A theme changes the values components read: colors, spacing, corner \
                    radii. Sometimes you want to change the components themselves \u{2014} every \
                    button a pill, a new raised style that idea-ui doesn't have. Because idea-ui is \
                    built with component_styles!, every slot of every component is open to the app. \
                    You don't fork the library or wrap each component; you register the change once, \
                    at boot.".to_string()
            )

            Typography(content = "Three ways in".to_string(), kind = typography_kind::H2)
            Typography(
                content = "set_default changes an axis's default for the whole app: every Button \
                    that doesn't pick a shape is now a pill. restyle(|s| ...) adds rules to slots: \
                    s.label().base(...) makes every label bold, and the rules merge \u{2014} anything \
                    a restyle doesn't set keeps idea-ui's value. And the styles prop changes one \
                    instance, as the last button does.".to_string()
            )
            Typography(
                content = "axis_values! adds a value to idea-ui's own variant axis. ButtonVariant \
                    is defined in idea-ui, so the new value arrives on a trait your app declares; \
                    with that trait in scope, ButtonVariant::Raised reads like a built-in. A value \
                    looks like the base until something styles it, which is what the two arm(...) \
                    calls do: one for the box, one for the label.".to_string()
            )
            Typography(
                content = "Try it: add a second app value, Subtle, to the axis_values! list, and \
                    give it an arm that sets only a border. Or change set_default to ButtonShape::Sm \
                    and watch every button square up.".to_string()
            )

            Callout(label = "Restyles are part of the build, not a runtime patch".to_string()) {
                Typography(
                    content = "A restyle is composed into the slot's stylesheet, so restyled \
                        components resolve as cheaply as stock ones, and a production web build can \
                        still turn them into static CSS. Register restyles before the first \
                        render: a restyle added later applies only to elements built after it. For \
                        a new value that should follow each button's tone the way the built-ins do, \
                        idea-ui's Button also offers ButtonStyles::restyle(|s| s.tone_variant(...)), \
                        which draws the value once per registered tone.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "Branding idea-ui end to end: restyles, app values and defaults in one boot hook.".to_string(),
                link_label = "Design systems guide".to_string(),
                doc_file = "/crates/mcp/catalog/guides/design-systems.md".to_string(),
            )
        }
    }
}
