//! The light / dark switch. The app owns the `dark` signal and installs
//! idea-ui's theme from it (`install_idea_theme_reactive` in `app()`), so
//! flipping it re-themes everything through the theme tokens.

use std::rc::Rc;

use idea_ui::{tone, variant, IconButton, IconButtonSize};
use runtime_core::{component, memo, ui, Element, Signal};

#[component]
pub fn ThemeToggle(dark: Signal<bool>) -> Element {
    // The icon names the mode a press switches TO.
    let icon = memo(move || Some(if dark.get() { icons_lucide::SUN } else { icons_lucide::MOON }));
    let flip = Rc::new(move || dark.update(|d| !*d)) as Rc<dyn Fn()>;
    ui! {
        IconButton(
            glyph = String::new(),
            icon = icon,
            on_click = flip,
            tone = tone::Neutral,
            variant = variant::Ghost,
            size = IconButtonSize::Md,
        )
    }
}
