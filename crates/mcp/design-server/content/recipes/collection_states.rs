//! # Collection screen: loading, error, empty and no-results states
//!
//! summary: A resource-backed list screen that designs all five states — skeleton while loading, inline error with retry, first-run empty with a call to action, no-results for a search, and the data.
//! tags: states, loading, empty, error, resource, search, list
//! render: / @phone => Customers, Northwind, Contoso, !No customers yet
//! render: / @desktop => Customers, Northwind, Enterprise, Contoso
//! render: /empty @phone => No customers yet, Add customer, !Northwind
//! render: /error @desktop => Couldn't load customers, Try again, !Northwind
//!
//! A screen that shows data has five states, and each is designed — not
//! left to whatever an empty `Vec` happens to render:
//!
//! | State | Show | Never |
//! | --- | --- | --- |
//! | loading | skeleton rows in the data's shape | a blank page, or a spinner over the whole screen |
//! | error | an inline `Alert` with what failed and a retry | a toast (it vanishes before it's read) |
//! | empty (first run) | what this is for + the action that fills it | "No data" |
//! | no results (filtered) | which filter matched nothing + how to clear it | the first-run empty state |
//! | data | the list | — |
//!
//! The page header, actions and search field stay put across every state —
//! only the region below them changes — so nothing jumps when data lands.
//!
//! The `/empty` and `/error` routes exist only so this demo can show those
//! states; in an app the one screen moves between them on its own. (The
//! skeleton arm is what shows until the fetch settles in a live client;
//! a server render waits for the fetch, so it never paints it.)
//!
//! Depends on the `page_layout` recipe (`crate::page_layout`).

use crate::page_layout::{Page, PageWidth};
use idea_ui::{
    tone, typography_kind, variant, Alert, Button, Card, CardPadding, Center, Field, List,
    ListItem, Skeleton, SkeletonWidth, Stack, StackAlign, StackGap, Typography,
};
use runtime_core::{
    resource, signal, ui, Element, NetworkState, Ref, Route, Screen, Signal,
};
use std::rc::Rc;
use swap_navigator::{SwapBuilder, SwapHandle, SwapNavigator};

#[derive(Clone, PartialEq)]
struct Customer {
    name: &'static str,
    plan: &'static str,
}

/// Which backend the demo pretends to talk to.
#[derive(Clone, Copy, PartialEq)]
enum Backend {
    Customers,
    Empty,
    Failing,
}

const ALL: &[Customer] = &[
    Customer { name: "Northwind", plan: "Enterprise" },
    Customer { name: "Contoso", plan: "Team" },
    Customer { name: "Fabrikam", plan: "Starter" },
];

/// Stand-in for a network call.
async fn fetch_customers(backend: Backend, query: String) -> Result<Vec<Customer>, String> {
    match backend {
        Backend::Customers => Ok(ALL
            .iter()
            .filter(|c| c.name.to_lowercase().contains(&query.to_lowercase()))
            .cloned()
            .collect()),
        Backend::Empty => Ok(Vec::new()),
        Backend::Failing => Err("The server didn't respond.".into()),
    }
}

pub fn app() -> Element {
    idea_ui::install_idea_theme(idea_ui::light_theme());
    const HOME: Route<()> = Route::<()>::new("customers", "/");
    const EMPTY: Route<()> = Route::<()>::new("empty", "/empty");
    const ERROR: Route<()> = Route::<()>::new("error", "/error");
    let nav: Ref<SwapHandle> = Ref::new();
    let builder = SwapNavigator::new(&HOME)
        .screen(HOME, |_| Screen::new(ui! { CustomersScreen(backend = Backend::Customers) }))
        .screen(EMPTY, |_| Screen::new(ui! { CustomersScreen(backend = Backend::Empty) }))
        .screen(ERROR, |_| Screen::new(ui! { CustomersScreen(backend = Backend::Failing) }));
    ui! { builder.bind(nav) }
}

#[runtime_core::component]
fn CustomersScreen(#[prop(static, default = Backend::Customers)] backend: Backend) -> Element {
    let query: Signal<String> = signal(String::new());
    // Refetches whenever `query` changes; the previous request is cancelled.
    let customers = resource(query, move |q, _cancel| fetch_customers(backend, q));

    let on_query: Rc<dyn Fn(String)> = Rc::new(move |v| query.set(v));
    let add: Rc<dyn Fn()> = Rc::new(|| {});
    let add_empty = add.clone();
    let retry: Rc<dyn Fn()> = Rc::new(move || customers.refetch());
    let clear: Rc<dyn Fn()> = Rc::new(move || query.set(String::new()));

    ui! {
        Page(
            title = "Customers",
            width = PageWidth::Narrow,
            actions = Some(ui! { Button(label = "Add customer", on_click = add) }),
        ) {
            // A search field is the one field with no visible label — its
            // placeholder and position say what it is — but it still names
            // itself for screen readers.
            Field(
                value = query,
                on_change = on_query,
                placeholder = Some("Search customers".to_string()),
                accessibility_label = Some("Search customers".to_string()),
            )
            match customers.network_state() {
                NetworkState::Success(rows) => {
                    CustomerResults(rows = rows.clone(), query = query, on_add = add_empty.clone(), on_clear = clear.clone())
                }
                NetworkState::Error(message) => {
                    Alert(
                        title = "Couldn't load customers",
                        body = Some(message.clone()),
                        tone = tone::Danger,
                        variant = variant::Soft,
                        action = Some(ui! { Button(label = "Try again", on_click = retry.clone(), tone = tone::Danger, variant = variant::Outlined) }),
                    )
                }
                _ => { ListSkeleton(rows = 3u32) }
            }
        }
    }
}

/// The data state — or, when it's empty, which KIND of empty: nothing
/// exists yet (first run) vs. the search matched nothing.
#[runtime_core::component]
fn CustomerResults(
    #[prop(static, default = Vec::new())] rows: Vec<Customer>,
    #[prop(default = signal(String::new()))] query: Signal<String>,
    #[prop(default = Rc::new(|| {}) as Rc<dyn Fn()>)] on_add: Rc<dyn Fn()>,
    #[prop(default = Rc::new(|| {}) as Rc<dyn Fn()>)] on_clear: Rc<dyn Fn()>,
) -> Element {
    if !rows.is_empty() {
        return ui! {
            List {
                for c in rows {
                    ListItem(label = c.name.to_string(), trailing = Some(ui! {
                        Typography(content = c.plan, muted = true, kind = typography_kind::BodySm)
                    }))
                }
            }
        };
    }
    // A search that matched nothing is not a first run: say which filter
    // emptied the list and offer to clear it.
    ui! {
        if query.get().is_empty() {
            EmptyState(
                title = "No customers yet",
                body = "Customers you add show up here, with their plan and usage.",
                action = Some(ui! { Button(label = "Add customer", on_click = on_add.clone()) }),
            )
        } else {
            EmptyState(
                title = "No matching customers",
                body = "Nothing matches that search.",
                action = Some(ui! {
                    Button(label = "Clear search", on_click = on_clear.clone(), tone = tone::Neutral, variant = variant::Outlined)
                }),
            )
        }
    }
}

/// Placeholder rows in the list's shape: the page doesn't jump when the
/// real rows arrive.
#[runtime_core::component]
fn ListSkeleton(#[prop(static, default = 3u32)] rows: u32) -> Element {
    ui! {
        Card(padding = CardPadding::Md) {
            Stack(gap = StackGap::Md) {
                for _ in 0..rows {
                    Stack(gap = StackGap::Xs) {
                        Skeleton(width = SkeletonWidth::Half, height = 16.0)
                        Skeleton(width = SkeletonWidth::ThreeQuarter, height = 12.0)
                    }
                }
            }
        }
    }
}

/// An empty state: what belongs here, and the action that puts it there.
#[runtime_core::component]
pub fn EmptyState(
    title: String,
    body: String,
    #[prop(static, default = None)] action: Option<Element>,
) -> Element {
    ui! {
        Card(padding = CardPadding::Lg) {
            Center {
                Stack(gap = StackGap::Sm, align = StackAlign::Center) {
                    Typography(content = title, kind = typography_kind::H3)
                    Typography(content = body, muted = true)
                    if let Some(action) = action {
                        action
                    }
                }
            }
        }
    }
}
