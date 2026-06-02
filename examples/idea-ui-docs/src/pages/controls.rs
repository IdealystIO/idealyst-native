//! New components — selection controls + navigation/data, on one
//! gallery page each.

use std::rc::Rc;

use runtime_core::{signal, ui, Element};
use idea_ui::{
    tone, Breadcrumbs, Card, Checkbox, Crumb, FieldAppearance, Grid, Image, List, ListItem,
    Pagination, Progress, RadioGroup, RadioOption, Stack, StackGap, Switch, Link, Textarea,
    Typography,
};

use crate::shell::{self, CodePanel, ComponentPage, DemoSurface, P, Prop, PropsTable, Section};

// =============================================================================
// Selection controls — Switch / Checkbox / Radio / Textarea / Progress
// =============================================================================

pub fn controls() -> Element {
    let wifi = signal!(true);
    let on_wifi: Rc<dyn Fn(bool)> = Rc::new(move |v| wifi.set(v));
    let bluetooth = signal!(false);
    let on_bt: Rc<dyn Fn(bool)> = Rc::new(move |v| bluetooth.set(v));

    let agree = signal!(false);
    let on_agree: Rc<dyn Fn(bool)> = Rc::new(move |v| agree.set(v));
    let subscribe = signal!(true);
    let on_sub: Rc<dyn Fn(bool)> = Rc::new(move |v| subscribe.set(v));

    let plan = signal!("pro".to_string());
    let on_plan: Rc<dyn Fn(String)> = Rc::new(move |id| plan.set(id));

    let bio = signal!(String::new());
    let on_bio: Rc<dyn Fn(String)> = Rc::new(move |s| bio.set(s));

    let note = signal!(String::new());
    let on_note: Rc<dyn Fn(String)> = Rc::new(move |s| note.set(s));

    let v_outline = signal!(String::new());
    let on_v_outline: Rc<dyn Fn(String)> = Rc::new(move |s| v_outline.set(s));
    let v_contained = signal!(String::new());
    let on_v_contained: Rc<dyn Fn(String)> = Rc::new(move |s| v_contained.set(s));
    let v_bare = signal!(String::new());
    let on_v_bare: Rc<dyn Fn(String)> = Rc::new(move |s| v_bare.set(s));

    shell::layout(ui! {
        ComponentPage(
            title = "Selection controls".to_string(),
            lead = "Switch, Checkbox, Radio, Textarea, and Progress — all drawn from \
                primitives so they share the tone × variant × size styling axes and look \
                identical on every backend.".to_string(),
        ) {
            Section(title = "Switch".to_string()) {
                P(content = "A styled slide-toggle (track + animated thumb), not the \
                    platform-native checkbox. The 'on' track takes the tone fill.".to_string())
                DemoSurface {
                    Stack(gap = StackGap::Md) {
                        Switch(label = Some("Wi-Fi".to_string()), value = wifi, on_change = on_wifi, tone = tone::Success)
                        Switch(label = Some("Bluetooth".to_string()), value = bluetooth, on_change = on_bt, tone = tone::Primary)
                    }
                }
            }

            Section(title = "Checkbox".to_string()) {
                DemoSurface {
                    Stack(gap = StackGap::Sm) {
                        Checkbox(label = Some("I agree to the terms".to_string()), value = agree, on_change = on_agree, tone = tone::Primary)
                        Checkbox(label = Some("Subscribe to the newsletter".to_string()), value = subscribe, on_change = on_sub, tone = tone::Success)
                    }
                }
            }

            Section(title = "Radio group".to_string()) {
                DemoSurface {
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
                }
            }

            Section(title = "Textarea".to_string()) {
                P(content = "Multi-line input that wraps long lines and grows to fit \
                    its content — it's sized to the text, the same way the text primitive \
                    is. `rows` sets the resting floor.".to_string())
                DemoSurface {
                    Textarea(
                        label = Some("Bio".to_string()),
                        value = bio,
                        on_change = on_bio,
                        placeholder = Some("Tell us about yourself…".to_string()),
                        rows = 4u32,
                    )
                }
            }

            Section(title = "Textarea — autogrow with a cap".to_string()) {
                P(content = "Starts at 2 lines, grows as you type, and stops at 8 lines \
                    — past that it scrolls. `max_rows` is the ceiling.".to_string())
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

            Section(title = "Textarea — variants".to_string()) {
                P(content = "`outline` (bordered, default), `contained` (filled), and \
                    `bare` (no chrome). All three show a focus ring when active. The same \
                    `variant` prop applies to Field.".to_string())
                DemoSurface {
                    Stack(gap = StackGap::Md) {
                        Textarea(
                            label = Some("Outline".to_string()),
                            value = v_outline,
                            on_change = on_v_outline,
                            placeholder = Some("Bordered surface".to_string()),
                            variant = FieldAppearance::Outline,
                            rows = 2u32,
                        )
                        Textarea(
                            label = Some("Contained".to_string()),
                            value = v_contained,
                            on_change = on_v_contained,
                            placeholder = Some("Filled, borderless".to_string()),
                            variant = FieldAppearance::Contained,
                            rows = 2u32,
                        )
                        Textarea(
                            label = Some("Bare".to_string()),
                            value = v_bare,
                            on_change = on_v_bare,
                            placeholder = Some("No chrome".to_string()),
                            variant = FieldAppearance::Bare,
                            rows = 2u32,
                        )
                    }
                }
            }

            Section(title = "Progress".to_string()) {
                DemoSurface {
                    Stack(gap = StackGap::Md) {
                        Progress(value = 0.65f32, tone = tone::Primary)
                        Progress(value = 0.3f32, tone = tone::Success)
                        Progress(indeterminate = true, tone = tone::Info)
                    }
                }
            }

            Section(title = "Switch — props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "label",     ty: "Reactive<Option<String>>", desc: "Optional label rendered to the left of the toggle." },
                    Prop { name: "value",     ty: "Signal<bool>",             desc: "Controlled on/off state. The host owns the signal." },
                    Prop { name: "on_change", ty: "Rc<dyn Fn(bool)>",         desc: "Fires with the new value when the user flips the toggle." },
                    Prop { name: "tone",      ty: "ToneRef",                  desc: "Semantic palette for the 'on' track fill. Default: Primary." },
                    Prop { name: "variant",   ty: "VariantRef",              desc: "Surface skeleton. Default: Filled." },
                    Prop { name: "size",      ty: "ControlSize",             desc: "Sm / Md / Lg track + thumb scale. Default: Md." },
                ])
            }

            Section(title = "Checkbox — props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "label",     ty: "Reactive<Option<String>>", desc: "Optional label rendered to the right of the box." },
                    Prop { name: "value",     ty: "Signal<bool>",             desc: "Controlled checked state. The host owns the signal." },
                    Prop { name: "on_change", ty: "Rc<dyn Fn(bool)>",         desc: "Fires with the new value when the user toggles the box." },
                    Prop { name: "tone",      ty: "ToneRef",                  desc: "Semantic palette for the checked fill. Default: Primary." },
                    Prop { name: "variant",   ty: "VariantRef",              desc: "Surface skeleton for the checked fill. Default: Filled." },
                    Prop { name: "size",      ty: "ControlSize",             desc: "Sm / Md / Lg box scale. Default: Md." },
                ])
                CodePanel(src = r##"let agree = signal!(false);
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

            Section(title = "Radio & RadioGroup — props".to_string()) {
                P(content = "`RadioGroup` coordinates exclusivity across its options and is \
                    what you reach for most of the time. A standalone `Radio` is the \
                    single-row primitive it's built from — use it directly only when you're \
                    laying out the rows yourself.".to_string())
                PropsTable(rows = vec![
                    Prop { name: "value",     ty: "Signal<String>",       desc: "RadioGroup: the selected option's id. The host owns the signal." },
                    Prop { name: "on_change", ty: "Rc<dyn Fn(String)>",   desc: "RadioGroup: fires with the picked id when the user selects an option." },
                    Prop { name: "options",   ty: "Vec<RadioOption>",     desc: "RadioGroup: options in render order. RadioOption::new(id, label)." },
                    Prop { name: "axis",      ty: "RadioAxis",            desc: "RadioGroup: Column (default) or Row layout." },
                    Prop { name: "tone",      ty: "ToneRef",              desc: "Semantic palette for the selected ring + dot. Default: Primary." },
                    Prop { name: "variant",   ty: "VariantRef",          desc: "Surface skeleton. Default: Filled." },
                    Prop { name: "size",      ty: "ControlSize",         desc: "Sm / Md / Lg indicator scale. Default: Md." },
                    Prop { name: "selected",  ty: "Signal<bool>",        desc: "Standalone Radio only: whether this row is selected." },
                    Prop { name: "on_select", ty: "Rc<dyn Fn()>",        desc: "Standalone Radio only: fires when the row is clicked. The host coordinates exclusivity." },
                ])
            }

            Section(title = "Textarea — props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "label",       ty: "Reactive<Option<String>>", desc: "Optional label above the input." },
                    Prop { name: "value",       ty: "Signal<String>",           desc: "Controlled input value." },
                    Prop { name: "on_change",   ty: "Rc<dyn Fn(String)>",        desc: "Fires on every keystroke." },
                    Prop { name: "placeholder", ty: "Option<String>",            desc: "Hint text shown when value is empty." },
                    Prop { name: "help",        ty: "Reactive<Option<String>>",  desc: "Helper text below the input." },
                    Prop { name: "error",       ty: "Reactive<Option<String>>",  desc: "Error text; takes precedence over help and auto-applies the Danger tone." },
                    Prop { name: "tone",        ty: "Option<ToneRef>",           desc: "Optional border + help-text color overlay." },
                    Prop { name: "size",        ty: "FieldSize",                 desc: "Sm / Md / Lg." },
                    Prop { name: "variant",     ty: "FieldAppearance",           desc: "Outline (default) / Contained / Bare." },
                    Prop { name: "rows",        ty: "u32",                       desc: "Resting height in lines — the floor the box grows from. Default: 3." },
                    Prop { name: "max_rows",    ty: "u32",                       desc: "Ceiling in lines before it stops growing and scrolls. 0 (default) = uncapped." },
                ])
            }

            Section(title = "Progress — props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "value",         ty: "Reactive<f32>",  desc: "Completion in 0.0..=1.0. Ignored when indeterminate." },
                    Prop { name: "indeterminate", ty: "bool",           desc: "When true, ignore value and show a pulsing indeterminate bar." },
                    Prop { name: "tone",          ty: "ToneRef",        desc: "Semantic palette for the fill. Default: Primary." },
                    Prop { name: "variant",       ty: "VariantRef",     desc: "Surface skeleton for the fill. Default: Filled." },
                    Prop { name: "size",          ty: "ControlSize",    desc: "Sm / Md / Lg bar thickness. Default: Md." },
                ])
                CodePanel(src = r##"// Live determinate bar driven by a signal
let pct = signal!(0.0f32);
ui! { Progress(value = pct, tone = tone::Primary) }

// Indeterminate (loading, unknown duration)
ui! { Progress(indeterminate = true, tone = tone::Info) }"##.to_string())
            }
        }
    })
}

// =============================================================================
// Navigation & data — Breadcrumbs / Pagination / List / Grid / Image / Link
// =============================================================================

pub fn data() -> Element {
    let noop: Rc<dyn Fn()> = Rc::new(|| {});
    let page = signal!(3usize);
    let on_page: Rc<dyn Fn(usize)> = Rc::new(move |p| page.set(p));

    shell::layout(ui! {
        ComponentPage(
            title = "Navigation & data".to_string(),
            lead = "Breadcrumbs, Pagination, List, Grid, Image, and Link.".to_string(),
        ) {
            Section(title = "Breadcrumbs".to_string()) {
                DemoSurface {
                    Breadcrumbs(items = vec![
                        Crumb::linked("Home", noop.clone()),
                        Crumb::linked("Library", noop.clone()),
                        Crumb::new("Settings"),
                    ])
                }
            }

            Section(title = "Pagination".to_string()) {
                DemoSurface {
                    Pagination(page = page, total = 20usize, on_change = on_page)
                }
            }

            Section(title = "List".to_string()) {
                DemoSurface {
                    List {
                        ListItem(label = "Profile", on_press = Some(noop.clone()))
                        ListItem(label = "Billing", on_press = Some(noop.clone()))
                        ListItem(label = "Sign out", on_press = Some(noop.clone()))
                    }
                }
            }

            Section(title = "Grid".to_string()) {
                DemoSurface {
                    Grid(columns = 3u32, gap = StackGap::Md) {
                        Card { Typography(content = "One".to_string()) }
                        Card { Typography(content = "Two".to_string()) }
                        Card { Typography(content = "Three".to_string()) }
                        Card { Typography(content = "Four".to_string()) }
                        Card { Typography(content = "Five".to_string()) }
                    }
                }
            }

            Section(title = "Link".to_string()) {
                DemoSurface {
                    Link(label = "idealyst on GitHub", url = "https://github.com")
                }
            }

            Section(title = "Image".to_string()) {
                DemoSurface {
                    Image(
                        src = "https://picsum.photos/96",
                        alt = Some("Random sample".to_string()),
                        width = Some(96.0f32),
                        height = Some(96.0f32),
                        rounded = true,
                    )
                }
            }

            Section(title = "Breadcrumbs — props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "items",     ty: "Vec<Crumb>",  desc: "Trail in order. Crumb::new(label) is plain; Crumb::linked(label, on_press) is clickable." },
                    Prop { name: "separator", ty: "String",      desc: "Glyph drawn between crumbs. Default: \"/\"." },
                ])
                CodePanel(src = r##"Breadcrumbs(items = vec![
    Crumb::linked("Home", go_home),
    Crumb::linked("Library", go_library),
    Crumb::new("Settings"), // current page — not linked
])"##.to_string())
            }

            Section(title = "Pagination — props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "page",      ty: "Signal<usize>",        desc: "Current page, 1-based. The host owns the signal." },
                    Prop { name: "total",     ty: "usize",                desc: "Total number of pages (>= 1)." },
                    Prop { name: "on_change", ty: "Rc<dyn Fn(usize)>",    desc: "Fires with the requested page when the user navigates." },
                ])
            }

            Section(title = "List & ListItem — props".to_string()) {
                P(content = "`List` is the container; each row is a `ListItem`. A row is \
                    clickable only when `on_press` is Some.".to_string())
                PropsTable(rows = vec![
                    Prop { name: "label",    ty: "Reactive<String>",     desc: "ListItem: row label. Static or live." },
                    Prop { name: "on_press", ty: "Option<Rc<dyn Fn()>>", desc: "ListItem: when Some, the row is clickable (hover highlight + handler)." },
                    Prop { name: "leading",  ty: "Option<Element>",      desc: "ListItem: optional leading element (icon, avatar)." },
                    Prop { name: "trailing", ty: "Option<Element>",      desc: "ListItem: optional trailing element (badge, chevron), pushed right." },
                    Prop { name: "active",   ty: "bool",                 desc: "ListItem: render the row in its highlighted/selected state." },
                ])
            }

            Section(title = "Grid — props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "columns",  ty: "u32",      desc: "Number of columns (>= 1). Default: 2." },
                    Prop { name: "gap",      ty: "StackGap", desc: "Gap between rows and columns. Default: Md." },
                    Prop { name: "children", ty: "Vec<Element>", desc: "Cells laid out left-to-right, wrapping into rows." },
                ])
            }

            Section(title = "Link — props".to_string()) {
                P(content = "An external hyperlink (opens a URL). For in-app navigation \
                    between routes, use the framework's `link` primitive with a `route` \
                    instead.".to_string())
                PropsTable(rows = vec![
                    Prop { name: "label", ty: "Reactive<String>", desc: "Link text. Static or live." },
                    Prop { name: "url",   ty: "String",           desc: "Destination URL (https:, mailto:, tel:, …)." },
                ])
            }

            Section(title = "Image — props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "src",     ty: "String",        desc: "Image source URL or asset path." },
                    Prop { name: "alt",     ty: "Option<String>", desc: "Accessible description. Maps to alt on web." },
                    Prop { name: "width",   ty: "Option<f32>",   desc: "Explicit width in px. None = natural / flex-sized." },
                    Prop { name: "height",  ty: "Option<f32>",   desc: "Explicit height in px." },
                    Prop { name: "rounded", ty: "bool",          desc: "Clip to a circle (pill radius)." },
                ])
            }
        }
    })
}
