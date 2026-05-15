//! Integration test that exercises every idea-ui component through
//! the `ui!` DSL — using the macros from a separate crate context,
//! which is what real consumers do. If the macros aren't properly
//! exported, this test fails at compile time.

use std::rc::Rc;

use framework_core::{component, install_theme, signal, ui, Primitive, Signal};
use idea_ui::{
    light_theme, BadgeTone, BodyTone, CardTone, FieldSize, HeadingKind, PressableKind,
    PressableSize, StackAlign, StackGap, StackJustify,
};
// Macros must be brought into scope by name for `ui!`'s lowered
// `vstack!(...)` / `card!(...)` etc. to resolve.
use idea_ui::{
    badge, body, caption, card, divider, field, heading, hstack, pressable, spinner, switch,
    vstack,
};

#[component]
fn demo() -> Primitive {
    install_theme(light_theme());

    let name: Signal<String> = signal!(String::new());
    let dark: Signal<bool> = signal!(false);

    let on_name_change: Rc<dyn Fn(String)> = Rc::new(move |v| name.set(v));
    let on_dark_change: Rc<dyn Fn(bool)> = Rc::new(move |v| dark.set(v));
    let on_save: Rc<dyn Fn()> = Rc::new(|| {});

    ui! {
        VStack(gap = StackGap::Lg, align = StackAlign::Stretch) {
            Heading(content = "Heading H1".to_string(), kind = HeadingKind::H1)
            Heading(content = "Heading H2".to_string(), kind = HeadingKind::H2)
            Body(content = "Body text".to_string(), tone = BodyTone::Default)
            Body(content = "Muted body".to_string(), tone = BodyTone::Muted)
            Caption(content = "Caption / helper text".to_string())

            HStack(gap = StackGap::Sm, justify = StackJustify::Between) {
                Pressable(
                    label = "Primary".to_string(),
                    on_click = on_save.clone(),
                    kind = PressableKind::Primary,
                    size = PressableSize::Md
                )
                Pressable(
                    label = "Secondary".to_string(),
                    on_click = on_save.clone(),
                    kind = PressableKind::Secondary
                )
                Pressable(
                    label = "Ghost".to_string(),
                    on_click = on_save.clone(),
                    kind = PressableKind::Ghost
                )
                Pressable(
                    label = "Danger".to_string(),
                    on_click = on_save.clone(),
                    kind = PressableKind::Danger
                )
            }

            Card(tone = CardTone::Elevated) {
                Heading(content = "Card title".to_string(), kind = HeadingKind::H3)
                Body(content = "Card body content goes here.".to_string())
                Badge(label = "New".to_string(), tone = BadgeTone::Primary)
            }

            Field(
                label = Some("Name".to_string()),
                value = name,
                on_change = on_name_change.clone(),
                placeholder = Some("Enter your name".to_string()),
                help = Some("Used only to greet you.".to_string()),
                size = FieldSize::Md
            )

            Field(
                label = Some("Email".to_string()),
                value = name,
                on_change = on_name_change.clone(),
                placeholder = Some("you@example.com".to_string()),
                error = Some("Invalid email".to_string())
            )

            Switch(
                label = Some("Dark mode".to_string()),
                value = dark,
                on_change = on_dark_change.clone()
            )

            Divider()
            Spinner()
        }
    }
}

#[test]
fn every_component_compiles() {
    let _tree: Primitive = demo();
}
