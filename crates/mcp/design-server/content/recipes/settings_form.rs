//! # Settings screen: sectioned form, validation, danger zone
//!
//! summary: A Narrow page of titled sections — explicit-save profile fields with blur/submit validation, immediate-apply switches, and a fenced-off destructive action behind an adaptive confirm.
//! tags: forms, settings, validation, modal, destructive, adaptive
//! render: / @phone => Settings, Profile, Display name, Save profile, Notifications, Danger zone, Delete workspace, !Delete this workspace?
//! render: / @desktop => Settings, Profile, Email, Product updates, Changes here apply immediately, Delete workspace
//!
//! The form rules this screen encodes (read_design_skill("forms") for the
//! rest):
//! - **One save model per section.** Text fields are drafts with an
//!   explicit Save at the end of their section; switches apply the moment
//!   they flip, and their section says so. Never mix the two in one section.
//! - **Validate on blur and on submit, never per keystroke.** A field shows
//!   its error once the user has left it (or tried to save), and clears it
//!   as soon as the value is valid again.
//! - **Labels always**, placeholders only as examples. Help text under a
//!   field says what the value is for, not how to type it.
//! - **Destructive actions are fenced**: their own toned section at the
//!   bottom, an outlined danger button, and a confirm that names the
//!   consequence. The confirm is a bottom sheet on phones (thumb reach) and
//!   a centered dialog on wide screens — one `Modal`, one reactive
//!   `presentation`.
//! - **Feedback**: success is a toast (it doesn't block); failure is an
//!   inline Alert at the section it belongs to (it must be read).
//!
//! Depends on the `page_layout` recipe (`crate::page_layout`).

use crate::page_layout::{Page, PageWidth, Section};
use idea_ui::{
    push_toast, tone, typography_kind, variant, Button, Card, CardPadding, Field, Modal,
    ModalPresentation, Stack, StackAxis, StackGap, StackJustify, Switch, ToastHost,
    ToastPlacement, Typography,
};
use runtime_core::{current_breakpoint, rx, signal, ui, Breakpoint, Element, Signal};
use std::rc::Rc;

pub fn app() -> Element {
    idea_ui::install_idea_theme(idea_ui::light_theme());
    // ONE ToastHost per app, at the root (normally in the shell).
    ui! {
        view {
            SettingsScreen()
            ToastHost(placement = ToastPlacement::BottomCenter)
        }
    }
}

/// A validation rule: the error message for an invalid value.
type Rule = fn(&str) -> Option<&'static str>;

/// A draft text field with blur/submit validation.
struct Validated {
    value: Signal<String>,
    touched: Signal<bool>,
    validate: Rule,
}

impl Validated {
    fn new(initial: &str, validate: Rule) -> Self {
        Self { value: signal(initial.to_string()), touched: signal(false), validate }
    }
}

fn no_rule(_: &str) -> Option<&'static str> {
    None
}

fn required(v: &str) -> Option<&'static str> {
    v.trim().is_empty().then_some("Enter a display name.")
}

fn email(v: &str) -> Option<&'static str> {
    let v = v.trim();
    (!v.contains('@') || v.starts_with('@') || v.ends_with('@')).then_some("Enter an email address like name@company.com.")
}

#[runtime_core::component]
fn SettingsScreen() -> Element {
    let name = Validated::new("Ada Lovelace", required);
    let mail = Validated::new("ada@example.com", email);
    let product_updates = signal(true);
    let weekly_digest = signal(false);
    let confirm_delete = signal(false);
    // Handlers are bound to typed `Rc<dyn Fn…>` locals before `ui!`: a bare
    // `Rc::new(closure)` in a prop position doesn't coerce to the prop's
    // trait-object type.
    let set_updates: Rc<dyn Fn(bool)> = Rc::new(move |v| product_updates.set(v));
    let set_digest: Rc<dyn Fn(bool)> = Rc::new(move |v| weekly_digest.set(v));

    let save_profile: Rc<dyn Fn()> = {
        let (n, m) = (name.touched, mail.touched);
        let valid = {
            let (nv, mv) = (name.value, mail.value);
            move || required(&nv.get()).is_none() && email(&mv.get()).is_none()
        };
        Rc::new(move || {
            // Submitting reveals every field's error at once.
            n.set(true);
            m.set(true);
            if valid() {
                push_toast("Profile saved", tone::Success);
            }
        })
    };

    let ask_delete: Rc<dyn Fn()> = Rc::new(move || confirm_delete.set(true));
    let cancel_delete: Rc<dyn Fn()> = Rc::new(move || confirm_delete.set(false));
    let do_delete: Rc<dyn Fn()> = Rc::new(move || {
        confirm_delete.set(false);
        push_toast("Workspace scheduled for deletion", tone::Danger);
    });

    ui! {
        Page(
            title = "Settings",
            subtitle = Some("Your profile and how Acme keeps in touch.".to_string()),
            width = PageWidth::Narrow,
        ) {
            Section(title = "Profile") {
                Card(padding = CardPadding::Md) {
                    Stack(gap = StackGap::Md) {
                        ValidatedField(
                            label = "Display name",
                            value = name.value,
                            touched = name.touched,
                            validate = name.validate,
                            help = Some("Shown to your teammates.".to_string()),
                        )
                        ValidatedField(label = "Email", value = mail.value, touched = mail.touched, validate = mail.validate)
                        Stack(axis = StackAxis::Row, justify = StackJustify::End) {
                            Button(label = "Save profile", on_click = save_profile)
                        }
                    }
                }
            }
            Section(
                title = "Notifications",
                description = Some("Changes here apply immediately.".to_string()),
            ) {
                Card(padding = CardPadding::Md) {
                    Stack(gap = StackGap::Md) {
                        Switch(
                            label = Some("Product updates".to_string()),
                            value = product_updates,
                            on_change = set_updates,
                        )
                        Switch(
                            label = Some("Weekly digest".to_string()),
                            value = weekly_digest,
                            on_change = set_digest,
                        )
                    }
                }
            }
            Section(title = "Danger zone") {
                Card(padding = CardPadding::Md, tone = tone::Danger) {
                    Stack(gap = StackGap::Sm) {
                        Typography(content = "Deleting the workspace removes every project and member. This can't be undone.")
                        Stack(axis = StackAxis::Row) {
                            Button(label = "Delete workspace", on_click = ask_delete, tone = tone::Danger, variant = variant::Outlined)
                        }
                    }
                }
            }
            Modal(
                open = confirm_delete,
                // Thumb-reachable sheet on phones, a dialog on wide screens.
                presentation = rx!(if current_breakpoint().get().is_at_least(Breakpoint::Md) {
                    ModalPresentation::Centered
                } else {
                    ModalPresentation::Sheet
                }),
                on_dismiss = Some(cancel_delete.clone()),
                content = move || {
                    let (cancel, delete) = (cancel_delete.clone(), do_delete.clone());
                    ui! {
                        Stack(gap = StackGap::Md) {
                            Typography(content = "Delete this workspace?", kind = typography_kind::H3)
                            Typography(content = "All 12 projects and their history will be permanently removed.", muted = true)
                            // Cancel first, the destructive action last (trailing).
                            Stack(axis = StackAxis::Row, justify = StackJustify::End, gap = StackGap::Sm) {
                                Button(label = "Cancel", on_click = cancel, tone = tone::Neutral, variant = variant::Ghost)
                                Button(label = "Delete workspace", on_click = delete, tone = tone::Danger)
                            }
                        }
                    }
                },
            )
        }
    }
}

/// A Field wired to a [`Validated`] draft: the error appears after blur or
/// submit and clears as soon as the value is valid.
#[runtime_core::component]
fn ValidatedField(
    label: String,
    #[prop(default = signal(String::new()))] value: Signal<String>,
    /// Set on blur, and by the section's Save for every field.
    #[prop(default = signal(false))] touched: Signal<bool>,
    #[prop(static, default = no_rule as Rule)] validate: Rule,
    #[prop(default = None)] help: Option<String>,
) -> Element {
    let error = rx!(if touched.get() { validate(&value.get()).map(str::to_string) } else { None });
    let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| value.set(v));
    let on_blur: Rc<dyn Fn(bool)> = Rc::new(move |focused| {
        if !focused {
            touched.set(true);
        }
    });
    ui! {
        Field(
            label = rx!(Some(label.get())),
            value = value,
            on_change = on_change,
            help = help,
            error = error,
            on_focus_change = Some(on_blur),
        )
    }
}
