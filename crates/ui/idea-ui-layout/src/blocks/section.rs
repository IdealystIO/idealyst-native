//! [`SectionHeading`] and [`Section`] — the named group a screen is
//! built from (rule `groups`).
//!
//! Both gaps come from the spacing ladder (rule `spacing-ladder`): a
//! title and its subtitle are parts of one thing, `xs`; a section's
//! heading and its body are distinct things in one section, `md`. The
//! `lg` between sections is the page's.

use idea_ui::{typography_kind, Stack, StackGap, Typography, TypographyKind};
use runtime_core::{component, ui, ChildList, Element, IdealystSchema};

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
}

impl Default for SectionHeadingProps {
    fn default() -> Self {
        Self { title: String::new(), subtitle: None, kind: default_kind() }
    }
}

/// A title and its optional muted subtitle, `xs` apart: the one place
/// that distance is decided.
///
/// ```ignore
/// SectionHeading(title = "Crew".to_string(), subtitle = Some("Who is on this shift".to_string()))
/// ```
#[component]
pub fn SectionHeading(props: &SectionHeadingProps) -> Element {
    let title = props.title.clone();
    let kind = props.kind.clone();
    let subtitle = shown_subtitle(props.subtitle.as_deref()).map(str::to_string);
    ui! {
        Stack(gap = StackGap::Xs) {
            Typography(content = title, kind = kind)
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
    /// The section's body.
    pub children: Vec<Element>,
}

impl Default for SectionProps {
    fn default() -> Self {
        Self { title: String::new(), subtitle: None, kind: default_kind(), children: Vec::new() }
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
            SectionHeading(title = props.title.clone(), subtitle = props.subtitle.clone(), kind = props.kind.clone())
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
