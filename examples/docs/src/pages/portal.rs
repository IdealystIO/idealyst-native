//! Portal page — built via the `docs!` macro.
//!
//! Covers `Primitive::Portal` as the one render-elsewhere primitive,
//! and how `overlay()` / `anchored_overlay()` lower to it.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{code_block, page_header, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{body, card, heading, stack};

docs! {
    slug = "portal",
    title = "Portal",
    category = Reference,
    description = "The one render-elsewhere primitive. Modals, popovers, drawers, sheets, tooltips all compose on top.",
    related = ["primitives", "components", "third-party-primitives"],
    concepts = [Portal, Overlay, AnchoredOverlay],

    section(heading = "What it is") {
        p(code("Primitive::Portal"),
          " renders a subtree at a different location in the host tree, \
           escaping the parent's layout and clipping. It's the only \
           render-elsewhere primitive the framework ships — modals, \
           popovers, dropdowns, tooltips, sheets, alerts are all \
           compositions on top."),
        p("On each backend the portal mounts at the platform's window-\
           level surface: a ", code("<div>"), " on ",
          code("document.body"), " for the web, ",
          code("keyWindow.addSubview:"), " on iOS, ",
          code("WindowManager.addView"), " on Android, and so on. \
           Children laid out inside the portal use the same flex model \
           as everywhere else — the portal container itself is what \
           does the escape."),
    },

    section(heading = "Targets") {
        p(code("PortalTarget"),
          " carries both the mount location AND the positioning intent \
           the backend uses to lay out the portal's frame within that \
           target:"),
        list(
            [code("Viewport(ViewportPlacement)"),
             " — viewport-rooted with a placement enum: ",
             code("Center"), ", ", code("Top"), ", ", code("Bottom"),
             ", ", code("Left"), ", ", code("Right"), ", ",
             code("FullScreen"), ". The common case for modals, drawers, \
              sheets, alerts."],
            [code("Anchor { target, side, align, offset }"),
             " — tracks an element. Backends subscribe to scroll / \
              layout / orientation events and re-query the anchor's \
              rect to reposition. The common case for popovers, \
              tooltips, dropdowns, context menus."],
            [code("Named(&'static str)"),
             " — mount into a named container. Reserved for future \
              \"slot\" routing; backends typically leave this \
              unimplemented today."],
        ),
        p(code("AnchorTarget"), " (the value inside ", code("Anchor"),
          ") is constructed from any ", code("Ref<H>"),
          " whose handle type implements ", code("AnchorableHandle"),
          ". The framework ships impls for the visible primitive \
           handles (View, Button, Pressable, etc.) so you can anchor \
           against anything you can grab a ref to."),
    },

    section(heading = "Direct portal use") {
        p("Most apps reach for the compositions below instead, but if \
           you're building a novel floating UX, ", code("portal(...)"),
          " is the entry point. It takes a target and a children list, \
           and exposes ", code(".on_dismiss(...)"), ", ",
          code(".trap_focus(...)"), ", ", code(".bind(...)"), "."),
        code(rust, r##"
            use runtime_core::{portal, PortalTarget, ViewportPlacement, view, text};

            portal(
                PortalTarget::Viewport(ViewportPlacement::Center),
                vec![
                    view(vec![
                        text("Custom floating layer").into(),
                    ]).into(),
                ],
            )
            .on_dismiss(move || open.set(false))
            .trap_focus(true)
        "##),
        p(code("on_dismiss"), " fires only for platform-level dismissal \
           events — Escape on web, back gesture on Android, swipe-down \
           on iOS modal presentations. Backdrop-tap dismissal is a \
           composition concern (a fullscreen ", code("Pressable"),
          " child whose ", code("on_click"),
          " flips your open-state signal). The framework never auto-\
           tears-down; the host's reactive state is the source of truth."),
    },

    section(heading = "Overlay and AnchoredOverlay — composed on top") {
        p(code("overlay()"), " and ", code("anchored_overlay()"),
          " are compositions, not primitives. They lower to ",
          code("Primitive::Portal"), " at conversion time, adding the \
           backdrop layer + content wrapper around your children:"),
        code(rust, r##"
            use runtime_core::{overlay, BackdropMode, ViewportPlacement, view, text};

            overlay(vec![
                view(vec![
                    text("Modal content").into(),
                ]).into(),
            ])
            .placement(ViewportPlacement::Center)
            .backdrop(BackdropMode::Dismiss)
            .on_dismiss(move || open.set(false))
            .trap_focus(true)
        "##),
        p("Defaults: ", code("Center"), " placement, ",
          code("Dismiss"), " backdrop, focus-trap on. The composition \
           builds a ", code("Portal"),
          " with two children — a fullscreen ", code("Pressable"),
          " wired to the dismiss handler (the backdrop) and a ",
          code("View"), " wrapping the caller's children (the content). \
           Backdrop tap → ", code("on_click"), " → ",
          code("on_dismiss"), " → host's open-state signal flips → \
           portal scope drops → backend releases."),
        p(code("anchored_overlay()"), " is the same shape with ",
          code("PortalTarget::Anchor"),
          " under it instead of ", code("Viewport"),
          ". Defaults: ", code("Below"), " side, ", code("Start"),
          " align, ", code("BackdropMode::None"),
          " (page behind stays interactive), focus-trap off — the \
           popover defaults."),
        p("Public API is identical to what it was when Overlay and \
           AnchoredOverlay were separate primitives — same builder \
           methods, same defaults, same call sites. The change is \
           entirely under the hood: one closed-enum primitive instead \
           of two, one ", code("Backend::create_portal"),
          " trait method instead of six. Backends ship less code; \
           defaults move to the composition layer where they're a UX \
           choice, not a framework obligation."),
    },

    section(heading = "Backdrop modes") {
        p(code("BackdropMode"), " lives on the composition (not on the \
           portal primitive itself). Three variants:"),
        list(
            [code("Dismiss"), " — semi-transparent scrim; tap fires ",
             code("on_dismiss"), ". The default for ", code("overlay()"), "."],
            [code("Opaque"), " — semi-transparent scrim; taps don't \
              dismiss. The host must drive close itself (close button, \
              keyboard escape, signal flip from elsewhere)."],
            [code("None"), " — no scrim. The viewport behind stays \
              interactive. The default for ", code("anchored_overlay()"),
             "; appropriate for popovers, tooltips, dropdowns."],
        ),
        p(code(".backdrop_style(s)"), " overrides the scrim's styling. \
           If you need fully custom backdrop behavior, drop down to ",
          code("portal()"), " directly and build your own backdrop \
           child."),
    },

    section(heading = "Stacking and dismissal") {
        p("Portals stack freely. Mounting a second portal while the \
           first is alive layers it on top — backends order by mount \
           order (z-index on web, addSubview order on iOS, \
           attachment order on Android). The framework doesn't \
           enforce a \"one at a time\" rule; that's a UX choice apps \
           make through their own signal logic."),
        p("Platform dismiss events (Android back, web Escape, iOS \
           swipe-down) route to the topmost portal whose ",
          code("on_dismiss"), " is set. If you have nested portals, \
           each one's ", code("on_dismiss"),
          " is responsible for that level of dismissal — the framework \
           doesn't automatically cascade."),
    },

    section(heading = "Focus trap") {
        p(code(".trap_focus(true)"),
          " tells the backend to confine keyboard / accessibility \
           focus inside the portal subtree until it closes. \
           Modal-grade overlays should set this on; non-modal popovers \
           and tooltips should leave it off so users can still \
           interact with the page behind."),
        p("Implementation differs per backend: a ",
          code("focusin"), " listener on web, ",
          code("accessibilityViewIsModal"),
          " on iOS, focusable popup containers on Android. The \
           framework's contract is \"focus stays inside\" — backends \
           implement it as their native a11y APIs allow."),
    },

    section(heading = "Authoring novel floating UX") {
        p("If neither ", code("overlay()"), " nor ",
          code("anchored_overlay()"),
          " fits your need — say you want a sheet with a drag-handle, \
           a non-rectangular overlay, or a sidebar that's neither \
           viewport-pinned nor anchor-tracked — reach for ",
          code("portal(target, children)"),
          " and compose your own. The portal primitive is the seam; \
           everything else is user-space code that happens to live in \
           runtime-core because it's broadly useful."),
        p("For totally platform-specific overlays (native MapKit \
           callouts, system share sheets, system pickers), see ",
          link("Third-party primitives", to = "third-party-primitives"),
          " — ", code("Primitive::External"),
          " is the right hatch for primitives whose implementation \
           is per-backend FFI rather than framework primitives."),
    },

    section(heading = "Where to read more") {
        list(
            [link("Primitives", to = "primitives"),
             " — the closed first-party primitive set, with Portal in \
              its proper place."],
            [link("Components", to = "components"),
             " — for layering your own modal / popover / drawer \
              compositions on top of ", code("overlay()"),
             " / ", code("anchored_overlay()"),
             " / ", code("portal()"), "."],
            [link("Third-party primitives", to = "third-party-primitives"),
             " — the ", code("External"),
             " escape hatch for primitives that need per-platform \
              FFI, outside what compositions on portal can express."],
        ),
    },
}
