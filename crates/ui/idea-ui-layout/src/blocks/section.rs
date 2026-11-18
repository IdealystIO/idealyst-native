//! [`SectionHeading`] and [`Section`] — the named group a screen is
//! built from (rule `groups`).
//!
//! Both gaps come from the spacing ladder (rule `spacing-ladder`): a
//! title and its subtitle are parts of one thing, `xs`; a section's
//! heading and its body are distinct things in one section, `md`. The
//! `lg` between sections is the page's.

use idea_ui::{typography_kind, Spacer, Stack, StackAlign, StackAxis, StackGap, Typography, TypographyKind};
use runtime_core::{component, ui, ChildList, Element, IdealystSchema};

use super::HelpHint;

fn default_kind() -> TypographyKind {
    typography_kind::H3
}

/// The subtitle worth mounting. A blank one is absent: a formatted line
/// can come out empty, and an empty text leaf would hold the gap open
/// under the heading for a line nobody can see.
fn shown_subtitle(subtitle: Option<&str>) -> Option<&str> {
    subtitle.map(str::trim).filter(|s| !s.is_empty())
}

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct SectionHeadingProps {
    /// What this section is.
    #[prop(static)]
    pub title: String,
    /// The muted line under the title. `None` or blank mounts nothing.
    #[prop(static)]
    pub subtitle: Option<String>,
    /// Heading level. Default `H3`.
    #[prop(static)]
    pub kind: TypographyKind,
    /// What the title cannot say for itself: a `?` beside it whose
    /// tooltip is this. `None` or blank mounts nothing.
    #[prop(static)]
    pub hint: Option<String>,
    /// The section's verb at the heading's right end: "Add", "Edit".
    /// `None` mounts nothing.
    pub action: Option<Element>,
}

impl Default for SectionHeadingProps {
    fn default() -> Self {
        Self { title: String::new(), subtitle: None, kind: default_kind(), hint: None, action: None }
    }
}

/// A title and its optional muted subtitle, `xs` apart: the one place
/// that distance is decided. A `hint` stands beside the title as a `?`;
/// an `action` stands at the heading's right end, level with the title.
///
/// ```ignore
/// SectionHeading(title = "Crew".to_string(), subtitle = Some("Who is on this shift".to_string()))
/// SectionHeading(
///     title = "Rotations".to_string(),
///     hint = Some("A rotation repeats until its end date.".to_string()),
///     action = Some(ui! { Button(label = "Add", on_click = add) }),
/// )
/// ```
#[component]
pub fn SectionHeading(props: SectionHeadingProps) -> Element {
    let title = props.title;
    let kind = props.kind;
    let subtitle = shown_subtitle(props.subtitle.as_deref()).map(str::to_string);
    let hint = shown_subtitle(props.hint.as_deref()).map(str::to_string);
    let action = props.action;
    // The title's own line: the title, its hint, and the action at the
    // far end. With neither, the title is the line.
    let titled = match (hint, action) {
        (None, None) => ui! { Typography(content = title, kind = kind) },
        (hint, action) => ui! {
            Stack(axis = StackAxis::Row, align = StackAlign::Center, gap = StackGap::Md) {
                Stack(axis = StackAxis::Row, align = StackAlign::Center, gap = StackGap::Sm) {
                    Typography(content = title, kind = kind)
                    if let Some(hint) = hint {
                        HelpHint(text = hint)
                    }
                }
                Spacer()
                if let Some(action) = action {
                    action
                }
            }
        },
    };
    ui! {
        Stack(gap = StackGap::Xs) {
            titled
            if let Some(subtitle) = subtitle {
                Typography(content = subtitle, kind = typography_kind::BodySm, muted = true)
            }
        }
    }
}

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct SectionProps {
    /// What this section is. A group that cannot be named is not a
    /// group: merge it into its neighbour or split it differently.
    #[prop(static)]
    pub title: String,
    /// The muted line under the title.
    #[prop(static)]
    pub subtitle: Option<String>,
    /// Heading level. Default `H3`.
    #[prop(static)]
    pub kind: TypographyKind,
    /// A `?` beside the title whose tooltip is this.
    #[prop(static)]
    pub hint: Option<String>,
    /// The section's verb at the heading's right end.
    pub action: Option<Element>,
    /// The section's body.
    pub children: Vec<Element>,
}

impl Default for SectionProps {
    fn default() -> Self {
        Self {
            title: String::new(),
            subtitle: None,
            kind: default_kind(),
            hint: None,
            action: None,
            children: Vec::new(),
        }
    }
}

/// A named group of controls: a [`SectionHeading`], then the body, `md`
/// apart. The children of a page's `lg` stack are these.
///
/// ```ignore
/// Section(title = "Shift".to_string()) {
///     FieldGrid(columns = 2_usize) {
///         TimeInput(label = Some("Starts".to_string()), …)
///         TimeInput(label = Some("Ends".to_string()), …)
///     }
/// }
/// ```
#[component(children)]
pub fn Section(props: SectionProps) -> Element {
    // The `#[component(children)]` slot's flatten, not a staged list.
    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    ui! {
        Stack(gap = StackGap::Md) {
            SectionHeading(
                title = props.title,
                subtitle = props.subtitle,
                kind = props.kind,
                hint = props.hint,
                action = props.action,
            )
            children
        }
    }
}

#[cfg(test)]
mod tests {
    use super::shown_subtitle;

    #[test]
    fn a_blank_subtitle_is_absent_rather_than_an_empty_line() {
        assert_eq!(shown_subtitle(None), None);
        assert_eq!(shown_subtitle(Some("")), None);
        assert_eq!(shown_subtitle(Some("   ")), None);
    }

    #[test]
    fn a_subtitle_is_trimmed() {
        assert_eq!(shown_subtitle(Some("  Who is on this shift  ")), Some("Who is on this shift"));
    }
}
