//! # Sign in: the auth screen outside the shell
//!
//! summary: A centered Narrow auth column — product name, one H1, labelled email/password fields with the right keyboards, a block primary button with loading, inline error, Enter-to-submit and the secondary paths as links.
//! tags: auth, forms, sign-in, keyboard, responsive
//! render: / @phone => Acme, Sign in to Acme, Email, Password, Forgot password?, Create an account, !Incorrect email or password
//! render: / @desktop => Sign in to Acme, Sign in, Create an account
//!
//! Auth screens sit outside the app shell (no sidebar, no tabs) and do one
//! thing. The layout is the same at every width — a single column capped
//! at 400 — because the content is a single short form; only the gutter
//! and the vertical position change.
//!
//! - **The keyboard is part of the design**: `KeyboardKind::Email` for the
//!   email field, `secure = true` for the password, Enter submits from the
//!   password field. The whole screen sits in a `keyboard_avoiding_view` so
//!   the button stays visible above a phone keyboard.
//! - **Errors are inline and generic**: "Incorrect email or password" in
//!   an Alert above the fields — never which of the two was wrong.
//! - **The button never disables to say "fill this in"**: pressing with an
//!   empty field shows the field error instead.
//! - **Secondary paths are links**, visually quieter than the primary.

use idea_ui::{
    tone, typography_kind, variant, Alert, Button, Card, CardPadding, Field, IdeaThemeRef, Link,
    Stack, StackAlign, StackGap, Typography,
};
use runtime_core::{
    rx, signal, stylesheet, ui, AlignItems, AlignSelf, Element,
    FlexDirection, JustifyContent, KeyEvent, KeyOutcome, KeyboardKind, Length, SafeAreaSides,
};
use std::rc::Rc;

stylesheet! {
    /// Fills the window and centers the auth column.
    AuthBackdrop<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            width: Length::pct(100.0),
            height: Length::pct(100.0),
            padding: t.spacing.md(),
            background: t.color.background(),
        }
    }
}

stylesheet! {
    /// The single, width-capped column.
    AuthColumn<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            max_width: Length::Px(400.0),
            align_self: AlignSelf::Center,
            gap: t.spacing.lg(),
        }
    }
}

pub fn app() -> Element {
    idea_ui::install_idea_theme(idea_ui::light_theme());
    let email = signal(String::new());
    let password = signal(String::new());
    let submitted = signal(false);
    let pending = signal(false);
    let failed = signal(false);

    let submit: Rc<dyn Fn()> = Rc::new(move || {
        submitted.set(true);
        if email.get().trim().is_empty() || password.get().is_empty() {
            return;
        }
        pending.set(true);
        // api::sign_in(email, password) — on failure:
        failed.set(true);
        pending.set(false);
    });
    let on_email: Rc<dyn Fn(String)> = Rc::new(move |v| email.set(v));
    let on_password: Rc<dyn Fn(String)> = Rc::new(move |v| password.set(v));
    let enter_submits: Rc<dyn Fn(&KeyEvent) -> KeyOutcome> = {
        let submit = submit.clone();
        Rc::new(move |e: &KeyEvent| {
            if e.key == "Enter" {
                submit();
                KeyOutcome::PreventDefault
            } else {
                KeyOutcome::Default
            }
        })
    };
    let forgot: Rc<dyn Fn()> = Rc::new(|| {});
    let sign_up: Rc<dyn Fn()> = Rc::new(|| {});

    ui! {
        keyboard_avoiding_view(style = AuthBackdrop(), safe_area = SafeAreaSides::ALL) {
            view(style = AuthColumn()) {
                Stack(gap = StackGap::Xs) {
                    Typography(content = "Acme", kind = typography_kind::Overline, muted = true)
                    Typography(content = "Sign in to Acme", kind = typography_kind::H1)
                }
                Card(padding = CardPadding::Md) {
                    Stack(gap = StackGap::Md) {
                        if failed.get() {
                            Alert(title = "Incorrect email or password", tone = tone::Danger, variant = variant::Soft)
                        }
                        Field(
                            label = Some("Email".to_string()),
                            value = email,
                            on_change = on_email,
                            keyboard = KeyboardKind::Email,
                            placeholder = Some("name@company.com".to_string()),
                            error = rx!((submitted.get() && email.get().trim().is_empty()).then(|| "Enter your email.".to_string())),
                        )
                        Field(
                            label = Some("Password".to_string()),
                            value = password,
                            on_change = on_password,
                            secure = true,
                            on_key_down = Some(enter_submits),
                            error = rx!((submitted.get() && password.get().is_empty()).then(|| "Enter your password.".to_string())),
                        )
                        Button(label = "Sign in", on_click = submit, block = true, loading = pending)
                        Stack(align = StackAlign::Center) {
                            Link(label = "Forgot password?", on_press = Some(forgot))
                        }
                    }
                }
                Stack(align = StackAlign::Center) {
                    Link(label = "Create an account", on_press = Some(sign_up))
                }
            }
        }
    }
}
