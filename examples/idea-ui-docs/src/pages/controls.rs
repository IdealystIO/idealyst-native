//! New components — selection controls + navigation/data, on one
//! gallery page each.

use std::rc::Rc;

use runtime_core::{signal, ui, Element};
use idea_ui::{
    tone, Breadcrumbs, Card, Checkbox, Crumb, Grid, ImageView, List, ListItem, Pagination, Progress,
    RadioGroup, RadioOption, Stack, StackGap, Switch, TextLink, Textarea, Typography,
};

use crate::shell::{self, ComponentPage, DemoSurface, P, Section};

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

            Section(title = "Progress".to_string()) {
                DemoSurface {
                    Stack(gap = StackGap::Md) {
                        Progress(value = 0.65f32, tone = tone::Primary)
                        Progress(value = 0.3f32, tone = tone::Success)
                        Progress(indeterminate = true, tone = tone::Info)
                    }
                }
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
                    TextLink(label = "idealyst on GitHub", url = "https://github.com")
                }
            }

            Section(title = "Image".to_string()) {
                DemoSurface {
                    ImageView(
                        src = "https://picsum.photos/96",
                        alt = Some("Random sample".to_string()),
                        width = Some(96.0f32),
                        height = Some(96.0f32),
                        rounded = true,
                    )
                }
            }
        }
    })
}
