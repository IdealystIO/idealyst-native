//! Forms — Checkbox, Radio, Switch, Slider, Field, Textarea, Select,
//! Autocomplete, SegmentedControl, Calendar, DatePicker, DateInput,
//! TimeInput.
//!
//! Each `pub fn name() -> Element` returns the page **body only** — a
//! column of demo `Section`s wrapped by `crate::pages::body`. The central
//! page frame in `lib.rs` renders the title, lead, status badge, and the
//! Usage code panel, so bodies never add their own title/lead/scroll
//! wrapper.

use std::rc::Rc;

use icons_lucide::{CHECK, EYE, EYE_OFF, HEART, SEARCH, STAR};
use runtime_core::{pressable, rx, signal, ui, Element, IntoElement, Signal};
use idea_ui::{
    date::{format_date, format_datetime, format_time},
    tone, Adornment, Autocomplete, Calendar, Checkbox, CivilDate, CivilDateTime, CivilTime,
    ControlSize, DateInput, DatePicker, DateRangePicker, DateTimeInput, DateTimePicker, Field,
    FieldSize, Icon, RadioGroup, RadioOption, RangeCalendar, SegmentOption, SegmentedControl,
    Select, SelectOption, Slider, Stack, StackGap, Switch, Textarea, TimeInput, Typography,
    Weekday,
};

use crate::pages::body;
use crate::shell::{Callout, CodePanel, Demo, DemoSurface, Prop, PropsTable, Section, H3, P};

// =============================================================================
// Checkbox
// =============================================================================

pub fn checkbox() -> Element {
    // A standing-on / standing-off pair for the States row plus a disabled
    // mock (idea-ui's Checkbox has no `disabled` prop, so we dim a static
    // checked box to show the intent).
    let off = signal(false);
    let on_off: Rc<dyn Fn(bool)> = Rc::new(move |v| off.set(v));
    let on = signal(true);
    let on_on: Rc<dyn Fn(bool)> = Rc::new(move |v| on.set(v));

    let agree = signal(false);
    let on_agree: Rc<dyn Fn(bool)> = Rc::new(move |v| agree.set(v));
    let subscribe = signal(true);
    let on_sub: Rc<dyn Fn(bool)> = Rc::new(move |v| subscribe.set(v));
    let favorite = signal(true);
    let on_fav: Rc<dyn Fn(bool)> = Rc::new(move |v| favorite.set(v));

    body(vec![ui! {
        Section(title = "States".to_string()) {
            P(content = "Off and on are the two committed states. idea-ui's Checkbox is a \
                two-state box — there is no separate indeterminate or disabled prop; \
                model those by controlling the bound signal and dimming the surrounding \
                row yourself.".to_string())
            DemoSurface {
                Stack(gap = StackGap::Sm) {
                    Checkbox(label = Some("Off".to_string()), value = off, on_change = on_off)
                    Checkbox(label = Some("On".to_string()), value = on, on_change = on_on, tone = tone::Primary)
                }
            }
        }
    }, ui! {
        Section(title = "Interactive".to_string()) {
            P(content = "The box is drawn from primitives (a pressable row + a tone/variant \
                styled box + a checkmark glyph), so it carries the same tone × variant × \
                size axes as the rest of idea-ui and looks identical on every backend. The \
                host owns the `Signal<bool>`.".to_string())
            DemoSurface {
                Stack(gap = StackGap::Sm) {
                    Checkbox(label = Some("I agree to the terms".to_string()), value = agree, on_change = on_agree, tone = tone::Primary)
                    Checkbox(label = Some("Subscribe to the newsletter".to_string()), value = subscribe, on_change = on_sub, tone = tone::Success)
                    Checkbox(label = Some("Custom icon (star)".to_string()), value = favorite, on_change = on_fav, tone = tone::Warning, icon = Some(STAR))
                }
            }
            CodePanel(src = r##"let agree = signal(false);
let on_agree: Rc<dyn Fn(bool)> = Rc::new(move |v| agree.set(v));

ui! {
    Checkbox(
        label = Some("I agree to the terms".into()),
        value = agree,
        on_change = on_agree,
        tone = tone::Primary,
    )
}"##.to_string())
        }
    }, ui! {
        Section(title = "Props".to_string()) {
            PropsTable(rows = vec![
                Prop { name: "label",     ty: "Reactive<Option<String>>", desc: "Optional label rendered to the right of the box." },
                Prop { name: "value",     ty: "Signal<bool>",             desc: "Controlled checked state. The host owns the signal." },
                Prop { name: "on_change", ty: "Rc<dyn Fn(bool)>",         desc: "Fires with the new value when the user toggles the box." },
                Prop { name: "tone",      ty: "ToneRef",                  desc: "Semantic palette for the checked fill. Default: Primary." },
                Prop { name: "variant",   ty: "VariantRef",              desc: "Surface skeleton for the checked fill. Default: Filled." },
                Prop { name: "size",      ty: "ControlSize",             desc: "Sm / Md / Lg box scale. Default: Md." },
            ])
        }
    }])
}

// =============================================================================
// Radio
// =============================================================================

pub fn radio() -> Element {
    let plan = signal("pro".to_string());
    let on_plan: Rc<dyn Fn(String)> = Rc::new(move |id| plan.set(id));

    let billing = signal("monthly".to_string());
    let on_billing: Rc<dyn Fn(String)> = Rc::new(move |id| billing.set(id));

    let current = runtime_core::switch(
        move || plan.get(),
        |v: &String| {
            let label = format!("Selected plan: {}", v);
            ui! { Typography(content = label, muted = true) }
        },
    );

    body(vec![ui! {
        Section(title = "Radio group".to_string()) {
            P(content = "`RadioGroup` coordinates single-select exclusivity over a \
                `Signal<String>`: each option is a row, and the one whose `id` matches \
                the bound value paints selected. Build options with \
                `RadioOption::new(id, label)`.".to_string())
            DemoSurface {
                Stack(gap = StackGap::Md) {
                    RadioGroup(
                        value = plan,
                        on_change = on_plan,
                        options = vec![
                            RadioOption::new("free", "Free"),
                            RadioOption::new("pro", "Pro"),
                            RadioOption::new("team", "Team"),
                        ],
                        tone = tone::Primary,
                    )
                    current
                }
            }
            CodePanel(src = r##"let plan = signal("pro".to_string());
let on_plan: Rc<dyn Fn(String)> = Rc::new(move |id| plan.set(id));

ui! {
    RadioGroup(
        value = plan,
        on_change = on_plan,
        options = vec![
            RadioOption::new("free", "Free"),
            RadioOption::new("pro",  "Pro"),
            RadioOption::new("team", "Team"),
        ],
        tone = tone::Primary,
    )
}"##.to_string())
        }
    }, ui! {
        Section(title = "Row layout & tones".to_string()) {
            P(content = "Pass `axis = RadioAxis::Row` to lay the options out horizontally, \
                and any `tone` to recolor the selected ring + dot. The same group can be \
                soft-filled with `variant = variant::Soft`.".to_string())
            DemoSurface {
                RadioGroup(
                    value = billing,
                    on_change = on_billing,
                    options = vec![
                        RadioOption::new("monthly", "Monthly"),
                        RadioOption::new("yearly", "Yearly"),
                    ],
                    tone = tone::Success,
                )
            }
        }
    }, ui! {
        Section(title = "Props".to_string()) {
            P(content = "`RadioGroup` is what you reach for most of the time. A standalone \
                `Radio` is the single-row primitive it's built from — use it directly only \
                when you're laying out the rows and coordinating exclusivity yourself.".to_string())
            PropsTable(rows = vec![
                Prop { name: "value",     ty: "Signal<String>",     desc: "RadioGroup: the selected option's id. The host owns the signal." },
                Prop { name: "on_change", ty: "Rc<dyn Fn(String)>", desc: "RadioGroup: fires with the picked id when the user selects an option." },
                Prop { name: "options",   ty: "Vec<RadioOption>",   desc: "RadioGroup: options in render order. RadioOption::new(id, label)." },
                Prop { name: "axis",      ty: "RadioAxis",          desc: "RadioGroup: Column (default) or Row layout." },
                Prop { name: "tone",      ty: "ToneRef",            desc: "Semantic palette for the selected ring + dot. Default: Primary." },
                Prop { name: "variant",   ty: "VariantRef",        desc: "Surface skeleton. Default: Filled." },
                Prop { name: "size",      ty: "ControlSize",       desc: "Sm / Md / Lg indicator scale. Default: Md." },
                Prop { name: "selected",  ty: "Signal<bool>",      desc: "Standalone Radio only: whether this row is selected." },
                Prop { name: "on_select", ty: "Rc<dyn Fn()>",      desc: "Standalone Radio only: fires when the row is clicked." },
            ])
        }
    }])
}

// =============================================================================
// Switch
// =============================================================================

pub fn switch() -> Element {
    let sm = signal(true);
    let on_sm: Rc<dyn Fn(bool)> = Rc::new(move |v| sm.set(v));
    let md = signal(true);
    let on_md: Rc<dyn Fn(bool)> = Rc::new(move |v| md.set(v));
    let lg = signal(false);
    let on_lg: Rc<dyn Fn(bool)> = Rc::new(move |v| lg.set(v));

    let t_primary = signal(true);
    let on_t_primary: Rc<dyn Fn(bool)> = Rc::new(move |v| t_primary.set(v));
    let t_success = signal(true);
    let on_t_success: Rc<dyn Fn(bool)> = Rc::new(move |v| t_success.set(v));
    let t_danger = signal(true);
    let on_t_danger: Rc<dyn Fn(bool)> = Rc::new(move |v| t_danger.set(v));

    let wifi = signal(true);
    let on_wifi: Rc<dyn Fn(bool)> = Rc::new(move |v| wifi.set(v));
    let bluetooth = signal(false);
    let on_bt: Rc<dyn Fn(bool)> = Rc::new(move |v| bluetooth.set(v));

    body(vec![ui! {
        Section(title = "Sizes & states".to_string()) {
            P(content = "A styled slide-toggle (a pressable track + an animated thumb) drawn by \
                the framework rather than the platform-native checkbox. The thumb's travel animates via \
                `AnimProp::TranslateX`, so it looks and moves identically on every \
                backend. `size` scales the track + thumb.".to_string())
            DemoSurface {
                Stack(gap = StackGap::Md) {
                    Switch(label = Some("Small".to_string()), value = sm, on_change = on_sm, size = ControlSize::Sm)
                    Switch(label = Some("Medium".to_string()), value = md, on_change = on_md, size = ControlSize::Md)
                    Switch(label = Some("Large".to_string()), value = lg, on_change = on_lg, size = ControlSize::Lg)
                }
            }
        }
    }, ui! {
        Section(title = "Tones".to_string()) {
            P(content = "The 'on' track takes the tone fill. Override the whole control's \
                appearance globally with \
                `install_switch_sheet(SwitchSheetBuilder::new().add_tone(Hype).build())`.".to_string())
            DemoSurface {
                Stack(gap = StackGap::Md) {
                    Switch(label = Some("Primary".to_string()), value = t_primary, on_change = on_t_primary, tone = tone::Primary)
                    Switch(label = Some("Success".to_string()), value = t_success, on_change = on_t_success, tone = tone::Success)
                    Switch(label = Some("Danger".to_string()), value = t_danger, on_change = on_t_danger, tone = tone::Danger)
                }
            }
        }
    }, ui! {
        Section(title = "In context".to_string()) {
            P(content = "Switch carries an optional inline label rendered to the left of \
                the track — the common settings-row shape.".to_string())
            DemoSurface {
                Stack(gap = StackGap::Md) {
                    Switch(label = Some("Wi-Fi".to_string()), value = wifi, on_change = on_wifi.clone(), tone = tone::Success)
                    Switch(label = Some("Bluetooth".to_string()), value = bluetooth, on_change = on_bt, tone = tone::Primary)
                    Switch(label = Some("Thumb icon".to_string()), value = wifi, on_change = on_wifi.clone(), tone = tone::Success, icon = Some(CHECK))
                }
            }
        }
    }, ui! {
        Section(title = "Props".to_string()) {
            PropsTable(rows = vec![
                Prop { name: "label",     ty: "Reactive<Option<String>>", desc: "Optional label rendered to the left of the toggle." },
                Prop { name: "value",     ty: "Signal<bool>",             desc: "Controlled on/off state. The host owns the signal." },
                Prop { name: "on_change", ty: "Rc<dyn Fn(bool)>",         desc: "Fires with the new value when the user flips the toggle." },
                Prop { name: "tone",      ty: "ToneRef",                  desc: "Semantic palette for the 'on' track fill. Default: Primary." },
                Prop { name: "variant",   ty: "VariantRef",              desc: "Surface skeleton. Default: Filled." },
                Prop { name: "size",      ty: "ControlSize",             desc: "Sm / Md / Lg track + thumb scale. Default: Md." },
            ])
        }
    }])
}

// =============================================================================
// Slider
// =============================================================================

pub fn slider() -> Element {
    let volume = signal(0.5f32);
    let on_volume: Rc<dyn Fn(f32)> = Rc::new(move |v| volume.set(v));

    let t_primary = signal(0.4f32);
    let on_t_primary: Rc<dyn Fn(f32)> = Rc::new(move |v| t_primary.set(v));
    let t_success = signal(0.7f32);
    let on_t_success: Rc<dyn Fn(f32)> = Rc::new(move |v| t_success.set(v));

    let readout = runtime_core::switch(
        move || (volume.get() * 100.0).round() as i32,
        |pct: &i32| {
            let label = format!("Value: {}%", pct);
            ui! { Typography(content = label, muted = true) }
        },
    );

    body(vec![ui! {
        Section(title = "Range".to_string()) {
            P(content = "A muted rail with a tone-colored fill from the left edge to a round \
                thumb; dragging anywhere on the track sets the value. Controlled: the host \
                owns a `Signal<f32>` and applies edits in `on_change`. The track has a \
                fixed px `width` so the drag math (`x / width`) stays stable.".to_string())
            DemoSurface {
                Stack(gap = StackGap::Md) {
                    Slider(value = volume, on_change = on_volume.clone(), tone = tone::Primary)
                    readout
                    Slider(value = volume, on_change = on_volume.clone(), tone = tone::Primary, leading_icon = Some(HEART), trailing_icon = Some(STAR))
                }
            }
            CodePanel(src = r##"let volume = signal(0.5_f32);
let on_volume: Rc<dyn Fn(f32)> = Rc::new(move |v| volume.set(v));

ui! {
    Slider(
        value = volume,
        on_change = on_volume,
        tone = tone::Primary,
    )
}"##.to_string())
        }
    }, ui! {
        Section(title = "Tones".to_string()) {
            P(content = "The fill + thumb take the tone palette. Set `min`/`max` to remap \
                the range and `step` to snap to increments.".to_string())
            DemoSurface {
                Stack(gap = StackGap::Md) {
                    Slider(value = t_primary, on_change = on_t_primary, tone = tone::Primary)
                    Slider(value = t_success, on_change = on_t_success, tone = tone::Success)
                }
            }
        }
    }, ui! {
        Section(title = "Props".to_string()) {
            PropsTable(rows = vec![
                Prop { name: "value",     ty: "Reactive<f32>",     desc: "Current value — a Signal<f32>, a literal, or a model-derived rx!(...)." },
                Prop { name: "on_change", ty: "Rc<dyn Fn(f32)>",   desc: "Fires with the new value while the user drags." },
                Prop { name: "min",       ty: "f32",               desc: "Lower bound. Default: 0.0." },
                Prop { name: "max",       ty: "f32",               desc: "Upper bound. Default: 1.0." },
                Prop { name: "step",      ty: "f32",               desc: "Snap increment. 0.0 (default) is continuous." },
                Prop { name: "width",     ty: "f32",               desc: "Track width in px. Default: 184.0 — fixed so drag math stays stable." },
                Prop { name: "tone",      ty: "ToneRef",           desc: "Semantic palette for the fill + thumb. Default: Primary." },
                Prop { name: "variant",   ty: "VariantRef",       desc: "Surface skeleton. Default: Filled." },
                Prop { name: "size",      ty: "ControlSize",      desc: "Sm / Md / Lg rail thickness + thumb size. Default: Md." },
                Prop { name: "disabled",  ty: "bool",              desc: "When true, blocks dragging and dims the control. Default: false." },
            ])
        }
    }])
}

// =============================================================================
// Field
// =============================================================================

pub fn field() -> Element {
    let email = signal(String::new());
    let on_email: Rc<dyn Fn(String)> = Rc::new(move |s| email.set(s));

    let query = signal(String::new());
    let on_query: Rc<dyn Fn(String)> = Rc::new(move |s| query.set(s));

    // Size demo — one shared value across the three densities.
    let sized = signal(String::new());
    let on_sized: Rc<dyn Fn(String)> = Rc::new(move |s| sized.set(s));

    // Password + visibility toggle. `secure` is now reactive, so there is NO
    // `switch` around the Field: the mask flips in place (on macOS, an
    // in-place secure-cell swap), the input is never rebuilt, and the typed
    // `pw` is never disturbed. Only the tiny eye icon swaps, in its own
    // reactive scope inside the trailing adornment.
    let pw = signal(String::new());
    let visible = signal(false);
    let on_pw: Rc<dyn Fn(String)> = Rc::new(move |s| pw.set(s));
    let pw_field = ui! {
        Field(
            label = Some("Password".to_string()),
            value = pw,
            on_change = on_pw,
            placeholder = Some("••••••••".to_string()),
            secure = rx!(!visible.get()),
            trailing = Adornment::element(move || {
                let glyph = runtime_core::switch(
                    move || visible.get(),
                    move |&shown| ui! {
                        Icon(data = if shown { EYE_OFF } else { EYE }, size = 16.0)
                    },
                );
                pressable(vec![glyph], move || visible.set(!visible.get()))
                    .with_style(crate::styles::IconToggleBtn())
                    .into_element()
            }),
        )
    };

    let validated = signal(String::new());
    let on_validated: Rc<dyn Fn(String)> = Rc::new(move |s| validated.set(s));
    // Live validation: the Field flips to the Danger tone automatically when
    // `error` is Some.
    let error = runtime_core::rx!(if validated.get().is_empty() || validated.get().contains('@') {
        None
    } else {
        Some("Enter a valid email address.".to_string())
    });

    body(vec![ui! {
        Section(title = "Live demo".to_string()) {
            P(content = "Themed text input with a label, helper text, and error tone. Field \
                is controlled — `value: Signal<String>` is the source of truth, read every \
                render, and `on_change: Rc<dyn Fn(String)>` fires on every keystroke.".to_string())
            DemoSurface {
                Field(
                    label = Some("Email".to_string()),
                    value = email,
                    on_change = on_email,
                    placeholder = Some("you@example.com".to_string()),
                    help = Some("We'll never share your email.".to_string()),
                )
            }
            CodePanel(src = r##"let email = signal("".to_string());
let on_email: Rc<dyn Fn(String)> = Rc::new(move |s| email.set(s));

ui! {
    Field(
        label = Some("Email".into()),
        value = email,
        on_change = on_email,
        placeholder = Some("you@example.com".into()),
        help = Some("We'll never share your email.".into()),
    )
}"##.to_string())
        }
    }, ui! {
        Section(title = "Validation".to_string()) {
            P(content = "Wire an error message reactively from your validator. When `error` \
                is Some, the Field flips to the Danger tone automatically — the border and \
                helper text both recolor.".to_string())
            DemoSurface {
                Field(
                    label = Some("Email (validated)".to_string()),
                    value = validated,
                    on_change = on_validated,
                    placeholder = Some("Type something without an @".to_string()),
                    error = error,
                )
            }
            CodePanel(src = r##"let email = signal("".to_string());
let error = rx!(if email.get().contains('@') { None } else { Some("Invalid email".into()) });

ui! {
    Field(
        label = Some("Email".into()),
        value = email,
        on_change = on_email,
        error = error,
    )
}"##.to_string())
        }
    }, ui! {
        Section(title = "Adornments".to_string()) {
            P(content = "Drop an `Adornment` into the `leading` or `trailing` slot — \
                `Adornment::Icon(data)` renders a vector icon (muted, auto-sized to the field) and \
                `Adornment::element(|| ui! { … })` renders any element (a clear button, a unit \
                suffix, a password-visibility toggle). Adornments lay out in a flex row inside the \
                field box, so any width works.".to_string())
            DemoSurface {
                Field(
                    value = query,
                    on_change = on_query,
                    placeholder = Some("Search components…".to_string()),
                    leading = Adornment::Icon(SEARCH),
                )
            }
            CodePanel(src = r##"Field(
    value = query,
    on_change = on_query,
    placeholder = Some("Search components…".into()),
    leading = Adornment::Icon(icons_lucide::SEARCH),
    // trailing = Adornment::element(move || ui! { IconButton(...) }),
)"##.to_string())
            Callout(label = "Focus ring".to_string()) {
                P(content = "Adorned fields don't show the focus ring yet: it's driven by the \
                    input's focus state, which the row wrapper can't receive until a text-input \
                    on_focus event lands. Plain (un-adorned) fields keep their ring.".to_string())
            }
        }
    }, ui! {
        Section(title = "Sizes".to_string()) {
            P(content = "`size = FieldSize::Sm | Md | Lg` scales padding + font; a leading \
                `Adornment::Icon` auto-sizes to match.".to_string())
            // Fields go DIRECTLY in DemoSurface (not a Stack): DemoSurface
            // centers its children (`align_items: center`), which collapses a
            // wrapping Stack to its content width. A Field fills the surface on
            // its own (`align_self: stretch` on FieldGroup) and DemoSurface's
            // own `gap` spaces them.
            DemoSurface {
                Field(value = sized, on_change = on_sized.clone(), placeholder = Some("Small".to_string()), size = FieldSize::Sm, leading = Adornment::Icon(SEARCH))
                Field(value = sized, on_change = on_sized.clone(), placeholder = Some("Medium".to_string()), size = FieldSize::Md, leading = Adornment::Icon(SEARCH))
                Field(value = sized, on_change = on_sized.clone(), placeholder = Some("Large".to_string()), size = FieldSize::Lg, leading = Adornment::Icon(SEARCH))
            }
            CodePanel(src = r##"Field(value = v, on_change = on_v, size = FieldSize::Lg, leading = Adornment::Icon(icons_lucide::SEARCH))"##.to_string())
        }
    }, ui! {
        Section(title = "Password + visibility toggle".to_string()) {
            P(content = "`secure` is a reactive prop, so `secure = rx!(!visible.get())` flips the \
                mask in place — no `switch` around the Field, the input is never rebuilt, and the \
                typed value is never disturbed (on macOS the backend swaps the secure cell in \
                place). Only the tiny eye icon swaps, in its own reactive scope.".to_string())
            DemoSurface {
                pw_field
            }
            CodePanel(src = r##"let pw = signal("".to_string());
let visible = signal(false);

ui! {
    Field(
        value = pw,
        on_change = on_pw,
        // Reactive mask — toggles in place, no Field rebuild.
        secure = rx!(!visible.get()),
        trailing = Adornment::element(move || {
            let glyph = switch(move || visible.get(), move |&shown| ui! {
                Icon(data = if shown { EYE_OFF } else { EYE }, size = 16.0)
            });
            pressable(vec![glyph], move || visible.set(!visible.get())).into_element()
        }),
    )
}"##.to_string())
        }
    }, ui! {
        Section(title = "Props".to_string()) {
            PropsTable(rows = vec![
                Prop { name: "label",       ty: "Reactive<Option<String>>", desc: "Optional label above the input." },
                Prop { name: "value",       ty: "Signal<String>",           desc: "Controlled input value." },
                Prop { name: "on_change",   ty: "Rc<dyn Fn(String)>",        desc: "Fires on every keystroke." },
                Prop { name: "placeholder", ty: "Option<String>",            desc: "Hint text shown when value is empty." },
                Prop { name: "help",        ty: "Reactive<Option<String>>",  desc: "Helper text below the input." },
                Prop { name: "error",       ty: "Reactive<Option<String>>",  desc: "Error text below the input. Takes precedence over help; auto-applies the Danger tone." },
                Prop { name: "tone",        ty: "Option<ToneRef>",           desc: "Optional border + help-text color overlay." },
                Prop { name: "size",        ty: "FieldSize",                 desc: "Sm / Md / Lg density." },
                Prop { name: "variant",     ty: "FieldAppearance",           desc: "Outline (default) / Contained / Bare." },
                Prop { name: "secure",      ty: "bool",                      desc: "Mask the entered text (password entry)." },
                Prop { name: "leading",     ty: "Adornment",                 desc: "Icon/element before the input (Adornment::Icon / ::element). Default None." },
                Prop { name: "trailing",    ty: "Adornment",                 desc: "Icon/element after the input — e.g. a clear button or password-visibility toggle." },
            ])
        }
    }])
}

// =============================================================================
// Textarea
// =============================================================================

pub fn textarea() -> Element {
    let bio = signal(String::new());
    let on_bio: Rc<dyn Fn(String)> = Rc::new(move |s| bio.set(s));

    let note = signal(String::new());
    let on_note: Rc<dyn Fn(String)> = Rc::new(move |s| note.set(s));

    // Live character count helper, derived from the bound value.
    let count = runtime_core::switch(
        move || bio.get().chars().count(),
        |n: &usize| {
            let label = format!("{} characters", n);
            ui! { Typography(content = label, muted = true) }
        },
    );

    body(vec![ui! {
        Section(title = "Multi-line input".to_string()) {
            P(content = "The multi-line sibling of Field: same label / helper / error / \
                tone / size surface, wrapping the framework's `text_area` primitive. It's \
                intrinsically sized to its content, so it grows to fit what's typed. `rows` \
                is the resting floor.".to_string())
            DemoSurface {
                Stack(gap = StackGap::Sm) {
                    Textarea(
                        label = Some("Bio".to_string()),
                        value = bio,
                        on_change = on_bio,
                        placeholder = Some("Tell us about yourself…".to_string()),
                        help = Some("A short blurb for your profile.".to_string()),
                        rows = 4u32,
                    )
                    count
                }
            }
        }
    }, ui! {
        Section(title = "Autogrow with a cap".to_string()) {
            P(content = "Starts at 2 lines, grows as you type, and stops at 8 — past that it \
                scrolls. `max_rows` is the ceiling; `0` (the default) leaves the autogrow \
                uncapped.".to_string())
            DemoSurface {
                Textarea(
                    label = Some("Release notes".to_string()),
                    value = note,
                    on_change = on_note,
                    placeholder = Some("Type a few lines and watch it grow…".to_string()),
                    rows = 2u32,
                    max_rows = 8u32,
                )
            }
        }
    }, ui! {
        Section(title = "Props".to_string()) {
            PropsTable(rows = vec![
                Prop { name: "label",       ty: "Reactive<Option<String>>", desc: "Optional label above the input." },
                Prop { name: "value",       ty: "Signal<String>",           desc: "Controlled input value." },
                Prop { name: "on_change",   ty: "Rc<dyn Fn(String)>",        desc: "Fires on every keystroke." },
                Prop { name: "placeholder", ty: "Option<String>",            desc: "Hint text shown when value is empty." },
                Prop { name: "help",        ty: "Reactive<Option<String>>",  desc: "Helper text below the input." },
                Prop { name: "error",       ty: "Reactive<Option<String>>",  desc: "Error text; takes precedence over help and auto-applies the Danger tone." },
                Prop { name: "tone",        ty: "Option<ToneRef>",           desc: "Optional border + help-text color overlay." },
                Prop { name: "size",        ty: "FieldSize",                 desc: "Sm / Md / Lg density." },
                Prop { name: "variant",     ty: "FieldAppearance",           desc: "Outline (default) / Contained / Bare." },
                Prop { name: "rows",        ty: "u32",                       desc: "Resting height in lines — the floor the box grows from. Default: 3." },
                Prop { name: "max_rows",    ty: "u32",                       desc: "Ceiling in lines before it stops growing and scrolls. 0 (default) = uncapped." },
            ])
        }
    }])
}

// =============================================================================
// Select
// =============================================================================

pub fn select() -> Element {
    let value = signal("pear".to_string());
    let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| value.set(v));

    let current = runtime_core::switch(
        move || value.get(),
        |v: &String| {
            let label = format!("Current value: {}", v);
            ui! { Typography(content = label, muted = true) }
        },
    );

    let preview = ui! {
        Select(
            value = value,
            on_change = on_change,
            options = vec![
                SelectOption::new("apple",  "Apple"),
                SelectOption::new("pear",   "Pear"),
                SelectOption::new("banana", "Banana"),
                SelectOption::new("cherry", "Cherry"),
            ],
            placeholder = Some("Choose a fruit".to_string()),
        )
    };

    let controls = ui! {
        Stack(gap = StackGap::Sm) {
            H3(content = "Notes".to_string())
            P(content = "Click the trigger to open the menu, pick an option, then click \
                outside or press Escape to dismiss.".to_string())
            current
        }
    };

    body(vec![ui! {
        Section(title = "Live demo".to_string()) {
            P(content = "Controlled dropdown. Options are { id, label } pairs; the bound \
                `Signal<String>` holds the active id. The value type is a string so it \
                round-trips through URL params, persisted state, and analytics events \
                without a generic.".to_string())
            Demo(preview = Some(preview), controls = Some(controls))
        }
    }, ui! {
        Section(title = "Props".to_string()) {
            PropsTable(rows = vec![
                Prop { name: "value",       ty: "Signal<String>",     desc: "The active option's id. The host owns the signal." },
                Prop { name: "on_change",   ty: "Rc<dyn Fn(String)>", desc: "Fires when the user picks an option; receives the new id." },
                Prop { name: "options",     ty: "Vec<SelectOption>",  desc: "Options to show. SelectOption::new(id, label)." },
                Prop { name: "size",        ty: "SelectSize",         desc: "Sm / Md / Lg — trigger height." },
                Prop { name: "placeholder", ty: "Option<String>",     desc: "Text shown on the trigger when no option matches the value." },
            ])
        }
    }, ui! {
        Callout(label = "For multi-select".to_string()) {
            P(content = "Select is single-select. For multiple values, compose Tag chips \
                alongside a Field for fast adds.".to_string())
        }
    }])
}

// =============================================================================
// Autocomplete
// =============================================================================

pub fn autocomplete() -> Element {
    let value = signal("pear".to_string());
    let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| value.set(v));

    let current = runtime_core::switch(
        move || value.get(),
        |v: &String| {
            let label = format!("Current value: {}", v);
            ui! { Typography(content = label, muted = true) }
        },
    );

    let preview = ui! {
        Autocomplete(
            value = value,
            on_change = on_change,
            options = vec![
                SelectOption::new("apple",      "Apple"),
                SelectOption::new("apricot",    "Apricot"),
                SelectOption::new("banana",     "Banana"),
                SelectOption::new("blueberry",  "Blueberry"),
                SelectOption::new("cherry",     "Cherry"),
                SelectOption::new("grape",      "Grape"),
                SelectOption::new("kiwi",       "Kiwi"),
                SelectOption::new("mango",      "Mango"),
                SelectOption::new("nectarine",  "Nectarine"),
                SelectOption::new("orange",     "Orange"),
                SelectOption::new("peach",      "Peach"),
                SelectOption::new("pear",       "Pear"),
                SelectOption::new("pineapple",  "Pineapple"),
                SelectOption::new("plum",       "Plum"),
                SelectOption::new("raspberry",  "Raspberry"),
                SelectOption::new("strawberry", "Strawberry"),
            ],
            placeholder = Some("Search fruit…".to_string()),
        )
    };

    let controls = ui! {
        Stack(gap = StackGap::Sm) {
            H3(content = "Notes".to_string())
            P(content = "Focus the input to open the full menu with the committed \
                selection painted solid and the keyboard cursor seeded on it; type to \
                filter. ArrowUp/ArrowDown move the cursor (the subtle highlight), Enter \
                commits it, Escape or focusing elsewhere dismisses and reverts unmatched \
                typing to the committed selection. The menu matches the input's width \
                and interacting with it never blurs the input.".to_string())
            current
        }
    };

    body(vec![ui! {
        Section(title = "Live demo".to_string()) {
            P(content = "A searchable combobox: the constrained-selection sibling of \
                Select. The value committed to the bound `Signal<String>` is always one \
                of the options' ids — never free text. Typing filters on a \
                case-insensitive substring of the label.".to_string())
            Demo(preview = Some(preview), controls = Some(controls))
        }
    }, ui! {
        Section(title = "Props".to_string()) {
            PropsTable(rows = vec![
                Prop { name: "value",       ty: "Signal<String>",     desc: "The committed option's id. The host owns the signal." },
                Prop { name: "on_change",   ty: "Rc<dyn Fn(String)>", desc: "Fires when the user commits a row; receives the option's id." },
                Prop { name: "options",     ty: "Vec<SelectOption>",  desc: "Rows to offer. SelectOption::new(id, label); the query filters by label." },
                Prop { name: "size",        ty: "SelectSize",         desc: "Sm / Md / Lg — input height. Shared with Select." },
                Prop { name: "placeholder", ty: "Option<String>",     desc: "Hint text shown while the input is empty." },
                Prop { name: "empty_text",  ty: "Option<String>",     desc: "Menu row shown when nothing matches. Default: \"No results\"." },
            ])
        }
    }, ui! {
        Callout(label = "Select or Autocomplete?".to_string()) {
            P(content = "Both commit an id from a fixed option list and drop the same \
                menu surface. Reach for Autocomplete when the list is long enough that \
                filtering beats scanning; reach for Select when seeing every option at \
                once is the point.".to_string())
        }
    }])
}

// =============================================================================
// SegmentedControl
// =============================================================================

pub fn segmented_control() -> Element {
    // "With icons" — SegmentOption.label is a string, so prefix glyphs in the
    // label to convey the icon-and-text segmented picker shape.
    let view = signal("list".to_string());
    let on_view: Rc<dyn Fn(String)> = Rc::new(move |v| view.set(v));

    let theme = signal("system".to_string());
    let on_theme: Rc<dyn Fn(String)> = Rc::new(move |v| theme.set(v));

    let current = runtime_core::switch(
        move || view.get(),
        |v: &String| {
            let label = format!("Showing: {}", v);
            ui! { Typography(content = label, muted = true) }
        },
    );

    body(vec![ui! {
        Section(title = "With icons".to_string()) {
            P(content = "A row of mutually-exclusive options — the iOS segmented-picker \
                pattern. Controlled by value: the host owns a `Signal<String>` holding the \
                selected segment's `id`, and the segment whose `id` equals the value paints \
                selected. Build segments with `SegmentOption::new(id, label)`.".to_string())
            DemoSurface {
                Stack(gap = StackGap::Md) {
                    SegmentedControl(
                        value = view,
                        on_change = on_view,
                        options = vec![
                            SegmentOption::new("list", "☰  List"),
                            SegmentOption::new("grid", "▦  Grid"),
                            SegmentOption::new("map", "◎  Map"),
                        ],
                    )
                    current
                }
            }
            CodePanel(src = r##"let view = signal("list".to_string());
let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| view.set(v));

ui! {
    SegmentedControl(
        value = view,
        on_change = on_change,
        options = vec![
            SegmentOption::new("list", "List"),
            SegmentOption::new("grid", "Grid"),
            SegmentOption::new("map",  "Map"),
        ],
    )
}"##.to_string())
        }
    }, ui! {
        Section(title = "Two options".to_string()) {
            P(content = "A two-segment control is the compact alternative to a Switch when \
                both states deserve an explicit label.".to_string())
            DemoSurface {
                SegmentedControl(
                    value = theme,
                    on_change = on_theme,
                    options = vec![
                        SegmentOption::new("system", "System"),
                        SegmentOption::new("light", "Light"),
                        SegmentOption::new("dark", "Dark"),
                    ],
                )
            }
        }
    }, ui! {
        Section(title = "Props".to_string()) {
            PropsTable(rows = vec![
                Prop { name: "value",     ty: "Reactive<String>",   desc: "Selected segment's id — a Signal<String> or a model-derived rx!(...)." },
                Prop { name: "on_change", ty: "Rc<dyn Fn(String)>", desc: "Fires with the chosen segment's id when the user taps a segment." },
                Prop { name: "options",   ty: "Vec<SegmentOption>", desc: "Segments, left-to-right. SegmentOption::new(id, label)." },
            ])
        }
    }])
}

// =============================================================================
// Calendar
// =============================================================================

pub fn calendar() -> Element {
    let picked: Signal<Option<CivilDate>> = signal(None);
    let on_pick: Rc<dyn Fn(CivilDate)> = Rc::new(move |d| picked.set(Some(d)));
    let picked_label = runtime_core::switch(
        move || picked.get(),
        |v: &Option<CivilDate>| {
            let label = match v {
                Some(d) => format!("Picked: {}", format_date(*d, "YYYY-MM-DD")),
                None => "Nothing picked yet.".to_string(),
            };
            ui! { Typography(content = label, muted = true) }
        },
    );

    let single = ui! {
        Calendar(value = picked, on_change = on_pick)
    };
    let single_controls = ui! {
        Stack(gap = StackGap::Sm) {
            H3(content = "Notes".to_string())
            P(content = "Press the month/year title to zoom out to a month grid, and \
                again for a 12-year page — picking steps back down toward days.".to_string())
            picked_label
        }
    };

    let bounded: Signal<Option<CivilDate>> = signal(None);
    let on_bounded: Rc<dyn Fn(CivilDate)> = Rc::new(move |d| bounded.set(Some(d)));
    let weekends: Rc<dyn Fn(CivilDate) -> bool> =
        Rc::new(|d| matches!(d.weekday(), Weekday::Saturday | Weekday::Sunday));

    let range: Signal<Option<(CivilDate, CivilDate)>> = signal(None);
    let on_range: Rc<dyn Fn(CivilDate, CivilDate)> = Rc::new(move |a, b| range.set(Some((a, b))));
    let range_label = runtime_core::switch(
        move || range.get(),
        |v: &Option<(CivilDate, CivilDate)>| {
            let label = match v {
                Some((a, b)) => format!(
                    "Range: {} – {}",
                    format_date(*a, "YYYY-MM-DD"),
                    format_date(*b, "YYYY-MM-DD")
                ),
                None => "Press once for the start, again for the end.".to_string(),
            };
            ui! { Typography(content = label, muted = true) }
        },
    );
    let range_demo = ui! {
        RangeCalendar(value = range, on_change = on_range)
    };
    let range_controls = ui! {
        Stack(gap = StackGap::Sm) {
            H3(content = "Notes".to_string())
            range_label
        }
    };

    body(vec![ui! {
        Section(title = "Inline calendar".to_string()) {
            P(content = "A controlled month grid over a `Signal<Option<CivilDate>>`. \
                `CivilDate` is idea-ui's plain timezone-less calendar value — no chrono \
                dependency; the host owns the signal and writes it from `on_change`.".to_string())
            Demo(preview = Some(single), controls = Some(single_controls))
        }
    }, ui! {
        Section(title = "Bounds and disabled days".to_string()) {
            P(content = "`min`/`max` clamp the pickable window (earlier/later days render \
                blocked), and `is_date_disabled` vetoes individual days — here, weekends. \
                Blocked days dim and take no press handler.".to_string())
            DemoSurface {
                Calendar(
                    value = bounded,
                    on_change = on_bounded,
                    min = CivilDate::today(),
                    is_date_disabled = Some(weekends),
                )
            }
            CodePanel(src = r##"let weekends: Rc<dyn Fn(CivilDate) -> bool> =
    Rc::new(|d| matches!(d.weekday(), Weekday::Saturday | Weekday::Sunday));

ui! {
    Calendar(
        value = date,
        on_change = on_pick,
        min = CivilDate::today(),        // bare dates coerce into Option props
        is_date_disabled = Some(weekends),
    )
}"##.to_string())
        }
    }, ui! {
        Section(title = "Range selection".to_string()) {
            P(content = "`RangeCalendar` binds a `Signal<Option<(CivilDate, CivilDate)>>`. \
                The first press anchors the start, the second commits the pair in \
                chronological order; the interior renders as a soft band.".to_string())
            Demo(preview = Some(range_demo), controls = Some(range_controls))
        }
    }, ui! {
        Section(title = "Props".to_string()) {
            PropsTable(rows = vec![
                Prop { name: "value",            ty: "Signal<Option<CivilDate>>",         desc: "Controlled selection. The host owns the signal." },
                Prop { name: "on_change",        ty: "Rc<dyn Fn(CivilDate)>",             desc: "Fires with the picked day; the host writes the signal." },
                Prop { name: "min / max",        ty: "Option<CivilDate>",                 desc: "Inclusive pickable window. Bare CivilDate values coerce." },
                Prop { name: "is_date_disabled", ty: "Option<Rc<dyn Fn(CivilDate)->bool>>", desc: "Per-day veto — blocked days dim and take no press." },
                Prop { name: "first_weekday",    ty: "Weekday",                           desc: "First grid column. Default Monday (ISO)." },
                Prop { name: "labels",           ty: "Option<Rc<DateLabels>>",            desc: "Month/weekday display names. Default English." },
                Prop { name: "framed",           ty: "bool",                              desc: "Draw the panel border. Off inside picker popups." },
            ])
        }
    }])
}

// =============================================================================
// DatePicker
// =============================================================================

pub fn date_picker() -> Element {
    let due: Signal<Option<CivilDate>> = signal(None);
    let on_due: Rc<dyn Fn(Option<CivilDate>)> = Rc::new(move |d| due.set(d));
    let due_label = runtime_core::switch(
        move || due.get(),
        |v: &Option<CivilDate>| {
            let label = match v {
                Some(d) => format!("Due: {}", format_date(*d, "YYYY-MM-DD")),
                None => "No deadline set.".to_string(),
            };
            ui! { Typography(content = label, muted = true) }
        },
    );
    let single = ui! {
        DatePicker(
            value = due,
            on_change = on_due,
            min = CivilDate::today(),
            placeholder = Some("Pick a deadline".to_string()),
            clearable = true,
        )
    };
    let single_controls = ui! {
        Stack(gap = StackGap::Sm) {
            H3(content = "Notes".to_string())
            P(content = "Picking a day commits and closes; `Clear` commits `None`. \
                Outside click or Escape dismisses without committing.".to_string())
            due_label
        }
    };

    let stay: Signal<Option<(CivilDate, CivilDate)>> = signal(None);
    let on_stay: Rc<dyn Fn(Option<(CivilDate, CivilDate)>)> = Rc::new(move |r| stay.set(r));

    let meet: Signal<Option<CivilDateTime>> = signal(None);
    let on_meet: Rc<dyn Fn(Option<CivilDateTime>)> = Rc::new(move |v| meet.set(v));
    let meet_label = runtime_core::switch(
        move || meet.get(),
        |v: &Option<CivilDateTime>| {
            let label = match v {
                Some(dt) => format!("Meeting: {}", format_datetime(*dt, "YYYY-MM-DD HH:mm")),
                None => "No meeting scheduled.".to_string(),
            };
            ui! { Typography(content = label, muted = true) }
        },
    );
    let datetime = ui! {
        DateTimePicker(
            value = meet,
            on_change = on_meet,
            display_format = "YYYY-MM-DD h:mm A".to_string(),
            time_format = "h:mm A".to_string(),
            placeholder = Some("Schedule a meeting".to_string()),
        )
    };
    let datetime_controls = ui! {
        Stack(gap = StackGap::Sm) {
            H3(content = "Notes".to_string())
            P(content = "The panel stays open across picks so date and time can both be \
                set; the trigger renders a 12-hour clock via the `h:mm A` tokens.".to_string())
            meet_label
        }
    };

    body(vec![ui! {
        Section(title = "Single date".to_string()) {
            P(content = "A Select-style trigger opening the Calendar in an anchored popup. \
                Controlled by a `Signal<Option<CivilDate>>`; `on_change` fires `Some(day)` \
                on pick and `None` from the Clear action.".to_string())
            Demo(preview = Some(single), controls = Some(single_controls))
        }
    }, ui! {
        Section(title = "Date range".to_string()) {
            P(content = "`DateRangePicker` binds `Signal<Option<(CivilDate, CivilDate)>>` — \
                first press anchors the start, the second commits the ordered pair and \
                closes. The trigger renders `start – end`.".to_string())
            DemoSurface {
                DateRangePicker(
                    value = stay,
                    on_change = on_stay,
                    placeholder = Some("Report period".to_string()),
                    clearable = true,
                )
            }
        }
    }, ui! {
        Section(title = "Date and time".to_string()) {
            P(content = "`DateTimePicker` adds a time row under the calendar and binds \
                `Signal<Option<CivilDateTime>>`. `default_time` seeds the time when a day \
                is picked before any time is set.".to_string())
            Demo(preview = Some(datetime), controls = Some(datetime_controls))
        }
    }, ui! {
        Section(title = "Props".to_string()) {
            PropsTable(rows = vec![
                Prop { name: "value",          ty: "Signal<Option<CivilDate>>",       desc: "Controlled value. The host owns the signal." },
                Prop { name: "on_change",      ty: "Rc<dyn Fn(Option<CivilDate>)>",   desc: "Some(day) on pick; None on an explicit clear." },
                Prop { name: "placeholder",    ty: "Option<String>",                  desc: "Trigger text when no value is set." },
                Prop { name: "display_format", ty: "String",                          desc: "Token format for the trigger text. Default YYYY-MM-DD." },
                Prop { name: "min / max",      ty: "Option<CivilDate>",               desc: "Inclusive pickable window in the popup." },
                Prop { name: "size",           ty: "SelectSize",                      desc: "Trigger height — shared scale with Select. Default Md." },
                Prop { name: "clearable",      ty: "bool",                            desc: "Offer a Clear footer action that commits None." },
            ])
        }
    }])
}

// =============================================================================
// DateInput
// =============================================================================

pub fn date_input() -> Element {
    let birthday: Signal<Option<CivilDate>> = signal(None);
    let on_birthday: Rc<dyn Fn(Option<CivilDate>)> = Rc::new(move |d| birthday.set(d));
    let birthday_label = runtime_core::switch(
        move || birthday.get(),
        |v: &Option<CivilDate>| {
            let label = match v {
                Some(d) => format!("Committed: {}", format_date(*d, "YYYY-MM-DD")),
                None => "Nothing committed yet.".to_string(),
            };
            ui! { Typography(content = label, muted = true) }
        },
    );
    let typed = ui! {
        DateInput(
            value = birthday,
            on_change = on_birthday,
            label = Some("Date of birth".to_string()),
            format = "D/M/YYYY".to_string(),
            help = Some("Just type 731994 — the delimiters insert themselves.".to_string()),
            max = CivilDate::today(),
        )
    };
    let typed_controls = ui! {
        Stack(gap = StackGap::Sm) {
            H3(content = "Notes".to_string())
            P(content = "Smart typing: digits auto-insert the format's delimiters and jump \
                to the next segment as soon as they're unambiguous (day 7 can't extend, so \
                7 becomes 7/ on its own). Tab commits an ambiguous partial segment in \
                place — 1 + Tab reads it as month 1 and moves on — while a partial no \
                value can commit (a bare 0) swallows the Tab. The trailing calendar \
                button opens the same popup Calendar as DatePicker.".to_string())
            birthday_label
        }
    };

    let appointment: Signal<Option<CivilDateTime>> = signal(None);
    let on_appointment: Rc<dyn Fn(Option<CivilDateTime>)> = Rc::new(move |v| appointment.set(v));

    body(vec![ui! {
        Section(title = "Typed input with popup".to_string()) {
            P(content = "A Field that parses typed text against a token format (`YYYY MM M \
                DD D`, any other character a literal) and shows an inline error while the \
                text doesn't parse. `min`/`max` gate the POPUP only — typed text is \
                validated for shape, not range; wire the `error` prop for range \
                feedback.".to_string())
            Demo(preview = Some(typed), controls = Some(typed_controls))
        }
    }, ui! {
        Section(title = "Date and time".to_string()) {
            P(content = "`DateTimeInput` parses a combined format — date tokens plus time \
                tokens in one string — and its popup adds the time row.".to_string())
            DemoSurface {
                DateTimeInput(
                    value = appointment,
                    on_change = on_appointment,
                    label = Some("Appointment".to_string()),
                    format = "YYYY-MM-DD HH:mm".to_string(),
                )
            }
        }
    }, ui! {
        Section(title = "Props".to_string()) {
            PropsTable(rows = vec![
                Prop { name: "value",           ty: "Signal<Option<CivilDate>>",     desc: "Controlled value. The host owns the signal." },
                Prop { name: "on_change",       ty: "Rc<dyn Fn(Option<CivilDate>)>", desc: "Some(date) on each valid commit; None when emptied." },
                Prop { name: "format",          ty: "String",                        desc: "Token format for parsing AND display. Default YYYY-MM-DD." },
                Prop { name: "label / help",    ty: "Option<String>",                desc: "Field label and helper text (see Field)." },
                Prop { name: "error",           ty: "Option<String>",                desc: "Host-side error; a parse error takes precedence while present." },
                Prop { name: "invalid_message", ty: "String",                        desc: "Error shown while the text doesn't parse. Default \"Invalid date\"." },
                Prop { name: "picker",          ty: "bool",                          desc: "Show the trailing calendar button + popup. Default true." },
                Prop { name: "min / max",       ty: "Option<CivilDate>",             desc: "Popup-only bounds — typed text is shape-validated, not range." },
            ])
        }
    }])
}

// =============================================================================
// TimeInput
// =============================================================================

pub fn time_input() -> Element {
    let alarm: Signal<Option<CivilTime>> = signal(None);
    let on_alarm: Rc<dyn Fn(Option<CivilTime>)> = Rc::new(move |t| alarm.set(t));
    let alarm_label = runtime_core::switch(
        move || alarm.get(),
        |v: &Option<CivilTime>| {
            let label = match v {
                Some(t) => format!("Committed: {}", format_time(*t, "HH:mm")),
                None => "Nothing committed yet.".to_string(),
            };
            ui! { Typography(content = label, muted = true) }
        },
    );
    let twenty_four = ui! {
        TimeInput(
            value = alarm,
            on_change = on_alarm,
            label = Some("Alarm".to_string()),
        )
    };
    let twenty_four_controls = ui! {
        Stack(gap = StackGap::Sm) {
            H3(content = "Notes".to_string())
            P(content = "Type 730 — hour 7 can't extend past 23, so the colon inserts \
                itself and the hour pads to 07:. Ambiguous starts (0, 1, 2) wait for a \
                second digit; Tab commits them in place.".to_string())
            alarm_label
        }
    };

    let checkin: Signal<Option<CivilTime>> = signal(None);
    let on_checkin: Rc<dyn Fn(Option<CivilTime>)> = Rc::new(move |t| checkin.set(t));

    body(vec![ui! {
        Section(title = "24-hour".to_string()) {
            P(content = "A typed Field bound to `Signal<Option<CivilTime>>` — a plain \
                timezone-less time of day. The default `HH:mm` format parses and renders \
                a 24-hour clock; the leading clock glyph is built in.".to_string())
            Demo(preview = Some(twenty_four), controls = Some(twenty_four_controls))
        }
    }, ui! {
        Section(title = "12-hour".to_string()) {
            P(content = "Pass `h:mm A` for a 12-hour clock with meridiem — typing `730p` \
                completes to `7:30 PM` (a single a/p finishes the meridiem) and parses \
                to 19:30. `12 am` maps to midnight and `12 pm` to noon.".to_string())
            DemoSurface {
                TimeInput(
                    value = checkin,
                    on_change = on_checkin,
                    label = Some("Check-in".to_string()),
                    format = "h:mm A".to_string(),
                )
            }
            CodePanel(src = r##"let checkin = signal(None);
let on_checkin: Rc<dyn Fn(Option<CivilTime>)> = Rc::new(move |t| checkin.set(t));

ui! {
    TimeInput(
        value = checkin,
        on_change = on_checkin,
        format = "h:mm A".to_string(),
    )
}"##.to_string())
        }
    }, ui! {
        Section(title = "Props".to_string()) {
            PropsTable(rows = vec![
                Prop { name: "value",           ty: "Signal<Option<CivilTime>>",     desc: "Controlled value. The host owns the signal." },
                Prop { name: "on_change",       ty: "Rc<dyn Fn(Option<CivilTime>)>", desc: "Some(time) on each valid parse; None when emptied." },
                Prop { name: "format",          ty: "String",                        desc: "Token format for parsing AND display. Default HH:mm." },
                Prop { name: "label / help",    ty: "Option<String>",                desc: "Field label and helper text (see Field)." },
                Prop { name: "invalid_message", ty: "String",                        desc: "Error shown while the text doesn't parse. Default \"Invalid time\"." },
                Prop { name: "icon",            ty: "bool",                          desc: "Show the leading clock glyph. Default true." },
            ])
        }
    }])
}
