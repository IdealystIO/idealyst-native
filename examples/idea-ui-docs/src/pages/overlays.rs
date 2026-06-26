//! Overlays — Tooltip, Popover, Modal, Collapsible, Alert, Toast.
//!
//! Each `pub fn name() -> Element` returns the page **body only** — a
//! column of demo `Section`s wrapped by `crate::pages::body`. The central
//! page frame in `lib.rs` renders the group overline, title, status badge,
//! lead, and the Usage code panel, so bodies never add their own
//! title/lead/scroll wrapper.

use std::rc::Rc;

use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{signal, ui, Element, PressableHandle, Ref};
use idea_ui::{
    push_toast, push_toast_with, tone, typography_kind, variant, Alert, AlertClose, Button,
    Collapsible, CollapsibleTransition, Modal, Popover, Stack, StackAxis, StackGap, ToastHost,
    ToastPlacement, Tooltip, Typography,
};

use crate::pages::body;
use crate::shell::{Callout, CodePanel, DemoSurface, Prop, PropsTable, Section, P};

// =============================================================================
// Tooltip
// =============================================================================

pub fn tooltip() -> Element {
    body(vec![ui! {
        Stack(gap = StackGap::Xl) {
            Section(title = "Live demo".to_string()) {
                P(content = "The Tooltip wraps its trigger and shows itself — hover the \
                    button (desktop/web) to reveal the bubble; on touch it appears on \
                    long-press and auto-dismisses. No host open-state signal.".to_string())
                DemoSurface {
                    Tooltip(text = "Resets everything to defaults".to_string()) {
                        Button(
                            label = "Hover me".to_string(),
                            on_click = (Rc::new(|| {}) as Rc<dyn Fn()>),
                            tone = tone::Neutral,
                            variant = variant::Soft,
                        )
                    }
                }
            }

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "text",       ty: "Reactive<String>", desc: "Bubble text. Static or live." },
                    Prop { name: "children",   ty: "Vec<Element>",     desc: "The trigger the tooltip wraps and anchors to." },
                    Prop { name: "side",       ty: "ElementSide",      desc: "Which side of the trigger the bubble sits on. Default: Above." },
                    Prop { name: "align",      ty: "ElementAlign",     desc: "Alignment along the anchor edge. Default: Center." },
                    Prop { name: "offset",     ty: "f32",              desc: "Gap in px between the trigger and the bubble. Default: 6." },
                    Prop { name: "dismiss_ms", ty: "u32",              desc: "How long a long-press (touch) tooltip stays up before auto-dismissing. Ignored for hover. Default: 1800." },
                ])
            }

            Section(title = "Recipe".to_string()) {
                P(content = "Wrap the trigger in a Tooltip and give it text — it manages \
                    its own visibility (hover on desktop, long-press on touch).".to_string())
                CodePanel(src = r##"ui! {
    Tooltip(text = "Resets to defaults".into()) {
        IconButton(glyph = "?".into(), on_press = move || reset())
    }
}"##.to_string())
            }

            Callout(label = "Tooltip vs Popover".to_string()) {
                P(content = "Tooltip is a single styled text node — non-interactive, no \
                    backdrop, no focus trap. The bubble inverts onto the theme's color-text \
                    so it reads against any surface. When you need clickable content (menu \
                    items, a form), reach for Popover instead.".to_string())
            }
        }
    }])
}

// =============================================================================
// Popover
// =============================================================================

pub fn popover() -> Element {
    let open = signal!(false);
    let trigger: Ref<PressableHandle> = Ref::new();
    let on_toggle: Rc<dyn Fn()> = Rc::new(move || open.update(|v| *v = !*v));
    let on_dismiss: Rc<dyn Fn()> = Rc::new(move || open.set(false));

    body(vec![ui! {
        Stack(gap = StackGap::Xl) {
            Section(title = "Live demo".to_string()) {
                P(content = "Click the trigger to open a scrim-less menu anchored to it. A tap \
                    anywhere off the surface (or Escape) dismisses.".to_string())
                DemoSurface {
                    Button(
                        label = "Open menu".to_string(),
                        on_click = on_toggle,
                        tone = tone::Neutral,
                        variant = variant::Soft,
                        bind_to = Some(trigger),
                    )
                    if open.get() {
                        Popover(
                            target = Some(AnchorTarget::from(trigger)),
                            side = ElementSide::Below,
                            align = ElementAlign::Start,
                            offset = 6.0,
                            on_dismiss = Some(on_dismiss.clone()),
                        ) {
                            Stack(gap = StackGap::Xs) {
                                Typography(content = "Edit".to_string())
                                Typography(content = "Duplicate".to_string())
                                Typography(content = "Delete".to_string(), tone = Some(tone::Danger.into()))
                            }
                        }
                    }
                }
            }

            Section(title = "Anchoring vs portaling".to_string()) {
                P(content = "Popover uses the framework's anchored_overlay primitive — the \
                    surface is portaled to the document body (so it escapes parent overflow \
                    and stacking contexts) but positioned relative to the bound trigger Ref. \
                    Scrolling the trigger's container moves the popover with it; resizing the \
                    window re-runs positioning.".to_string())
            }

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "target",     ty: "Option<AnchorTarget>", desc: "Element to anchor against — AnchorTarget::from(some_ref). A None target renders nothing (the host gates open-state)." },
                    Prop { name: "side",       ty: "ElementSide",          desc: "Which side of the target the popover sits on. Default: Below." },
                    Prop { name: "align",      ty: "ElementAlign",         desc: "Alignment along the anchor's edge. Default: Start." },
                    Prop { name: "offset",     ty: "f32",                  desc: "Gap in px between the anchor and the popover. Default: 4." },
                    Prop { name: "on_dismiss", ty: "Option<Rc<dyn Fn()>>", desc: "Fires on Escape and on an outside click. Flip your open-state signal here. Unset → can't self-dismiss." },
                ])
            }

            Section(title = "Recipe — context menu".to_string()) {
                CodePanel(src = r##"let trigger: Ref<PressableHandle> = Ref::new();
let open = signal!(false);

ui! {
    Button(
        label = "More".into(),
        on_click = move || open.update(|v| *v = !*v),
        tone = tone::Neutral,
        variant = variant::Soft,
        bind_to = Some(trigger),
    )
    if open.get() {
        Popover(
            target = Some(AnchorTarget::from(trigger)),
            side = ElementSide::Below,
            align = ElementAlign::Start,
            offset = 6.0,
            on_dismiss = Some(Rc::new(move || open.set(false))),
        ) {
            Stack(gap = StackGap::Xs) {
                Typography(content = "Edit".into())
                Typography(content = "Duplicate".into())
                Typography(content = "Delete".into(), tone = Some(tone::Danger.into()))
            }
        }
    }
}"##.to_string())
            }
        }
    }])
}

// =============================================================================
// Modal
// =============================================================================

pub fn modal() -> Element {
    let open = signal!(false);
    let on_open: Rc<dyn Fn()> = Rc::new(move || open.set(true));
    let on_close: Rc<dyn Fn()> = Rc::new(move || open.set(false));
    let content_close = on_close.clone();

    body(vec![ui! {
        Stack(gap = StackGap::Xl) {
            Section(title = "Live demo".to_string()) {
                P(content = "A viewport-centered overlay with a dimming, dismiss-on-tap scrim. \
                    The surface fades and slides in, caps to the viewport, and scrolls \
                    internally when its content is taller than the screen.".to_string())
                DemoSurface {
                    Button(
                        label = "Open modal".to_string(),
                        on_click = on_open,
                        tone = tone::Primary,
                        variant = variant::Filled,
                    )
                    Modal(
                        open = open,
                        on_dismiss = Some(on_close.clone()),
                        content = move || ui! {
                            Stack(gap = StackGap::Md) {
                                Typography(content = "Confirm".to_string(), kind = typography_kind::H3)
                                Typography(
                                    content = "Tap outside, press Escape, or use the buttons \
                                        below to dismiss.".to_string(),
                                    muted = true,
                                )
                                Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
                                    Button(
                                        label = "Cancel".to_string(),
                                        on_click = content_close.clone(),
                                        tone = tone::Neutral,
                                        variant = variant::Soft,
                                    )
                                    Button(
                                        label = "Confirm".to_string(),
                                        on_click = content_close.clone(),
                                        tone = tone::Primary,
                                        variant = variant::Filled,
                                    )
                                }
                            }
                        },
                    )
                }
            }

            Section(title = "Pattern".to_string()) {
                P(content = "Modal is controlled — the host owns a Signal<bool> and passes it as \
                    `open`. The Modal is ALWAYS mounted (no `if open { … }` gate): flipping \
                    `open` false plays the exit animation and then unmounts via presence. \
                    `content` is a closure (not a `{ }` block) because it's rebuilt fresh on \
                    each open. on_dismiss (backdrop tap / Escape / back) fires the host's \
                    close.".to_string())
                CodePanel(src = r##"let open = signal!(false);
let on_close: Rc<dyn Fn()> = Rc::new(move || open.set(false));

ui! {
    Button(label = "Open".into(), on_click = move || open.set(true),
           tone = tone::Primary, variant = variant::Filled)
    // Always mounted — no `if open { … }`. presence animates the exit.
    Modal(
        open = open,
        on_dismiss = Some(on_close.clone()),
        content = move || ui! {
            Typography(content = "Confirm".into(), kind = typography_kind::H3)
            Typography(content = "Are you sure?".into())
            Button(label = "OK".into(), on_click = on_close.clone(),
                   tone = tone::Primary, variant = variant::Filled)
        },
    )
}"##.to_string())
            }

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "open",             ty: "Reactive<bool>",       desc: "Open state — pass your Signal<bool>. Always mounted; flipping false animates out then unmounts. Do NOT wrap in `if open { … }`." },
                    Prop { name: "content",          ty: "Rc<dyn Fn()->Vec<Element>>", desc: "Builds the modal body (a closure, rebuilt fresh on each open). Author as content = Rc::new(|| ui! { … })." },
                    Prop { name: "on_dismiss",       ty: "Option<Rc<dyn Fn()>>", desc: "Fires on backdrop tap (unless overridden), Escape, or back. Flip your open-state signal here." },
                    Prop { name: "on_backdrop_press", ty: "Option<Rc<dyn Fn()>>", desc: "Intercepts the backdrop tap. Unset → falls back to on_dismiss when dismissable." },
                    Prop { name: "dismissable",      ty: "bool",                 desc: "true (default) lets the backdrop tap and Escape dismiss; false makes the backdrop inert unless on_backdrop_press is set." },
                    Prop { name: "width",            ty: "f32",                  desc: "Desired surface width on a roomy viewport (DIPs). Capped to the viewport reactively so it never overflows a phone. Default: 520." },
                ])
            }

            Callout(label = "Dismissal is delegated".to_string()) {
                P(content = "idea-ui's Modal doesn't decide what \"dismiss\" means — it routes \
                    the gesture to your on_dismiss and lets the host close. That keeps the \
                    backdrop tap, Escape, and an explicit Close button all flowing through one \
                    signal, and lets you intercept (confirm-before-close) without fighting the \
                    component.".to_string())
            }
        }
    }])
}

// =============================================================================
// Collapsible
// =============================================================================

pub fn collapsible() -> Element {
    body(vec![ui! {
        Stack(gap = StackGap::Xl) {
            Section(title = "Live demo".to_string()) {
                P(content = "Click the header to toggle. The body always stays mounted; \
                    visibility flows through a measured animation (or an instant Snap) per \
                    the transition prop.".to_string())
                collapsible_demo_measured()
            }

            Section(title = "Snap (no animation)".to_string()) {
                P(content = "Sets transition = Snap — the body appears in one frame. Cheap, \
                    predictable, and the right call for reduced-motion users.".to_string())
                collapsible_demo_snap()
            }

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "title",       ty: "Reactive<String>",      desc: "Header text. Static literal, Signal<String>, or rx!(...)." },
                    Prop { name: "value",       ty: "Signal<bool>",          desc: "Controlled open state. The host owns this — pass signal!(false) for default-closed." },
                    Prop { name: "on_change",   ty: "Rc<dyn Fn(bool)>",      desc: "Fires on header click with the requested new state. Wire Rc::new(move |v| value.set(v)) for standard toggle." },
                    Prop { name: "transition",  ty: "CollapsibleTransition", desc: "Measured (default) — animates MaxHeight 0↔measured-content-height. Snap — instant." },
                    Prop { name: "duration_ms", ty: "u32",                   desc: "Open/close duration. Default 240; only meaningful with Measured. Keep near 240 to match baked chrome transitions." },
                    Prop { name: "children",    ty: "Vec<Element>",          desc: "Body contents. Always mounted; visibility flows through the transition strategy." },
                ])
            }

            Section(title = "Recipe".to_string()) {
                CodePanel(src = r##"let open = signal!(false);
let on_change: Rc<dyn Fn(bool)> = Rc::new(move |v| open.set(v));

ui! {
    Collapsible(
        title = "Advanced settings".into(),
        value = open,
        on_change = on_change,
    ) {
        Stack(gap = StackGap::Md) {
            Field(label = Some("API key".into()), value = key, on_change = on_key)
            Switch(label = Some("Beta features".into()), value = beta, on_change = on_beta)
        }
    }
}"##.to_string())
            }

            Callout(label = "Controlled, like every disclosure here".to_string()) {
                P(content = "The host owns the open-state signal, so the same pattern that \
                    drives Tabs / Field / Switch applies: flip the signal from anywhere (an \
                    Expand-all button, a URL param) and the section follows. For a coordinated \
                    group where only one item opens at a time, reach for Accordion.".to_string())
            }
        }
    }])
}

fn collapsible_demo_measured() -> Element {
    let open = signal!(false);
    let on_change: Rc<dyn Fn(bool)> = Rc::new(move |v| open.set(v));
    ui! {
        DemoSurface {
            Collapsible(
                title = "Measured — click to expand".to_string(),
                value = open,
                on_change = on_change,
                transition = CollapsibleTransition::Measured,
            ) {
                Stack(gap = StackGap::Sm) {
                    Typography(
                        content = "The body's natural height is measured via the framework's \
                            ViewHandle::on_layout (web ResizeObserver, iOS layoutSubviews, \
                            Android OnLayoutChangeListener). The animator tweens \
                            AnimProp::MaxHeight between 0 and that measured value.".to_string(),
                    )
                    Typography(
                        content = "Content changes re-measure automatically — the next toggle \
                            uses the new natural height. No fixed cap.".to_string(),
                        muted = true,
                    )
                }
            }
        }
    }
}

fn collapsible_demo_snap() -> Element {
    let open = signal!(false);
    let on_change: Rc<dyn Fn(bool)> = Rc::new(move |v| open.set(v));
    ui! {
        DemoSurface {
            Collapsible(
                title = "Snap — click to expand".to_string(),
                value = open,
                on_change = on_change,
                transition = CollapsibleTransition::Snap,
            ) {
                Stack(gap = StackGap::Sm) {
                    Typography(
                        content = "No animation — the body appears in one frame.".to_string(),
                    )
                }
            }
        }
    }
}

// =============================================================================
// Alert
// =============================================================================

pub fn alert() -> Element {
    body(vec![ui! {
        Stack(gap = StackGap::Xl) {
            Section(title = "Intents (soft)".to_string()) {
                P(content = "Tone drives the surface color; the soft variant tints a muted \
                    background behind the text.".to_string())
                DemoSurface {
                    Stack(gap = StackGap::Sm) {
                        Alert(title = "Info: cache refreshed".to_string(),  tone = tone::Info,    variant = variant::Soft)
                        Alert(title = "Success: saved 12 rows".to_string(), tone = tone::Success, variant = variant::Soft)
                        Alert(title = "Warning: quota at 80%".to_string(),  tone = tone::Warning, variant = variant::Soft)
                        Alert(title = "Danger: payment failed".to_string(), tone = tone::Danger,  variant = variant::Soft)
                    }
                }
            }

            Section(title = "Solid".to_string()) {
                P(content = "The filled variant carries the tone as a solid fill with inverted \
                    text — for the loudest, most urgent banners.".to_string())
                DemoSurface {
                    Stack(gap = StackGap::Sm) {
                        Alert(title = "Deploy complete".to_string(),          tone = tone::Success, variant = variant::Filled)
                        Alert(title = "Action required: verify email".to_string(), tone = tone::Warning, variant = variant::Filled)
                        Alert(title = "Outage: API unreachable".to_string(),  tone = tone::Danger,  variant = variant::Filled)
                    }
                }
            }

            Section(title = "With body & dismiss".to_string()) {
                P(content = "Pass a body for a second detail line, and on_dismiss to surface a \
                    close affordance in the top-right.".to_string())
                alert_dismissible_demo()
            }

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "title",      ty: "Reactive<String>",         desc: "Headline text." },
                    Prop { name: "body",       ty: "Reactive<Option<String>>", desc: "Optional second-line detail." },
                    Prop { name: "tone",       ty: "ToneRef",                  desc: "Semantic palette. Default: Info." },
                    Prop { name: "variant",    ty: "VariantRef",               desc: "Filled / Soft / Outlined. Default: Soft." },
                    Prop { name: "action",     ty: "Option<Element>",          desc: "Optional trailing action slot (e.g. a Retry button)." },
                    Prop { name: "close",      ty: "AlertClose",               desc: "None (default) / Button(handler) for the × / Custom(element)." },
                ])
            }

            Callout(label = "Alert vs Toast".to_string()) {
                P(content = "Alert is in-flow — it pushes content and stays until dismissed. \
                    For transient, self-dismissing notifications stacked over the app, use \
                    Toast (same tone × variant styling, different lifecycle).".to_string())
            }
        }
    }])
}

fn alert_dismissible_demo() -> Element {
    let shown = signal!(true);
    let on_dismiss: Rc<dyn Fn()> = Rc::new(move || shown.set(false));
    ui! {
        DemoSurface {
            if shown.get() {
                Alert(
                    title = "Storage almost full".to_string(),
                    body = Some("You're using 92% of your plan's quota. Upgrade or free up space.".to_string()),
                    tone = tone::Warning,
                    variant = variant::Soft,
                    close = AlertClose::Button(on_dismiss.clone()),
                )
            } else {
                P(content = "Dismissed. (Reload the page to bring it back.)".to_string())
            }
        }
    }
}

// =============================================================================
// Toast
// =============================================================================

pub fn toast() -> Element {
    // push_toast / push_toast_with enqueue onto a process-global queue; a
    // single ToastHost mounted anywhere renders them. We mount one here so
    // the demo is self-contained on this page.
    let push_success: Rc<dyn Fn()> =
        Rc::new(|| { push_toast("Saved successfully", tone::Success); });
    let push_danger: Rc<dyn Fn()> =
        Rc::new(|| { push_toast_with("Upload failed", tone::Danger, variant::Filled); });

    body(vec![ui! {
        Stack(gap = StackGap::Xl) {
            Section(title = "Live demo".to_string()) {
                P(content = "Toasts are pushed imperatively from anywhere and rendered by a \
                    single ToastHost mounted near the app root (one is mounted on this page). \
                    Each fades in, shows for a few seconds, then animates out and removes \
                    itself.".to_string())
                DemoSurface {
                    Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
                        Button(
                            label = "Push success".to_string(),
                            on_click = push_success,
                            tone = tone::Success,
                            variant = variant::Soft,
                        )
                        Button(
                            label = "Push error".to_string(),
                            on_click = push_danger,
                            tone = tone::Danger,
                            variant = variant::Filled,
                        )
                    }
                    // The host: a non-modal, touch-passthrough overlay anchored
                    // to a viewport region. Mount once per app, not per call
                    // site — here it's page-local for the demo.
                    ToastHost(placement = ToastPlacement::BottomLeft)
                }
            }

            Section(title = "Pushing & dismissing".to_string()) {
                P(content = "The queue is process-global, so push_toast works from event \
                    handlers, async completions — anywhere, no component scope required. The \
                    returned id lets you close a toast early.".to_string())
                CodePanel(src = r##"// Mount the host once, near the app root:
ui! { ToastHost(placement = ToastPlacement::BottomLeft) }

// Push from anywhere — returns the toast's id:
let id = idea_ui::push_toast("Saved!", tone::Success);
idea_ui::push_toast_with("Upload failed", tone::Danger, variant::Filled);

// Close one early:
idea_ui::dismiss_toast(id);"##.to_string())
            }

            Section(title = "Props (ToastHost)".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "placement", ty: "ToastPlacement", desc: "One of nine viewport regions (TopLeft…BottomRight). Default BottomLeft." },
                    Prop { name: "edge_gap", ty: "f32", desc: "Gap (px) from the hugged viewport edge(s). Default 16." },
                ])
            }

            Callout(label = "Same styling as Alert".to_string()) {
                P(content = "Toast surfaces reuse the installed Alert stylesheet, so they carry \
                    the same tone × variant treatment and theme tokens — override globally via \
                    install_alert_sheet(...). The difference is lifecycle: Alert is in-flow \
                    and persistent, Toast is overlaid and transient.".to_string())
            }
        }
    }])
}
